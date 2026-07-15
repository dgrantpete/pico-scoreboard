"""
On-device league-select menu (button B hold).

MenuController owns the WHOLE menu session on Core 0: the full item list,
the working checkbox flags, cursor + scroll window, and the input timeout.
Everything Core 1 draws is pre-built here — each label is rendered to a
1-bit strip ONCE per open, and the visible 5-row window, highlight index,
and scrollbar thumb are computed per publish — then handed across the state
mailbox via set_menu (wholesale list replacement; see MenuView in state.py).
display.render_menu is a pure reader of that view.

Semantics (user-locked design):
- The checked set is a SESSION rotation filter over the configured league
  sources — GamePoller.set_league_filter — generalizing the old one-league
  lock. It resets to all-checked on reboot; the persisted config still owns
  which leagues are polled at all.
- A short advances the cursor (items top to bottom, then DONE, wrap to the
  top); B short toggles the checkbox (or activates DONE).
- EVERY exit applies — DONE, B hold, and the 10 s input timeout. There is
  deliberately no cancel path.
- The last checked league cannot be unchecked (silent no-op): rotation
  always keeps at least one league. Toast feedback is unavailable under the
  menu by design (the take-over bypasses the toast-drawing renderers).

Button routing: the controller is the single dispatch point for both
buttons — main.py's _PressTrackers bind to a_short/a_long/b_short/b_long,
which fall through to the poller actions (skip / league skip / rotation
lock) whenever the menu is closed. The timeout is checked from the same
50 ms button poll loop; no extra task.
"""

import time

from .fonts import render_strip, measure_text, unscii_8
from .state import get_write_state, set_menu, clear_menu
import scoreboard.logger as logger

_VISIBLE_ROWS = 5     # display's _MENU_VISIBLE_ROWS; window height in items
_TIMEOUT_MS = 10_000  # inactivity -> apply + close (same as DONE)

# Scrollbar thumb geometry, pre-computed here so Core 1 draws two rects
# verbatim. Must mirror display.py's menu track: y 0..(_TRACK_H-1).
_TRACK_H = 50
_MIN_THUMB_H = 4


class MenuController:
    """Core 0 owner of the league menu session (see module docstring)."""

    def __init__(self, poller, sources) -> None:
        self._poller = poller
        self._sources = sources  # boot-static list, shared with the poller
        self._active = False
        self._items: list = []    # (source key, strip) — built per open
        self._checked: list = []  # parallel working flags
        self._cursor = 0          # 0..n-1 = items, n = DONE
        self._scroll = 0          # first visible item index
        self._last_input_ms = 0

    # --- Button routing (bound by main.py's _PressTrackers) -----------------

    def a_short(self) -> None:
        if self._active:
            self._touch()
            self._advance()
        else:
            self._poller.skip()

    def a_long(self) -> None:
        if self._active:
            self._touch()  # deliberate no-op, but it still counts as input
        else:
            self._poller.skip_league()

    def b_short(self) -> None:
        if self._active:
            self._touch()
            self._select()
        else:
            self._poller.toggle_lock()

    def b_long(self) -> None:
        if self._active:
            self._apply_and_close()
        else:
            self._open()

    def check_timeout(self) -> None:
        """Called every 50 ms button-poll iteration while the loop runs."""
        if self._active and time.ticks_diff(
                time.ticks_ms(), self._last_input_ms) >= _TIMEOUT_MS:
            logger.debug("[MENU] input timeout: applying")
            self._apply_and_close()

    # --- Session ------------------------------------------------------------

    def _open(self) -> None:
        if not self._sources:
            return
        if get_write_state().mode == 'updating':
            # OTA progress must stay visible and a reboot is imminent.
            return
        current = self._poller.league_filter
        items = []
        checked = []
        for source in self._sources:
            label = source.display_name
            # One-shot per-open allocation (QR-generation precedent): strips
            # are immutable for the session, so no ping-pong pool is needed;
            # cap must be a multiple of 8 (MONO_HLSB byte-padded rows).
            cap = ((measure_text(label, unscii_8) + 7) // 8) * 8
            items.append((source.key, render_strip(bytearray(cap), cap, label, unscii_8)))
            checked.append(current is None or source.key in current)
        self._items = items
        self._checked = checked
        self._active = True
        self._cursor = 0
        self._scroll = 0
        self._touch()
        logger.debug(f"[MENU] open: {len(items)} leagues")
        self._publish()

    def _advance(self) -> None:
        n = len(self._items)
        self._cursor = (self._cursor + 1) % (n + 1)
        # Keep the cursor inside the visible window (DONE is always visible).
        if self._cursor == 0:
            self._scroll = 0
        elif self._cursor < n and self._cursor >= self._scroll + _VISIBLE_ROWS:
            self._scroll = self._cursor - _VISIBLE_ROWS + 1
        self._publish()

    def _select(self) -> None:
        if self._cursor == len(self._items):
            self._apply_and_close()
            return
        if self._checked[self._cursor]:
            remaining = 0
            for c in self._checked:
                if c:
                    remaining += 1
            if remaining == 1:
                # Never allow an empty filter: silent no-op (spec).
                logger.debug("[MENU] refused unchecking the last league")
                return
        self._checked[self._cursor] = not self._checked[self._cursor]
        self._publish()

    def _apply_and_close(self) -> None:
        keys = set()
        for i in range(len(self._items)):
            if self._checked[i]:
                keys.add(self._items[i][0])
        self._active = False
        self._items = []  # release the session's strip buffers
        self._checked = []
        logger.debug(f"[MENU] apply: {len(keys)} leagues checked")
        self._poller.set_league_filter(keys)
        clear_menu()

    def _touch(self) -> None:
        self._last_input_ms = time.ticks_ms()

    def _publish(self) -> None:
        """Build the visible window + thumb and publish via set_menu.

        Fresh lists every time — the previously published ones may still be
        latched by Core 1 (wholesale-replacement contract, MenuView)."""
        n = len(self._items)
        strips = []
        checked = []
        for i in range(self._scroll, min(self._scroll + _VISIBLE_ROWS, n)):
            strips.append(self._items[i][1])
            checked.append(self._checked[i])
        highlight = self._cursor - self._scroll if self._cursor < n else -1
        if n > _VISIBLE_ROWS:
            thumb_h = _TRACK_H * _VISIBLE_ROWS // n
            if thumb_h < _MIN_THUMB_H:
                thumb_h = _MIN_THUMB_H
            thumb_y = (_TRACK_H - thumb_h) * self._scroll // (n - _VISIBLE_ROWS)
        else:
            thumb_y = -1
            thumb_h = 0
        set_menu(strips, checked, highlight, thumb_y, thumb_h)
