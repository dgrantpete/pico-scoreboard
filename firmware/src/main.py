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
# Button A: GPIO 10
# Button B: GPIO 22

"""
Pico Scoreboard Web Server.

Serves the SvelteKit frontend from flash memory and provides
network connectivity.

Automatically enters setup mode (AP) when no WiFi is configured
or when connection to the configured network fails. Once properly
configured, connects to the specified WiFi network.
"""

import network
import time
import machine
import uasyncio as asyncio
import gc
import os
import rp2
import hashlib
import _thread
from microdot import Microdot, Request, Response, send_file
from scoreboard import Config, ScoreboardApiClient
from scoreboard.mlb_poller import MlbPoller
from scoreboard.state import set_startup_step, finish_startup, set_display_driver, get_display_state
from scoreboard.dns import run_dns_server
from scoreboard.api_routes import create_api
from scoreboard.display import init_display, run_display_thread, Regions
from scoreboard.fonts import FontWriter
from hub75 import Hub75Display, Hub75Driver
from machine import I2C, Pin
from veml7700 import VEML7700
from scoreboard import brightness
from scoreboard.logger import DEBUG, ERROR

# Reduce buffer size for memory-constrained environment
Response.send_file_buffer_size = 2048

app: Microdot = Microdot()
config: Config = Config()


# Compute ETag for index.html.gz once at startup
def _compute_index_etag() -> str | None:
    try:
        h = hashlib.sha1()
        with open('/index.html.gz', 'rb') as f:
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

def update_startup_display(step: int, operation: str, detail: str = '') -> None:
    """Update startup progress state. The display thread renders it on its next tick."""
    set_startup_step(step, 5, operation, detail)


def _resize_setup_regions_for_qr(regions: Regions) -> None:
    """
    Resize setup-screen text regions to fit beside the current QR code.

    Must be called on Core 0 after finish_startup('setup', ...) has generated
    the QR. Reads QR dimensions from state (which was just committed by
    finish_startup) and rebuilds the affected Regions so text won't overlap
    the QR footprint.
    """
    state = get_display_state()
    regions.update_for_qr(state.setup.qr_width, state.setup.qr_height)


# Track setup mode state
app.setup_mode = False
app.setup_reason = None  # 'no_network_configured' | 'connection_failed' | None


def get_memory_stats() -> dict:
    """Get current memory usage statistics."""
    gc.collect()  # Run GC first for accurate reading
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
            'configured_ssid': config.ssid if setup_reason == 'connection_failed' else None,
            'ip': None,
            'hostname': None,
            'ap_ip': ap.ifconfig()[0],
            'ap_ssid': config.device_name,
            'memory_used': memory['memory_used'],
            'memory_free': memory['memory_free'],
            'flash_used': memory['flash_used'],
            'flash_free': memory['flash_free']
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
            'flash_free': memory['flash_free']
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
            'flash_free': memory['flash_free']
        }


async def _sync_time_from_backend() -> int:
    """
    Fetch current time from the backend API and set the Pico's RTC.

    Returns:
        UTC offset in seconds for local time display (0 if unknown).
    """
    import aiohttp

    try:
        url = f"{config.api_url.rstrip('/')}/time"
        if config.log_level >= DEBUG:
            print(f"[TIME] sync started: url={url}")
        async with aiohttp.ClientSession() as session:
            async with session.get(url, ssl=True) as resp:
                if resp.status != 200:
                    if config.log_level >= ERROR:
                        print(f"[TIME] sync failed: http={resp.status}")
                    return 0

                data = await resp.json()
                unix_ts = data['timestamp']
                utc_offset = data.get('utc_offset') or 0

                tm = time.gmtime(unix_ts)
                # gmtime returns: (year, month, mday, hour, minute, second, weekday, yearday)
                # RTC.datetime expects: (year, month, day, weekday, hours, minutes, seconds, subseconds)
                machine.RTC().datetime((tm[0], tm[1], tm[2], tm[6], tm[3], tm[4], tm[5], 0))
                if config.log_level >= DEBUG:
                    print(f"[TIME] rtc synced: {tm[0]:04d}-{tm[1]:02d}-{tm[2]:02d} {tm[3]:02d}:{tm[4]:02d}:{tm[5]:02d} UTC offset={utc_offset}s")
                return utc_offset
    except Exception as e:
        if config.log_level >= ERROR:
            print(f"[TIME] sync failed: {e}")
        return 0


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

    response = send_file('/index.html.gz', content_type='text/html', compressed='gzip')

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
    if config.log_level >= DEBUG:
        print(f"[WIFI] ap mode started: ssid={config.device_name}")
        print(f"[WIFI] ap ip: {ap.ifconfig()[0]}")
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
    if config.log_level >= DEBUG:
        print("[WIFI] reset attempt: deinit -> reinit, pm=0xa11140")
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
        if config.log_level >= ERROR:
            print("[WIFI] connection failed: no ssid configured")
        return None

    # Set country code for proper channel/power configuration
    rp2.country('US')

    network.hostname(config.device_name)
    wlan = network.WLAN(network.STA_IF)

    max_retries = 3
    per_attempt_timeout = 20  # Base timeout (extended for LINK_NOIP)
    noip_extension = 15  # Extra time if we reach LINK_NOIP state

    for attempt in range(1, max_retries + 1):
        if config.log_level >= DEBUG:
            print(f"[WIFI] connection attempt: {attempt}/{max_retries}")

        # Full reset before each attempt
        reset_wlan(wlan)

        # Scan for available networks
        if config.log_level >= DEBUG:
            print("[WIFI] scan started")
        update_startup_display(2, "WiFi scan", "Scanning...")
        target_found = False
        try:
            networks = wlan.scan()
            update_startup_display(2, "WiFi scan", f"Found {len(networks)}")
            for net in networks:
                ssid = net[0].decode('utf-8', 'replace')
                if ssid == config.ssid:
                    target_found = True
            if config.log_level >= DEBUG:
                print(f"[WIFI] scan complete: found={len(networks)}, target_visible={target_found}")
        except Exception as e:
            if config.log_level >= ERROR:
                print(f"[WIFI] scan failed: {e}")
            update_startup_display(2, "WiFi scan", "Scan failed")

        if config.log_level >= DEBUG:
            print(f"[WIFI] connecting to ssid={config.ssid}")
        # Show SSID in detail line (up to 20 chars), attempt counter in operation
        ssid_display = config.ssid[:20] if len(config.ssid) > 20 else config.ssid
        update_startup_display(3, f"Connecting ({attempt}/{max_retries})", ssid_display)

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
                if config.log_level >= ERROR:
                    print("[WIFI] auth failed: bad_auth detected, clearing password")
                app.setup_reason = "bad_auth"
                time.sleep(1)
                break

            # Handle early LINK_FAIL - retry connect within same attempt
            if status == -1 and elapsed < 5 and retry_connect_count < 2:
                retry_connect_count += 1
                if config.log_level >= DEBUG:
                    print(f"[WIFI] early fail retry: attempt={retry_connect_count}/2")
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
                if config.log_level >= DEBUG:
                    status_str = ' -> '.join(status_history) if status_history else 'DIRECT'
                    print(f"[WIFI] status: {status_str}")
                    print(f"[WIFI] connected: ip={ip}")
                    print(f"[WIFI] hostname: {config.device_name}.local")
                update_startup_display(4, "Connected", ip)
                app.wlan = wlan
                return wlan
            else:
                if config.log_level >= DEBUG:
                    print("[WIFI] connected but no valid ip, retrying")

    # All retries exhausted
    if config.log_level >= ERROR:
        print(f"[WIFI] all attempts failed: {max_retries} retries exhausted")
    update_startup_display(4, "WiFi", "FAILED")
    wlan.active(False)
    return None


# TODO(tech-debt): this module-level bool is an ad-hoc cross-thread sync primitive —
# Core 1 writes it inside start_display_thread's wrapper, Core 0's watchdog_task
# reads it. It functions, but a mutable global as a sync flag is not good practice.
# Replace with an explicit shared-state object (e.g. a small ThreadHealth class
# passed into both start_display_thread and watchdog_task) so ownership shows up
# in the signatures instead of being hidden at module scope. Don't leave this as-is
# long-term.
_display_thread_healthy: bool = False


def start_display_thread(display: Hub75Display, writer: FontWriter, regions: Regions, cfg: Config) -> None:
    """
    Spawn display loop on Core 1.

    The display thread runs independently of the networking thread,
    ensuring smooth display updates even during network blocking operations.

    Args:
        display: Hub75Display instance (pre-initialized)
        writer: FontWriter instance (pre-initialized)
        regions: Pre-allocated framebuffer regions for all text slots
        cfg: Config instance for UI colors
    """
    def wrapper():
        global _display_thread_healthy
        try:
            _display_thread_healthy = True
            if config.log_level >= DEBUG:
                print("[DISPLAY] thread started: core=1")
            run_display_thread(display, writer, regions, config)
        except Exception as e:
            if config.log_level >= ERROR:
                print(f"[DISPLAY] thread crashed: {e}")
            _display_thread_healthy = False

    _thread.start_new_thread(wrapper, ())


async def watchdog_task() -> None:
    """
    Monitor display thread health and reset device if it crashes.

    Checks the display thread health every 30 seconds. If the thread
    is unhealthy, triggers a device reset to recover.
    """
    global _display_thread_healthy
    await asyncio.sleep(10)  # Initial delay to let things stabilize

    while True:
        await asyncio.sleep(30)
        if not _display_thread_healthy:
            if config.log_level >= ERROR:
                print("[DISPLAY] watchdog triggered: unhealthy, resetting")
            await asyncio.sleep(1)
            machine.reset()


def _try_init_veml(i2c: I2C) -> VEML7700 | None:
    """Attempt to create VEML7700 sensor. Returns None on failure."""
    try:
        sensor = VEML7700(i2c=i2c, it=100, gain=1)
        if config.log_level >= DEBUG:
            print("[MAIN] sensor ok: veml7700")
        return sensor
    except Exception as e:
        if config.log_level >= ERROR:
            print(f"[MAIN] sensor init failed: veml7700 {e}")
        return None


# TODO(tech-debt): these are module-level globals because auto_brightness_loop
# retries VEML7700 init at runtime by mutating _light_sensor. That mutation makes
# them true shared runtime state rather than boot-time constants, but they'd be
# cleaner as a small LightSensor class that owns its own retry state and i2c bus.
# Refactor when touching auto_brightness_loop next; don't leave this as-is long-term.
_i2c: I2C | None = None
_light_sensor: VEML7700 | None = None


async def auto_brightness_loop(driver, cfg: Config) -> None:
    """
    Periodically read ambient light and update display brightness.

    Sole owner of driver.set_brightness(). Reads config.brightness
    as the user preference on every tick.
    """
    global _light_sensor

    smoothed_lux = 0.0
    ambient_bri = cfg.brightness / 100.0
    initialized = False
    retry_ticks = 0

    if cfg.log_level >= DEBUG:
        print("[DISPLAY] auto-brightness started")

    while True:
        # Retry sensor init every 3s (15 ticks) if not available
        if _light_sensor is None:
            retry_ticks += 1
            if retry_ticks >= 15 and _i2c is not None:
                retry_ticks = 0
                _light_sensor = _try_init_veml(_i2c)
        else:
            # Read sensor
            try:
                lux = _light_sensor.read_lux()
                if not initialized:
                    smoothed_lux = lux
                    initialized = True
                else:
                    smoothed_lux = brightness.smooth_lux(smoothed_lux, lux)
            except Exception as e:
                if cfg.log_level >= ERROR:
                    print(f"[DISPLAY] brightness sensor error: {e}")

        # Compute brightness
        target_ambient = brightness.lux_to_ambient(smoothed_lux)
        ambient_bri = brightness.ramp(ambient_bri, target_ambient)
        final = brightness.apply_preference(ambient_bri, cfg.brightness)

        # Set
        driver.set_brightness(final)

        await asyncio.sleep_ms(200)


async def main(regions: Regions, driver: Hub75Driver) -> None:
    """Main entry point. Display thread is already running by the time we get here."""
    # Start watchdog to monitor display thread health
    asyncio.create_task(watchdog_task())

    # Start auto-brightness (runs in all modes)
    asyncio.create_task(auto_brightness_loop(driver, config))

    if not config.ssid:
        # No network configured - fresh setup mode
        app.setup_mode = True
        app.setup_reason = "no_network_configured"
        ap = start_ap_mode()
        ap_ip = ap.ifconfig()[0]
        if config.log_level >= DEBUG:
            print(f"[MAIN] mode change: startup -> setup (reason=no_network_configured, ap_ssid={config.device_name}, ap_ip={ap_ip})")
        # Explicit transition: startup → setup
        finish_startup('setup',
            reason="no_config",
            ap_ssid=config.device_name,
            ap_ip=ap_ip
        )
        _resize_setup_regions_for_qr(regions)
        asyncio.create_task(run_dns_server(config, ap_ip))
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
            if config.log_level >= DEBUG:
                print(f"[MAIN] mode change: startup -> setup (reason={app.setup_reason}, ap_ssid={config.device_name}, ap_ip={ap_ip})")
            # Explicit transition: startup → setup
            finish_startup('setup',
                reason=app.setup_reason if app.setup_reason == "bad_auth" else "connection_failed",
                ap_ssid=config.device_name,
                ap_ip=ap_ip,
                wifi_ssid=config.ssid
            )
            _resize_setup_regions_for_qr(regions)
            asyncio.create_task(run_dns_server(config, ap_ip))
        else:
            # Normal operation - sync time then start services
            app.setup_mode = False
            app.setup_reason = None

            # Sync RTC from backend for accurate timestamps
            utc_offset = await _sync_time_from_backend()

            update_startup_display(5, "Starting", "Services")
            if config.log_level >= DEBUG:
                print(f"[MAIN] mode change: startup -> idle (time_sync_ok={utc_offset != 0})")
            # Explicit transition: startup → idle
            finish_startup('idle')

            api_client = ScoreboardApiClient(config)
            poller = MlbPoller(config, api_client)
            asyncio.create_task(poller.run())
            if config.log_level >= DEBUG:
                print("[MAIN] mlb poller task started")

    if config.log_level >= DEBUG:
        print("[MAIN] web server starting: port=80")
    await app.start_server(port=80)


if __name__ == '__main__':
    # Initialize display hardware and rendering primitives. All Core-0-only
    # setup (glyph caches, UI colors, state) happens here before the display
    # thread is spawned — once the thread is running, Core 0 must not touch
    # the framebuffer directly.
    if config.log_level >= DEBUG:
        print("[MAIN] display init started")
    driver, display, writer, regions = init_display(config)
    set_display_driver(driver)
    if config.log_level >= DEBUG:
        print("[MAIN] display initialized")

    # Pre-cache digit glyphs for zero-allocation score/clock rendering on Core 1.
    from scoreboard.fonts import unscii_16
    writer.init_clock(unscii_16)   # Clock digits + colon
    writer.init_digits(unscii_16)  # Score digits
    if config.log_level >= DEBUG:
        print("[MAIN] glyph caches initialized")

    # Pre-compute UI colors into both state buffers so the first rendered
    # frame has correct colors rather than white defaults.
    from scoreboard.state import update_ui_colors
    update_ui_colors(config)
    if config.log_level >= DEBUG:
        print("[MAIN] ui colors initialized")

    # Initialize light sensor for auto-brightness. See the tech-debt note on
    # _i2c / _light_sensor above — these remain module-level for now.
    _i2c = I2C(0, sda=Pin(0), scl=Pin(1), freq=100000)
    _light_sensor = _try_init_veml(_i2c)

    # Commit the first startup step to state. The display thread (spawned
    # next) will render it on its first tick.
    update_startup_display(1, "Display", "Initialized")

    # Spawn the Core-1 render loop. From this point on, Core 0 only mutates
    # state; Core 1 owns the framebuffer.
    start_display_thread(display, writer, regions, config)

    asyncio.run(main(regions, driver))
