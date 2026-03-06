use config::{Config, Environment, File};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct AppConfig {
    /// API key for authentication. When None, auth is disabled (development mode).
    /// Set via APP_API_KEY env var or api_key in config files.
    #[serde(default)]
    pub api_key: Option<String>,

    /// Server configuration
    #[serde(default)]
    pub server: ServerConfig,

    /// ESPN API configuration
    #[serde(default)]
    pub espn: EspnConfig,

    /// GeoIP configuration
    #[serde(default)]
    pub geoip: GeoipConfig,
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    /// Host to bind to (default: 0.0.0.0)
    #[serde(default = "default_host")]
    pub host: String,

    /// Port to listen on (default: 3000)
    #[serde(default = "default_port")]
    pub port: u16,
}

#[derive(Debug, Deserialize)]
pub struct EspnConfig {
    /// ESPN API base URL for sport endpoints
    #[serde(default = "default_base_url")]
    pub base_url: String,

    /// ESPN CDN base URL for team logos
    #[serde(default = "default_logo_url")]
    pub logo_url: String,

    /// User agent for ESPN requests (default: pico-scoreboard/1.0)
    #[serde(default = "default_user_agent")]
    pub user_agent: String,

    /// Request timeout in seconds (default: 10)
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

#[derive(Debug, Deserialize)]
pub struct GeoipConfig {
    /// Path to MaxMind GeoLite2-City .mmdb file
    #[serde(default = "default_mmdb_path")]
    pub mmdb_path: String,
}

impl Default for GeoipConfig {
    fn default() -> Self {
        Self {
            mmdb_path: default_mmdb_path(),
        }
    }
}

fn default_mmdb_path() -> String {
    "/app/GeoLite2-City.mmdb".to_string()
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}

fn default_port() -> u16 {
    3000
}

fn default_timeout() -> u64 {
    10
}

fn default_base_url() -> String {
    "https://site.api.espn.com/apis/site/v2/sports".to_string()
}

fn default_logo_url() -> String {
    "https://a.espncdn.com".to_string()
}

fn default_user_agent() -> String {
    "pico-scoreboard/1.0".to_string()
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
        }
    }
}

impl Default for EspnConfig {
    fn default() -> Self {
        Self {
            base_url: default_base_url(),
            logo_url: default_logo_url(),
            user_agent: default_user_agent(),
            timeout_secs: default_timeout(),
        }
    }
}

impl AppConfig {
    pub fn load() -> Self {
        let config: Self = Config::builder()
            // 1. Base config file (committed - non-secret defaults)
            .add_source(File::with_name("config/default").required(false))
            // 2. Local config file (gitignored - secrets and local overrides)
            //    Similar to appsettings.local.json in .NET
            .add_source(File::with_name("config/local").required(false))
            // 3. Environment variables (highest priority - for production/CI)
            //    APP_API_KEY → api_key (single underscore stays in field name)
            //    APP_SERVER__PORT → server.port (double underscore = nesting)
            //    APP_ESPN__TIMEOUT_SECS → espn.timeout_secs
            .add_source(
                Environment::with_prefix("APP")
                    .prefix_separator("_")  // Handle the underscore between "APP" and the rest
                    .separator("__"),       // Double underscore for nested fields
            )
            .build()
            .expect("Failed to build configuration")
            .try_deserialize()
            .expect("Failed to deserialize configuration");

        // Normalize empty string to None so APP_API_KEY="" is treated as unconfigured
        Self {
            api_key: config.api_key.filter(|k| !k.is_empty()),
            ..config
        }
    }

    /// Get the server bind address as "host:port"
    pub fn bind_address(&self) -> String {
        format!("{}:{}", self.server.host, self.server.port)
    }
}
