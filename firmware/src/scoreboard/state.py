"""
Global display state for the Pico Scoreboard.

Shared between the networking thread (Core 0) and display thread (Core 1)
via a triple-buffered mailbox (one writer, one reader):

- Core 0 mutates the write buffer, then commit() publishes it as `latest`.
- Core 1 latches `latest` at the start of each frame and renders from that
  buffer for the whole frame. The writer can never touch a buffer that is
  published or being read, so frames are always internally consistent.
- A lock protects only the index bookkeeping (microseconds); neither the
  render nor the carry-forward copy ever holds it, so the cores never block
  each other for more than an index swap.

Every commit bumps a sequence number; the display thread uses it to skip
re-rendering static screens that haven't changed.

All strings the display thread draws are pre-built here on Core 0 at write
time (see StartupState/SetupState/ErrorState). Render functions are pure
readers — Core 1 does no per-frame string formatting.
"""

import time
import _thread
import framebuf

from hub75 import Hub75Driver, gamma as gamma_mod
from scoreboard.config import Config
import scoreboard.logger as logger
from scoreboard.fonts import rgb565, measure_text, fit_text, render_strip, spleen_5x8, unscii_8, unscii_16
from scoreboard import screen_geometry
from scoreboard.mlb import LiveGame
from scoreboard.nba import PHASE_END_OF_PERIOD, PHASE_HALFTIME, period_name
from scoreboard.soccer import (
    EVENT_GOAL, HALF_ET_FIRST, SIDE_AWAY, SIDE_HOME, base_minutes,
)


# Minimum brightest-channel value for team colors. Teams whose primary is
# darker than this (e.g. Yankees/Brewers navy, White Sox near-black) get
# scaled up proportionally so the text stays legible on the black panel.
# Preserves hue for chromatic colors; near-neutrals move toward bright gray.
#
# Lives here (not display.py) so Core 0 state setters can pre-brighten team
# colors at commit time without importing display; display.render_game imports
# _team_color_to_rgb565 from this module for its per-frame use.
_TEAM_COLOR_MIN_CHANNEL = 128


def _team_color_to_rgb565(packed: int) -> int:
    r = (packed >> 16) & 0xFF
    g = (packed >> 8) & 0xFF
    b = packed & 0xFF
    m = r if r >= g and r >= b else (g if g >= b else b)
    if m < _TEAM_COLOR_MIN_CHANNEL:
        if m == 0:
            r = g = b = _TEAM_COLOR_MIN_CHANNEL
        else:
            scale = _TEAM_COLOR_MIN_CHANNEL / m
            r = int(r * scale)
            g = int(g * scale)
            b = int(b * scale)
    return rgb565(r, g, b)


class ThreadHealth:
    """
    Cross-core health signals for the display thread.

    Core 1 writes both fields; Core 0's watchdog feeder reads them:
    - `healthy` flips False when the render loop crashes out.
    - `frame_seq` increments once per render-loop tick (not per render, so
      the static-screen skip can't false-positive). A stalled value with
      `healthy` still True means the thread is *hung*, not crashed.
    """

    def __init__(self) -> None:
        self.healthy: bool = False
        self.frame_seq: int = 0


# Length cap for one line of spleen_5x8 text across the full display.
_LINE_MAX_CHARS = 25


def _truncate_line(text: str) -> str:
    """Cap a line at the display width, marking truncation with a dot."""
    if len(text) > _LINE_MAX_CHARS:
        return text[:_LINE_MAX_CHARS - 1] + '.'
    return text


# =============================================================================
# Typed state classes
# =============================================================================
# Each class is plain data plus a copy_from() used by the carry-forward copy
# after a commit. Adding a field means adding it to __init__ AND copy_from of
# the same class — the two live side by side so they can't drift apart.


class StartupState:
    """Boot progress state. Strings are pre-built by set_startup_step."""

    def __init__(self) -> None:
        self.step: int = 1
        self.total_steps: int = 5
        self.step_text: str = ''    # e.g. "2/5" — pre-built, drawn verbatim
        self.operation: str = ''    # pre-truncated
        self.detail: str = ''       # pre-truncated
        self.attempt: int = 0          # WiFi attempt in progress (0 = hide dots)
        self.attempts_total: int = 0   # retry budget (0 = hide dots)

    def copy_from(self, other: "StartupState") -> None:
        self.step = other.step
        self.total_steps = other.total_steps
        self.step_text = other.step_text
        self.operation = other.operation
        self.detail = other.detail
        self.attempt = other.attempt
        self.attempts_total = other.attempts_total


class SetupState:
    """WiFi setup / AP mode state. Display lines are pre-built by set_setup_mode."""

    def __init__(self) -> None:
        self.reason: str | None = None       # 'no_config' | 'connection_failed' | 'bad_auth'
        self.ap_ssid: str = ''               # AP network name to connect to
        self.ap_ip: str = ''                 # IP address to open in browser
        self.wifi_ssid: str = ''             # Failed SSID (for error context)
        self.title: str = ''                 # Pre-built screen title
        self.line_18: str = ''               # Pre-built text lines by Y position
        self.line_28: str = ''
        self.line_44: str = ''
        self.line_54: str = ''
        self.qr_fb: framebuf.FrameBuffer | None = None        # FrameBuffer (MONO_HLSB format)
        self.qr_width: int = 0              # QR code width in pixels
        self.qr_height: int = 0             # QR code height in pixels
        self.qr_palette: framebuf.FrameBuffer | None = None   # RGB565 palette for display blitting

    def copy_from(self, other: "SetupState") -> None:
        self.reason = other.reason
        self.ap_ssid = other.ap_ssid
        self.ap_ip = other.ap_ip
        self.wifi_ssid = other.wifi_ssid
        self.title = other.title
        self.line_18 = other.line_18
        self.line_28 = other.line_28
        self.line_44 = other.line_44
        self.line_54 = other.line_54
        self.qr_fb = other.qr_fb
        self.qr_width = other.qr_width
        self.qr_height = other.qr_height
        self.qr_palette = other.qr_palette


class ErrorState:
    """Error display state. Title and lines are pre-truncated by set_error."""

    def __init__(self) -> None:
        self.title: str = ''          # Short title (e.g., "API ERROR"), <= 12 chars
        self.lines: list[str] = []    # Up to 4 pre-truncated detail lines

    def copy_from(self, other: "ErrorState") -> None:
        self.title = other.title
        self.lines = other.lines


class UpdatingState:
    """OTA update screen state. Strings are pre-built by set_updating_*."""

    def __init__(self) -> None:
        self.progress: int = 0        # Progress bar fill, 0-100
        self.percent_text: str = ''   # e.g. "34%" — beside the bar; '' hides it
        self.phase: str = ''          # "Downloading" / "Restarting in N"
        self.detail: str = ''         # e.g. "v a3f9c21"

    def copy_from(self, other: "UpdatingState") -> None:
        self.progress = other.progress
        self.percent_text = other.percent_text
        self.phase = other.phase
        self.detail = other.detail


class PlayState:
    """Most-recent play display state. Plain data — no methods.

    `id` is used by the poller to detect when a new play has arrived;
    `text` + `updated_ms` + `display_ms` are read by the display thread to
    render the scrolling play description for one full scroll cycle after
    each change.
    """

    def __init__(self) -> None:
        self.id: str = ''          # ESPN play id — poller compares to detect changes
        self.text: str = ''        # Play description — rendered by display thread
        self.updated_ms: int = 0   # time.ticks_ms() when id last changed
        self.display_ms: int = 0   # Window length: pause + scroll-to-end + pause
        # Pre-rendered strip of `text` (see fonts.render_strip); None when the
        # text exceeded the pool and Core 1 falls back to per-glyph drawing.
        self.strip = None

    def copy_from(self, other: "PlayState") -> None:
        self.id = other.id
        self.text = other.text
        self.updated_ms = other.updated_ms
        self.display_ms = other.display_ms
        self.strip = other.strip


class MlbGameSnapshot:
    """Current MLB game for the display thread to read. Plain data — no methods."""

    def __init__(self) -> None:
        self.game_id: str = ''
        self.live: LiveGame | None = None
        self.fetched_ms: int = 0
        self.play: PlayState = PlayState()

    def copy_from(self, other: "MlbGameSnapshot") -> None:
        self.game_id = other.game_id
        self.live = other.live
        self.fetched_ms = other.fetched_ms
        self.play.copy_from(other.play)


# Toast kinds: text renders in the bottom strip; the icon kinds render as a
# centered overlay (display._render_toast_overlay). Spinner is the in-flight
# skip indicator; lock/unlock are rotation-lock feedback.
TOAST_TEXT = 0
TOAST_LOCK = 1
TOAST_UNLOCK = 2
TOAST_SPINNER = 3

# How long a transient toast stays on screen after set_toast(). Lives here
# (not display.py) so clear_toast_if_sticky can re-stamp a cleared sticky
# toast as just-expired without importing display.
TOAST_DISPLAY_MS = 1500

# Sticky toast (an in-flight SKIP) is torn down by the poller's tick
# `finally`; this is only a belt against a logic bug stranding it on screen.
# Requests hard cap at 15s, so 20s can only be reached by a bug.
TOAST_STICKY_MAX_MS = 20_000


class ToastState:
    """Transient overlay (button feedback). Rendered for a short window.

    `kind` selects the presentation: TOAST_TEXT draws `text` in the bottom
    strip; the icon kinds draw a centered icon (text ignored). `sticky`
    toasts (a fired-but-in-flight SKIP spinner) persist until the operation
    that set them clears them, not on the usual short timer. `pulse_ms`
    stamps the start of a one-shot "rejected press" dim cycle (0 = not
    pulsing).
    """

    def __init__(self) -> None:
        self.text: str = ''
        self.kind: int = TOAST_TEXT
        self.updated_ms: int = 0   # time.ticks_ms() when the toast was set; 0 = never
        self.sticky: bool = False  # persists past TOAST_DISPLAY_MS until cleared
        self.pulse_ms: int = 0     # ticks_ms() of a rejected-press dim; 0 = none

    def copy_from(self, other: "ToastState") -> None:
        self.text = other.text
        self.kind = other.kind
        self.updated_ms = other.updated_ms
        self.sticky = other.sticky
        self.pulse_ms = other.pulse_ms


class PregameView:
    """Pre-built pregame screen data. Every string and color is finished on
    Core 0 by set_pregame; Core 1 only reads and draws.

    Cycling info (variant A/C) is expressed as parallel lists walked by the
    renderer with pure modular arithmetic (no per-frame allocation): `cycle_*`
    covers venue/first-pitch/weather (variant A), `alt_*` covers
    venue<->weather only (variant C, which shows the time statically). Lists
    are replaced wholesale by Core 0 -- never mutated in place -- so the
    reference hand-off is safe (ErrorState.lines precedent).
    """

    def __init__(self) -> None:
        # Identity of the game on screen: used to decide when to restart the
        # cycling animation (only on a view change, not on every re-poll).
        self.game_id: str = ''
        # Records (empty string == not advertised).
        self.away_wins: str = ''
        self.away_losses: str = ''
        self.home_wins: str = ''
        self.home_losses: str = ''
        self.away_record: str = ''   # horizontal "41-28" form (variant B)
        self.home_record: str = ''
        # Raw info lines (empty == absent; time empty when utc offset unknown).
        self.venue_text: str = ''
        self.time_text: str = ''
        self.weather_text: str = ''
        # Probable pitchers (empty == not advertised).
        self.away_pitcher: str = ''
        self.home_pitcher: str = ''
        # Pre-brightened team colors (RGB565).
        self.away_color: int = 0xFFFF
        self.home_color: int = 0xFFFF
        # Cycle A: venue / first pitch / weather.
        self.cycle_labels: list[str] = []
        self.cycle_texts: list[str] = []
        self.cycle_big: list[bool] = []   # True -> unscii_16 centered (no scroll)
        self.cycle_ends: list[int] = []   # cumulative dwell (ms); [-1] = full cycle
        # Cycle C: venue <-> weather only (time shown statically).
        self.alt_texts: list[str] = []
        self.alt_ends: list[int] = []
        # Pre-rendered strips parallel to cycle_texts / alt_texts (None for
        # big-font phases, which never scroll), plus named strips for the
        # non-cycling variant-B slots and the two pitcher lines.
        self.cycle_strips: list = []
        self.alt_strips: list = []
        self.venue_strip = None
        self.weather_strip = None
        self.away_pitcher_strip = None
        self.home_pitcher_strip = None

    def copy_from(self, other: "PregameView") -> None:
        self.game_id = other.game_id
        self.away_wins = other.away_wins
        self.away_losses = other.away_losses
        self.home_wins = other.home_wins
        self.home_losses = other.home_losses
        self.away_record = other.away_record
        self.home_record = other.home_record
        self.venue_text = other.venue_text
        self.time_text = other.time_text
        self.weather_text = other.weather_text
        self.away_pitcher = other.away_pitcher
        self.home_pitcher = other.home_pitcher
        self.away_color = other.away_color
        self.home_color = other.home_color
        self.cycle_labels = other.cycle_labels
        self.cycle_texts = other.cycle_texts
        self.cycle_big = other.cycle_big
        self.cycle_ends = other.cycle_ends
        self.alt_texts = other.alt_texts
        self.cycle_strips = other.cycle_strips
        self.alt_strips = other.alt_strips
        self.venue_strip = other.venue_strip
        self.weather_strip = other.weather_strip
        self.away_pitcher_strip = other.away_pitcher_strip
        self.home_pitcher_strip = other.home_pitcher_strip
        self.alt_ends = other.alt_ends


class FinalView:
    """Pre-built final screen data. Line-score rows are equal-char-count
    strings (3 chars per inning column) so the three rows measure identically
    and scroll in lockstep with zero extra mechanism (see set_final)."""

    def __init__(self) -> None:
        # Identity of the game on screen: gates the line-score scroll restart
        # (only on a view change, not on every re-poll).
        self.game_id: str = ''
        self.away_score: int = 0
        self.home_score: int = 0
        self.final_text: str = 'FINAL'   # "FINAL", "F/10" (extras), "F/OT"
        self.total_label: str = 'R'      # pinned totals header: R (MLB) / T (NBA)
        self.ls_header: str = ''          # inning numbers, 3 chars/col
        self.ls_away: str = ''            # away runs, 3 chars/col
        self.ls_home: str = ''            # home runs, 3 chars/col ("  X" for missing)
        # Pre-rendered strips of the three rows (None = fall back to glyphs).
        self.ls_header_strip = None
        self.ls_away_strip = None
        self.ls_home_strip = None
        self.home_won: bool = False
        self.away_color: int = 0xFFFF
        self.home_color: int = 0xFFFF

    def copy_from(self, other: "FinalView") -> None:
        self.game_id = other.game_id
        self.away_score = other.away_score
        self.home_score = other.home_score
        self.final_text = other.final_text
        self.total_label = other.total_label
        self.ls_header = other.ls_header
        self.ls_away = other.ls_away
        self.ls_home = other.ls_home
        self.ls_header_strip = other.ls_header_strip
        self.ls_away_strip = other.ls_away_strip
        self.ls_home_strip = other.ls_home_strip
        self.home_won = other.home_won
        self.away_color = other.away_color
        self.home_color = other.home_color


class SoccerLiveView:
    """Pre-built live soccer screen data plus the match-clock anchor.

    The clock is NOT stored as a display string: Core 0 anchors it at commit
    time (`clock_anchor_s` elapsed match seconds + `clock_anchor_ms`
    ticks_ms) and Core 1 extrapolates the displayed minute per frame with
    integer math (display._draw_soccer_clock). The clock therefore ticks
    between polls with zero Core 0 involvement — an event-loop stall (TLS
    reconnect, GC pause) can never freeze or jump it, and no per-second
    commits churn the mailbox. `clock_running` is False at halftime; the
    poller additionally clears it when the upstream value stops advancing
    between polls (stale-clock guard), so a paused match doesn't run away.
    """

    def __init__(self) -> None:
        self.game_id: str = ''
        self.away_score: int = 0
        self.home_score: int = 0
        self.clock_anchor_s: int = 0    # elapsed match seconds at anchor time
        self.clock_anchor_ms: int = 0   # time.ticks_ms() when anchored
        self.clock_running: bool = False
        self.base_min: int = 45         # stoppage threshold of current period
        self.halftime: bool = False
        self.phase_text: str = ''       # "1ST" / "2ND" / "ET" ('' at halftime)
        self.phase_long: str = ''       # "1ST HALF" / "2ND HALF" / "EXTRA TIME"
        # Last goal / red card, pre-built: "GOAL 90'+3'" over the scorer name.
        self.event_top: str = ''
        self.event_name: str = ''
        self.event_color: int = 0xFFFF
        self.event_name_strip = None

    def copy_from(self, other: "SoccerLiveView") -> None:
        self.game_id = other.game_id
        self.away_score = other.away_score
        self.home_score = other.home_score
        self.clock_anchor_s = other.clock_anchor_s
        self.clock_anchor_ms = other.clock_anchor_ms
        self.clock_running = other.clock_running
        self.base_min = other.base_min
        self.halftime = other.halftime
        self.phase_text = other.phase_text
        self.phase_long = other.phase_long
        self.event_top = other.event_top
        self.event_name = other.event_name
        self.event_color = other.event_color
        self.event_name_strip = other.event_name_strip


class SoccerFinalView:
    """Pre-built full-time soccer screen data. Scorer lines are display
    strings built on Core 0; a draw colors both teams (no dim loser)."""

    def __init__(self) -> None:
        self.game_id: str = ''
        self.away_score: int = 0
        self.home_score: int = 0
        self.home_won: bool = False
        self.draw: bool = False
        self.away_color: int = 0xFFFF
        self.home_color: int = 0xFFFF
        self.ft_text: str = 'FULL TIME'
        self.scorers_away: str = ''
        self.scorers_home: str = ''
        self.scorers_away_strip = None
        self.scorers_home_strip = None

    def copy_from(self, other: "SoccerFinalView") -> None:
        self.game_id = other.game_id
        self.away_score = other.away_score
        self.home_score = other.home_score
        self.home_won = other.home_won
        self.draw = other.draw
        self.away_color = other.away_color
        self.home_color = other.home_color
        self.ft_text = other.ft_text
        self.scorers_away = other.scorers_away
        self.scorers_home = other.scorers_home
        self.scorers_away_strip = other.scorers_away_strip
        self.scorers_home_strip = other.scorers_home_strip


class NbaLiveView:
    """Pre-built live NBA screen data.

    Unlike soccer there is no clock anchor: a basketball stop-clock cannot be
    extrapolated (no clock-running signal from ESPN), so `clock_text` is the
    exact poll-time string and is simply re-drawn until the next poll.
    Break states swap it for "HT" / "END" (accent color) — the phase, not the
    clock, is the render signal during breaks.
    """

    def __init__(self) -> None:
        self.game_id: str = ''
        self.away_score: int = 0
        self.home_score: int = 0
        self.phase_text: str = ''       # "Q3" / "OT" / "2OT" ('' at halftime)
        self.clock_text: str = ''       # "4:37" / "53.0", or "HT" / "END"
        self.clock_accent: bool = False  # break state: draw in accent color
        self.clock_low: bool = False     # sub-minute in-play clock: warning color

    def copy_from(self, other: "NbaLiveView") -> None:
        self.game_id = other.game_id
        self.away_score = other.away_score
        self.home_score = other.home_score
        self.phase_text = other.phase_text
        self.clock_text = other.clock_text
        self.clock_accent = other.clock_accent
        self.clock_low = other.clock_low


class UiColors:
    """Pre-computed UI colors (RGB565), set by Core 0."""

    def __init__(self) -> None:
        self.primary: int = 0xFFFF
        self.secondary: int = 0xFFFF
        self.accent: int = 0xFFFF
        self.clock_normal: int = 0xFFFF
        self.clock_warning: int = 0xFFFF

    def copy_from(self, other: "UiColors") -> None:
        self.primary = other.primary
        self.secondary = other.secondary
        self.accent = other.accent
        self.clock_normal = other.clock_normal
        self.clock_warning = other.clock_warning


class StateBuffer:
    """Complete display state snapshot. Pre-allocated, mutated in place."""

    def __init__(self) -> None:
        self.mode: str = 'idle'
        self.last_update_ms: int = 0
        self.animation_start_ms: int = 0   # Reset scrolling animations when state changes
        self.startup: StartupState = StartupState()
        self.setup: SetupState = SetupState()
        self.error: ErrorState = ErrorState()
        self.updating: UpdatingState = UpdatingState()
        self.ui_colors: UiColors = UiColors()
        self.game: MlbGameSnapshot = MlbGameSnapshot()
        self.pregame: PregameView = PregameView()
        self.final: FinalView = FinalView()
        self.soccer_live: SoccerLiveView = SoccerLiveView()
        self.soccer_final: SoccerFinalView = SoccerFinalView()
        self.nba_live: NbaLiveView = NbaLiveView()
        self.toast: ToastState = ToastState()
        self.home_logo: framebuf.FrameBuffer | None = None
        self.away_logo: framebuf.FrameBuffer | None = None

    def copy_from(self, other: "StateBuffer") -> None:
        self.mode = other.mode
        self.last_update_ms = other.last_update_ms
        self.animation_start_ms = other.animation_start_ms
        self.startup.copy_from(other.startup)
        self.setup.copy_from(other.setup)
        self.error.copy_from(other.error)
        self.updating.copy_from(other.updating)
        self.ui_colors.copy_from(other.ui_colors)
        self.game.copy_from(other.game)
        self.pregame.copy_from(other.pregame)
        self.final.copy_from(other.final)
        self.soccer_live.copy_from(other.soccer_live)
        self.soccer_final.copy_from(other.soccer_final)
        self.nba_live.copy_from(other.nba_live)
        self.toast.copy_from(other.toast)
        self.home_logo = other.home_logo
        self.away_logo = other.away_logo


# =============================================================================
# Triple buffering
# =============================================================================

# commit_seq wraps below MicroPython's small-int limit so incrementing never
# promotes to a heap-allocated big int. Consumers compare with != only.
_SEQ_MASK = 0x3FFFFFF


class TripleBufferedState:
    """
    Triple-buffered mailbox for one writer (Core 0) and one reader (Core 1).

    Three buffers are provably sufficient for a single reader/writer pair:
    at any moment one buffer is `latest` (published), one may be `reading`
    (latched by the display thread for the current frame), and the writer
    gets whichever buffer is neither. The writer therefore never mutates a
    buffer the reader can observe — no torn frames, no blocking.

    The lock guards only the index bookkeeping. commit() performs the
    carry-forward copy (latest -> new write buffer) outside the lock; that
    copy reads a published buffer (concurrent reads are safe) and writes the
    writer's new private buffer.
    """

    def __init__(self) -> None:
        self._buffers: list[StateBuffer] = [StateBuffer(), StateBuffer(), StateBuffer()]
        self._latest: int = 0    # Most recently committed buffer
        self._reading: int = 0   # Buffer latched by the display thread
        self._writing: int = 1   # Writer's private buffer
        self._commit_seq: int = 0
        self._lock = _thread.allocate_lock()

    def acquire_read(self) -> tuple[StateBuffer, int]:
        """Core 1: latch the latest committed buffer for this frame.

        Returns (buffer, commit_seq). The buffer stays safe to read until the
        next acquire_read() call.
        """
        with self._lock:
            self._reading = self._latest
            return self._buffers[self._reading], self._commit_seq

    def get_write(self) -> StateBuffer:
        """Core 0: the writer's private buffer (carry-forward copy of latest)."""
        return self._buffers[self._writing]

    def commit(self) -> None:
        """Core 0: publish the write buffer and prepare the next one."""
        with self._lock:
            self._latest = self._writing
            # The new write buffer is whichever one is neither published nor
            # latched by the reader (they may be the same buffer).
            if self._latest != 0 and self._reading != 0:
                self._writing = 0
            elif self._latest != 1 and self._reading != 1:
                self._writing = 1
            else:
                self._writing = 2
            self._commit_seq = (self._commit_seq + 1) & _SEQ_MASK
        # Carry forward outside the lock: the new write buffer is private to
        # this thread, and reading `latest` concurrently with Core 1 is safe.
        self._buffers[self._writing].copy_from(self._buffers[self._latest])


# Singleton instance
_state_mailbox: TripleBufferedState = TripleBufferedState()

# Phase flag: True during synchronous startup, False after finish_startup().
_startup_phase: bool = True


def acquire_display_state() -> tuple[StateBuffer, int]:
    """Core 1: latch the latest committed state for one frame. Returns (state, seq)."""
    return _state_mailbox.acquire_read()


def get_write_state() -> StateBuffer:
    """Core 0: get the write buffer. Mutate it, then call commit_state()."""
    return _state_mailbox.get_write()


def commit_state() -> None:
    """Core 0: publish the write buffer to the display thread."""
    _state_mailbox.commit()


def set_mode(mode: str) -> None:
    """Set display mode (thread-safe: writes the back buffer and commits)."""
    state = get_write_state()
    state.mode = mode
    commit_state()


def set_startup_step(step: int, total: int, operation: str, detail: str = '',
                     attempt: int = 0, attempts_total: int = 0) -> None:
    """
    Update startup progress display.

    No-op after finish_startup() is called. Pre-builds the strings the
    display thread draws so Core 1 never formats text.

    The visible step is monotonic: a caller re-entering an earlier step
    (the WiFi retry loop) keeps the bar and counter at their high-water
    mark — retries read as text + attempt dots, never as backward motion.
    `attempt`/`attempts_total` drive the dots (0 hides them).
    """
    if not _startup_phase:
        return

    state = get_write_state()
    state.mode = 'startup'
    startup = state.startup
    if step < startup.step:
        step = startup.step
    startup.step = step
    startup.total_steps = total
    startup.step_text = f"{step}/{total}"
    startup.operation = _truncate_line(operation)
    startup.detail = _truncate_line(detail)
    startup.attempt = attempt
    startup.attempts_total = attempts_total
    commit_state()


def _clear_startup_state(state: StateBuffer) -> None:
    """Reset startup fields on the write buffer; carry-forward propagates it."""
    startup = state.startup
    startup.step = 1
    startup.total_steps = 5
    startup.step_text = ''
    startup.operation = ''
    startup.detail = ''
    startup.attempt = 0
    startup.attempts_total = 0


def finish_startup(target_mode: str, **mode_kwargs) -> None:
    """
    Explicitly end startup phase and transition to runtime.

    This is the single transition point from synchronous startup to
    threaded operation. After this call, set_startup_step() becomes a no-op.

    Args:
        target_mode: 'idle', 'setup', or 'error'
        **mode_kwargs: Arguments passed to the target mode setter
    """
    global _startup_phase
    _startup_phase = False

    _clear_startup_state(get_write_state())

    if target_mode == 'setup':
        set_setup_mode(**mode_kwargs)
    elif target_mode == 'error':
        set_error(**mode_kwargs)
    else:
        set_mode(target_mode)


# =============================================================================
# WiFi QR code generation (for setup screen)
# =============================================================================

_qr_palette_buf: bytearray = bytearray(4)
_qr_palette: framebuf.FrameBuffer = framebuf.FrameBuffer(_qr_palette_buf, 2, 1, framebuf.RGB565)
_qr_palette.pixel(0, 0, 0xFFFF)  # Index 0: white (QR background/light modules)
_qr_palette.pixel(1, 0, 0x0000)  # Index 1: black (QR dark modules)


_QR_QUIET_ZONE = 4  # Minimum quiet zone per QR spec (4 modules)


def _generate_wifi_qr(ssid: str, password: str = '') -> tuple[framebuf.FrameBuffer, int, int, framebuf.FrameBuffer]:
    """
    Generate a QR code encoding WiFi credentials with quiet zone.

    Uses lazy import of miqro to avoid loading it at startup when not needed.
    The returned framebuffer includes a white quiet zone border around the QR
    code, which is required by the QR spec for reliable scanning.
    """
    from miqro import QRCode

    if password:
        wifi_str = f"WIFI:T:WPA;S:{ssid};P:{password};;"
    else:
        wifi_str = f"WIFI:T:nopass;S:{ssid};;"

    qr = QRCode(wifi_str)

    # Add quiet zone: create a larger framebuffer and blit QR into the center.
    # In MONO_HLSB, a zeroed bytearray = all pixels at index 0 = white via palette.
    pad = _QR_QUIET_ZONE
    padded_w = qr.width + pad * 2
    padded_h = qr.height + pad * 2
    row_bytes = (padded_w + 7) // 8
    padded_buf = bytearray(row_bytes * padded_h)
    padded_fb = framebuf.FrameBuffer(padded_buf, padded_w, padded_h, framebuf.MONO_HLSB)
    padded_fb.blit(qr.data, pad, pad)

    return (padded_fb, padded_w, padded_h, _qr_palette)


def set_setup_mode(reason: str, ap_ssid: str = '', ap_ip: str = '', wifi_ssid: str = '') -> None:
    """
    Set setup mode with detailed context for display.

    Thread-safe: writes to the back buffer and commits. Pre-builds the title
    and text lines the display thread draws, and generates the WiFi QR code
    (the user always needs to join the AP).
    """
    state = get_write_state()
    state.mode = 'setup'
    setup = state.setup
    setup.reason = reason
    setup.ap_ssid = ap_ssid
    setup.ap_ip = ap_ip
    setup.wifi_ssid = wifi_ssid

    shown_ssid = ap_ssid or 'scoreboard'
    shown_ip = ap_ip or '192.168.4.1'
    if reason == 'bad_auth':
        setup.title = "WRONG PASS"
        setup.line_18 = f'for "{wifi_ssid}"'
        setup.line_28 = f'Scan/join "{shown_ssid}"'
        setup.line_44 = f"Then go to {shown_ip}"
        setup.line_54 = "to fix password"
    elif reason == 'connection_failed':
        setup.title = "WIFI FAIL"
        setup.line_18 = f'"{wifi_ssid}"'
        setup.line_28 = f'Scan/join "{shown_ssid}"'
        setup.line_44 = f"Then go to {shown_ip}"
        setup.line_54 = "to reconfigure"
    else:
        setup.title = "SETUP"
        setup.line_18 = "Scan QR or join"
        setup.line_28 = f'"{shown_ssid}" WiFi'
        setup.line_44 = "Then go to"
        setup.line_54 = shown_ip

    if ap_ssid:
        try:
            qr_fb, qr_w, qr_h, qr_palette = _generate_wifi_qr(ap_ssid)
            setup.qr_fb = qr_fb
            setup.qr_width = qr_w
            setup.qr_height = qr_h
            setup.qr_palette = qr_palette
        except Exception as e:
            logger.error(f"[MAIN] qr generation failed: {e}")
            setup.qr_fb = None
            setup.qr_width = 0
            setup.qr_height = 0
            setup.qr_palette = None

    commit_state()


def set_error(title: str, lines: list[str] | None = None) -> None:
    """
    Set error mode with title and multi-line details.

    Thread-safe: writes to the back buffer and commits. Truncates the title
    and lines here so the display thread draws them verbatim.
    """
    state = get_write_state()
    state.mode = 'error'
    state.error.title = title[:12] if title else 'ERROR'
    state.error.lines = [_truncate_line(line) for line in (lines or [])[:4]]
    commit_state()


def set_updating_progress(percent: int, version_short: str) -> None:
    """
    Show the OTA update screen with download progress.

    Thread-safe: writes the back buffer and commits. Called from the OTA
    progress callback on Core 0 (once per percent change).
    """
    state = get_write_state()
    state.mode = 'updating'
    updating = state.updating
    updating.progress = percent
    updating.percent_text = f"{percent}%"
    updating.phase = "Downloading"
    updating.detail = _truncate_line(f"v {version_short}")
    commit_state()


def set_updating_countdown(seconds: int) -> None:
    """
    Show the pre-restart countdown on the OTA update screen (bar full).

    Keeps the version detail from the download phase via carry-forward.
    """
    state = get_write_state()
    state.mode = 'updating'
    updating = state.updating
    updating.progress = 100
    updating.percent_text = ''
    updating.phase = f"Restarting in {seconds}"
    commit_state()


def set_toast(text: str = '', sticky: bool = False, kind: int = TOAST_TEXT) -> None:
    """
    Show a transient overlay (button feedback): bottom-strip text
    (TOAST_TEXT) or a centered icon (lock/unlock/spinner kinds).

    Thread-safe: writes to the back buffer and commits. A non-sticky toast
    renders while it's within TOAST_DISPLAY_MS of updated_ms; a sticky one
    (an in-flight SKIP spinner) persists until clear_toast_if_sticky()
    clears it. Setting a toast resets any in-progress rejected-press dim
    pulse.
    """
    state = get_write_state()
    toast = state.toast
    toast.text = text
    toast.kind = kind
    toast.updated_ms = time.ticks_ms()
    toast.sticky = sticky
    toast.pulse_ms = 0
    commit_state()


def clear_toast_if_sticky() -> None:
    """Clear a sticky toast (no-op otherwise).

    Called from the SKIP tick's `finally` so a LOCKED/one-shot toast fired by
    an unrelated press mid-skip is never clobbered -- only the sticky SKIPPING
    toast this tick owns is torn down.

    The stamp is rewound to "just expired" rather than zeroed, and `kind` is
    kept, so the display thread's overlay fade-out eases the dim back out
    instead of snapping (it needs to know an icon toast was up).
    """
    state = get_write_state()
    toast = state.toast
    if not toast.sticky:
        return
    toast.text = ''
    toast.updated_ms = time.ticks_add(time.ticks_ms(), -TOAST_DISPLAY_MS)
    toast.sticky = False
    toast.pulse_ms = 0
    commit_state()


def pulse_toast() -> None:
    """Stamp a one-shot rejected-press dim on the current toast.

    A press that lands while a skip is already in flight is rejected (not
    re-queued); the visible toast dims one cycle as feedback. Restamps on each
    press so hammering the button dims per press.
    """
    state = get_write_state()
    state.toast.pulse_ms = time.ticks_ms()
    commit_state()


# =============================================================================
# Pregame / final screen setters (Core 0 string pre-build)
# =============================================================================
# These own the string building for the pre/post screens, mirroring the
# existing set_startup_step / set_error precedent: every string and packed
# color the display thread draws is finished here, so Core 1 never formats
# text. The preview exercises the identical path the poller's commit helpers
# will.


class _StripPool:
    """Ping-pong pair of 1-bit strip buffers for one logical text slot.

    Strips are shared by reference across the triple-buffered StateBuffers,
    and Core 1 may still be blitting the outgoing view's strip for a frame
    after a commit — so each rebuild lands in the buffer the outgoing view is
    NOT using. Two buffers suffice: rebuilds are seconds apart and Core 1
    lags the latest commit by at most one frame.

    `cap_px` must be a multiple of 8 (MONO_HLSB byte-padded rows).
    """

    def __init__(self, cap_px: int, height: int) -> None:
        self.cap_px = cap_px
        self.height = height
        size = (cap_px // 8) * height
        self._bufs = (bytearray(size), bytearray(size))
        self._idx = 0

    def render(self, text: str, font):
        """Pre-render `text` into the inactive buffer; returns a blit tuple
        or None when the text exceeds capacity (caller falls back to glyphs)."""
        self._idx ^= 1
        return render_strip(self._bufs[self._idx], self.cap_px, text, font)


# One pool per scrolling text slot, pre-allocated at import (Core 0) so the
# steady state never churns strip-sized objects (~6 KB total; heap
# fragmentation matters more than size here — see BACKLOG on TLS allocs).
_LS_HEADER_POOL = _StripPool(320, 8)     # 21 innings at 3 cols x 5 px
_LS_AWAY_POOL = _StripPool(320, 8)
_LS_HOME_POOL = _StripPool(320, 8)
_VENUE_POOL = _StripPool(256, 8)         # 51 spleen chars
_WEATHER_POOL = _StripPool(256, 8)
_AWAY_PITCHER_POOL = _StripPool(128, 8)
_HOME_PITCHER_POOL = _StripPool(128, 8)
# Sized to the wire format's 255-byte string cap: 255 unscii_16 chars x 8 px
# = 2040 px <= 2048, so NO legal play/commentary text can overflow the pool
# and the per-glyph fallback is structurally unreachable (it halved the
# effective frame rate — and with it the visible scroll speed — on long
# lines, measured 2026-07-12). Costs 8 KB (2 x 4 KB ping-pong) vs 2.5 KB at
# the old 640 px; blit cost per frame is unchanged (clipped to the region).
_PLAY_POOL = _StripPool(2048, 16)
_EVENT_NAME_POOL = _StripPool(128, 8)    # soccer scorer short name (16 unscii_8 chars)
_SCORERS_AWAY_POOL = _StripPool(320, 8)  # soccer full-time scorer lists
_SCORERS_HOME_POOL = _StripPool(320, 8)


def fit_play_text(text: str) -> str:
    """Clamp a play/commentary line to the play strip pool's capacity.

    Apply BEFORE build_play_strip / play_text_display_ms so the stored
    text, its display window, and the strip all describe the same string.
    With the pool sized past the wire format's 255-char string cap this is
    a no-op for anything the backend can send — kept as the belt that
    makes the strip's existence an invariant (the renderer has no glyph
    fallback for flashes; Core 1 must never glyph-loop one)."""
    return fit_text(text, unscii_16, _PLAY_POOL.cap_px)


def build_play_strip(text: str):
    """Pre-render a play-flash strip (Core 0, on new-play commit).

    Callers pass fit_play_text()'d text, so this never returns None."""
    return _PLAY_POOL.render(text, unscii_16)


def _pregame_phase_dwell(text_w: int, width: int) -> int:
    """Milliseconds one info phase stays up: at least PREGAME_INFO_DWELL_MS,
    and never less than one full scroll cycle of its text in `width` px."""
    max_scroll = text_w - width
    if max_scroll > 0:
        scroll_ms = (max_scroll * 1000) // screen_geometry.PREGAME_SCROLL_PX_PER_SEC
    else:
        scroll_ms = 0
    cycle = screen_geometry.PREGAME_SCROLL_PAUSE_MS + scroll_ms + screen_geometry.PREGAME_SCROLL_PAUSE_MS
    floor = screen_geometry.PREGAME_INFO_DWELL_MS
    return cycle if cycle > floor else floor


def _build_pregame_cycle(entries: list, width: int) -> tuple:
    """Build parallel (labels, texts, bigs, ends, strips) lists for the cycle.

    `entries` is a list of (label, text, big, strip); empty-text entries are
    skipped. `ends` are cumulative dwell ms so the renderer can locate the
    active phase with `elapsed % ends[-1]`. Big phases (unscii_16, centered)
    never scroll at these widths, so their dwell floors at
    PREGAME_INFO_DWELL_MS and their strip slot is None.
    """
    labels: list[str] = []
    texts: list[str] = []
    bigs: list[bool] = []
    ends: list[int] = []
    strips: list = []
    running = 0
    for label, text, big, strip in entries:
        if not text:
            continue
        labels.append(label)
        texts.append(text)
        bigs.append(big)
        strips.append(strip)
        text_w = 0 if big else measure_text(text, spleen_5x8)
        running += _pregame_phase_dwell(text_w, width)
        ends.append(running)
    return labels, texts, bigs, ends, strips


def set_pregame(game, home_logo, away_logo, utc_offset_s: int | None) -> None:
    """Publish a pregame screen from a parsed PregameGame.

    Pre-builds record strings, local first-pitch time ("7:05 PM"; omitted
    entirely when utc_offset_s is None -- a wrong-timezone time is worse than
    none), weather ("72F PARTLY CLOUDY"), the cycling info phase lists, and
    pre-brightened team colors. Logos are stored into the shared logo slots.

    The animation clock is restarted only when the displayed view identity
    (mode + game_id) changes -- a standing re-poll of the same pregame keeps
    the info cycle where it is, mirroring the live screen (which stamps
    animation_start_ms on rotation/skip, not on every poll). Everything else
    (strings, colors, logos) is rebuilt every call so late data corrections
    still flow through.
    """
    state = get_write_state()
    view_changed = state.mode != 'pregame' or state.pregame.game_id != game.game_id
    state.mode = 'pregame'
    if view_changed:
        state.animation_start_ms = time.ticks_ms()
    state.away_logo = away_logo
    state.home_logo = home_logo
    pv = state.pregame
    pv.game_id = game.game_id

    away = game.away
    home = game.home

    pv.away_wins = str(away.wins) if away.wins is not None else ''
    pv.away_losses = str(away.losses) if away.losses is not None else ''
    pv.home_wins = str(home.wins) if home.wins is not None else ''
    pv.home_losses = str(home.losses) if home.losses is not None else ''
    pv.away_record = ("%d-%d" % (away.wins, away.losses)) if away.wins is not None and away.losses is not None else ''
    pv.home_record = ("%d-%d" % (home.wins, home.losses)) if home.wins is not None and home.losses is not None else ''

    pv.venue_text = game.venue or ''

    if utc_offset_s is not None:
        tm = time.gmtime(game.start_epoch + utc_offset_s)
        hour = tm[3]
        minute = tm[4]
        ampm = 'AM' if hour < 12 else 'PM'
        h12 = hour % 12
        if h12 == 0:
            h12 = 12
        pv.time_text = "%d:%02d %s" % (h12, minute, ampm)
    else:
        pv.time_text = ''

    if game.weather_condition and game.weather_temp is not None:
        pv.weather_text = "%dF %s" % (game.weather_temp, game.weather_condition.upper())
    else:
        pv.weather_text = ''

    pv.away_pitcher = away.pitcher or ''
    pv.home_pitcher = home.pitcher or ''

    pv.away_color = _team_color_to_rgb565(away.colors.primary)
    pv.home_color = _team_color_to_rgb565(home.colors.primary)

    # Strips for the spleen text slots (big-font phases never scroll and keep
    # the glyph path — None placeholder). Venue/weather tuples are shared by
    # the A-cycle and C-alt lists (same pool render, referenced twice).
    venue_strip = _VENUE_POOL.render(pv.venue_text, spleen_5x8) if pv.venue_text else None
    weather_strip = _WEATHER_POOL.render(pv.weather_text, spleen_5x8) if pv.weather_text else None
    pv.venue_strip = venue_strip
    pv.weather_strip = weather_strip
    pv.away_pitcher_strip = (
        _AWAY_PITCHER_POOL.render(pv.away_pitcher, spleen_5x8) if pv.away_pitcher else None
    )
    pv.home_pitcher_strip = (
        _HOME_PITCHER_POOL.render(pv.home_pitcher, spleen_5x8) if pv.home_pitcher else None
    )

    width = screen_geometry.pregame_value_width()
    labels, texts, bigs, ends, strips = _build_pregame_cycle(
        [("VENUE", pv.venue_text, False, venue_strip),
         ("1ST PITCH", pv.time_text, True, None),
         ("WEATHER", pv.weather_text, False, weather_strip)],
        width,
    )
    pv.cycle_labels = labels
    pv.cycle_texts = texts
    pv.cycle_big = bigs
    pv.cycle_ends = ends
    pv.cycle_strips = strips

    _, alt_texts, _, alt_ends, alt_strips = _build_pregame_cycle(
        [("VENUE", pv.venue_text, False, venue_strip),
         ("WEATHER", pv.weather_text, False, weather_strip)],
        width,
    )
    pv.alt_texts = alt_texts
    pv.alt_ends = alt_ends
    pv.alt_strips = alt_strips

    commit_state()


def _final_ls_cell(run: int) -> str:
    """One line-score cell: 2-digit right-aligned run + trailing space (3 chars)."""
    return "%2d " % run


def _build_linescore(fv, count: int, away_line, home_line) -> None:
    """Build the final screen's per-period rows + pre-rendered strips.

    Shared by every sport that shows a line score (MLB innings, NBA
    quarters). Equal char counts guarantee lockstep scroll; they are equal
    by construction, but an anomaly (e.g. a 3-digit run) logs and pads to
    the widest rather than crash the display thread. Rows are pre-rendered
    once here; Core 1 blits a window per frame instead of glyph-looping
    ~90 chars (measured at ~41 ms/frame — the 9 FPS stutter).
    """
    header: list[str] = []
    away_row: list[str] = []
    home_row: list[str] = []
    for i in range(count):
        header.append("%2d " % (i + 1))
        away_row.append(_final_ls_cell(away_line[i]) if i < len(away_line) else " X ")
        home_row.append(_final_ls_cell(home_line[i]) if i < len(home_line) else " X ")
    fv.ls_header = "".join(header)
    fv.ls_away = "".join(away_row)
    fv.ls_home = "".join(home_row)

    n = len(fv.ls_header)
    if not (len(fv.ls_away) == n and len(fv.ls_home) == n):
        logger.error("[FINAL] linescore width mismatch h=%d a=%d o=%d" % (
            len(fv.ls_header), len(fv.ls_away), len(fv.ls_home)))
        n = max(len(fv.ls_header), len(fv.ls_away), len(fv.ls_home))
        fv.ls_header += " " * (n - len(fv.ls_header))
        fv.ls_away += " " * (n - len(fv.ls_away))
        fv.ls_home += " " * (n - len(fv.ls_home))

    fv.ls_header_strip = _LS_HEADER_POOL.render(fv.ls_header, spleen_5x8)
    fv.ls_away_strip = _LS_AWAY_POOL.render(fv.ls_away, spleen_5x8)
    fv.ls_home_strip = _LS_HOME_POOL.render(fv.ls_home, spleen_5x8)


def set_final(game, home_logo, away_logo) -> None:
    """Publish a final screen from a parsed mlb.FinalGame.

    Line-score rows (header / away / home) are built as equal-char-count
    strings -- 3 chars per inning column -- so the three rows measure
    identically in the fixed-width font and scroll in lockstep. A team with
    fewer entries than innings_played gets " X " for missing trailing columns
    (walk-off convention). Team colors are pre-brightened.

    The line-score scroll is restarted only when the displayed view identity
    (mode + game_id) changes, so a standing re-poll of the same final keeps the
    scroll in progress (same rule as set_pregame / the live screen). The rows
    themselves are rebuilt every call so a late score correction still lands.
    """
    state = get_write_state()
    view_changed = state.mode != 'final' or state.final.game_id != game.game_id
    state.mode = 'final'
    if view_changed:
        state.animation_start_ms = time.ticks_ms()
    state.away_logo = away_logo
    state.home_logo = home_logo
    fv = state.final
    fv.game_id = game.game_id

    away = game.away
    home = game.home
    innings = game.innings_played

    fv.away_score = away.score
    fv.home_score = home.score
    fv.home_won = home.score > away.score
    fv.away_color = _team_color_to_rgb565(away.colors.primary)
    fv.home_color = _team_color_to_rgb565(home.colors.primary)
    fv.final_text = ("F/%d" % innings) if innings > 9 else "FINAL"
    # Stamped every call: the write buffer carries the previous commit
    # forward, which may have been another sport's final.
    fv.total_label = 'R'

    _build_linescore(fv, innings, away.line, home.line)

    commit_state()


# =============================================================================
# Soccer screen setters (Core 0 string pre-build + clock anchoring)
# =============================================================================

# Phase strings per period, index min(half, 3): short form (variants A/C,
# where the MLB inning ordinal sat) and spelled-out form (variant B).
_SOCCER_PHASES = (("", ""), ("1ST", "1ST HALF"), ("2ND", "2ND HALF"), ("ET", "EXTRA TIME"))


def set_soccer_live(game, home_logo, away_logo, prev_clock_s: int | None = None) -> None:
    """Publish a live soccer screen from a parsed soccer.LiveGame.

    Anchors the match clock: stores the fetched elapsed seconds alongside
    time.ticks_ms() so the display thread can extrapolate the running minute
    between polls (see SoccerLiveView). `prev_clock_s` is the previous poll's
    clock_seconds for the same game: when the upstream value stops advancing
    while claiming to be in play (weather delay, stale feed), local ticking
    stops and the display holds the exact upstream value.

    Pre-builds the phase strings and the last-event lines ("GOAL 90'+3'" over
    the scorer name, in the scoring team's pre-brightened color).
    """
    state = get_write_state()
    view_changed = state.mode != 'soccer_live' or state.soccer_live.game_id != game.game_id
    state.mode = 'soccer_live'
    if view_changed:
        state.animation_start_ms = time.ticks_ms()
    state.away_logo = away_logo
    state.home_logo = home_logo
    sv = state.soccer_live
    sv.game_id = game.game_id

    sv.away_score = game.away.score
    sv.home_score = game.home.score

    sv.clock_anchor_s = game.clock_seconds
    sv.clock_anchor_ms = time.ticks_ms()
    stale = prev_clock_s is not None and prev_clock_s == game.clock_seconds
    sv.clock_running = not game.halftime and not stale
    sv.base_min = base_minutes(game.half)
    sv.halftime = game.halftime

    if game.halftime:
        # The clock region renders "HT"; the phase slots stay empty so the
        # state isn't announced twice.
        sv.phase_text = ''
        sv.phase_long = ''
    else:
        short, long_ = _SOCCER_PHASES[game.half if game.half < HALF_ET_FIRST else HALF_ET_FIRST]
        sv.phase_text = short
        sv.phase_long = long_

    ev = game.last_event
    if ev is not None:
        label = "GOAL" if ev.kind == EVENT_GOAL else "RED CARD"
        sv.event_top = label + " " + ev.clock_text if ev.clock_text else label
        sv.event_name = ev.name
        if ev.side == SIDE_HOME:
            sv.event_color = _team_color_to_rgb565(game.home.colors.primary)
        elif ev.side == SIDE_AWAY:
            sv.event_color = _team_color_to_rgb565(game.away.colors.primary)
        else:
            sv.event_color = 0xFFFF
        sv.event_name_strip = (
            _EVENT_NAME_POOL.render(sv.event_name, unscii_8) if sv.event_name else None
        )
    else:
        sv.event_top = ''
        sv.event_name = ''
        sv.event_color = 0xFFFF
        sv.event_name_strip = None

    commit_state()


def set_soccer_final(game, home_logo, away_logo) -> None:
    """Publish a full-time soccer screen from a parsed soccer.FinalGame.

    Winner emphasis mirrors set_final (winner in team color, loser DIM), but
    soccer draws are real: a level score colors both teams. Scorer lines are
    pre-rendered into strips for the Core 1 scroll fast path.
    """
    state = get_write_state()
    view_changed = state.mode != 'soccer_final' or state.soccer_final.game_id != game.game_id
    state.mode = 'soccer_final'
    if view_changed:
        state.animation_start_ms = time.ticks_ms()
    state.away_logo = away_logo
    state.home_logo = home_logo
    fv = state.soccer_final
    fv.game_id = game.game_id

    away = game.away
    home = game.home
    fv.away_score = away.score
    fv.home_score = home.score
    fv.home_won = home.score > away.score
    fv.draw = home.score == away.score
    fv.away_color = _team_color_to_rgb565(away.colors.primary)
    fv.home_color = _team_color_to_rgb565(home.colors.primary)
    fv.ft_text = 'FULL TIME'

    fv.scorers_away = away.scorers or ''
    fv.scorers_home = home.scorers or ''
    fv.scorers_away_strip = (
        _SCORERS_AWAY_POOL.render(fv.scorers_away, spleen_5x8) if fv.scorers_away else None
    )
    fv.scorers_home_strip = (
        _SCORERS_HOME_POOL.render(fv.scorers_home, spleen_5x8) if fv.scorers_home else None
    )

    commit_state()


# =============================================================================
# NBA screen setters (Core 0 string pre-build)
# =============================================================================

def set_nba_live(game, home_logo, away_logo) -> None:
    """Publish a live NBA screen from a parsed nba.LiveGame.

    Pre-builds the period chip ("Q3" / "OT") and the clock slot. The clock
    is the poll-time display string, never extrapolated (a stop-clock has no
    usable run signal); break states replace it with "HT" / "END" in the
    accent color and an in-play sub-minute clock ("53.0" — no colon) draws
    in the warning color for crunch time.
    """
    state = get_write_state()
    view_changed = state.mode != 'nba_live' or state.nba_live.game_id != game.game_id
    state.mode = 'nba_live'
    if view_changed:
        state.animation_start_ms = time.ticks_ms()
    state.away_logo = away_logo
    state.home_logo = home_logo
    nv = state.nba_live
    nv.game_id = game.game_id

    nv.away_score = game.away.score
    nv.home_score = game.home.score

    if game.phase == PHASE_HALFTIME:
        # The clock slot renders "HT"; the period chip stays empty so the
        # state isn't announced twice (soccer halftime convention).
        nv.phase_text = ''
        nv.clock_text = 'HT'
        nv.clock_accent = True
        nv.clock_low = False
    elif game.phase == PHASE_END_OF_PERIOD:
        nv.phase_text = period_name(game.period)
        nv.clock_text = 'END'
        nv.clock_accent = True
        nv.clock_low = False
    else:
        nv.phase_text = period_name(game.period)
        nv.clock_text = game.clock
        nv.clock_accent = False
        nv.clock_low = ':' not in game.clock

    commit_state()


def set_nba_final(game, home_logo, away_logo) -> None:
    """Publish a final NBA screen from a parsed nba.FinalGame.

    Reuses the (sport-agnostic) final screen wholesale: mode 'final', the
    same FinalView, geometry, and renderer as MLB — quarters in the header
    columns instead of innings, "T" over the pinned totals instead of "R",
    and "F/OT" ("F/2OT", ...) when overtime was played.
    """
    state = get_write_state()
    view_changed = state.mode != 'final' or state.final.game_id != game.game_id
    state.mode = 'final'
    if view_changed:
        state.animation_start_ms = time.ticks_ms()
    state.away_logo = away_logo
    state.home_logo = home_logo
    fv = state.final
    fv.game_id = game.game_id

    away = game.away
    home = game.home
    periods = game.periods_played

    fv.away_score = away.score
    fv.home_score = home.score
    fv.home_won = home.score > away.score
    fv.away_color = _team_color_to_rgb565(away.colors.primary)
    fv.home_color = _team_color_to_rgb565(home.colors.primary)
    if periods <= 4:
        fv.final_text = "FINAL"
    elif periods == 5:
        fv.final_text = "F/OT"
    else:
        fv.final_text = "F/%dOT" % (periods - 4)
    fv.total_label = 'T'

    _build_linescore(fv, periods, away.line, home.line)

    commit_state()


# =============================================================================
# Pre-computed display values (set by Core 0, read by Core 1)
# =============================================================================

def update_ui_colors(config: Config) -> None:
    """Pre-compute UI colors on Core 0. Call at startup and when config changes."""
    from scoreboard.fonts import rgb565

    def to_rgb565(color_dict: dict) -> int:
        return rgb565(color_dict["r"], color_dict["g"], color_dict["b"])

    state = get_write_state()
    colors = state.ui_colors
    colors.primary = to_rgb565(config.get_color('primary'))
    colors.secondary = to_rgb565(config.get_color('secondary'))
    colors.accent = to_rgb565(config.get_color('accent'))
    colors.clock_normal = to_rgb565(config.get_color('clock_normal'))
    colors.clock_warning = to_rgb565(config.get_color('clock_warning'))
    commit_state()
    logger.debug("[CONFIG] ui colors updated from config")


# =============================================================================
# Display driver frequency control
# =============================================================================

_display_driver: Hub75Driver | None = None

# The Regions instance, registered by main.py after display init, so config
# updates can rebuild the variant-driven region tables at runtime (same
# registration pattern as the driver below).
_display_regions = None


def set_display_driver(driver: Hub75Driver) -> None:
    """Set the display driver reference for runtime frequency updates."""
    global _display_driver
    _display_driver = driver


def set_display_regions(regions) -> None:
    """Register the Regions instance for runtime variant rebuilds."""
    global _display_regions
    _display_regions = regions


def update_screen_variants(config: Config) -> None:
    """Apply the configured screen-layout variants (config display.variants).

    Sets the screen_geometry selectors first, then rebuilds the variant
    Regions (when registered — at boot this runs before display init to
    seed the selectors, and Regions are then built directly from them).
    Applied live: the game-facing screens re-read geometry every frame.
    Scroll dwell strings pre-built against the previous variant's widths
    (set_pregame) refresh on the next poll commit.
    """
    v = config.screen_variants
    active = screen_geometry.set_variants(
        v.get("pregame"), v.get("final"), v.get("soccer_live")
    )
    if _display_regions is not None:
        _display_regions.rebuild_variant_regions()
    logger.debug("[CONFIG] screen variants: pregame=%s final=%s soccer=%s" % active)


def update_show_dividers(config: Config) -> None:
    """Apply config display.show_dividers (live; read per frame)."""
    on = screen_geometry.set_show_dividers(config.show_dividers)
    logger.debug("[CONFIG] dividers: %s" % ("on" if on else "off"))


def update_scroll_speed(config: Config) -> None:
    """Apply config display.scroll_speed_px_per_sec (live; read per use)."""
    v = screen_geometry.set_scroll_speed(config.scroll_speed_px_per_sec)
    logger.debug("[CONFIG] scroll speed: %d px/s" % v)


def update_display_frequency(config: Config) -> None:
    """Update display data frequency at runtime."""
    if _display_driver is None:
        return

    data_freq = config.data_frequency_hz
    _display_driver.set_frequency(data_freq)
    logger.debug(f"[CONFIG] display frequency updated: {data_freq // 1000}kHz")


def update_display_refresh_rate(config: Config) -> None:
    """Update display target refresh rate at runtime."""
    if _display_driver is None:
        return

    rate = _display_driver.set_target_refresh_rate(config.target_refresh_rate)
    logger.debug(f"[CONFIG] display refresh rate updated: {rate:.1f}Hz")


def update_display_gamma(config: Config) -> None:
    """Update display gamma correction at runtime."""
    if _display_driver is None:
        return

    gamma_value = config.gamma
    _display_driver.set_gamma(gamma_value)
    if gamma_value is None:
        logger.debug("[CONFIG] display gamma updated: none (linear)")
    elif isinstance(gamma_value, gamma_mod.Power):
        logger.debug(f"[CONFIG] display gamma updated: power={gamma_value.value}")
    else:
        logger.debug("[CONFIG] display gamma updated: srgb")


def update_display_blanking_time(config: Config) -> None:
    """Update display blanking (dead) time at runtime."""
    if _display_driver is None:
        return

    _display_driver.set_blanking_time(config.blanking_time_ns)
    rate = _display_driver.set_target_refresh_rate(config.target_refresh_rate)
    logger.debug(f"[CONFIG] display blanking time updated: {config.blanking_time_ns}ns (refresh recomputed: {rate:.1f}Hz)")
