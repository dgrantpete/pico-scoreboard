use config::{Config, Environment, File};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct AppConfig {
    /// API key for authentication (required, no default - must be set via env var)
    pub api_key: String,

    /// Server configuration
    #[serde(default)]
    pub server: ServerConfig,

    /// ESPN API configuration
    #[serde(default)]
    pub espn: EspnConfig,
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
    /// ESPN API scoreboard URL (default: NFL scoreboard)
    #[serde(default = "default_scoreboard_url")]
    pub scoreboard_url: String,

    /// ESPN CDN combiner URL for team logos
    #[serde(default = "default_logo_url")]
    pub logo_url: String,

    /// User agent for ESPN requests (default: pico-scoreboard/1.0)
    #[serde(default = "default_user_agent")]
    pub user_agent: String,

    /// Request timeout in seconds (default: 10)
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
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

fn default_scoreboard_url() -> String {
    "https://site.api.espn.com/apis/site/v2/sports/football/nfl/scoreboard".to_string()
}

fn default_logo_url() -> String {
    "https://a.espncdn.com/combiner/i".to_string()
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
            scoreboard_url: default_scoreboard_url(),
            logo_url: default_logo_url(),
            user_agent: default_user_agent(),
            timeout_secs: default_timeout(),
        }
    }
}

impl AppConfig {
    pub fn load() -> Self {
        Config::builder()
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
            .expect("Failed to deserialize configuration - is APP_API_KEY set?")
    }

    /// Get the server bind address as "host:port"
    pub fn bind_address(&self) -> String {
        format!("{}:{}", self.server.host, self.server.port)
    }
}
