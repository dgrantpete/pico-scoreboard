"""
Background API polling for game data updates.

Runs as an asyncio task on Core 0 (networking thread), fetching game data
from the backend and updating the display state via double buffering.
Cycles through all available games on each poll.

This module handles ALL network operations including logo fetching,
ensuring the display thread (Core 1) never blocks on network I/O.
"""

import time
import uasyncio as asyncio
from lib.scoreboard.state import (
    get_write_state, commit_state, parse_clock, set_error,
    format_quarter, format_situation, parse_pregame_datetime
)
from lib.scoreboard.models import STATE_PREGAME, STATE_LIVE, STATE_FINAL
from lib.display_loop import get_logo_framebuffer


async def api_polling_loop(config, api_client):
    """
    Background task that polls the API and cycles through all games.

    Fetches all available games, logos, and writes complete state to
    the back buffer before committing. This ensures the display thread
    always sees a consistent state snapshot.

    Args:
        config: Config instance with poll_interval_seconds
        api_client: ScoreboardApiClient instance
    """
    consecutive_failures = 0
    MAX_FAILURES_BEFORE_ERROR = 5

    # Track game cycling state locally (not in double buffer)
    game_index = -1  # Will be incremented to 0 on first poll

    # Initial delay to let system stabilize
    await asyncio.sleep(2)

    print(f"API poller started (interval: {config.poll_interval_seconds}s, cycling all games)")

    while True:
        try:
            games = api_client.get_all_games_safe()

            if games:
                consecutive_failures = 0

                # Advance to next game, wrapping around
                game_index = (game_index + 1) % len(games)
                game = games[game_index]

                # Fetch logos for this game (blocking network calls, but we're on Core 0)
                home_logo = None
                away_logo = None
                if hasattr(game, 'home') and hasattr(game.home, 'abbreviation'):
                    home_logo = get_logo_framebuffer(api_client, game.home.abbreviation)
                if hasattr(game, 'away') and hasattr(game.away, 'abbreviation'):
                    away_logo = get_logo_framebuffer(api_client, game.away.abbreviation)

                # Write complete state to back buffer
                state = get_write_state()

                # Detect score changes for flash animation
                prev_game = state.get('game')
                if prev_game and hasattr(game, 'home') and hasattr(prev_game, 'home'):
                    # Only compare if same matchup (same teams)
                    same_matchup = (
                        hasattr(game.home, 'abbreviation') and
                        hasattr(prev_game.home, 'abbreviation') and
                        game.home.abbreviation == prev_game.home.abbreviation
                    )
                    if same_matchup:
                        if game.home.score > prev_game.home.score:
                            state['home_scored_ms'] = time.ticks_ms()
                        if game.away.score > prev_game.away.score:
                            state['away_scored_ms'] = time.ticks_ms()

                state['mode'] = 'game'
                state['game'] = game
                state['games'] = games
                state['game_index'] = game_index
                state['home_logo'] = home_logo
                state['away_logo'] = away_logo
                state['error_message'] = None
                state['last_update_ms'] = time.ticks_ms()
                state['animation_start_ms'] = time.ticks_ms()  # Reset scroll animations
                state['dirty'] = True

                # Sync clock for live games
                if hasattr(game, 'clock') and game.clock:
                    state['clock_seconds'] = parse_clock(game.clock)
                    state['clock_last_tick_ms'] = time.ticks_ms()

                # Pre-format display strings for Core 1 (no allocations on display thread)
                display = state['display']
                if game.state == STATE_LIVE or game.state == STATE_FINAL:
                    display['quarter'] = format_quarter(game.quarter) if hasattr(game, 'quarter') else ''
                    display['situation'] = format_situation(game.situation) if hasattr(game, 'situation') and game.situation else ''
                    display['pregame_date'] = ''
                    display['pregame_time'] = ''
                    # Last play text for live games
                    if game.state == STATE_LIVE and hasattr(game, 'last_play') and game.last_play and game.last_play.text:
                        display['last_play_text'] = game.last_play.text
                    else:
                        display['last_play_text'] = ''
                elif game.state == STATE_PREGAME:
                    display['quarter'] = ''
                    display['situation'] = ''
                    display['last_play_text'] = ''
                    if hasattr(game, 'start_time') and game.start_time:
                        date_display, time_display = parse_pregame_datetime(game.start_time)
                        display['pregame_date'] = date_display
                        display['pregame_time'] = time_display
                    else:
                        display['pregame_date'] = ''
                        display['pregame_time'] = ''

                # Atomic swap - display thread now sees this state
                commit_state()

                print(f"Game {game_index + 1}/{len(games)}: {game}")
            else:
                # No games is a normal state, not an error
                # Reset failure counter since API responded successfully
                consecutive_failures = 0

                state = get_write_state()
                state['mode'] = 'no_games'
                state['game'] = None
                state['games'] = []
                state['dirty'] = True
                commit_state()

                print("No games scheduled")

        except Exception as e:
            consecutive_failures += 1
            print(f"API polling error ({consecutive_failures}): {e}")

            if consecutive_failures >= MAX_FAILURES_BEFORE_ERROR:
                set_error('API ERROR', [
                    'Backend unreachable',
                    f'after {consecutive_failures} tries',
                    'Check network config'
                ])

        await asyncio.sleep(config.poll_interval_seconds)
