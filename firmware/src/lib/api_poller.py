"""
Background API polling for game data updates.

Runs as an asyncio task, fetching game data from the backend
and updating the global display state. Cycles through all
available games on each poll.
"""

import uasyncio as asyncio
from lib.scoreboard.state import update_games_and_cycle, set_mode


async def api_polling_loop(config, api_client):
    """
    Background task that polls the API and cycles through all games.

    Fetches all available games and displays the next one in the cycle
    on each poll interval. State is kept in RAM only.

    Args:
        config: Config instance with poll_interval_seconds
        api_client: ScoreboardApiClient instance
    """
    poll_interval_s = config.poll_interval_seconds
    consecutive_failures = 0
    MAX_FAILURES_BEFORE_ERROR = 5

    # Initial delay to let system stabilize
    await asyncio.sleep(2)

    print(f"API poller started (interval: {poll_interval_s}s, cycling all games)")

    while True:
        try:
            games = api_client.get_all_games_safe()

            if update_games_and_cycle(games):
                consecutive_failures = 0
                # Log which game is now showing
                from lib.scoreboard.state import display_state
                current_game = display_state['game']
                game_index = display_state['game_index']
                print(f"Game {game_index + 1}/{len(games)}: {current_game}")
            else:
                consecutive_failures += 1
                print(f"No games available (failures: {consecutive_failures})")

                if consecutive_failures >= MAX_FAILURES_BEFORE_ERROR:
                    set_mode('error', 'No Games')

        except Exception as e:
            consecutive_failures += 1
            print(f"API polling error ({consecutive_failures}): {e}")

            if consecutive_failures >= MAX_FAILURES_BEFORE_ERROR:
                set_mode('error', 'API Error')

        await asyncio.sleep(poll_interval_s)
