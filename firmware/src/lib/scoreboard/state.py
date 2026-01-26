"""
Global display state for the Pico Scoreboard.

Shared between web server, API poller, and display loop.
Uses a one-writer, one-reader pattern so no locks are needed.
"""

import time

# Display state dictionary
# - mode: 'idle' | 'game' | 'setup' | 'error'
# - game: Game object (PregameGame/LiveGame/FinalGame) or None
# - games: List of all games for cycling
# - game_index: Current position in the games cycle
# - error_message: Error text when mode='error'
# - last_update_ms: time.ticks_ms() of last successful update
# - dirty: Flag for display to know when to redraw
display_state = {
    'mode': 'idle',
    'game': None,
    'games': [],
    'game_index': 0,
    'error_message': None,
    'last_update_ms': 0,
    'dirty': True,
}


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
    display_state['game'] = games[next_index]
    display_state['mode'] = 'game'
    display_state['last_update_ms'] = time.ticks_ms()
    display_state['dirty'] = True

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
