"""
Pico Scoreboard Web Server.

Serves the SvelteKit frontend from flash memory and provides
network connectivity in either AP or Station mode.

On first boot (ap_mode=true), creates an open WiFi network
for initial setup. Once configured, connects to the specified
WiFi network with automatic fallback to AP mode on failure.
"""

import network
import time
import machine
import uasyncio as asyncio
from lib.microdot import Microdot, Response, send_file
from lib.scoreboard import Config
from lib.dns import run_dns_server
from lib.api import create_api

# Reduce buffer size for memory-constrained environment
Response.send_file_buffer_size = 512

app = Microdot()
config = Config()

# Track whether we're in fallback AP mode (connection failed vs intentional AP mode)
app.is_fallback_ap = False


def get_network_status():
    """Build current network status dict for API."""
    ap = getattr(app, 'ap', None)
    wlan = getattr(app, 'wlan', None)

    if ap and ap.active():
        return {
            'mode': 'ap',
            'connected': False,
            'is_fallback': app.is_fallback_ap,
            'configured_ssid': config.ssid if app.is_fallback_ap else None,
            'ip': None,
            'hostname': None,
            'ap_ip': ap.ifconfig()[0],
            'ap_ssid': config.device_name
        }
    elif wlan and wlan.isconnected():
        return {
            'mode': 'station',
            'connected': True,
            'is_fallback': False,
            'configured_ssid': None,
            'ip': wlan.ifconfig()[0],
            'hostname': f'{config.device_name}.local',
            'ap_ip': None
        }
    else:
        return {
            'mode': 'unknown',
            'connected': False,
            'is_fallback': False,
            'configured_ssid': None,
            'ip': None,
            'hostname': None,
            'ap_ip': None
        }


# Create and mount API under /api prefix
api = create_api(config, get_network_status)
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

    # If this is a hijacked request (DNS lie), redirect to our IP to trigger portal
    if ap and host not in get_my_hosts(ap):
        redirect_ip = ap.ifconfig()[0]
        return '', 302, {'Location': f'http://{redirect_ip}/'}

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

    # Hijacked request (DNS lie) -> redirect to trigger captive portal
    redirect_ip = ap.ifconfig()[0] if ap else '192.168.4.1'
    return '', 302, {'Location': f'http://{redirect_ip}/'}


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
    if config.ap_mode:
        ap = start_ap_mode()
        # Start DNS server for captive portal
        asyncio.create_task(run_dns_server(ap.ifconfig()[0]))
    else:
        wlan = start_station_mode()
        if wlan is None:
            print("Falling back to AP mode...")
            app.is_fallback_ap = True  # Mark as fallback (connection failed)
            ap = start_ap_mode()
            asyncio.create_task(run_dns_server(ap.ifconfig()[0]))

    print("Starting web server on port 80...")
    await app.start_server(port=80)


if __name__ == '__main__':
    asyncio.run(main())
