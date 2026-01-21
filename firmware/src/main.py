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
from lib.microdot import Microdot, Response, send_file
from lib.scoreboard import Config
from lib.scoreboard.api_client import ScoreboardApiClient
from lib.dns import run_dns_server
from lib.api import create_api

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

    network.hostname(config.device_name)
    wlan = network.WLAN(network.STA_IF)
    wlan.active(True)

    print(f"Connecting to '{config.ssid}'...")
    wlan.connect(config.ssid, config.password)

    start = time.time()
    timeout = config.connect_timeout_seconds
    while not wlan.isconnected():
        if time.time() - start > timeout:
            print(f"Connection timeout after {timeout} seconds")
            wlan.active(False)
            return None
        time.sleep(0.5)
        print(".", end="")

    print()
    print(f"Connected! IP: {wlan.ifconfig()[0]}")
    print(f"Hostname: {config.device_name}.local")
    app.wlan = wlan  # Store on app object for API status endpoint
    return wlan


async def main():
    """Main entry point."""
    if not config.ssid:
        # No network configured - fresh setup mode
        print("No WiFi configured. Starting setup mode...")
        app.setup_mode = True
        app.setup_reason = "no_network_configured"
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
            ap = start_ap_mode()
            asyncio.create_task(run_dns_server(ap.ifconfig()[0]))
        else:
            # Normal operation
            app.setup_mode = False
            app.setup_reason = None

    print("Starting web server on port 80...")
    await app.start_server(port=80)


if __name__ == '__main__':
    asyncio.run(main())
