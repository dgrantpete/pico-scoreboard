"""
Global display state for the Pico Scoreboard.

Shared between networking thread (Core 0) and display thread (Core 1).
Uses double buffering for lock-free thread-safe state sharing:
- Networking thread writes to back buffer
- Display thread reads from front buffer
- Atomic pointer swap when update is complete
"""

import time


class DoubleBufferedState:
    """
    Lock-free double buffering for thread-safe state sharing.

    The networking thread writes complete state updates to the back buffer,
    then calls swap() to atomically make them visible to the display thread.
    The display thread reads from the front buffer without any locking.
    """

    def __init__(self):
        # Pre-allocate both buffers with identical structure
        self._buffers = [
            self._create_state_buffer(),
            self._create_state_buffer(),
        ]
        self._front_index = 0  # Display reads from this
        # back index = 1 - front_index

    def _create_state_buffer(self):
        """Create a complete state buffer with all fields."""
        return {
            'mode': 'idle',
            'game': None,
            'games': [],
            'game_index': 0,
            'home_logo': None,   # FrameBuffer or None
            'away_logo': None,   # FrameBuffer or None
            'error_message': None,
            'last_update_ms': 0,
            'dirty': True,
            'clock_dirty': False,
            'clock_seconds': None,
            'clock_last_tick_ms': 0,
            'animation_start_ms': 0,  # Reset scrolling animations on each API poll
            # Score flash animation timestamps (set by Core 0 when score changes)
            'home_scored_ms': 0,
            'away_scored_ms': 0,
            'startup': {
                'step': 1,
                'total_steps': 5,
                'operation': '',
                'detail': '',
            },
            'setup': {
                'reason': None,      # 'no_config' | 'connection_failed' | 'bad_auth'
                'ap_ssid': '',       # AP network name to connect to
                'ap_ip': '',         # IP address to open in browser
                'wifi_ssid': '',     # Failed SSID (for error context)
                # Dynamic QR code (generated on Core 0)
                'qr_fb': None,       # FrameBuffer (MONO_HLSB format)
                'qr_width': 0,       # QR code width in pixels
                'qr_height': 0,      # QR code height in pixels
                'qr_palette': None,  # RGB565 palette for display blitting
            },
            'error': {
                'title': '',         # Short title (e.g., "API ERROR")
                'lines': [],         # Up to 4 detail lines
            },
            # Pre-computed UI colors (RGB565) - set by Core 0 via update_ui_colors()
            'ui_colors': {
                'primary': 0xFFFF,
                'secondary': 0xFFFF,
                'accent': 0xFFFF,
                'clock_normal': 0xFFFF,
                'clock_warning': 0xFFFF,
            },
            # Pre-formatted display strings - set by Core 0 when game updates
            # Scores stay as integers on game object, rendered via writer.integer()
            'display': {
                'quarter': '',         # "Q1", "Q2", "OT", etc.
                'situation': '',       # "3rd & 7" or ''
                'possession': '',      # "home", "away", or '' (for possession arrow)
                'pregame_date': '',    # "SUN 01/15"
                'pregame_time': '',    # "7:30 PM"
                'last_play_text': '',  # Last play description for live games
            },
        }

    def get_front(self) -> dict:
        """Get the front buffer for reading (display thread)."""
        return self._buffers[self._front_index]

    def get_back(self) -> dict:
        """Get the back buffer for writing (networking thread)."""
        return self._buffers[1 - self._front_index]

    def swap(self):
        """
        Swap front and back buffers.

        Called by networking thread after completing a state update.
        This is atomic in Python (single integer assignment).
        """
        self._front_index = 1 - self._front_index


# Singleton instance for double buffering
_double_buffer = DoubleBufferedState()

# Phase flag: True during synchronous startup, False after display thread takes over
_startup_phase = True


def get_display_state():
    """Get front buffer for display thread to read (lock-free)."""
    return _double_buffer.get_front()


def get_write_state():
    """Get back buffer for networking thread to write."""
    return _double_buffer.get_back()


def commit_state():
    """Swap buffers - makes back buffer visible to display thread."""
    _double_buffer.swap()


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


def set_mode(mode, error_message=None):
    """
    Set display mode (called during setup/error states).

    Thread-safe: writes to back buffer and commits.

    Args:
        mode: One of 'idle', 'game', 'setup', 'error'
        error_message: Optional error text (only used when mode='error')
    """
    state = get_write_state()
    state['mode'] = mode
    state['error_message'] = error_message
    state['dirty'] = True
    commit_state()


def mark_dirty():
    """Mark display state as needing a redraw (thread-safe)."""
    state = get_write_state()
    state['dirty'] = True
    commit_state()


def set_startup_step(step, total, operation, detail=''):
    """
    Update startup progress display.

    No-op after finish_startup() is called. During startup phase,
    writes to BOTH buffers since there's no race condition yet.

    Args:
        step: Current step number (1-based)
        total: Total number of steps
        operation: Short operation name (e.g., "WiFi scan")
        detail: Optional detail text (e.g., "Found 8 networks")
    """
    if not _startup_phase:
        return  # Ignore calls after startup phase ends

    # Write to BOTH buffers during startup (no race condition yet)
    for buf in _double_buffer._buffers:
        buf['mode'] = 'startup'
        buf['startup']['step'] = step
        buf['startup']['total_steps'] = total
        buf['startup']['operation'] = operation
        buf['startup']['detail'] = detail
        buf['dirty'] = True


def clear_startup_state():
    """Clear startup state after boot completes to free memory."""
    # Clear in both buffers to ensure consistency
    for buf in _double_buffer._buffers:
        buf['startup'] = {
            'step': 1,
            'total_steps': 5,
            'operation': '',
            'detail': '',
        }


def finish_startup(target_mode, **mode_kwargs):
    """
    Explicitly end startup phase and transition to runtime.

    This is the single transition point from synchronous startup to
    threaded operation. After this call, set_startup_step() becomes a no-op.

    Args:
        target_mode: 'idle', 'setup', or 'error'
        **mode_kwargs: Arguments passed to the target mode setter
            - For 'setup': reason, ap_ssid, ap_ip, wifi_ssid
            - For 'error': title, lines
    """
    global _startup_phase
    _startup_phase = False

    # Clear startup state in both buffers
    clear_startup_state()

    # Set the target mode (these use back buffer + swap)
    if target_mode == 'setup':
        set_setup_mode(**mode_kwargs)
    elif target_mode == 'error':
        set_error(**mode_kwargs)
    else:
        set_mode(target_mode)


def set_setup_mode(reason, ap_ssid='', ap_ip='', wifi_ssid=''):
    """
    Set setup mode with detailed context for display.

    Thread-safe: writes to back buffer and commits.
    Generates WiFi QR code for all setup reasons (user always needs to join AP).

    Args:
        reason: 'no_config' | 'connection_failed' | 'bad_auth'
        ap_ssid: Access point SSID (device name)
        ap_ip: Access point IP address
        wifi_ssid: Configured WiFi SSID (for failed connections)
    """
    state = get_write_state()
    state['mode'] = 'setup'
    state['setup']['reason'] = reason
    state['setup']['ap_ssid'] = ap_ssid
    state['setup']['ap_ip'] = ap_ip
    state['setup']['wifi_ssid'] = wifi_ssid

    # Generate WiFi QR code for any setup mode
    # User always needs to (re)join the AP network to access config page
    if ap_ssid:
        try:
            from lib.scoreboard.qr_generator import generate_wifi_qr
            qr_fb, qr_w, qr_h, qr_palette = generate_wifi_qr(ap_ssid)
            state['setup']['qr_fb'] = qr_fb
            state['setup']['qr_width'] = qr_w
            state['setup']['qr_height'] = qr_h
            state['setup']['qr_palette'] = qr_palette
        except Exception as e:
            print(f"QR generation failed: {e}")
            state['setup']['qr_fb'] = None
            state['setup']['qr_width'] = 0
            state['setup']['qr_height'] = 0
            state['setup']['qr_palette'] = None

    state['dirty'] = True
    commit_state()


def set_error(title, lines=None):
    """
    Set error mode with title and multi-line details.

    Thread-safe: writes to back buffer and commits.

    Args:
        title: Short error title (max ~12 chars for unscii_16)
        lines: List of detail strings (up to 4 lines)
    """
    state = get_write_state()
    state['mode'] = 'error'
    state['error']['title'] = title[:12] if title else 'ERROR'
    state['error']['lines'] = lines[:4] if lines else []
    state['dirty'] = True
    commit_state()


# =============================================================================
# Pre-computed display values (set by Core 0, read by Core 1)
# =============================================================================

# Module-level lookup dicts - allocated once at import, not per call
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

# Day name abbreviations for pregame date parsing
_DAY_NAMES = ("MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN")


def update_ui_colors(config):
    """
    Pre-compute UI colors on Core 0. Call at startup and when config changes.

    Updates both buffers to ensure consistency regardless of which is active.

    Args:
        config: Config instance with get_color() method
    """
    from lib.color import rgb565

    def to_rgb565(color_dict):
        return rgb565(color_dict["r"], color_dict["g"], color_dict["b"])

    # Update both buffers to ensure consistency
    for buf in _double_buffer._buffers:
        colors = buf['ui_colors']
        colors['primary'] = to_rgb565(config.get_color('primary'))
        colors['secondary'] = to_rgb565(config.get_color('secondary'))
        colors['accent'] = to_rgb565(config.get_color('accent'))
        colors['clock_normal'] = to_rgb565(config.get_color('clock_normal'))
        colors['clock_warning'] = to_rgb565(config.get_color('clock_warning'))
        buf['dirty'] = True


# =============================================================================
# Display driver frequency control
# =============================================================================

# Module-level driver reference (set by main.py after init)
_display_driver = None


def set_display_driver(driver):
    """
    Set the display driver reference for runtime frequency updates.

    Called once from main.py after display initialization.

    Args:
        driver: Hub75Driver instance
    """
    global _display_driver
    _display_driver = driver


def update_display_frequency(config):
    """
    Update display data frequency at runtime.

    Called when display.data_frequency_khz changes via API.
    Safe to call from Core 0 - directly updates PIO clock divider registers.

    Args:
        config: Config instance with frequency settings
    """
    if _display_driver is None:
        return

    data_freq = config.data_frequency_hz
    _display_driver.set_frequency(data_freq)
    print(f"Display frequency updated: data={data_freq // 1000}kHz")


def _recompute_refresh_rate(config):
    """
    Recompute base_cycles after changing brightness or blanking time.

    The driver's set_target_refresh_rate() runs a binary search that accounts
    for the current brightness and blanking_time when computing base_cycles.
    Without this, changing brightness or blanking_time alone would alter the
    timing buffer without adjusting base_cycles, causing unexpected dimming.
    """
    rate = _display_driver.set_target_refresh_rate(config.target_refresh_rate)
    print(f"Display refresh rate recomputed: {rate:.1f} Hz")


def update_display_brightness(config):
    """
    Update display brightness at runtime.

    Args:
        config: Config instance with brightness setting (0-100)
    """
    if _display_driver is None:
        return

    brightness = config.brightness / 100.0
    _display_driver.set_brightness(brightness)
    _recompute_refresh_rate(config)
    print(f"Display brightness updated: {config.brightness}%")


def update_display_refresh_rate(config):
    """
    Update display target refresh rate at runtime.

    Args:
        config: Config instance with target_refresh_rate setting (Hz)
    """
    if _display_driver is None:
        return

    rate = _display_driver.set_target_refresh_rate(config.target_refresh_rate)
    print(f"Display refresh rate updated: {rate:.1f} Hz")


def update_display_gamma(config):
    """
    Update display gamma correction at runtime.

    Args:
        config: Config instance with gamma setting (1.0-3.0)
    """
    if _display_driver is None:
        return

    _display_driver.set_gamma(config.gamma)
    print(f"Display gamma updated: {config.gamma}")


def update_display_blanking_time(config):
    """
    Update display blanking (dead) time at runtime.

    Args:
        config: Config instance with blanking_time_us setting (microseconds)
    """
    if _display_driver is None:
        return

    _display_driver.set_blanking_time(config.blanking_time_ns)
    _recompute_refresh_rate(config)
    print(f"Display blanking time updated: {config.blanking_time_ns}ns")


def format_quarter(quarter):
    """
    Format quarter for display. Uses module-level dict (no allocation).

    Args:
        quarter: Quarter string from API (e.g., "first", "second", "OT")

    Returns:
        Formatted string (e.g., "Q1", "Q2", "OT")
    """
    if not quarter:
        return ""
    return _QUARTER_MAP.get(quarter, quarter[:3].upper())


def format_situation(situation):
    """
    Format down and distance for display.

    Args:
        situation: Situation object with .down and .distance attributes

    Returns:
        Formatted string (e.g., "3rd & 7") or empty string if None
    """
    if situation is None:
        return ''
    down_str = _DOWN_MAP.get(situation.down, situation.down)
    return f"{down_str} & {situation.distance}"


def parse_pregame_datetime(iso_str):
    """
    Parse ISO datetime string for pregame display.

    Args:
        iso_str: ISO datetime string (e.g., "2024-01-15T19:30:00Z")

    Returns:
        Tuple of (date_display, time_display) strings
    """
    # Check if it looks like ISO format (contains 'T')
    if 'T' not in iso_str:
        # Old format - just return the time portion
        time_str = iso_str
        for tz in [" ET", " PT", " CT", " MT", " EST", " PST", " CST", " MST"]:
            if time_str.endswith(tz):
                time_str = time_str[:-len(tz)]
                break
        return ("", time_str)

    try:
        # Split date and time parts
        date_part = iso_str[0:10]   # "2024-01-15"
        time_part = iso_str[11:16]  # "19:30"

        # Extract year, month, day as integers
        year = int(date_part[0:4])
        month = int(date_part[5:7])
        day = int(date_part[8:10])

        # Calculate day of week using time module
        time_tuple = (year, month, day, 0, 0, 0, 0, 0)
        timestamp = time.mktime(time_tuple)
        weekday = time.localtime(timestamp)[6]  # 0=Monday, 6=Sunday
        day_abbr = _DAY_NAMES[weekday]

        # Format date as "SUN 01/15"
        date_display = f"{day_abbr} {month:02d}/{day:02d}"

        # Format time as 12-hour with AM/PM
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
        # Parsing failed - return original string
        return ("", iso_str)
