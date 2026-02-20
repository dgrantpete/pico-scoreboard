"""Configuration and Game API endpoints."""

import machine
import uasyncio as asyncio
from microdot import Microdot, Request, Response
from scoreboard.config import Config
from scoreboard.api_client import ScoreboardApiClient

try:
    from typing import Callable
except ImportError:
    pass
from scoreboard.state import (
    update_ui_colors, update_display_frequency, update_display_brightness,
    update_display_refresh_rate, update_display_gamma, update_display_blanking_time
)


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
    # These forward raw bytes from the Rust API without JSON parsing
    if api_client is not None:
        @api.get('/games')
        async def get_all_games(request: Request) -> Response | tuple:
            """Fetch all games from backend and forward raw response."""
            try:
                status, body = api_client.get_all_games_raw()
                # Copy body - api_client returns memoryview to shared buffer
                return Response(body=bytes(body), status_code=status,
                                headers={'Content-Type': 'application/json'})
            except Exception as e:
                return {'error': 'internal_error', 'message': str(e)}, 500

        @api.get('/games/<event_id>')
        async def get_game(request: Request, event_id: str) -> Response | tuple:
            """Fetch single game from backend and forward raw response."""
            try:
                status, body = api_client.get_game_raw(event_id)
                # Copy body - api_client returns memoryview to shared buffer
                return Response(body=bytes(body), status_code=status,
                                headers={'Content-Type': 'application/json'})
            except Exception as e:
                return {'error': 'internal_error', 'message': str(e)}, 500

        @api.get('/teams/<team_id>/logo')
        async def get_team_logo(request: Request, team_id: str) -> Response | tuple:
            """Proxy team logo request to backend."""
            try:
                # Extract query params
                width = request.args.get('width')
                height = request.args.get('height')
                background_color = request.args.get('background_color')

                # Forward Accept header (default to PNG)
                accept = request.headers.get('Accept', 'image/png')

                # Fetch from backend
                status, body = api_client.get_team_logo_raw(
                    team_id,
                    width=int(width) if width else None,
                    height=int(height) if height else None,
                    background_color=background_color,
                    accept=accept
                )

                # Copy body - the api_client returns a memoryview to a shared buffer
                # that can be overwritten by concurrent requests
                body = bytes(body)

                # Determine content type from Accept header
                content_type = 'image/x-portable-pixmap' if 'image/x-portable-pixmap' in accept else 'image/png'

                return Response(body=body, status_code=status,
                                headers={
                                    'Content-Type': content_type,
                                    'Cache-Control': 'public, max-age=86400'
                                })
            except Exception as e:
                return {'error': 'internal_error', 'message': str(e)}, 500

    return api


async def _delayed_reboot() -> None:
    """Wait briefly then reset the device."""
    await asyncio.sleep(1)
    machine.reset()
