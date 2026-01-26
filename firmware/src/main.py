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
from lib.microdot import Microdot, Response, send_file
from lib.scoreboard import Config
from lib.scoreboard.api_client import ScoreboardApiClient
from lib.scoreboard.state import set_mode
from lib.dns import run_dns_server
from lib.api import create_api
from lib.display_loop import display_loop
from lib.api_poller import api_polling_loop

# Reduce buffer size for memory-constrained environment
Response.send_file_buffer_size = 512

app = Microdot()
config = Config()

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

    response = send_file('/index.html.gz', content_type='text/html', compressed='gzip')

    # Add cache control if configured
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
        target_found = False
        try:
            networks = wlan.scan()
            print(f"Found {len(networks)} networks:")
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

        print(f"Connecting to '{config.ssid}'...")

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
                app.wlan = wlan
                return wlan
            else:
                print("Connected but no valid IP, retrying...")

    # All retries exhausted
    print(f"All {max_retries} connection attempts failed")
    wlan.active(False)
    return None


async def main():
    """Main entry point."""
    # Start display loop (runs in all modes)
    asyncio.create_task(display_loop(config))

    if not config.ssid:
        # No network configured - fresh setup mode
        print("No WiFi configured. Starting setup mode...")
        app.setup_mode = True
        app.setup_reason = "no_network_configured"
        set_mode('setup')
        ap = start_ap_mode()
        asyncio.create_task(run_dns_server(ap.ifconfig()[0]))
    else:
        # Try to connect to configured network
        wlan = start_station_mode()
        if wlan is None:
            # Connection failed - emergency setup mode
            print("Connection failed. Starting setup mode...")
            app.setup_mode = True
            app.setup_reason = "connection_failed"
            set_mode('error', 'WiFi Failed')
            ap = start_ap_mode()
            asyncio.create_task(run_dns_server(ap.ifconfig()[0]))
        else:
            # Normal operation - start API polling
            app.setup_mode = False
            app.setup_reason = None
            asyncio.create_task(api_polling_loop(config, api_client))

    print("Starting web server on port 80...")
    await app.start_server(port=80)


if __name__ == '__main__':
    asyncio.run(main())
