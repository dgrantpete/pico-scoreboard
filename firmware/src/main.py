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
# Button A: GPIO 10 (skip to next game)
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
from scoreboard.state import set_startup_step, finish_startup, set_display_driver, get_write_state
from scoreboard.dns import run_dns_server
from scoreboard.api_routes import create_api
from scoreboard.display import init_display, run_display_thread, Regions, LogoPool
from scoreboard.fonts import FontWriter
from hub75 import Hub75Display, Hub75Driver
from machine import I2C, Pin
from veml7700 import VEML7700
from button import Button
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
        if config.log_level >= DEBUG:
            print(f"[TIME] sync started: url={url}")

        async def _fetch() -> int | None:
            async with aiohttp.ClientSession() as session:
                async with session.get(url, ssl=True) as resp:
                    if resp.status != 200:
                        if config.log_level >= ERROR:
                            print(f"[TIME] sync failed: http={resp.status}")
                        return None

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

        return await asyncio.wait_for(_fetch(), 15)
    except Exception as e:
        if config.log_level >= ERROR:
            print(f"[TIME] sync failed: {e}")
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
    # Per-attempt timeout comes from config; NOIP extension grants extra time
    # once association succeeded and only DHCP remains.
    per_attempt_timeout = config.connect_timeout_seconds
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


class ThreadHealth:
    """
    Cross-core health flag for the display thread.

    Core 1 writes `healthy`, Core 0's watchdog reads it. Passing this object
    into both sides makes the shared state explicit in the signatures instead
    of hiding it at module scope.
    """

    def __init__(self) -> None:
        self.healthy: bool = False


def start_display_thread(display: Hub75Display, writer: FontWriter, regions: Regions,
                         cfg: Config, health: ThreadHealth) -> None:
    """
    Spawn display loop on Core 1.

    The display thread runs independently of the networking thread,
    ensuring smooth display updates even during network blocking operations.

    Args:
        display: Hub75Display instance (pre-initialized)
        writer: FontWriter instance (pre-initialized)
        regions: Pre-allocated framebuffer regions for all text slots
        cfg: Config instance for UI colors
        health: Shared health flag the watchdog monitors
    """
    def wrapper():
        try:
            health.healthy = True
            if cfg.log_level >= DEBUG:
                print("[DISPLAY] thread started: core=1")
            run_display_thread(display, writer, regions, cfg)
        except Exception as e:
            if cfg.log_level >= ERROR:
                print(f"[DISPLAY] thread crashed: {e}")
            health.healthy = False

    _thread.start_new_thread(wrapper, ())


async def watchdog_task(health: ThreadHealth) -> None:
    """
    Monitor display thread health and reset device if it crashes.

    Checks the display thread health every 30 seconds. If the thread
    is unhealthy, triggers a device reset to recover.
    """
    await asyncio.sleep(10)  # Initial delay to let things stabilize

    while True:
        await asyncio.sleep(30)
        if not health.healthy:
            if config.log_level >= ERROR:
                print("[DISPLAY] watchdog triggered: unhealthy, resetting")
            await asyncio.sleep(1)
            machine.reset()


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
        self._sensor: VEML7700 | None = self._try_init()

    def _try_init(self) -> VEML7700 | None:
        try:
            sensor = VEML7700(i2c=self._i2c, it=100, gain=1)
            if self._config.log_level >= DEBUG:
                print("[MAIN] sensor ok: veml7700")
            return sensor
        except Exception as e:
            if self._config.log_level >= ERROR:
                print(f"[MAIN] sensor init failed: veml7700 {e}")
            return None

    def read_lux(self) -> float | None:
        if self._sensor is None:
            self._retry_ticks += 1
            if self._retry_ticks >= self.RETRY_TICKS:
                self._retry_ticks = 0
                self._sensor = self._try_init()
            return None

        try:
            return self._sensor.read_lux()
        except Exception as e:
            if self._config.log_level >= ERROR:
                print(f"[DISPLAY] brightness sensor error: {e}")
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

    if cfg.log_level >= DEBUG:
        print("[DISPLAY] auto-brightness started")

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
        if config.log_level >= DEBUG:
            print("[INPUT] buttons initialized: A=skip(GPIO10) B=lock(GPIO22) pio=1")
        return btn_skip, btn_lock
    except Exception as e:
        if config.log_level >= ERROR:
            print(f"[INPUT] button init failed: {e}")
        return None, None


async def button_input_loop(poller: MlbPoller, btn_skip: Button, btn_lock: Button) -> None:
    """
    Poll both buttons and dispatch press edges to the poller.

    Folds the drivers' event streams per button.py's contract: a rising edge
    (previous state released, new state pressed) triggers the action; repeated
    same-state events (swallowed sub-debounce blips) are ignored. The 50ms
    poll period is well inside the 4-event FIFO's tolerance.
    """
    skip_state = btn_skip.initial
    lock_state = btn_lock.initial

    while True:
        for ev in btn_skip.read():
            if ev.pressed and not skip_state.pressed:
                if config.log_level >= DEBUG:
                    print("[INPUT] button A pressed: skip")
                poller.skip()
            skip_state = ev

        for ev in btn_lock.read():
            if ev.pressed and not lock_state.pressed:
                if config.log_level >= DEBUG:
                    print("[INPUT] button B pressed: toggle lock")
                poller.toggle_lock()
            lock_state = ev

        await asyncio.sleep_ms(50)


async def main(regions: Regions, driver: Hub75Driver, health: ThreadHealth, light_sensor: LightSensor) -> None:
    """Main entry point. Display thread is already running by the time we get here."""
    # Start watchdog to monitor display thread health
    asyncio.create_task(watchdog_task(health))

    # Start auto-brightness (runs in all modes)
    asyncio.create_task(auto_brightness_loop(driver, config, light_sensor))

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
                reason=app.setup_reason,
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
                print(f"[MAIN] mode change: startup -> idle (time_sync_ok={utc_offset is not None})")
            # Explicit transition: startup → idle
            finish_startup('idle')

            api_client = ScoreboardApiClient(config)
            logo_pool = LogoPool(api_client)
            poller = MlbPoller(config, api_client, logo_pool)
            asyncio.create_task(poller.run())
            if config.log_level >= DEBUG:
                print("[MAIN] mlb poller task started")

            # Physical buttons drive the poller (skip / rotation lock)
            btn_skip, btn_lock = init_buttons()
            if btn_skip is not None and btn_lock is not None:
                asyncio.create_task(button_input_loop(poller, btn_skip, btn_lock))

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

    # Pre-compute UI colors so the first rendered frame has correct colors
    # rather than white defaults.
    from scoreboard.state import update_ui_colors
    update_ui_colors(config)
    if config.log_level >= DEBUG:
        print("[MAIN] ui colors initialized")

    # Light sensor for auto-brightness (owns its own runtime re-init retries).
    _i2c = I2C(0, sda=Pin(0), scl=Pin(1), freq=100000)
    light_sensor = LightSensor(_i2c, config)

    # Commit the first startup step to state. The display thread (spawned
    # next) will render it on its first tick.
    update_startup_display(1, "Display", "Initialized")

    # Spawn the Core-1 render loop. From this point on, Core 0 only mutates
    # state; Core 1 owns the framebuffer.
    display_health = ThreadHealth()
    start_display_thread(display, writer, regions, config, display_health)

    asyncio.run(main(regions, driver, display_health, light_sensor))
