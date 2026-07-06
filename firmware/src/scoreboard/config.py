"""
Configuration management for the Pico Scoreboard.

Handles reading and writing config.json with sensible defaults.
The config file is stored at the root of the Pico filesystem.
"""

import json
from hub75 import gamma
# Module-path import (not `from scoreboard import logger`): config is itself
# imported during the scoreboard package's __init__, and binding the submodule
# by full path is safe regardless of the package's partial-init state.
import scoreboard.logger as logger
from scoreboard.logger import NONE, ERROR, DEBUG

_LOG_LEVEL_MAP = {"none": NONE, "error": ERROR, "debug": DEBUG}

# RP2350 hardware watchdog limits (machine.WDT): max ~8.3s, and we keep a
# sane floor so the feeder (timeout/4) never has to run more than ~2x/sec.
_WDT_TIMEOUT_MIN_MS = 2000
_WDT_TIMEOUT_MAX_MS = 8300

# Default config path on Pico filesystem
CONFIG_PATH = "/config.json"

# Default configuration values
_DEFAULTS = {
    "network": {
        "ssid": "",
        "password": "",
        "device_name": "scoreboard",
        "connect_timeout_seconds": 60
    },
    "api": {
        "url": "",
        "key": ""
    },
    "display": {
        "brightness": 100,
        "poll_interval_seconds": 30,
        "game_rotation_seconds": 60,
        "data_frequency_khz": 20000,
        "target_refresh_rate": 120,
        "gamma": {"type": "srgb"},
        "blanking_time_ns": 0
    },
    "colors": {
        "primary": {"r": 255, "g": 255, "b": 255},      # White - dividers, status text
        "secondary": {"r": 128, "g": 128, "b": 128},    # Gray - venue, subtle elements
        "accent": {"r": 255, "g": 255, "b": 0},         # Yellow - highlights, time
        "clock_normal": {"r": 0, "g": 255, "b": 0},     # Green - clock with time remaining
        "clock_warning": {"r": 255, "g": 10, "b": 10}   # Red - low time, errors
    },
    "log": {
        "level": "debug"
    },
    "server": {
        "cache_max_age_seconds": 600
    },
    # Hardware watchdog. Default OFF: once armed, machine.WDT cannot be
    # disarmed, and it will reboot the device ~timeout_ms after mpremote
    # interrupts the script — enable per-device once it's deployed/stable.
    "watchdog": {
        "enabled": False,
        "timeout_ms": 8000
    }
}


def _deep_merge(base: dict, override: dict) -> dict:
    """
    Deep merge two dictionaries.

    Values from override take precedence. Nested dicts are merged recursively.
    """
    result = base.copy()

    for key, value in override.items():
        if key in result and isinstance(result[key], dict) and isinstance(value, dict):
            result[key] = _deep_merge(result[key], value)
        else:
            result[key] = value

    return result


class CadenceError(ValueError):
    """Raised when poll_interval_seconds >= game_rotation_seconds."""
    pass


def _validate_cadence(poll_interval: int, rotation: int) -> None:
    # Rotation must strictly exceed poll interval so the inner poll for the
    # current game fires at least once before rotation advances the index.
    if poll_interval >= rotation:
        raise CadenceError(
            f"poll_interval_seconds ({poll_interval}) must be < game_rotation_seconds ({rotation})"
        )


def _deep_copy(d: dict) -> dict:
    """Create a deep copy of a nested dictionary."""
    result = {}
    for key, value in d.items():
        if isinstance(value, dict):
            result[key] = _deep_copy(value)
        else:
            result[key] = value
    return result


class Config:
    """
    Configuration manager for the Pico Scoreboard.

    Reads config.json on initialization, merging with defaults for any
    missing values. Provides property accessors for common settings and
    methods to update and save the configuration.

    Example usage:
        cfg = Config()
        print(cfg.api_url)
        cfg.update("display", "brightness", 80)
    """

    def __init__(self, path: str = CONFIG_PATH) -> None:
        """
        Initialize configuration from file.

        Args:
            path: Path to config.json (default: /config.json)
        """
        self._path: str = path
        self._data: dict = self._load()
        self._log_level: int = self._compute_log_level()
        logger.set_level(self._log_level)

    def _load(self) -> dict:
        """Load config from file, merging with defaults.

        Never raises: a corrupt or hand-edited config file must not be able
        to brick boot (Config() is constructed at import time in main.py).
        Invalid values fall back to defaults with a logged complaint.
        """
        try:
            with open(self._path, 'r') as f:
                data = json.load(f)

            merged = _deep_merge(_deep_copy(_DEFAULTS), data)
        except (OSError, ValueError):
            merged = _deep_copy(_DEFAULTS)

        try:
            _validate_cadence(
                merged["display"]["poll_interval_seconds"],
                merged["display"]["game_rotation_seconds"],
            )
        except CadenceError as e:
            # logger.error is safe here: the module default level is DEBUG
            # until this Config finishes loading and pushes the real level.
            logger.error(f"[CONFIG] invalid cadence in {self._path}, using defaults: {e}")
            merged["display"]["poll_interval_seconds"] = _DEFAULTS["display"]["poll_interval_seconds"]
            merged["display"]["game_rotation_seconds"] = _DEFAULTS["display"]["game_rotation_seconds"]
        return merged

    def _compute_log_level(self) -> int:
        return _LOG_LEVEL_MAP.get(self._data["log"]["level"], DEBUG)

    def reload(self) -> None:
        """Reload configuration from file."""
        self._data = self._load()
        self._log_level = self._compute_log_level()
        logger.set_level(self._log_level)
        logger.debug(f"[CONFIG] reloaded: {self._path}")

    def save(self) -> None:
        """Write current configuration to file."""
        with open(self._path, 'w') as f:
            json.dump(self._data, f)

    def update(self, section: str, key: str, value: object) -> None:
        """
        Update a configuration value and save to file.

        Args:
            section: Top-level section (e.g., "network", "api", "display")
            key: Key within section (e.g., "ssid", "url", "brightness")
            value: New value to set

        Raises:
            CadenceError: If the write would violate poll_interval < game_rotation.
        """
        if section not in self._data:
            return

        if section == "display" and key in ("poll_interval_seconds", "game_rotation_seconds"):
            display = self._data["display"]
            poll = value if key == "poll_interval_seconds" else display["poll_interval_seconds"]
            rotation = value if key == "game_rotation_seconds" else display["game_rotation_seconds"]
            _validate_cadence(int(poll), int(rotation))  # type: ignore[arg-type]

        self._data[section][key] = value
        if section == "log":
            self._log_level = self._compute_log_level()
            logger.set_level(self._log_level)
        self.save()
        logger.debug(f"[CONFIG] updated: {section}.{key}={value}")

    def update_many(self, data: dict) -> None:
        """
        Merge a {section: {key: value}} update into the config with ONE flash
        write, validating cross-key invariants against the merged result.

        Unknown sections and non-dict section values are ignored (same policy
        as update()). Raises CadenceError before anything is applied if the
        merged poll/rotation pair would be invalid.
        """
        # Validate the cadence pair as it will exist AFTER the merge, so a
        # jointly-valid pair can't be rejected for arriving in the "wrong"
        # key order (and a jointly-invalid one can't slip through).
        display = data.get("display")
        if isinstance(display, dict) and (
            "poll_interval_seconds" in display or "game_rotation_seconds" in display
        ):
            current = self._data["display"]
            poll = display.get("poll_interval_seconds", current["poll_interval_seconds"])
            rotation = display.get("game_rotation_seconds", current["game_rotation_seconds"])
            _validate_cadence(int(poll), int(rotation))  # type: ignore[arg-type]

        changed = False
        for section, values in data.items():
            if section not in self._data or not isinstance(values, dict):
                continue
            for key, value in values.items():
                self._data[section][key] = value
                changed = True

        if not changed:
            return
        if "log" in data:
            self._log_level = self._compute_log_level()
            logger.set_level(self._log_level)
        self.save()
        logger.debug(f"[CONFIG] updated: {', '.join(data.keys())} (batched)")

    def get(self, section: str, key: str, default: object = None) -> object:
        """
        Get a configuration value.

        Args:
            section: Top-level section
            key: Key within section
            default: Default value if not found

        Returns:
            The configuration value or default
        """
        if section in self._data and key in self._data[section]:
            return self._data[section][key]
        return default

    @property
    def raw(self) -> dict:
        """Get the raw configuration dictionary."""
        return self._data

    # Network properties
    @property
    def ssid(self) -> str:
        """WiFi network name to connect to in station mode."""
        return self._data["network"]["ssid"]

    @property
    def password(self) -> str:
        """WiFi password for station mode."""
        return self._data["network"]["password"]

    @property
    def device_name(self) -> str:
        """Device name used for mDNS hostname and AP SSID."""
        return self._data["network"]["device_name"]

    @property
    def connect_timeout_seconds(self) -> int:
        """How long to wait for WiFi connection before falling back to AP mode."""
        return self._data["network"]["connect_timeout_seconds"]

    # API properties
    @property
    def api_url(self) -> str:
        """Backend API base URL (no trailing slash)."""
        return self._data["api"]["url"]

    @property
    def api_key(self) -> str:
        """API key for X-Api-Key header."""
        return self._data["api"]["key"]

    # Display properties
    @property
    def brightness(self) -> int:
        """LED display brightness (0-100)."""
        return self._data["display"]["brightness"]

    @property
    def poll_interval_seconds(self) -> int:
        """How often to poll the API in seconds."""
        return self._data["display"]["poll_interval_seconds"]

    @property
    def game_rotation_seconds(self) -> int:
        """How often to rotate to the next live game in seconds."""
        return self._data["display"]["game_rotation_seconds"]

    @property
    def data_frequency_khz(self) -> int:
        """Data clock frequency in kHz (2-50000)."""
        return self._data["display"]["data_frequency_khz"]

    @property
    def data_frequency_hz(self) -> int:
        """Data clock frequency in Hz (for driver)."""
        return self._data["display"]["data_frequency_khz"] * 1_000

    @property
    def target_refresh_rate(self) -> float:
        """Target display refresh rate in Hz (30-240)."""
        return float(self._data["display"]["target_refresh_rate"])

    @property
    def gamma(self) -> gamma.SRGB | gamma.Power | None:
        """Gamma correction setting (SRGB, Power, or None)."""
        raw = self._data["display"]["gamma"]
        t = raw.get("type", "srgb")
        if t == "power":
            return gamma.Power(raw.get("value", 2.2))
        elif t == "none":
            return None
        else:
            return gamma.SRGB()

    @property
    def blanking_time_ns(self) -> int:
        """Blanking time in nanoseconds (0-3000)."""
        return self._data["display"]["blanking_time_ns"]

    # Server properties
    @property
    def cache_max_age_seconds(self) -> int:
        """Cache-Control max-age for static content (0 = no caching)."""
        return self._data["server"]["cache_max_age_seconds"]

    # Watchdog properties
    @property
    def watchdog_enabled(self) -> bool:
        """Whether the hardware watchdog is armed at runtime."""
        return bool(self._data["watchdog"]["enabled"])

    @property
    def watchdog_timeout_ms(self) -> int:
        """Hardware watchdog timeout, clamped to the RP2350's valid range."""
        raw = int(self._data["watchdog"]["timeout_ms"])
        if raw < _WDT_TIMEOUT_MIN_MS:
            return _WDT_TIMEOUT_MIN_MS
        if raw > _WDT_TIMEOUT_MAX_MS:
            return _WDT_TIMEOUT_MAX_MS
        return raw

    # Log properties
    @property
    def log_level(self) -> int:
        """Log level as integer: NONE=0, ERROR=1, DEBUG=2.

        Cached as a plain int (recomputed on load/update) because this is
        checked before every log statement, including on hot paths.
        """
        return self._log_level

    # Color properties
    def get_color(self, name: str) -> dict:
        """
        Get RGB color dict by name.

        Args:
            name: Color name (primary, secondary, accent, clock_normal, clock_warning)

        Returns:
            Dict with r, g, b keys (0-255 values)
        """
        return self._data["colors"].get(name, _DEFAULTS["colors"].get(name))
