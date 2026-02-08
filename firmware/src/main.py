"""
Pico Scoreboard Web Server.

Serves the SvelteKit frontend from flash memory and provides
network connectivity.

Automatically enters setup mode (AP) when no WiFi is configured
or when connection to the configured network fails. Once properly
configured, connects to the specified WiFi network.
"""

import sys

sys.path.append('lib')

import network
import time
import machine
import uasyncio as asyncio
import gc
import os
import rp2
import hashlib
import _thread
from lib.microdot import Microdot, Response, send_file
from lib.scoreboard import Config
from lib.scoreboard.api_client import ScoreboardApiClient
from lib.scoreboard.state import set_mode, set_startup_step, finish_startup, set_display_driver
from lib.dns import run_dns_server
from lib.api import create_api
from lib.display_loop import init_display, render_startup, get_ui_colors
from lib.display_thread import run_display_thread
from lib.api_poller import api_polling_loop

# Reduce buffer size for memory-constrained environment
Response.send_file_buffer_size = 512

app = Microdot()
config = Config()


# Compute ETag for index.html.gz once at startup
def _compute_index_etag():
    try:
        h = hashlib.sha1()
        with open('/index.html.gz', 'rb') as f:
            while True:
                chunk = f.read(512)
                if not chunk:
                    break
                h.update(chunk)
        # Convert first 8 bytes to hex string (16 chars)
        return ''.join('{:02x}'.format(b) for b in h.digest()[:8])
    except OSError:
        return None


INDEX_ETAG = _compute_index_etag()

# Display components (initialized before asyncio for startup display)
_display = None
_writer = None

def update_startup_display(step, operation, detail=''):
    """
    Update startup progress on the display.

    Called during synchronous startup before async loop takes over.

    Args:
        step: Current step number (1-5)
        operation: Short operation name
        detail: Optional detail text
    """
    global _display, _writer
    if _display is None or _writer is None:
        return

    from lib.scoreboard.state import get_display_state
    set_startup_step(step, 5, operation, detail)
    colors = get_ui_colors(config)
    render_startup(_display, _writer, get_display_state(), colors)
    _display.show()


# Track setup mode state
app.setup_mode = False
app.setup_reason = None  # 'no_network_configured' | 'connection_failed' | None


def get_memory_stats():
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


def get_network_status():
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


# Create API client for backend communication
api_client = ScoreboardApiClient(config)

# Create and mount API under /api prefix
api = create_api(config, get_network_status, api_client)
app.mount(api, url_prefix='/api')


def get_my_hosts(ap):
    """
    Get the set of hostnames that belong to us.
    Built dynamically from config and the provided AP interface.
    """
    hosts = set()

    # Add configured device name (e.g., "scoreboard.local")
    hosts.add(f"{config.device_name}.local")
    hosts.add(config.device_name)  # Some clients might omit .local

    # Add AP IP address
    if ap:
        hosts.add(ap.ifconfig()[0])

    return hosts


@app.get('/')
async def index(request):
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
async def catch_all(request, path):
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


def start_ap_mode():
    """
    Start Access Point mode for initial setup.

    Creates an open WiFi network that users can connect to
    for configuring the device. Stores the AP interface on the
    app object so routes can access it for captive portal logic.

    Returns:
        The AP WLAN interface
    """
    ap = network.WLAN(network.AP_IF)
    ap.config(essid=config.device_name, security=0)  # security=0 means open network
    ap.active(True)

    while not ap.active():
        machine.idle()  # Low-power wait instead of hot loop

    app.ap = ap  # Store on app object for routes to access
    print(f"AP Mode active. Connect to: {config.device_name}")
    print(f"IP: {ap.ifconfig()[0]}")
    return ap


def get_wlan_status_string(status):
    """Convert WLAN status code to human-readable string."""
    status_map = {
        0: "LINK_DOWN",
        1: "LINK_JOIN",
        2: "LINK_NOIP",
        3: "LINK_UP",
        -1: "LINK_FAIL",
        -2: "LINK_NONET (SSID not found)",
        -3: "LINK_BADAUTH (wrong password)",
    }
    return status_map.get(status, f"UNKNOWN({status})")


def reset_wlan(wlan):
    """Full reset of WLAN interface to clear stale state."""
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


def start_station_mode():
    """
    Connect to configured WiFi network.

    Sets the hostname before connecting to enable mDNS discovery.
    Falls back to AP mode if connection times out.

    Returns:
        The STA WLAN interface if connected, None if timed out
    """
    if not config.ssid:
        print("Error: WiFi SSID not configured")
        return None

    # Set country code for proper channel/power configuration
    rp2.country('US')

    network.hostname(config.device_name)
    wlan = network.WLAN(network.STA_IF)

    max_retries = 3
    per_attempt_timeout = 20  # Base timeout (extended for LINK_NOIP)
    noip_extension = 15  # Extra time if we reach LINK_NOIP state

    for attempt in range(1, max_retries + 1):
        print(f"=== WiFi Connection Attempt {attempt}/{max_retries} ===")

        # Full reset before each attempt
        reset_wlan(wlan)

        # Scan for available networks
        print("Scanning...")
        update_startup_display(2, "WiFi scan", "Scanning...")
        target_found = False
        try:
            networks = wlan.scan()
            print(f"Found {len(networks)} networks:")
            update_startup_display(2, "WiFi scan", f"Found {len(networks)}")
            for net in networks:
                ssid = net[0].decode('utf-8', 'replace')
                channel = net[2]
                rssi = net[3]
                is_target = ssid == config.ssid
                if is_target:
                    target_found = True
                marker = "  <-- TARGET" if is_target else ""
                print(f"  '{ssid}' ch:{channel} rssi:{rssi}dBm{marker}")
        except Exception as e:
            print(f"Scan failed: {e}")
            update_startup_display(2, "WiFi scan", "Scan failed")

        print(f"Connecting to '{config.ssid}'...")
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
                print("Authentication failed - wrong password?")
                app.setup_reason = "bad_auth"
                time.sleep(1)
                break

            # Handle early LINK_FAIL - retry connect within same attempt
            if status == -1 and elapsed < 5 and retry_connect_count < 2:
                retry_connect_count += 1
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
                print(f"Status: {status_str}")
                print(f"Connected! IP: {ip}")
                print(f"Hostname: {config.device_name}.local")
                update_startup_display(4, "Connected", ip)
                app.wlan = wlan
                return wlan
            else:
                print("Connected but no valid IP, retrying...")

    # All retries exhausted
    print(f"All {max_retries} connection attempts failed")
    update_startup_display(4, "WiFi", "FAILED")
    wlan.active(False)
    return None


# Display thread health tracking
_display_thread_healthy = False


def start_display_thread(display, writer, cfg):
    """
    Spawn display loop on Core 1.

    The display thread runs independently of the networking thread,
    ensuring smooth display updates even during network blocking operations.

    Args:
        display: Hub75Display instance (pre-initialized)
        writer: FontWriter instance (pre-initialized)
        cfg: Config instance for UI colors
    """
    global _display_thread_healthy

    def wrapper():
        global _display_thread_healthy
        try:
            _display_thread_healthy = True
            print("Display thread started on Core 1")
            run_display_thread(display, writer, cfg)
        except Exception as e:
            print(f"Display thread crashed: {e}")
            _display_thread_healthy = False

    _thread.start_new_thread(wrapper, ())


async def watchdog_task():
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
            print("Display thread unhealthy, resetting device...")
            await asyncio.sleep(1)
            machine.reset()


async def main():
    """Main entry point."""
    global _display, _writer

    # Start display thread on Core 1 (runs in all modes)
    start_display_thread(_display, _writer, config)

    # Start watchdog to monitor display thread health
    asyncio.create_task(watchdog_task())

    if not config.ssid:
        # No network configured - fresh setup mode
        print("No WiFi configured. Starting setup mode...")
        app.setup_mode = True
        app.setup_reason = "no_network_configured"
        ap = start_ap_mode()
        # Explicit transition: startup → setup
        finish_startup('setup',
            reason="no_config",
            ap_ssid=config.device_name,
            ap_ip=ap.ifconfig()[0]
        )
        asyncio.create_task(run_dns_server(ap.ifconfig()[0]))
    else:
        # Try to connect to configured network
        wlan = start_station_mode()
        if wlan is None:
            # Connection failed - emergency setup mode
            print("Connection failed. Starting setup mode...")
            app.setup_mode = True
            # app.setup_reason may already be set to "bad_auth" from the connection loop
            if app.setup_reason is None:
                app.setup_reason = "connection_failed"
            ap = start_ap_mode()
            # Explicit transition: startup → setup
            finish_startup('setup',
                reason=app.setup_reason if app.setup_reason == "bad_auth" else "connection_failed",
                ap_ssid=config.device_name,
                ap_ip=ap.ifconfig()[0],
                wifi_ssid=config.ssid
            )
            asyncio.create_task(run_dns_server(ap.ifconfig()[0]))
        else:
            # Normal operation - start API polling
            app.setup_mode = False
            app.setup_reason = None
            update_startup_display(5, "Starting", "Services")
            # Explicit transition: startup → idle
            finish_startup('idle')
            asyncio.create_task(api_polling_loop(config, api_client))

    print("Starting web server on port 80...")
    await app.start_server(port=80)


if __name__ == '__main__':
    # Initialize display before asyncio for startup progress
    print("Initializing display...")
    _driver, _display, _writer = init_display(config)
    set_display_driver(_driver)
    print("Display initialized")

    # Initialize glyph caches on Core 0 (before display thread starts)
    # This pre-caches digits for zero-allocation rendering
    from lib.fonts import unscii_16
    _writer.init_clock(unscii_16)   # Clock digits + colon
    _writer.init_digits(unscii_16)  # Score digits
    print("Glyph caches initialized")

    # Pre-compute UI colors on Core 0 (before display thread starts)
    from lib.scoreboard.state import update_ui_colors
    update_ui_colors(config)
    print("UI colors initialized")

    # Show first startup step
    update_startup_display(1, "Display", "Initialized")

    asyncio.run(main())
