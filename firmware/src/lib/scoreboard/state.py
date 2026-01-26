"""
Global display state for the Pico Scoreboard.

Shared between web server, API poller, and display loop.
Uses a one-writer, one-reader pattern so no locks are needed.
"""

import time

# Display state dictionary
# - mode: 'idle' | 'game' | 'setup' | 'error'
# - game: Game object (PregameGame/LiveGame/FinalGame) or None
# - error_message: Error text when mode='error'
# - last_update_ms: time.ticks_ms() of last successful update
# - dirty: Flag for display to know when to redraw
display_state = {
    'mode': 'idle',
    'game': None,
    'error_message': None,
    'last_update_ms': 0,
    'dirty': True,
}


def update_game(game):
    """
    Update game state (called by API poller).

    Args:
        game: A PregameGame, LiveGame, or FinalGame object
    """
    display_state['game'] = game
    display_state['mode'] = 'game'
    display_state['last_update_ms'] = time.ticks_ms()
    display_state['dirty'] = True


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
