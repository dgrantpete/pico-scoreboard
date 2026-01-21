"""Configuration and Game API endpoints."""

import machine
import uasyncio as asyncio
from lib.microdot import Microdot, Response


def create_api(config, get_network_status, api_client=None):
    """
    Create API sub-application.

    Args:
        config: Config instance for reading/writing settings
        get_network_status: Callable that returns current network state dict
        api_client: Optional ScoreboardApiClient for game data endpoints
    """
    api = Microdot()

    @api.get('/config')
    async def get_config(request):
        """Return the full configuration object."""
        return config.raw

    @api.put('/config')
    async def update_config(request):
        """Merge provided fields into existing config."""
        data = request.json
        for section, values in data.items():
            if section in config.raw and isinstance(values, dict):
                for key, value in values.items():
                    config.update(section, key, value)
        return config.raw

    @api.get('/status')
    async def get_status(request):
        """Return current device network status."""
        return get_network_status()

    @api.post('/reboot')
    async def reboot(request):
        """Trigger a device restart after a brief delay."""
        asyncio.create_task(_delayed_reboot())
        return {'message': 'Rebooting in 1 second...'}

    @api.post('/reset-network')
    async def reset_network(request):
        """Clear network credentials to trigger fresh setup on next boot."""
        config.update('network', 'ssid', '')
        config.update('network', 'password', '')
        return {'message': 'Network configuration cleared. Reboot to enter setup mode.'}

    # Game endpoints (only if api_client is provided)
    # These forward raw bytes from the Rust API without JSON parsing
    if api_client is not None:
        @api.get('/games')
        async def get_all_games(request):
            """Fetch all games from backend and forward raw response."""
            try:
                status, body = api_client.get_all_games_raw()
                return Response(body=body, status_code=status,
                                headers={'Content-Type': 'application/json'})
            except Exception as e:
                return {'error': 'internal_error', 'message': str(e)}, 500

        @api.get('/games/<event_id>')
        async def get_game(request, event_id):
            """Fetch single game from backend and forward raw response."""
            try:
                status, body = api_client.get_game_raw(event_id)
                return Response(body=body, status_code=status,
                                headers={'Content-Type': 'application/json'})
            except Exception as e:
                return {'error': 'internal_error', 'message': str(e)}, 500

    return api


async def _delayed_reboot():
    """Wait briefly then reset the device."""
    await asyncio.sleep(1)
    machine.reset()
