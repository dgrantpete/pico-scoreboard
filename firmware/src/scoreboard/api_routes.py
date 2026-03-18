"""Configuration API endpoints."""

import machine
import uasyncio as asyncio
from microdot import Microdot, Request
from scoreboard.config import Config

try:
    from typing import Callable
except ImportError:
    pass

from scoreboard.state import (
    update_ui_colors, update_display_frequency,
    update_display_refresh_rate, update_display_gamma, update_display_blanking_time
)

def create_api(config: Config, get_network_status: "Callable[[], dict]") -> Microdot:
    """
    Create API sub-application.

    Args:
        config: Config instance for reading/writing settings
        get_network_status: Callable that returns current network state dict
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
        from scoreboard.logger import DEBUG
        if config.log_level >= DEBUG:
            print("[MAIN] reboot scheduled: delay=1s")
        asyncio.create_task(_delayed_reboot())
        return {'message': 'Rebooting in 1 second...'}

    @api.post('/reset-network')
    async def reset_network(request: Request) -> dict:
        """Clear network credentials to trigger fresh setup on next boot."""
        from scoreboard.logger import DEBUG
        config.update('network', 'ssid', '')
        config.update('network', 'password', '')
        if config.log_level >= DEBUG:
            print("[CONFIG] network credentials cleared: will enter setup on reboot")
        return {'message': 'Network configuration cleared. Reboot to enter setup mode.'}

    return api


async def _delayed_reboot() -> None:
    """Wait briefly then reset the device."""
    await asyncio.sleep(1)
    machine.reset()
