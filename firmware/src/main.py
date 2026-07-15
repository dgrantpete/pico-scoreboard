# GPIO Pin assignments
# HUB75:
#   GPIO 11-15: Address pins (A-E)
#   GPIO 16-21: Data pins (R1, G1, B1, R2, G2, B2)
#   GPIO 26: Clock
#   GPIO 27: Latch
#   GPIO 28: Output Enable (OE)
#
# VEML7700:
#   GPIO 0: SDA
#   GPIO 1: SCL
#
# Rotary Encoder:
#   GPIO 2: Button
#   GPIO 3: Channel A
#   GPIO 4: Channel B
#
# Button A: GPIO 10 (skip to next game; held at power-up = safe mode)
# Button B: GPIO 22 (toggle rotation lock)
#
# PIO allocation: PIO0 = HUB75 driver, PIO1 = buttons (SM0 = A, SM1 = B)

"""
Pico Scoreboard Web Server.

Serves the SvelteKit frontend from flash memory and provides
network connectivity.

Automatically enters setup mode (AP) when no WiFi is configured
or when connection to the configured network fails. Once properly
configured, connects to the specified WiFi network.
"""

# --- Safe-mode escape hatch -------------------------------------------------
# MUST stay first: once the Core 1 display thread starts, mpremote filesystem
# commands hang (micropython#13476) and a soft reset can wedge TinyUSB via a
# spinlock held by the dying core (micropython#8494) — flashing a *running*
# scoreboard is inherently unreliable. Skipping the app entirely leaves a
# clean single-core REPL that mpremote can always talk to.
#
# Two triggers:
#   - Button A (GPIO 10, active-low) held at power-up — the manual escape,
#     works even when the firmware wedges USB (hold + power-cycle).
#   - /update flag file — written by `tools/build.py flash`, consumed here,
#     so the tool can reboot the device into a flashable state on its own.

def _safe_mode_reason() -> "str | None":
    """Why safe mode was requested ('Button held' / 'Update flag'), or None."""
    import machine
    import os
    import time

    try:
        pin = machine.Pin(10, machine.Pin.IN, machine.Pin.PULL_UP)
        time.sleep_ms(5)  # let the pull-up settle before sampling
        if pin.value() == 0:
            print("[BOOT] safe mode: Button A held at power-up")
            return "Button held"
    except Exception:
        pass

    try:
        os.stat('/update')
        os.remove('/update')
        print("[BOOT] safe mode: /update flag consumed")
        return "Update flag"
    except OSError:
        pass

    return None


def _early_display_show(lines) -> "object | None":
    """One-shot panel bring-up for the pre-app boot screens (safe mode, OTA
    apply). The HUB75 driver is CPU-free after construction — PIO + DMA
    refresh the panel continuously — so a single load+flip keeps the message
    lit with zero CPU, even after sys.exit() to the REPL.

    Best-effort by design: hub75 may live in ROMFS on release builds and the
    panel may be absent, so any failure returns None and boot continues.
    Uses framebuf's built-in 8x8 font (no scoreboard.fonts dependency);
    lines must be <= 16 chars. First line is the title (amber), rest white.
    Returns the driver (caller deinit()s it if the app will start) or None.
    """
    try:
        import framebuf
        from machine import Pin
        from hub75 import Hub75Driver, row_addressing

        # Same panel wiring as scoreboard.display.init_display; driver
        # defaults cover frequency/gamma/refresh. Fixed dim brightness —
        # config.json isn't worth parsing this early.
        driver = Hub75Driver(
            row_addressing=row_addressing.Binary(base_pin=Pin(11, Pin.OUT), bit_count=5),
            shift_register_depth=128,
            output_enable_pin=Pin(28, Pin.OUT),
            base_clock_pin=Pin(26, Pin.OUT),
            base_data_pin=Pin(16, Pin.OUT),
            brightness=0.3,
        )
        width, height = 128, 64
        buf = bytearray(width * height * 2)
        fb = framebuf.FrameBuffer(buf, width, height, framebuf.RGB565)
        line_pitch = 12
        y = (height - (len(lines) * line_pitch - 4)) // 2
        for i, line in enumerate(lines):
            color = 0xFDE0 if i == 0 else 0xFFFF  # amber title, white body
            fb.text(line, (width - len(line) * 8) // 2, y, color)
            y += line_pitch
        driver.load_rgb565(buf)
        driver.flip()
        return driver
    except Exception as e:
        print("[BOOT] early display unavailable:", e)
        return None


_safe_reason = _safe_mode_reason()
if _safe_reason is not None:
    # Positive sentinel for tools/build.py: mpremote `exec` shares this
    # namespace, so the flash flow can PROVE safe mode took (a timing
    # heuristic once false-positived and erased ROMFS under the running app).
    _SAFE_MODE = True
    print("[BOOT] application NOT started — REPL is free for mpremote/flashing")
    # MUST NOT sys.exit() here: SystemExit from main.py is a forced exit to
    # the rp2 port (ports/rp2/main.c), which SOFT-REBOOTS and re-runs
    # main.py — with the trigger consumed, that re-entered the app and
    # silently defeated safe mode every time (2026-07-11). Raising any
    # other exception halts main.py and drops to the REPL with this
    # namespace (and the sentinel) intact. The traceback below is expected.
    # Also keep this path display-free (BACKLOG 38).
    raise RuntimeError("SAFE MODE (%s) — not an error; REPL is free" % _safe_reason)
# ----------------------------------------------------------------------------

import network
import time
import machine
import uasyncio as asyncio
import gc
import os
import rp2
import hashlib
import _thread
import ota

# --- OTA apply-at-boot --------------------------------------------------------
# Apply any staged app image BEFORE importing the app: at this point nothing
# from ROMFS is running or imported and Core 1 hasn't started, so rewriting
# the partition cannot erase code mid-execution. This is the ONLY place a
# staged image is committed (see firmware/src/ota.py for the full lifecycle).
# NOTE: the "UPDATING" splash that lived here shares the safe-mode splash's
# early-boot hard fault (BACKLOG 38) — a fault at this point would reset
# mid-OTA-window. Keep this path splash-free until that's debugged.
ota.apply_staged()

# --- Crash-loop self-heal -----------------------------------------------------
# ota.recover() only triggers on ImportError; an app image that imports but
# crashes before main() settles would otherwise crash-loop forever with no
# OTA escape (friends' devices have no USB). Count consecutive boots that
# (a) were not a power-cycle (crash resets report as WDT on RP2) and
# (b) never reached the healthy point in main() that clears the counter;
# at the limit, force a full OTA re-download of the published app.
# Best-effort: a failure in the counter itself must never block a boot.
_BOOT_FAILS_FILE = '/boot_fails'
_BOOT_FAIL_LIMIT = 5
_boot_fails = 0


def _clear_boot_fails() -> None:
    """Called from main() once the app reaches a healthy state."""
    global _boot_fails
    if _boot_fails:
        _boot_fails = 0
        try:
            os.remove(_BOOT_FAILS_FILE)
        except OSError:
            pass


try:
    if machine.reset_cause() == machine.PWRON_RESET:
        # A human power-cycled us: whatever happened before, it wasn't a
        # crash loop we can heal. Start the count over.
        try:
            os.remove(_BOOT_FAILS_FILE)
        except OSError:
            pass
    else:
        try:
            with open(_BOOT_FAILS_FILE) as _f:
                _boot_fails = int(_f.read())
        except (OSError, ValueError):
            _boot_fails = 0
        _boot_fails += 1
        with open(_BOOT_FAILS_FILE, 'w') as _f:
            _f.write(str(_boot_fails))
        if _boot_fails >= _BOOT_FAIL_LIMIT:
            print('[BOOT] %d consecutive failed boots; forcing OTA recovery'
                  % _boot_fails)
            os.remove(_BOOT_FAILS_FILE)
            _boot_fails = 0
            ota.recover()  # loops until healed + reset; returns if impossible
except Exception as _e:
    print('[BOOT] boot-fail counter error:', _e)
# ------------------------------------------------------------------------------

# --- App imports: the app may live in ROMFS (release deploys) ----------------
# A corrupt or missing ROMFS (e.g. power loss mid-OTA-write) must never brick
# the device: attempt OTA self-recovery, else drop to a clean REPL so the
# image can be re-deployed. Dev deploys on littlefs shadow ROMFS and are
# unaffected.
try:
    from microdot import Microdot, Request, Response, send_file
    from scoreboard import Config, ScoreboardApiClient
    from scoreboard.poller import GamePoller, sources_from_config
    from scoreboard.state import (
        set_startup_step, finish_startup, set_display_driver, get_write_state,
        set_mode, set_updating_progress, set_updating_countdown, ThreadHealth,
    )
    from scoreboard.dns import run_dns_server
    from scoreboard.api_routes import create_api
    from scoreboard.display import init_display, run_display_thread, Regions, LogoPool
    from scoreboard.fonts import FontWriter
    from hub75 import Hub75Display, Hub75Driver
    from machine import I2C, Pin
    from veml7700 import VEML7700
    from button import Button
    from scoreboard import brightness
    import scoreboard.logger as logger
except ImportError as e:
    print(f"[BOOT] app import failed: {e}")
    print("[BOOT] ROMFS may be corrupt/missing; attempting OTA self-recovery")
    ota.recover()  # loops until healed + reset; returns only if unrecoverable
    print("[BOOT] recovery not possible; re-deploy with "
          "'python tools/build.py flash [--release]'. REPL is free.")
    # Not sys.exit(): that soft-reboots and re-runs main.py (see the safe-
    # mode block above), which would loop recovery forever. Raise instead.
    raise RuntimeError("app not started — not an error beyond the above; REPL is free")

# Reduce buffer size for memory-constrained environment
Response.send_file_buffer_size = 2048

# Collect after ~48 KB of allocation instead of waiting for the heap to
# fill: post-ROMFS churn is ~4 KB/s, so this trades a cheap collection every
# ~12s for never letting free memory grind near zero — headroom the ~33 KB
# contiguous TLS buffers want on reconnect. (Calibrated 2026-07-06; see
# BACKLOG history for the measurements.)
gc.threshold(48 * 1024)

# The app's OTA identity (sha256 of the ROMFS image), None on dev/littlefs
# deploys. Read once: it can only change via a reboot.
APP_VERSION: str | None = ota.current_version()

app: Microdot = Microdot()
config: Config = Config()

# --- On-demand OTA check ------------------------------------------------------
# The ROMFS app's POST /api/check-update calls request_ota_check() through the
# `app.request_ota_check` attribute — an attribute seam so the two sides can
# version independently: an old ROMFS app never calls it, and a new ROMFS app
# on an old main.py sees it absent and answers 'unsupported'.
_ota_check_event = asyncio.Event()
_ota_task_started = False


async def _kick_ota_check() -> None:
    """Signal the OTA task after a short grace period. Setting the event
    directly from the request handler wakes the task on the very next
    scheduler tick — and its synchronous download then freezes the loop
    before microdot has flushed the HTTP response (observed as a client
    timeout, 2026-07-14). Two seconds lets the response out first."""
    await asyncio.sleep(2)
    _ota_check_event.set()


def request_ota_check() -> dict:
    """Probe the backend manifest now; kick ota_check_task if an update exists.

    Synchronous by design: the manifest fetch blocks the asyncio loop ~1-2 s.
    The download/reboot lifecycle stays single-owner in ota_check_task — this
    only signals it, so a concurrent daily check can't double-stage.
    """
    if not config.ota_enabled:
        return {'status': 'disabled'}
    if ota.dev_marker():
        return {'status': 'dev_deploy'}
    if not _ota_task_started:
        return {'status': 'no_network'}
    try:
        manifest = ota.fetch_manifest(config.api_url, config.api_key)
    except Exception as e:
        return {'status': 'error', 'message': f'{type(e).__name__}: {e}'}
    if manifest['sha256'] == ota.current_version():
        return {'status': 'current', 'version': manifest['sha256']}
    asyncio.create_task(_kick_ota_check())
    return {'status': 'updating', 'version': manifest['sha256']}


app.request_ota_check = request_ota_check


def _find_index() -> str | None:
    """Locate the web bundle: littlefs root first (dev deploys shadow ROMFS),
    then the ROMFS copy (release deploys)."""
    for path in ('/index.html.gz', '/rom/index.html.gz'):
        try:
            os.stat(path)
            return path
        except OSError:
            pass
    return None


INDEX_PATH: str | None = _find_index()


# Compute ETag for the web bundle once at startup
def _compute_index_etag() -> str | None:
    if INDEX_PATH is None:
        return None
    try:
        h = hashlib.sha1()
        with open(INDEX_PATH, 'rb') as f:
            while True:
                chunk: bytes = f.read(512)
                if not chunk:
                    break
                h.update(chunk)
        # Convert first 8 bytes to hex string (16 chars)
        return ''.join('{:02x}'.format(b) for b in h.digest()[:8])
    except OSError:
        return None


INDEX_ETAG: str | None = _compute_index_etag()

def update_startup_display(step: int, operation: str, detail: str = '',
                           attempt: int = 0, attempts_total: int = 0) -> None:
    """Update startup progress state. The display thread renders it on its next tick."""
    set_startup_step(step, 5, operation, detail, attempt, attempts_total)


def _resize_setup_regions_for_qr(regions: Regions) -> None:
    """
    Resize setup-screen text regions to fit beside the current QR code.

    Must be called on Core 0 after finish_startup('setup', ...) has generated
    the QR. Reads QR dimensions from the write buffer (which carries forward
    the just-committed state) and rebuilds the affected Regions so text won't
    overlap the QR footprint.
    """
    state = get_write_state()
    regions.update_for_qr(state.setup.qr_width, state.setup.qr_height)


# Track setup mode state
app.setup_mode = False
app.setup_reason = None  # 'no_network_configured' | 'connection_failed' | 'bad_auth' | None


def get_memory_stats() -> dict:
    """Get current memory usage statistics.

    Deliberately does NOT gc.collect() first: observing memory must not
    change runtime behavior. The reading therefore includes garbage
    accumulated since the last automatic collection — expect a sawtooth
    that climbs and drops; the drops are MicroPython's GC doing its job.
    """
    memory_used = gc.mem_alloc()
    memory_free = gc.mem_free()

    # Flash filesystem usage via statvfs
    stat = os.statvfs('/')
    block_size = stat[0]
    total_blocks = stat[2]
    free_blocks = stat[3]
    flash_total = block_size * total_blocks
    flash_free = block_size * free_blocks
    flash_used = flash_total - flash_free

    return {
        'memory_used': memory_used,
        'memory_free': memory_free,
        'flash_used': flash_used,
        'flash_free': flash_free
    }


def get_network_status() -> dict:
    """Build current network status dict for API."""
    ap = getattr(app, 'ap', None)
    wlan = getattr(app, 'wlan', None)
    setup_mode = getattr(app, 'setup_mode', False)
    setup_reason = getattr(app, 'setup_reason', None)

    # Get memory stats (same for all modes)
    memory = get_memory_stats()

    if ap and ap.active():
        return {
            'mode': 'ap',
            'connected': False,
            'setup_mode': setup_mode,
            'setup_reason': setup_reason,
            'configured_ssid': config.ssid if setup_reason in ('connection_failed', 'bad_auth') else None,
            'ip': None,
            'hostname': None,
            'ap_ip': ap.ifconfig()[0],
            'ap_ssid': config.device_name,
            'memory_used': memory['memory_used'],
            'memory_free': memory['memory_free'],
            'flash_used': memory['flash_used'],
            'flash_free': memory['flash_free'],
            'app_version': APP_VERSION
        }
    elif wlan and wlan.isconnected():
        return {
            'mode': 'station',
            'connected': True,
            'setup_mode': False,
            'setup_reason': None,
            'configured_ssid': None,
            'ip': wlan.ifconfig()[0],
            'hostname': f'{config.device_name}.local',
            'ap_ip': None,
            'ap_ssid': None,
            'memory_used': memory['memory_used'],
            'memory_free': memory['memory_free'],
            'flash_used': memory['flash_used'],
            'flash_free': memory['flash_free'],
            'app_version': APP_VERSION
        }
    else:
        return {
            'mode': 'unknown',
            'connected': False,
            'setup_mode': False,
            'setup_reason': None,
            'configured_ssid': None,
            'ip': None,
            'hostname': None,
            'ap_ip': None,
            'ap_ssid': None,
            'memory_used': memory['memory_used'],
            'memory_free': memory['memory_free'],
            'flash_used': memory['flash_used'],
            'flash_free': memory['flash_free'],
            'app_version': APP_VERSION
        }


async def _sync_time_from_backend() -> int | None:
    """
    Fetch current time from the backend API and set the Pico's RTC.

    Returns:
        UTC offset in seconds for local time display, or None if the sync
        failed. (A successful sync can legitimately return 0 — UTC itself.)
    """
    import aiohttp

    try:
        url = f"{config.api_url.rstrip('/')}/time"
        logger.debug(f"[TIME] sync started: url={url}")

        async def _fetch() -> int | None:
            async with aiohttp.ClientSession() as session:
                async with session.get(url, ssl=None) as resp:
                    if resp.status != 200:
                        logger.error(f"[TIME] sync failed: http={resp.status}")
                        return None

                    data = await resp.json()
                    unix_ts = data['timestamp']
                    utc_offset = data.get('utc_offset') or 0

                    tm = time.gmtime(unix_ts)
                    # gmtime returns: (year, month, mday, hour, minute, second, weekday, yearday)
                    # RTC.datetime expects: (year, month, day, weekday, hours, minutes, seconds, subseconds)
                    machine.RTC().datetime((tm[0], tm[1], tm[2], tm[6], tm[3], tm[4], tm[5], 0))
                    logger.debug(f"[TIME] rtc synced: {tm[0]:04d}-{tm[1]:02d}-{tm[2]:02d} {tm[3]:02d}:{tm[4]:02d}:{tm[5]:02d} UTC offset={utc_offset}s")
                    return utc_offset

        return await asyncio.wait_for(_fetch(), 15)
    except Exception as e:
        logger.error(f"[TIME] sync failed: {e}")
        return None


# Create and mount API under /api prefix
api: Microdot = create_api(config, get_network_status)
app.mount(api, url_prefix='/api')


def get_my_hosts(ap: network.WLAN | None) -> set:
    """
    Get the set of hostnames that belong to us.
    Built dynamically from config and the provided AP interface.
    """
    hosts: set = set()

    # Add configured device name (e.g., "scoreboard.local")
    hosts.add(f"{config.device_name}.local")
    hosts.add(config.device_name)  # Some clients might omit .local

    # Add AP IP address
    if ap:
        hosts.add(ap.ifconfig()[0])

    return hosts


@app.get('/')
async def index(request: Request) -> Response | tuple:
    """Serve the SPA, or redirect hijacked requests to trigger captive portal."""
    ap = getattr(app, 'ap', None)
    host = request.headers.get('Host', '').split(':')[0]

    # If this is a hijacked request (DNS lie), redirect to setup page to trigger portal
    if ap and host not in get_my_hosts(ap):
        redirect_ip = ap.ifconfig()[0]
        return '', 302, {'Location': f'http://{redirect_ip}/#/setup'}

    # Check for conditional request (304 Not Modified)
    if INDEX_ETAG and request.headers.get('If-None-Match') == INDEX_ETAG:
        return '', 304, {'ETag': INDEX_ETAG}

    if INDEX_PATH is None:
        return 'Web bundle missing - redeploy the app', 500

    response = send_file(INDEX_PATH, content_type='text/html', compressed='gzip')

    # Add caching headers
    if INDEX_ETAG:
        response.headers['ETag'] = INDEX_ETAG
    if config.cache_max_age_seconds > 0:
        response.headers['Cache-Control'] = f'max-age={config.cache_max_age_seconds}'

    return response


@app.route('/<path:path>')
async def catch_all(request: Request, path: str) -> tuple:
    """
    Handle unknown paths using Host header to distinguish:
    - Legitimate requests (Host is our IP/hostname) -> 404
    - Hijacked requests (Host is external domain) -> redirect to portal
    """
    ap = getattr(app, 'ap', None)  # Get AP from app object
    host = request.headers.get('Host', '').split(':')[0]  # strip port if present

    if host in get_my_hosts(ap):
        return 'Not found', 404  # Legit request for path that doesn't exist

    # Hijacked request (DNS lie) -> redirect to setup page to trigger captive portal
    redirect_ip = ap.ifconfig()[0] if ap else '192.168.4.1'
    return '', 302, {'Location': f'http://{redirect_ip}/#/setup'}


def start_ap_mode() -> network.WLAN:
    """
    Start Access Point mode for initial setup.

    Creates an open WiFi network that users can connect to
    for configuring the device. Stores the AP interface on the
    app object so routes can access it for captive portal logic.

    Returns:
        The AP WLAN interface
    """
    ap: network.WLAN = network.WLAN(network.AP_IF)
    ap.config(essid=config.device_name, security=0)  # security=0 means open network
    ap.active(True)

    while not ap.active():
        machine.idle()  # Low-power wait instead of hot loop

    app.ap = ap  # Store on app object for routes to access
    logger.debug(f"[WIFI] ap mode started: ssid={config.device_name}")
    logger.debug(f"[WIFI] ap ip: {ap.ifconfig()[0]}")
    return ap


def get_wlan_status_string(status: int) -> str:
    """Convert WLAN status code to human-readable string."""
    status_map: dict = {
        0: "LINK_DOWN",
        1: "LINK_JOIN",
        2: "LINK_NOIP",
        3: "LINK_UP",
        -1: "LINK_FAIL",
        -2: "LINK_NONET (SSID not found)",
        -3: "LINK_BADAUTH (wrong password)",
    }
    return status_map.get(status, f"UNKNOWN({status})")


def reset_wlan(wlan: network.WLAN) -> None:
    """Full reset of WLAN interface to clear stale state."""
    logger.debug("[WIFI] reset attempt: deinit -> reinit, pm=0xa11140")
    try:
        wlan.disconnect()
    except:
        pass

    # deinit() completely wipes chip state (better than just active(False))
    try:
        wlan.deinit()
    except:
        pass

    time.sleep(1)  # Allow chip to fully power down

    # Re-initialize
    wlan.active(True)
    time.sleep(1)

    # Use documented power management disable value
    try:
        wlan.config(pm=0xa11140)
    except:
        pass

    time.sleep(0.5)


def start_station_mode() -> network.WLAN | None:
    """
    Connect to configured WiFi network.

    Sets the hostname before connecting to enable mDNS discovery.
    Falls back to AP mode if connection times out.

    Returns:
        The STA WLAN interface if connected, None if timed out
    """
    if not config.ssid:
        logger.error("[WIFI] connection failed: no ssid configured")
        return None

    # Set country code for proper channel/power configuration
    rp2.country('US')

    network.hostname(config.device_name)
    wlan = network.WLAN(network.STA_IF)

    max_retries = 3
    # Per-attempt timeout comes from config; NOIP extension grants extra time
    # once association succeeded and only DHCP remains.
    per_attempt_timeout = config.connect_timeout_seconds
    noip_extension = 15  # Extra time if we reach LINK_NOIP state

    for attempt in range(1, max_retries + 1):
        logger.debug(f"[WIFI] connection attempt: {attempt}/{max_retries}")

        # Full reset before each attempt
        reset_wlan(wlan)

        # Retries never move the step counter backward (set_startup_step is
        # monotonic); attempts >1 read as "Retry n/3" plus the attempt dots.
        scan_op = "WiFi scan" if attempt == 1 else f"Retry {attempt}/{max_retries}"
        connect_op = "Connecting" if attempt == 1 else f"Retry {attempt}/{max_retries}"

        # Scan for available networks
        logger.debug("[WIFI] scan started")
        update_startup_display(2, scan_op, "Scanning...", attempt, max_retries)
        target_found = False
        try:
            networks = wlan.scan()
            update_startup_display(2, scan_op, f"Found {len(networks)}", attempt, max_retries)
            for net in networks:
                ssid = net[0].decode('utf-8', 'replace')
                if ssid == config.ssid:
                    target_found = True
            logger.debug(f"[WIFI] scan complete: found={len(networks)}, target_visible={target_found}")
        except Exception as e:
            logger.error(f"[WIFI] scan failed: {e}")
            update_startup_display(2, scan_op, "Scan failed", attempt, max_retries)

        logger.debug(f"[WIFI] connecting to ssid={config.ssid}")
        # Show SSID in detail line (up to 20 chars)
        ssid_display = config.ssid[:20] if len(config.ssid) > 20 else config.ssid
        update_startup_display(3, connect_op, ssid_display, attempt, max_retries)

        wlan.connect(config.ssid, config.password)

        start = time.time()
        last_status = None
        status_history = []
        reached_noip = False
        retry_connect_count = 0

        while not wlan.isconnected():
            elapsed = time.time() - start
            status = wlan.status()

            # Track status changes for summary output
            if status != last_status:
                status_history.append(get_wlan_status_string(status))
                last_status = status

                # Track if we've reached LINK_NOIP (connected, waiting for DHCP)
                if status == 2:  # LINK_NOIP
                    reached_noip = True

            # Handle BADAUTH - break to try next attempt
            if status == -3:  # LINK_BADAUTH
                logger.error("[WIFI] auth failed: bad_auth detected, clearing password")
                app.setup_reason = "bad_auth"
                time.sleep(1)
                break

            # Handle early LINK_FAIL - retry connect within same attempt
            if status == -1 and elapsed < 5 and retry_connect_count < 2:
                retry_connect_count += 1
                logger.debug(f"[WIFI] early fail retry: attempt={retry_connect_count}/2")
                wlan.connect(config.ssid, config.password)
                time.sleep(1)
                continue

            # Calculate effective timeout (extended if we're in NOIP state)
            effective_timeout = per_attempt_timeout
            if reached_noip:
                effective_timeout = per_attempt_timeout + noip_extension

            if elapsed > effective_timeout:
                break

            time.sleep(0.5)

        # Check for successful connection with valid IP
        if wlan.isconnected():
            ip = wlan.ifconfig()[0]
            if ip and ip != '0.0.0.0':
                status_str = ' -> '.join(status_history) if status_history else 'DIRECT'
                logger.debug(f"[WIFI] status: {status_str}")
                logger.debug(f"[WIFI] connected: ip={ip}")
                logger.debug(f"[WIFI] hostname: {config.device_name}.local")
                update_startup_display(4, "Connected", ip)
                app.wlan = wlan
                return wlan
            else:
                logger.debug("[WIFI] connected but no valid ip, retrying")

    # All retries exhausted
    logger.error(f"[WIFI] all attempts failed: {max_retries} retries exhausted")
    update_startup_display(4, "WiFi", "FAILED")
    wlan.active(False)
    return None


def start_display_thread(display: Hub75Display, writer: FontWriter, regions: Regions,
                         health: ThreadHealth) -> None:
    """
    Spawn display loop on Core 1.

    The display thread runs independently of the networking thread,
    ensuring smooth display updates even during network blocking operations.

    Args:
        display: Hub75Display instance (pre-initialized)
        writer: FontWriter instance (pre-initialized)
        regions: Pre-allocated framebuffer regions for all text slots
        health: Shared health signals the watchdog feeder monitors
    """
    def wrapper():
        try:
            health.healthy = True
            logger.debug("[DISPLAY] thread started: core=1")
            run_display_thread(display, writer, regions, health)
        except Exception as e:
            logger.error(f"[DISPLAY] thread crashed: {type(e).__name__}: {e}")
            health.healthy = False

    _thread.start_new_thread(wrapper, ())


# Set by watchdog_feeder when armed; ota_check_task feeds it during the
# blocking OTA download (which would otherwise starve the feeder past the
# 8s hardware limit and reset the device mid-download).
_wdt = None


async def watchdog_feeder(cfg: Config, health: ThreadHealth) -> None:
    """
    Arm and feed the hardware watchdog while both cores are healthy.

    Unlike a software watchdog task, the hardware WDT survives the death of
    its feeder: if Core 0's asyncio loop dies or wedges (the overnight
    failure mode), feeds simply stop and the chip resets itself. The feeder
    also stops feeding *deliberately* — after flushing the log ring to
    flash — when Core 1 has crashed (`healthy` False) or hung
    (`frame_seq` stalled).

    Only started when config `watchdog.enabled` is true: an armed WDT can't
    be disarmed and will reboot the device shortly after mpremote interrupts
    the script, so it stays opt-in for dev workflows.
    """
    from machine import WDT

    global _wdt
    timeout_ms = cfg.watchdog_timeout_ms
    wdt = WDT(timeout=timeout_ms)
    _wdt = wdt
    logger.debug(f"[WD] hardware watchdog armed: timeout={timeout_ms}ms")

    feed_interval = timeout_ms // 4
    last_frame_seq = -1

    while True:
        await asyncio.sleep_ms(feed_interval)

        if not health.healthy:
            reason = "display thread crashed"
        elif health.frame_seq == last_frame_seq:
            reason = "display thread hung (frame_seq stalled)"
        else:
            last_frame_seq = health.frame_seq
            wdt.feed()
            continue

        logger.error(f"[WD] starving watchdog: {reason}; hardware reset in <={timeout_ms}ms")
        logger.flush_to_flash()
        return  # stop feeding; the WDT resets the device


async def log_flush_task() -> None:
    """Periodically give the logger a chance to persist the ring to flash.

    The policy (error-triggered with rate limit, hourly heartbeat) lives in
    logger.maybe_flush(); this task just ticks it from Core 0.
    """
    while True:
        await asyncio.sleep(30)
        logger.maybe_flush()


async def ota_check_task(cfg: Config) -> None:
    """OTA check loop: daily when healthy, hourly after a failed check, and
    immediately when request_ota_check() signals _ota_check_event. Stages a
    new app image if the backend has one, then reboots — the partition
    rewrite itself only ever happens at early boot (see ota.py). The
    download is synchronous and blocks Core 0 for ~10s; Core 1 keeps
    rendering, and the watchdog is hand-fed via the tick.

    Once a download starts, the screen switches to the 'updating' mode
    (progress bar, then a restart countdown). The countdown deliberately
    uses blocking time.sleep: with no await between the first 'updating'
    commit and machine.reset(), the poller can never repaint the screen.
    """
    global _ota_task_started
    _ota_task_started = True

    def wdt_tick():
        if _wdt is not None:
            _wdt.feed()

    # Let boot traffic settle first; an on-demand request skips the wait.
    try:
        await asyncio.wait_for(_ota_check_event.wait(), 120)
    except asyncio.TimeoutError:
        pass

    while True:
        _ota_check_event.clear()
        delay = 24 * 3600
        if cfg.ota_enabled:
            prev_mode = None
            last_pct = -1

            def on_progress(done: int, total: int, sha: str) -> None:
                # Commit only on percent changes (~100 commits per image, vs
                # one per 4 KB chunk). Lazy mode entry: the daily "already
                # current" manifest check never touches the screen.
                nonlocal prev_mode, last_pct
                pct = done * 100 // total if total else 0
                if pct == last_pct:
                    return
                if prev_mode is None:
                    prev_mode = get_write_state().mode
                last_pct = pct
                set_updating_progress(pct, sha[:7])

            try:
                if ota.check_and_stage(cfg.api_url, cfg.api_key, tick=wdt_tick, progress=on_progress):
                    logger.error("[OTA] update staged; rebooting to apply")
                    logger.flush_to_flash()
                    for n in range(5, 0, -1):
                        set_updating_countdown(n)
                        wdt_tick()
                        time.sleep(1)
                    machine.reset()
                else:
                    logger.debug("[OTA] dev deploy marker; check skipped"
                                 if ota.dev_marker() else "[OTA] app is current")
            except Exception as e:
                logger.error(f"[OTA] check failed: {type(e).__name__}: {e}")
                if prev_mode is not None:
                    # Failed mid-download: put the previous screen back
                    # (carry-forward kept its content); the poller repaints
                    # within a poll interval as backup.
                    set_mode(prev_mode)
                delay = 3600  # transient failure: retry sooner than daily
        try:
            await asyncio.wait_for(_ota_check_event.wait(), delay)
        except asyncio.TimeoutError:
            pass


class LightSensor:
    """
    Owns the VEML7700 and its runtime re-init retry state.

    read_lux() returns None while the sensor is unavailable and transparently
    retries initialization every RETRY_TICKS calls (3s at the auto-brightness
    tick rate).
    """

    RETRY_TICKS = 15

    def __init__(self, i2c: I2C, cfg: Config) -> None:
        self._i2c = i2c
        self._config = cfg
        self._retry_ticks = 0
        self._read_failing = False
        self._sensor: VEML7700 | None = self._try_init()

    def _try_init(self) -> VEML7700 | None:
        try:
            sensor = VEML7700(i2c=self._i2c, it=100, gain=1)
            logger.debug("[MAIN] sensor ok: veml7700")
            return sensor
        except Exception as e:
            logger.error(f"[MAIN] sensor init failed: veml7700 {e}")
            return None

    def read_lux(self) -> float | None:
        if self._sensor is None:
            self._retry_ticks += 1
            if self._retry_ticks >= self.RETRY_TICKS:
                self._retry_ticks = 0
                self._sensor = self._try_init()
            return None

        try:
            lux = self._sensor.read_lux()
            if self._read_failing:
                self._read_failing = False
                logger.error("[BRIGHT] sensor recovered")
            return lux
        except Exception as e:
            # Log the failing->recovered transitions, not every 200ms tick —
            # a broken sensor would otherwise flood the log ring and evict
            # the history that matters.
            if not self._read_failing:
                self._read_failing = True
                logger.error(f"[BRIGHT] sensor read failing: {e}")
            return None


async def auto_brightness_loop(driver, cfg: Config, sensor: LightSensor) -> None:
    """
    Periodically read ambient light and update display brightness.

    Sole owner of driver.set_brightness(). Reads config.brightness
    as the user preference on every tick.
    """
    smoothed_lux = 0.0
    ambient_bri = cfg.brightness / 100.0
    initialized = False

    logger.debug("[DISPLAY] auto-brightness started")

    while True:
        lux = sensor.read_lux()
        if lux is not None:
            if not initialized:
                smoothed_lux = lux
                initialized = True
            else:
                smoothed_lux = brightness.smooth_lux(smoothed_lux, lux)

        # Without a reading yet (sensor absent/failed), assume a bright room
        # rather than dimming to the floor.
        if initialized:
            target_ambient = brightness.lux_to_ambient(smoothed_lux)
        else:
            target_ambient = brightness.BRI_MAX
        ambient_bri = brightness.ramp(ambient_bri, target_ambient)
        final = brightness.apply_preference(ambient_bri, cfg.brightness)

        driver.set_brightness(final)

        await asyncio.sleep_ms(brightness.TICK_MS)


def init_buttons() -> tuple[Button | None, Button | None]:
    """
    Create the two physical buttons on PIO1 (PIO0 belongs to the HUB75 driver).

    Both share PIO1's program memory (button.py reuses the loaded program per
    block); SM0 = Button A (skip), SM1 = Button B (lock). Returns (None, None)
    if init fails — buttons are an enhancement, never boot-blocking.
    """
    try:
        pio1 = rp2.PIO(1)
        btn_skip = Button(pin=Pin(10, Pin.IN, Pin.PULL_UP), pio=pio1, sm_offset=0)
        btn_lock = Button(pin=Pin(22, Pin.IN, Pin.PULL_UP), pio=pio1, sm_offset=1)
        logger.debug("[INPUT] buttons initialized: A=skip(GPIO10) B=lock(GPIO22) pio=1")
        return btn_skip, btn_lock
    except Exception as e:
        logger.error(f"[INPUT] button init failed: {e}")
        return None, None


# Hold a button this long for the league-level action (league skip / league
# lock). The threshold check runs on the 50ms poll, so the long action fires
# while the button is STILL HELD (~800-850ms in) — immediate feedback — and
# the short action moves to the release edge so the two can't both fire.
_LONG_PRESS_MS = 800


class _PressTracker:
    """Fold one button's event stream into short/long press callbacks.

    Edge-triggered against the previous debounced state (a swallowed
    sub-debounce blip surfaces as two same-state events and produces no
    edge). Short press fires on RELEASE (only if the hold stayed under the
    long threshold); the long action fires mid-hold the moment the
    threshold passes, checked per poll iteration — button.py emits no
    events while a button is steadily held, so this must be time-polled.
    """

    def __init__(self, button: Button, name: str, on_short, on_long) -> None:
        self._button = button
        self._name = name
        self._on_short = on_short
        self._on_long = on_long
        self._state = button.initial
        self._press_ms: int | None = None  # None = released or consumed

    def poll(self) -> None:
        for ev in self._button.read():
            if ev.pressed and not self._state.pressed:
                self._press_ms = ev.ticks_ms
            elif not ev.pressed and self._state.pressed:
                if self._press_ms is not None:
                    logger.debug(f"[INPUT] {self._name}: short press")
                    self._on_short()
                self._press_ms = None
            self._state = ev

        if (
            self._press_ms is not None
            and self._state.pressed
            and time.ticks_diff(time.ticks_ms(), self._press_ms) >= _LONG_PRESS_MS
        ):
            logger.debug(f"[INPUT] {self._name}: long press")
            self._on_long()
            self._press_ms = None  # consumed: the release won't fire short


async def button_input_loop(poller: GamePoller, btn_skip: Button, btn_lock: Button) -> None:
    """
    Poll both buttons and dispatch press edges to the poller.

    Button A (skip): short = next game, long = next league.
    Button B (lock): short = freeze rotation on this game, long = restrict
    rotation to this game's league.

    The 50ms poll period is well inside the 4-event FIFO's tolerance.
    """
    trackers = (
        _PressTracker(btn_skip, "button A", poller.skip, poller.skip_league),
        _PressTracker(btn_lock, "button B", poller.toggle_lock, poller.toggle_league_lock),
    )

    while True:
        for tracker in trackers:
            tracker.poll()
        await asyncio.sleep_ms(50)


async def main(regions: Regions, driver: Hub75Driver, health: ThreadHealth, light_sensor: LightSensor) -> None:
    """Main entry point. Display thread is already running by the time we get here."""
    # Give the logger a Core 0 heartbeat for its flash-flush policy
    asyncio.create_task(log_flush_task())

    # Start auto-brightness (runs in all modes)
    asyncio.create_task(auto_brightness_loop(driver, config, light_sensor))

    if not config.ssid:
        # No network configured - fresh setup mode
        app.setup_mode = True
        app.setup_reason = "no_network_configured"
        ap = start_ap_mode()
        ap_ip = ap.ifconfig()[0]
        logger.debug(f"[MAIN] mode change: startup -> setup (reason=no_network_configured, ap_ssid={config.device_name}, ap_ip={ap_ip})")
        # Explicit transition: startup → setup
        finish_startup('setup',
            reason="no_config",
            ap_ssid=config.device_name,
            ap_ip=ap_ip
        )
        _resize_setup_regions_for_qr(regions)
        asyncio.create_task(run_dns_server(ap_ip))
    else:
        # Try to connect to configured network
        wlan = start_station_mode()
        if wlan is None:
            # Connection failed - emergency setup mode
            app.setup_mode = True
            # app.setup_reason may already be set to "bad_auth" from the connection loop
            if app.setup_reason is None:
                app.setup_reason = "connection_failed"
            ap = start_ap_mode()
            ap_ip = ap.ifconfig()[0]
            logger.debug(f"[MAIN] mode change: startup -> setup (reason={app.setup_reason}, ap_ssid={config.device_name}, ap_ip={ap_ip})")
            # Explicit transition: startup → setup
            finish_startup('setup',
                reason=app.setup_reason,
                ap_ssid=config.device_name,
                ap_ip=ap_ip,
                wifi_ssid=config.ssid
            )
            _resize_setup_regions_for_qr(regions)
            asyncio.create_task(run_dns_server(ap_ip))
        else:
            # Normal operation - sync time then start services
            app.setup_mode = False
            app.setup_reason = None

            # Sync RTC from backend for accurate timestamps
            utc_offset = await _sync_time_from_backend()

            update_startup_display(5, "Starting", "Services")
            logger.debug(f"[MAIN] mode change: startup -> idle (time_sync_ok={utc_offset is not None})")
            # Explicit transition: startup → idle
            finish_startup('idle')

            api_client = ScoreboardApiClient(config)
            logo_pool = LogoPool(api_client)
            # utc_offset is None when the time sync failed; the poller then
            # omits local start times rather than show a wrong-tz one.
            sources = sources_from_config(config)
            poller = GamePoller(config, api_client, logo_pool, sources, utc_offset)
            asyncio.create_task(poller.run())
            logger.debug(f"[MAIN] game poller task started ({len(sources)} league sources)")

            # OTA app-update checks need the network; station mode only
            asyncio.create_task(ota_check_task(config))

            # Physical buttons drive the poller (skip / rotation lock)
            btn_skip, btn_lock = init_buttons()
            if btn_skip is not None and btn_lock is not None:
                asyncio.create_task(button_input_loop(poller, btn_skip, btn_lock))

    # The app reached an interactive state (idle rotation or setup portal):
    # this boot was not a crash, so the crash-loop counter starts over.
    _clear_boot_fails()

    # Arm the hardware watchdog once the (blocking, slow) network phase is
    # behind us — from here on, everything is cooperative tasks the feeder
    # can vouch for. The boot/WiFi phase stays unprotected by design.
    if config.watchdog_enabled:
        asyncio.create_task(watchdog_feeder(config, health))
    else:
        logger.debug("[WD] hardware watchdog disabled (config watchdog.enabled=false)")

    # The web server must never take Core 0 down with it: retry on exit or
    # exception. A genuinely wedged loop is the watchdog's job, not ours.
    while True:
        logger.debug("[MAIN] web server starting: port=80")
        try:
            await app.start_server(port=80)
            logger.error("[MAIN] web server exited; restarting in 5s")
        except Exception as e:
            logger.error(f"[MAIN] web server crashed: {type(e).__name__}: {e}; restarting in 5s")
        await asyncio.sleep(5)


def _reset_cause_name() -> str:
    """Human-readable machine.reset_cause(). Note: on RP2 chips machine.reset()
    itself resets via the watchdog, so soft resets also report as WDT — the
    flushed log's final lines disambiguate."""
    cause = machine.reset_cause()
    if cause == machine.PWRON_RESET:
        return "power_on"
    if cause == machine.WDT_RESET:
        return "watchdog_or_soft_reset"
    return f"unknown({cause})"


if __name__ == '__main__':
    # Preserve the previous session's flushed log before anything else can
    # write, and stamp this boot with its reset cause for post-mortems.
    logger.rotate_boot_log()
    logger.debug(f"[MAIN] boot: reset_cause={_reset_cause_name()}")
    logger.debug(f"[MAIN] app version: {APP_VERSION[:12] if APP_VERSION else 'dev (littlefs)'}")

    # Initialize display hardware and rendering primitives. All Core-0-only
    # setup (glyph caches, UI colors, state) happens here before the display
    # thread is spawned — once the thread is running, Core 0 must not touch
    # the framebuffer directly.
    logger.debug("[MAIN] display init started")
    # Seed the screen-layout variant selectors from config BEFORE Regions are
    # built (regions not registered yet -> selectors only), then register the
    # built Regions so later config saves can rebuild them live. Dividers and
    # scroll speed are plain screen_geometry globals — seed them here too.
    from scoreboard.state import (
        update_screen_variants, update_show_dividers, update_scroll_speed,
        set_display_regions,
    )
    update_screen_variants(config)
    update_show_dividers(config)
    update_scroll_speed(config)
    driver, display, writer, regions = init_display(config)
    set_display_driver(driver)
    set_display_regions(regions)
    logger.debug("[MAIN] display initialized")

    # Pre-compute UI colors so the first rendered frame has correct colors
    # rather than white defaults.
    from scoreboard.state import update_ui_colors
    update_ui_colors(config)
    logger.debug("[MAIN] ui colors initialized")

    # Light sensor for auto-brightness (owns its own runtime re-init retries).
    _i2c = I2C(0, sda=Pin(0), scl=Pin(1), freq=100000)
    light_sensor = LightSensor(_i2c, config)

    # Commit the first startup step to state. The display thread (spawned
    # next) will render it on its first tick.
    update_startup_display(1, "Display", "Initialized")

    # Spawn the Core-1 render loop. From this point on, Core 0 only mutates
    # state; Core 1 owns the framebuffer.
    display_health = ThreadHealth()
    start_display_thread(display, writer, regions, display_health)

    # Last-resort supervisor: if Core 0's loop ever dies (the overnight
    # failure), record the evidence and restart instead of leaving a
    # half-alive device. The sleep keeps mpremote interruptible and
    # throttles flash writes if a crash loop develops.
    try:
        asyncio.run(main(regions, driver, display_health, light_sensor))
        logger.error("[MAIN] core 0 event loop exited unexpectedly")
    except Exception as e:
        logger.error(f"[MAIN] core 0 crashed: {type(e).__name__}: {e}")
    logger.flush_to_flash()
    time.sleep(10)
    machine.reset()
