"""Configuration and Game API endpoints."""

import machine
import uasyncio as asyncio
from lib.microdot import Microdot
from lib.scoreboard.api_client import ApiError


def _color_to_dict(color):
    """Convert Color object to dict."""
    return {'r': color.r, 'g': color.g, 'b': color.b}


def _team_to_dict(team):
    """Convert Team object to dict."""
    result = {
        'abbreviation': team.abbreviation,
        'color': _color_to_dict(team.color)
    }
    if team.record is not None:
        result['record'] = team.record
    return result


def _team_with_score_to_dict(team):
    """Convert TeamWithScore object to dict."""
    result = {
        'abbreviation': team.abbreviation,
        'color': _color_to_dict(team.color),
        'score': team.score,
        'timeouts': team.timeouts
    }
    if team.record is not None:
        result['record'] = team.record
    return result


def _weather_to_dict(weather):
    """Convert Weather object to dict."""
    return {'temp': weather.temp, 'description': weather.description}


def _situation_to_dict(situation):
    """Convert Situation object to dict."""
    return {
        'down': situation.down,
        'distance': situation.distance,
        'yard_line': situation.yard_line,
        'possession': situation.possession,
        'red_zone': situation.red_zone
    }


def _game_to_dict(game):
    """Convert game model object back to JSON-serializable dict."""
    result = {'state': game.state, 'event_id': game.event_id}

    if game.state == 'pregame':
        result['home'] = _team_to_dict(game.home)
        result['away'] = _team_to_dict(game.away)
        result['start_time'] = game.start_time
        if game.venue is not None:
            result['venue'] = game.venue
        if game.broadcast is not None:
            result['broadcast'] = game.broadcast
        if game.weather is not None:
            result['weather'] = _weather_to_dict(game.weather)

    elif game.state == 'live':
        result['home'] = _team_with_score_to_dict(game.home)
        result['away'] = _team_with_score_to_dict(game.away)
        result['quarter'] = game.quarter
        result['clock'] = game.clock
        if game.situation is not None:
            result['situation'] = _situation_to_dict(game.situation)

    elif game.state == 'final':
        result['home'] = _team_with_score_to_dict(game.home)
        result['away'] = _team_with_score_to_dict(game.away)
        result['status'] = game.status
        result['winner'] = game.winner

    return result


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
    if api_client is not None:
        @api.get('/games')
        async def get_all_games(request):
            """Fetch all games from backend and return to frontend."""
            try:
                games = api_client.get_all_games()
                return [_game_to_dict(g) for g in games]
            except ApiError as e:
                return {'error': e.error, 'message': e.message}, e.status_code
            except Exception as e:
                return {'error': 'internal_error', 'message': str(e)}, 500

        @api.get('/games/<event_id>')
        async def get_game(request, event_id):
            """Fetch single game from backend and return to frontend."""
            try:
                game = api_client.get_game(event_id)
                return _game_to_dict(game)
            except ApiError as e:
                return {'error': e.error, 'message': e.message}, e.status_code
            except Exception as e:
                return {'error': 'internal_error', 'message': str(e)}, 500

    return api


async def _delayed_reboot():
    """Wait briefly then reset the device."""
    await asyncio.sleep(1)
    machine.reset()
