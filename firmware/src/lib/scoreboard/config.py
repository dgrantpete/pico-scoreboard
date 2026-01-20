"""
Configuration management for the Pico Scoreboard.

Handles reading and writing config.json with sensible defaults.
The config file is stored at the root of the Pico filesystem.
"""

import json

# Default config path on Pico filesystem
CONFIG_PATH = "/config.json"

# Default configuration values
_DEFAULTS = {
    "network": {
        "ssid": "",
        "password": "",
        "ap_mode": True,
        "device_name": "scoreboard",
        "connect_timeout_seconds": 15
    },
    "api": {
        "url": "",
        "key": ""
    },
    "display": {
        "brightness": 100,
        "poll_interval_seconds": 30
    },
    "server": {
        "cache_max_age_seconds": 0
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
        cfg.update("game", "event_id", "401547417")
    """

    def __init__(self, path: str = CONFIG_PATH):
        """
        Initialize configuration from file.

        Args:
            path: Path to config.json (default: /config.json)
        """
        self._path = path
        self._data = self._load()

    def _load(self) -> dict:
        """Load config from file, merging with defaults."""
        try:
            with open(self._path, 'r') as f:
                data = json.load(f)

            # Migrate old hostname/ap_ssid to device_name
            if 'network' in data:
                if 'hostname' in data['network'] and 'device_name' not in data['network']:
                    data['network']['device_name'] = data['network'].pop('hostname')
                if 'ap_ssid' in data['network']:
                    del data['network']['ap_ssid']

            return _deep_merge(_deep_copy(_DEFAULTS), data)
        except (OSError, ValueError):
            # File doesn't exist or is invalid JSON - use defaults
            return _deep_copy(_DEFAULTS)

    def reload(self) -> None:
        """Reload configuration from file."""
        self._data = self._load()

    def save(self) -> None:
        """Write current configuration to file."""
        with open(self._path, 'w') as f:
            json.dump(self._data, f)

    def update(self, section: str, key: str, value) -> None:
        """
        Update a configuration value and save to file.

        Args:
            section: Top-level section (e.g., "network", "api", "game", "display")
            key: Key within section (e.g., "ssid", "url", "event_id")
            value: New value to set
        """
        if section in self._data:
            self._data[section][key] = value
            self.save()

    def get(self, section: str, key: str, default=None):
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
    def ap_mode(self) -> bool:
        """Whether to run in Access Point mode (True) or Station mode (False)."""
        return self._data["network"]["ap_mode"]

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

    # Server properties
    @property
    def cache_max_age_seconds(self) -> int:
        """Cache-Control max-age for static content (0 = no caching)."""
        return self._data["server"]["cache_max_age_seconds"]
