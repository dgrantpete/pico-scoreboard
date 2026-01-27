"""
Global display state for the Pico Scoreboard.

Shared between web server, API poller, and display loop.
Uses a one-writer, one-reader pattern so no locks are needed.
"""

import time

# Display state dictionary
# - mode: 'idle' | 'game' | 'setup' | 'error' | 'startup'
# - game: Game object (PregameGame/LiveGame/FinalGame) or None
# - games: List of all games for cycling
# - game_index: Current position in the games cycle
# - error_message: Error text when mode='error'
# - last_update_ms: time.ticks_ms() of last successful update
# - dirty: Flag for display to know when to redraw (full redraw)
# - clock_dirty: Flag for clock-only redraw (partial update)
# - clock_seconds: Parsed clock in total seconds for local countdown
# - clock_last_tick_ms: ticks_ms() when we last decremented the clock
# - startup: Startup progress state (step, total_steps, operation, detail)
display_state = {
    'mode': 'idle',
    'game': None,
    'games': [],
    'game_index': 0,
    'error_message': None,
    'last_update_ms': 0,
    'dirty': True,
    'clock_dirty': False,
    'clock_seconds': None,
    'clock_last_tick_ms': 0,
    'startup': {
        'step': 1,
        'total_steps': 5,
        'operation': '',
        'detail': '',
    },
}


def parse_clock(clock_str):
    """
    Parse clock string to total seconds.

    Args:
        clock_str: Clock string like "3:45", "0:05", or "15:00"

    Returns:
        Total seconds as integer (e.g., "3:45" -> 225)
    """
    if ':' in clock_str:
        parts = clock_str.split(':')
        return int(parts[0]) * 60 + int(parts[1])
    # Edge case: just seconds (shouldn't happen but handle gracefully)
    try:
        return int(clock_str)
    except ValueError:
        return 0


def format_clock(seconds):
    """
    Format seconds back to clock string.

    Args:
        seconds: Total seconds (e.g., 225)

    Returns:
        Clock string like "3:45"
    """
    if seconds < 0:
        seconds = 0
    minutes = seconds // 60
    secs = seconds % 60
    return f"{minutes}:{secs:02d}"


def sync_clock(game):
    """
    Sync local clock state from a game object.

    Called when API updates arrive to reset the local countdown.

    Args:
        game: LiveGame object with clock attribute
    """
    if hasattr(game, 'clock') and game.clock:
        display_state['clock_seconds'] = parse_clock(game.clock)
        display_state['clock_last_tick_ms'] = time.ticks_ms()


def update_games_and_cycle(games):
    """
    Update with all games and cycle to the next one.

    Stores the games list in RAM and advances to the next game in the cycle.
    Wraps around to the first game when reaching the end.

    Args:
        games: List of game objects (PregameGame/LiveGame/FinalGame)

    Returns:
        True if a game was displayed, False if list was empty
    """
    if not games:
        return False

    display_state['games'] = games

    # Advance to next game, wrapping around
    current_index = display_state['game_index']
    next_index = (current_index + 1) % len(games)
    display_state['game_index'] = next_index

    # Set the current game to display
    game = games[next_index]
    display_state['game'] = game
    display_state['mode'] = 'game'
    display_state['last_update_ms'] = time.ticks_ms()
    display_state['dirty'] = True

    # Sync clock for live games
    sync_clock(game)

    return True


def set_mode(mode, error_message=None):
    """
    Set display mode (called during setup/error states).

    Args:
        mode: One of 'idle', 'game', 'setup', 'error'
        error_message: Optional error text (only used when mode='error')
    """
    display_state['mode'] = mode
    display_state['error_message'] = error_message
    display_state['dirty'] = True


def mark_dirty():
    """Mark display state as needing a redraw."""
    display_state['dirty'] = True


def set_startup_step(step, total, operation, detail=''):
    """
    Update startup progress display.

    Args:
        step: Current step number (1-based)
        total: Total number of steps
        operation: Short operation name (e.g., "WiFi scan")
        detail: Optional detail text (e.g., "Found 8 networks")
    """
    display_state['mode'] = 'startup'
    display_state['startup']['step'] = step
    display_state['startup']['total_steps'] = total
    display_state['startup']['operation'] = operation
    display_state['startup']['detail'] = detail
    display_state['dirty'] = True


def clear_startup_state():
    """Clear startup state after boot completes to free memory."""
    display_state['startup'] = {
        'step': 1,
        'total_steps': 5,
        'operation': '',
        'detail': '',
    }
