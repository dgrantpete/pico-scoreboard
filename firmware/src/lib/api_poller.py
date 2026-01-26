"""
Background API polling for game data updates.

Runs as an asyncio task, fetching game data from the backend
and updating the global display state.
"""

import uasyncio as asyncio
from lib.scoreboard.state import update_game, set_mode
from lib.scoreboard.models import STATE_LIVE


async def api_polling_loop(config, api_client):
    """
    Background task that polls the API and updates display state.

    Only runs in station mode when connected to WiFi.

    Args:
        config: Config instance with poll_interval_seconds and game.event_id
        api_client: ScoreboardApiClient instance
    """
    poll_interval_s = config.poll_interval_seconds
    consecutive_failures = 0
    MAX_FAILURES_BEFORE_ERROR = 5

    # Initial delay to let system stabilize
    await asyncio.sleep(2)

    print(f"API poller started (interval: {poll_interval_s}s)")

    while True:
        try:
            # Get event_id from config
            event_id = config.get('game', 'event_id')

            game = None

            if event_id:
                # Specific game configured
                game = api_client.get_game_safe(event_id)
            else:
                # No specific game - fetch all and pick the best one
                games = api_client.get_all_games_safe()
                game = select_best_game(games)

            if game:
                consecutive_failures = 0
                update_game(game)
                print(f"Game updated: {game}")
            else:
                consecutive_failures += 1
                print(f"No game data (failures: {consecutive_failures})")

                # After repeated failures, show error state
                if consecutive_failures >= MAX_FAILURES_BEFORE_ERROR:
                    set_mode('error', 'No Data')

        except Exception as e:
            consecutive_failures += 1
            print(f"API polling error ({consecutive_failures}): {e}")

            if consecutive_failures >= MAX_FAILURES_BEFORE_ERROR:
                set_mode('error', 'API Error')

        await asyncio.sleep(poll_interval_s)


def select_best_game(games):
    """
    Select the best game to display from a list.

    Priority:
    1. Live games (currently playing)
    2. First game in list (usually sorted by start time)

    Args:
        games: List of game objects

    Returns:
        Selected game or None if list is empty
    """
    if not games:
        return None

    # Prefer live games
    for game in games:
        if game.state == STATE_LIVE:
            return game

    # Fall back to first game
    return games[0]
