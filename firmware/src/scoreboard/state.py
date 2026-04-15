"""
Global display state for the Pico Scoreboard.

Shared between networking thread (Core 0) and display thread (Core 1).
Uses double buffering with lock-protected swap for thread-safe state sharing:
- Networking thread writes to back buffer
- Display thread reads from front buffer
- Lock-protected swap + carry-forward when update is complete
"""

import _thread
import framebuf

from hub75 import Hub75Driver, gamma as gamma_mod
from scoreboard.config import Config
from scoreboard.logger import DEBUG


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


class StateBuffer:
    """Complete display state snapshot. Pre-allocated, mutated in place."""

    def __init__(self) -> None:
        self.mode: str = 'idle'
        self.last_update_ms: int = 0
        self.dirty: bool = True
        self.animation_start_ms: int = 0   # Reset scrolling animations when state changes
        self.startup: StartupState = StartupState()
        self.setup: SetupState = SetupState()
        self.error: ErrorState = ErrorState()
        self.ui_colors: UiColors = UiColors()


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
        back.last_update_ms = front.last_update_ms
        back.dirty = front.dirty
        back.animation_start_ms = front.animation_start_ms

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


def set_mode(mode: str) -> None:
    """
    Set display mode (called during setup/error states).

    Thread-safe: writes to back buffer and commits.
    """
    state = get_write_state()
    state.mode = mode
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
            print(f"[MAIN] qr generation failed: {e}")
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


def _recompute_refresh_rate(config: Config) -> None:
    """Recompute base_cycles after changing brightness or blanking time."""
    if _display_driver is None:
        return
    rate = _display_driver.set_target_refresh_rate(config.target_refresh_rate)
    if config.log_level >= DEBUG:
        print(f"[CONFIG] refresh rate recomputed due to blanking time change: {rate:.1f}Hz")



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
    _recompute_refresh_rate(config)
    if config.log_level >= DEBUG:
        print(f"[CONFIG] display blanking time updated: {config.blanking_time_ns}ns")
