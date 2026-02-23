"""Configuration and Game API endpoints."""

import machine
import time
import uasyncio as asyncio
from microdot import Microdot, Request, Response
from scoreboard.config import Config
from scoreboard.api_client import ScoreboardApiClient
from scoreboard.hmac import sign_path

try:
    from typing import Callable
except ImportError:
    pass
from scoreboard.state import (
    update_ui_colors, update_display_frequency, update_display_brightness,
    update_display_refresh_rate, update_display_gamma, update_display_blanking_time
)

# Offset between Unix epoch (1970) and MicroPython epoch (2000)
_EPOCH_OFFSET = 946684800


def create_api(config: Config, get_network_status: "Callable[[], dict]", api_client: ScoreboardApiClient | None = None) -> Microdot:
    """
    Create API sub-application.

    Args:
        config: Config instance for reading/writing settings
        get_network_status: Callable that returns current network state dict
        api_client: Optional ScoreboardApiClient for game data endpoints
    """
    api: Microdot = Microdot()

    @api.get('/config')
    async def get_config(request: Request) -> dict:
        """Return the full configuration object."""
        return config.raw

    @api.put('/config')
    async def update_config(request: Request) -> dict:
        """Merge provided fields into existing config."""
        data = request.json
        if data is None:
            return config.raw
        for section, values in data.items():
            if section in config.raw and isinstance(values, dict):
                for key, value in values.items():
                    config.update(section, key, value)
        # Re-compute UI colors if colors section was updated
        if 'colors' in data:
            update_ui_colors(config)
        # Update display driver settings as needed
        if 'display' in data:
            if 'data_frequency_khz' in data['display']:
                update_display_frequency(config)
            if 'brightness' in data['display']:
                update_display_brightness(config)
            if 'target_refresh_rate' in data['display']:
                update_display_refresh_rate(config)
            if 'gamma' in data['display']:
                update_display_gamma(config)
            if 'blanking_time_ns' in data['display']:
                update_display_blanking_time(config)
        return config.raw

    @api.get('/status')
    async def get_status(request: Request) -> dict:
        """Return current device network status."""
        return get_network_status()

    @api.post('/reboot')
    async def reboot(request: Request) -> dict:
        """Trigger a device restart after a brief delay."""
        asyncio.create_task(_delayed_reboot())
        return {'message': 'Rebooting in 1 second...'}

    @api.post('/reset-network')
    async def reset_network(request: Request) -> dict:
        """Clear network credentials to trigger fresh setup on next boot."""
        config.update('network', 'ssid', '')
        config.update('network', 'password', '')
        return {'message': 'Network configuration cleared. Reboot to enter setup mode.'}

    # Game endpoints (only if api_client is provided)
    # These redirect the frontend to the backend with a signed URL,
    # avoiding the need to proxy large responses through the Pico.
    if api_client is not None:
        def _signed_redirect_url(path: str, extra_params: str = '') -> str:
            """Build a signed redirect URL to the backend."""
            base_url = config.api_url.rstrip('/')
            expires = int(time.time()) + _EPOCH_OFFSET + 30
            sig = sign_path(config.api_key, path, expires)
            sep = '&' if extra_params else ''
            return f"{base_url}{path}?expires={expires}&sig={sig}{sep}{extra_params}"

        @api.get('/games')
        async def get_all_games(request: Request) -> tuple:
            """Redirect to backend games endpoint with signed URL."""
            path = api_client._games_path()
            return '', 302, {'Location': _signed_redirect_url(path)}

        @api.get('/games/<event_id>')
        async def get_game(request: Request, event_id: str) -> tuple:
            """Redirect to backend single game endpoint with signed URL."""
            path = f"{api_client._games_path()}/{event_id}"
            return '', 302, {'Location': _signed_redirect_url(path)}

        @api.get('/teams/<team_id>/logo')
        async def get_team_logo(request: Request, team_id: str) -> tuple:
            """Redirect to backend logo endpoint with signed URL."""
            path = f"/api/football/nfl/{team_id}/logo"

            # Forward original query params
            params = []
            width = request.args.get('width')
            height = request.args.get('height')
            background_color = request.args.get('background_color')
            if width:
                params.append(f"width={width}")
            if height:
                params.append(f"height={height}")
            if background_color:
                params.append(f"background_color={background_color}")
            extra = '&'.join(params)

            return '', 302, {'Location': _signed_redirect_url(path, extra)}

    return api


async def _delayed_reboot() -> None:
    """Wait briefly then reset the device."""
    await asyncio.sleep(1)
    machine.reset()
