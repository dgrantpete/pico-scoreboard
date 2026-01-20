"""Configuration API endpoints."""

import machine
import uasyncio as asyncio
from lib.microdot import Microdot


def create_api(config, get_network_status):
    """
    Create API sub-application.

    Args:
        config: Config instance for reading/writing settings
        get_network_status: Callable that returns current network state dict
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

    return api


async def _delayed_reboot():
    """Wait briefly then reset the device."""
    await asyncio.sleep(1)
    machine.reset()
