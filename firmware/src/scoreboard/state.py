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
from scoreboard.logger import DEBUG
from scoreboard.mlb import LiveGame


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

    def copy_from(self, other: "StartupState") -> None:
        self.step = other.step
        self.total_steps = other.total_steps
        self.step_text = other.step_text
        self.operation = other.operation
        self.detail = other.detail


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

    def copy_from(self, other: "PlayState") -> None:
        self.id = other.id
        self.text = other.text
        self.updated_ms = other.updated_ms
        self.display_ms = other.display_ms


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


class ToastState:
    """Transient text overlay (button feedback). Rendered for a short window."""

    def __init__(self) -> None:
        self.text: str = ''
        self.updated_ms: int = 0   # time.ticks_ms() when the toast was set; 0 = never

    def copy_from(self, other: "ToastState") -> None:
        self.text = other.text
        self.updated_ms = other.updated_ms


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
        self.ui_colors: UiColors = UiColors()
        self.game: MlbGameSnapshot = MlbGameSnapshot()
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
        self.ui_colors.copy_from(other.ui_colors)
        self.game.copy_from(other.game)
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


def set_startup_step(step: int, total: int, operation: str, detail: str = '') -> None:
    """
    Update startup progress display.

    No-op after finish_startup() is called. Pre-builds the strings the
    display thread draws so Core 1 never formats text.
    """
    if not _startup_phase:
        return

    state = get_write_state()
    state.mode = 'startup'
    startup = state.startup
    startup.step = step
    startup.total_steps = total
    startup.step_text = f"{step}/{total}"
    startup.operation = _truncate_line(operation)
    startup.detail = _truncate_line(detail)
    commit_state()


def _clear_startup_state(state: StateBuffer) -> None:
    """Reset startup fields on the write buffer; carry-forward propagates it."""
    startup = state.startup
    startup.step = 1
    startup.total_steps = 5
    startup.step_text = ''
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
            print(f"[MAIN] qr generation failed: {e}")
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


def set_toast(text: str) -> None:
    """
    Show a transient text overlay (e.g. button feedback) for a short window.

    Thread-safe: writes to the back buffer and commits. The display thread
    renders the toast while it's within TOAST_DISPLAY_MS of updated_ms.
    """
    state = get_write_state()
    state.toast.text = text
    state.toast.updated_ms = time.ticks_ms()
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
    if config.log_level >= DEBUG:
        print("[CONFIG] ui colors updated from config")


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
    if config.log_level >= DEBUG:
        print(f"[CONFIG] display frequency updated: {data_freq // 1000}kHz")


def update_display_refresh_rate(config: Config) -> None:
    """Update display target refresh rate at runtime."""
    if _display_driver is None:
        return

    rate = _display_driver.set_target_refresh_rate(config.target_refresh_rate)
    if config.log_level >= DEBUG:
        print(f"[CONFIG] display refresh rate updated: {rate:.1f}Hz")


def update_display_gamma(config: Config) -> None:
    """Update display gamma correction at runtime."""
    if _display_driver is None:
        return

    gamma_value = config.gamma
    _display_driver.set_gamma(gamma_value)
    if config.log_level >= DEBUG:
        if gamma_value is None:
            print("[CONFIG] display gamma updated: none (linear)")
        elif isinstance(gamma_value, gamma_mod.Power):
            print(f"[CONFIG] display gamma updated: power={gamma_value.value}")
        else:
            print("[CONFIG] display gamma updated: srgb")


def update_display_blanking_time(config: Config) -> None:
    """Update display blanking (dead) time at runtime."""
    if _display_driver is None:
        return

    _display_driver.set_blanking_time(config.blanking_time_ns)
    rate = _display_driver.set_target_refresh_rate(config.target_refresh_rate)
    if config.log_level >= DEBUG:
        print(f"[CONFIG] display blanking time updated: {config.blanking_time_ns}ns (refresh recomputed: {rate:.1f}Hz)")
