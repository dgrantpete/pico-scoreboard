"""Configuration API endpoints."""

import json
import machine
import uasyncio as asyncio
from microdot import Microdot, Request, send_file
from scoreboard.config import Config, CadenceError
import scoreboard.logger as logger

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
    async def update_config(request: Request) -> dict | tuple:
        """Merge provided fields into existing config (single flash write)."""
        data = request.json
        if data is None:
            return config.raw

        try:
            config.update_many(data)
        except CadenceError as e:
            return {'error': 'invalid_cadence', 'message': str(e)}, 400

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

    @api.get('/logs')
    async def get_logs(request: Request):
        """
        Stream the in-RAM log ring as NDJSON: one `[seq, ts, level, msg]`
        array per line. `?since=<seq>` returns only newer entries, enabling
        tail-follow polling; clients use the last line's seq as the next
        `since`. Streaming via a generator avoids building one large JSON
        body on-device.
        """
        since = 0
        raw_since = request.args.get('since')
        if raw_since is not None:
            try:
                since = int(raw_since)
            except ValueError:
                pass

        entries, _latest = logger.entries_since(since)

        def stream():
            for entry in entries:
                yield json.dumps(entry) + "\n"

        return stream(), 200, {'Content-Type': 'application/x-ndjson'}

    @api.get('/logs/previous')
    async def get_previous_log(request: Request):
        """Serve the previous boot's flushed log file (rotated at startup)."""
        try:
            return send_file(logger.PREVIOUS_LOG, content_type='text/plain')
        except OSError:
            return {'error': 'not_found', 'message': 'No previous-boot log on flash'}, 404

    @api.post('/reboot')
    async def reboot(request: Request) -> dict:
        """Trigger a device restart after a brief delay."""
        logger.debug("[MAIN] reboot scheduled: delay=1s")
        asyncio.create_task(_delayed_reboot())
        return {'message': 'Rebooting in 1 second...'}

    @api.post('/reset-network')
    async def reset_network(request: Request) -> dict:
        """Clear network credentials to trigger fresh setup on next boot."""
        config.update_many({'network': {'ssid': '', 'password': ''}})
        logger.debug("[CONFIG] network credentials cleared: will enter setup on reboot")
        return {'message': 'Network configuration cleared. Reboot to enter setup mode.'}

    return api


async def _delayed_reboot() -> None:
    """Wait briefly, persist the log ring, then reset the device."""
    await asyncio.sleep(1)
    logger.flush_to_flash()
    machine.reset()
