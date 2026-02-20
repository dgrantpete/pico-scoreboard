"""
Global display state for the Pico Scoreboard.

Shared between networking thread (Core 0) and display thread (Core 1).
Uses double buffering with lock-protected swap for thread-safe state sharing:
- Networking thread writes to back buffer
- Display thread reads from front buffer
- Lock-protected swap + carry-forward when update is complete
"""

import time
import _thread
import framebuf

from hub75 import Hub75Driver, gamma as gamma_mod
from scoreboard.config import Config
from scoreboard.models import PregameGame, LiveGame, FinalGame, Situation


# =============================================================================
# Typed state classes
# =============================================================================

class StartupState:
    """Boot progress state."""

    def __init__(self) -> None:
        self.step: int = 1
        self.total_steps: int = 5
        self.operation: str = ''
        self.detail: str = ''


class SetupState:
    """WiFi setup / AP mode state."""

    def __init__(self) -> None:
        self.reason: str | None = None       # 'no_config' | 'connection_failed' | 'bad_auth'
        self.ap_ssid: str = ''               # AP network name to connect to
        self.ap_ip: str = ''                 # IP address to open in browser
        self.wifi_ssid: str = ''             # Failed SSID (for error context)
        self.qr_fb: framebuf.FrameBuffer | None = None        # FrameBuffer (MONO_HLSB format)
        self.qr_width: int = 0              # QR code width in pixels
        self.qr_height: int = 0             # QR code height in pixels
        self.qr_palette: framebuf.FrameBuffer | None = None   # RGB565 palette for display blitting


class ErrorState:
    """Error display state."""

    def __init__(self) -> None:
        self.title: str = ''          # Short title (e.g., "API ERROR")
        self.lines: list[str] = []    # Up to 4 detail lines


class UiColors:
    """Pre-computed UI colors (RGB565), set by Core 0."""

    def __init__(self) -> None:
        self.primary: int = 0xFFFF
        self.secondary: int = 0xFFFF
        self.accent: int = 0xFFFF
        self.clock_normal: int = 0xFFFF
        self.clock_warning: int = 0xFFFF


class DisplayStrings:
    """Pre-formatted display strings, set by Core 0 when game updates."""

    def __init__(self) -> None:
        self.quarter: str = ''         # "Q1", "Q2", "OT", etc.
        self.situation: str = ''       # "3rd & 7" or ''
        self.possession: str = ''      # "home", "away", or '' (for possession arrow)
        self.pregame_date: str = ''    # "SUN 01/15"
        self.pregame_time: str = ''    # "7:30 PM"
        self.last_play_text: str = ''  # Last play description for live games


class StateBuffer:
    """Complete display state snapshot. Pre-allocated, mutated in place."""

    def __init__(self) -> None:
        self.mode: str = 'idle'
        self.game: PregameGame | LiveGame | FinalGame | None = None
        self.games: list[PregameGame | LiveGame | FinalGame] = []
        self.game_index: int = 0
        self.home_logo: framebuf.FrameBuffer | None = None
        self.away_logo: framebuf.FrameBuffer | None = None
        self.error_message: str | None = None
        self.last_update_ms: int = 0
        self.dirty: bool = True
        self.clock_dirty: bool = False
        self.clock_seconds: int | None = None
        self.clock_last_tick_ms: int = 0
        self.animation_start_ms: int = 0   # Reset scrolling animations on each API poll
        self.home_scored_ms: int = 0       # Score flash timestamp (set by Core 0)
        self.away_scored_ms: int = 0       # Score flash timestamp (set by Core 0)
        self.startup: StartupState = StartupState()
        self.setup: SetupState = SetupState()
        self.error: ErrorState = ErrorState()
        self.ui_colors: UiColors = UiColors()
        self.display: DisplayStrings = DisplayStrings()


# =============================================================================
# Double buffering
# =============================================================================

class DoubleBufferedState:
    """
    Double buffering for thread-safe state sharing between Core 0 and Core 1.

    The networking thread writes complete state updates to the back buffer,
    then calls swap() to make them visible to the display thread.
    A lock protects swap+sync and get_front to ensure the display thread
    always captures a consistent buffer reference.
    """

    def __init__(self) -> None:
        self._buffers: list[StateBuffer] = [StateBuffer(), StateBuffer()]
        self._front_index: int = 0  # Display reads from this
        self._lock = _thread.allocate_lock()

    def get_front(self) -> StateBuffer:
        """Get the front buffer for reading (display thread)."""
        with self._lock:
            return self._buffers[self._front_index]

    def get_back(self) -> StateBuffer:
        """Get the back buffer for writing (networking thread)."""
        return self._buffers[1 - self._front_index]

    def swap(self) -> None:
        """
        Swap front and back buffers, then carry forward state.

        Called by networking thread after completing a state update.
        The lock ensures the display thread never captures a buffer
        reference during the swap+sync window.
        """
        with self._lock:
            self._front_index = 1 - self._front_index
            self._sync_after_swap()

    def _sync_after_swap(self) -> None:
        """
        Copy state from new front to new back buffer after swap.

        Ensures the writer always starts from the most recent committed
        state, preventing the back buffer from containing stale data
        from 2 cycles ago. No memory allocation — copies references
        for objects, values for scalars, field-by-field for sub-objects.
        """
        front = self._buffers[self._front_index]
        back = self._buffers[1 - self._front_index]

        # Scalar and reference fields
        back.mode = front.mode
        back.game = front.game
        back.games = front.games
        back.game_index = front.game_index
        back.home_logo = front.home_logo
        back.away_logo = front.away_logo
        back.error_message = front.error_message
        back.last_update_ms = front.last_update_ms
        back.dirty = front.dirty
        back.clock_dirty = front.clock_dirty
        back.clock_seconds = front.clock_seconds
        back.clock_last_tick_ms = front.clock_last_tick_ms
        back.animation_start_ms = front.animation_start_ms
        back.home_scored_ms = front.home_scored_ms
        back.away_scored_ms = front.away_scored_ms

        # Sub-objects: field-by-field to preserve pre-allocated instances
        back.startup.step = front.startup.step
        back.startup.total_steps = front.startup.total_steps
        back.startup.operation = front.startup.operation
        back.startup.detail = front.startup.detail

        back.setup.reason = front.setup.reason
        back.setup.ap_ssid = front.setup.ap_ssid
        back.setup.ap_ip = front.setup.ap_ip
        back.setup.wifi_ssid = front.setup.wifi_ssid
        back.setup.qr_fb = front.setup.qr_fb
        back.setup.qr_width = front.setup.qr_width
        back.setup.qr_height = front.setup.qr_height
        back.setup.qr_palette = front.setup.qr_palette

        back.error.title = front.error.title
        back.error.lines = front.error.lines

        back.ui_colors.primary = front.ui_colors.primary
        back.ui_colors.secondary = front.ui_colors.secondary
        back.ui_colors.accent = front.ui_colors.accent
        back.ui_colors.clock_normal = front.ui_colors.clock_normal
        back.ui_colors.clock_warning = front.ui_colors.clock_warning

        back.display.quarter = front.display.quarter
        back.display.situation = front.display.situation
        back.display.possession = front.display.possession
        back.display.pregame_date = front.display.pregame_date
        back.display.pregame_time = front.display.pregame_time
        back.display.last_play_text = front.display.last_play_text


# Singleton instance
_double_buffer: DoubleBufferedState = DoubleBufferedState()

# Phase flag: True during synchronous startup, False after display thread takes over
_startup_phase: bool = True


def get_display_state() -> StateBuffer:
    """Get front buffer for display thread to read."""
    return _double_buffer.get_front()


def get_write_state() -> StateBuffer:
    """Get back buffer for networking thread to write."""
    return _double_buffer.get_back()


def commit_state() -> None:
    """Swap buffers — makes back buffer visible to display thread."""
    _double_buffer.swap()


def parse_clock(clock_str: str) -> int:
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
    try:
        return int(clock_str)
    except ValueError:
        return 0


def format_clock(seconds: int) -> str:
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


def set_mode(mode: str, error_message: str | None = None) -> None:
    """
    Set display mode (called during setup/error states).

    Thread-safe: writes to back buffer and commits.
    """
    state = get_write_state()
    state.mode = mode
    state.error_message = error_message
    state.dirty = True
    commit_state()


def mark_dirty() -> None:
    """Mark display state as needing a redraw (thread-safe)."""
    state = get_write_state()
    state.dirty = True
    commit_state()


def set_startup_step(step: int, total: int, operation: str, detail: str = '') -> None:
    """
    Update startup progress display.

    No-op after finish_startup() is called. During startup phase,
    writes to BOTH buffers since there's no race condition yet.
    """
    if not _startup_phase:
        return

    for buf in _double_buffer._buffers:
        buf.mode = 'startup'
        buf.startup.step = step
        buf.startup.total_steps = total
        buf.startup.operation = operation
        buf.startup.detail = detail
        buf.dirty = True


def clear_startup_state() -> None:
    """Clear startup state after boot completes to free memory."""
    for buf in _double_buffer._buffers:
        startup = buf.startup
        startup.step = 1
        startup.total_steps = 5
        startup.operation = ''
        startup.detail = ''


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

    clear_startup_state()

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


def _generate_wifi_qr(ssid: str, password: str = '') -> tuple[framebuf.FrameBuffer, int, int, framebuf.FrameBuffer]:
    """
    Generate a QR code encoding WiFi credentials.

    Uses lazy import of miqro to avoid loading it at startup when not needed.
    """
    from miqro import QRCode

    if password:
        wifi_str = f"WIFI:T:WPA;S:{ssid};P:{password};;"
    else:
        wifi_str = f"WIFI:T:nopass;S:{ssid};;"

    qr = QRCode(wifi_str)
    return (qr.data, qr.width, qr.height, _qr_palette)


def set_setup_mode(reason: str, ap_ssid: str = '', ap_ip: str = '', wifi_ssid: str = '') -> None:
    """
    Set setup mode with detailed context for display.

    Thread-safe: writes to back buffer and commits.
    Generates WiFi QR code for all setup reasons (user always needs to join AP).
    """
    state = get_write_state()
    state.mode = 'setup'
    setup = state.setup
    setup.reason = reason
    setup.ap_ssid = ap_ssid
    setup.ap_ip = ap_ip
    setup.wifi_ssid = wifi_ssid

    if ap_ssid:
        try:
            qr_fb, qr_w, qr_h, qr_palette = _generate_wifi_qr(ap_ssid)
            setup.qr_fb = qr_fb
            setup.qr_width = qr_w
            setup.qr_height = qr_h
            setup.qr_palette = qr_palette
        except Exception as e:
            print(f"QR generation failed: {e}")
            setup.qr_fb = None
            setup.qr_width = 0
            setup.qr_height = 0
            setup.qr_palette = None

    state.dirty = True
    commit_state()


def set_error(title: str, lines: list[str] | None = None) -> None:
    """
    Set error mode with title and multi-line details.

    Thread-safe: writes to back buffer and commits.
    """
    state = get_write_state()
    state.mode = 'error'
    state.error.title = title[:12] if title else 'ERROR'
    state.error.lines = lines[:4] if lines else []
    state.dirty = True
    commit_state()


# =============================================================================
# Pre-computed display values (set by Core 0, read by Core 1)
# =============================================================================

_QUARTER_MAP = {
    "first": "Q1",
    "second": "Q2",
    "third": "Q3",
    "fourth": "Q4",
    "OT": "OT",
    "OT2": "2OT",
}

_DOWN_MAP = {
    "first": "1st",
    "second": "2nd",
    "third": "3rd",
    "fourth": "4th",
}

_DAY_NAMES = ("MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN")


def update_ui_colors(config: Config) -> None:
    """
    Pre-compute UI colors on Core 0. Call at startup and when config changes.

    Updates both buffers to ensure consistency regardless of which is active.
    """
    from scoreboard.fonts import rgb565

    def to_rgb565(color_dict: dict) -> int:
        return rgb565(color_dict["r"], color_dict["g"], color_dict["b"])

    for buf in _double_buffer._buffers:
        colors = buf.ui_colors
        colors.primary = to_rgb565(config.get_color('primary'))
        colors.secondary = to_rgb565(config.get_color('secondary'))
        colors.accent = to_rgb565(config.get_color('accent'))
        colors.clock_normal = to_rgb565(config.get_color('clock_normal'))
        colors.clock_warning = to_rgb565(config.get_color('clock_warning'))
        buf.dirty = True


# =============================================================================
# Display driver frequency control
# =============================================================================

_display_driver: Hub75Driver | None = None


def set_display_driver(driver: Hub75Driver) -> None:
    """Set the display driver reference for runtime frequency updates."""
    global _display_driver
    _display_driver = driver


def update_display_frequency(config: Config) -> None:
    """Update display data frequency at runtime."""
    if _display_driver is None:
        return

    data_freq = config.data_frequency_hz
    _display_driver.set_frequency(data_freq)
    print(f"Display frequency updated: data={data_freq // 1000}kHz")


def _recompute_refresh_rate(config: Config) -> None:
    """Recompute base_cycles after changing brightness or blanking time."""
    if _display_driver is None:
        return
    rate = _display_driver.set_target_refresh_rate(config.target_refresh_rate)
    print(f"Display refresh rate recomputed: {rate:.1f} Hz")


def update_display_brightness(config: Config) -> None:
    """Update display brightness at runtime."""
    if _display_driver is None:
        return

    brightness = config.brightness / 100.0
    _display_driver.set_brightness(brightness)
    _recompute_refresh_rate(config)
    print(f"Display brightness updated: {config.brightness}%")


def update_display_refresh_rate(config: Config) -> None:
    """Update display target refresh rate at runtime."""
    if _display_driver is None:
        return

    rate = _display_driver.set_target_refresh_rate(config.target_refresh_rate)
    print(f"Display refresh rate updated: {rate:.1f} Hz")


def update_display_gamma(config: Config) -> None:
    """Update display gamma correction at runtime."""
    if _display_driver is None:
        return

    gamma_value = config.gamma
    _display_driver.set_gamma(gamma_value)
    if gamma_value is None:
        print("Display gamma updated: none (linear)")
    elif isinstance(gamma_value, gamma_mod.Power):
        print(f"Display gamma updated: power ({gamma_value.value})")
    else:
        print("Display gamma updated: sRGB")


def update_display_blanking_time(config: Config) -> None:
    """Update display blanking (dead) time at runtime."""
    if _display_driver is None:
        return

    _display_driver.set_blanking_time(config.blanking_time_ns)
    _recompute_refresh_rate(config)
    print(f"Display blanking time updated: {config.blanking_time_ns}ns")


def format_quarter(quarter: str) -> str:
    """Format quarter for display. Uses module-level dict (no allocation)."""
    if not quarter:
        return ""
    return _QUARTER_MAP.get(quarter, quarter[:3].upper())


def format_situation(situation: Situation | None) -> str:
    """Format down and distance for display."""
    if situation is None:
        return ''
    down_str = _DOWN_MAP.get(situation.down, situation.down)
    return f"{down_str} & {situation.distance}"


def parse_pregame_datetime(iso_str: str) -> tuple[str, str]:
    """
    Parse ISO datetime string for pregame display.

    Returns:
        Tuple of (date_display, time_display) strings
    """
    if 'T' not in iso_str:
        time_str = iso_str
        for tz in [" ET", " PT", " CT", " MT", " EST", " PST", " CST", " MST"]:
            if time_str.endswith(tz):
                time_str = time_str[:-len(tz)]
                break
        return ("", time_str)

    try:
        date_part = iso_str[0:10]
        time_part = iso_str[11:16]

        year = int(date_part[0:4])
        month = int(date_part[5:7])
        day = int(date_part[8:10])

        time_tuple = (year, month, day, 0, 0, 0, 0, 0)
        timestamp = time.mktime(time_tuple)
        weekday = time.localtime(timestamp)[6]
        day_abbr = _DAY_NAMES[weekday]

        date_display = f"{day_abbr} {month:02d}/{day:02d}"

        hour = int(time_part[0:2])
        minute = time_part[3:5]
        am_pm = "AM" if hour < 12 else "PM"
        if hour == 0:
            hour = 12
        elif hour > 12:
            hour -= 12
        time_display = f"{hour}:{minute} {am_pm}"

        return (date_display, time_display)
    except (ValueError, IndexError):
        return ("", iso_str)
