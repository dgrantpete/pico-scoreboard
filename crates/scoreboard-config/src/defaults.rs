//! `config.py`'s `_DEFAULTS`, one function per value.
//!
//! Functions rather than constants because that is the form
//! `#[serde(default = "...")]` takes, and routing every default through the
//! attribute is what makes deserializing a partial document equivalent to the
//! deep merge it replaces. A default that existed only in a `Default` impl
//! would apply to a missing *section* but not to a missing *key inside a
//! present section*, which is exactly the case the merge existed for.

use heapless::String;

use crate::{
    ApiConfig, ColorsConfig, DisplayConfig, GammaConfig, LogConfig, MAX_CHANNEL, NetworkConfig,
    OtaConfig, Rgb, ServerConfig, SportLeagues, SportToggle, SportsConfig, VariantsConfig,
    WatchdogConfig,
};

fn text<const N: usize>(value: &str) -> String<N> {
    let mut out = String::new();
    // Every caller passes a literal that fits; a truncation here would be a
    // typo in this file, not a runtime condition.
    let _ = out.push_str(value);
    out
}

pub(crate) fn device_name() -> String<{ crate::MAX_SSID }> {
    text("scoreboard")
}

pub(crate) fn connect_timeout_seconds() -> u32 {
    60
}

pub(crate) fn brightness() -> u8 {
    100
}

pub(crate) fn poll_interval_seconds() -> u32 {
    30
}

pub(crate) fn game_rotation_seconds() -> u32 {
    60
}

pub(crate) fn data_frequency_khz() -> u32 {
    20_000
}

pub(crate) fn target_refresh_rate() -> f64 {
    120.0
}

pub(crate) fn show_dividers() -> bool {
    true
}

/// Deliberately the render crate's own degrade target rather than a repeated
/// literal: a fresh device and a device whose stored speed is illegal have no
/// business scrolling at different rates.
pub(crate) fn scroll_speed() -> i32 {
    scoreboard_render::geometry::DEFAULT_SCROLL_SPEED
}

pub(crate) fn variant_a() -> String<{ crate::MAX_VARIANT }> {
    text("A")
}

pub(crate) fn variant_c() -> String<{ crate::MAX_VARIANT }> {
    text("C")
}

pub(crate) fn white() -> Rgb {
    Rgb::new(255, 255, 255)
}

pub(crate) fn gray() -> Rgb {
    Rgb::new(128, 128, 128)
}

pub(crate) fn yellow() -> Rgb {
    Rgb::new(255, 255, 0)
}

pub(crate) fn green() -> Rgb {
    Rgb::new(0, 255, 0)
}

pub(crate) fn red() -> Rgb {
    Rgb::new(255, 10, 10)
}

/// MLB is the one sport on by default — the others wait for their season.
pub(crate) fn mlb() -> SportToggle {
    SportToggle { enabled: true }
}

pub(crate) fn log_level() -> String<8> {
    text("debug")
}

pub(crate) fn cache_max_age_seconds() -> u32 {
    600
}

pub(crate) fn watchdog_timeout_ms() -> u32 {
    8_000
}

/// OTA defaults **on**: the whole point is that friends' devices update
/// themselves.
pub(crate) fn ota_enabled() -> bool {
    true
}

/// The channel a device follows unless somebody deliberately moves it. Every
/// gift unit is on this one and nothing in the settings page offers the other.
pub(crate) fn ota_channel() -> String<MAX_CHANNEL> {
    String::try_from("stable").expect("fits MAX_CHANNEL")
}

impl Default for NetworkConfig {
    fn default() -> NetworkConfig {
        NetworkConfig {
            ssid: String::new(),
            password: String::new(),
            device_name: device_name(),
            connect_timeout_seconds: connect_timeout_seconds(),
        }
    }
}

impl Default for ApiConfig {
    fn default() -> ApiConfig {
        ApiConfig {
            url: String::new(),
            key: String::new(),
        }
    }
}

impl Default for DisplayConfig {
    fn default() -> DisplayConfig {
        DisplayConfig {
            brightness: brightness(),
            poll_interval_seconds: poll_interval_seconds(),
            game_rotation_seconds: game_rotation_seconds(),
            data_frequency_khz: data_frequency_khz(),
            target_refresh_rate: target_refresh_rate(),
            gamma: GammaConfig::default(),
            blanking_time_ns: 0,
            variants: VariantsConfig::default(),
            show_dividers: show_dividers(),
            scroll_speed_px_per_sec: scroll_speed(),
        }
    }
}

impl Default for GammaConfig {
    fn default() -> GammaConfig {
        GammaConfig {
            kind: crate::GammaKind::Srgb,
            value: None,
        }
    }
}

impl Default for VariantsConfig {
    fn default() -> VariantsConfig {
        VariantsConfig {
            mlb_final: variant_c(),
            nba_final: variant_c(),
            football_final: variant_c(),
            soccer_live: variant_a(),
        }
    }
}

impl Default for ColorsConfig {
    fn default() -> ColorsConfig {
        ColorsConfig {
            primary: white(),
            secondary: gray(),
            accent: yellow(),
            clock_normal: green(),
            clock_warning: red(),
        }
    }
}

impl Default for SportsConfig {
    fn default() -> SportsConfig {
        SportsConfig {
            mlb: mlb(),
            nba: SportToggle::default(),
            football: SportLeagues::default(),
            soccer: SportLeagues::default(),
        }
    }
}

impl Default for LogConfig {
    fn default() -> LogConfig {
        LogConfig { level: log_level() }
    }
}

impl Default for ServerConfig {
    fn default() -> ServerConfig {
        ServerConfig {
            cache_max_age_seconds: cache_max_age_seconds(),
        }
    }
}

impl Default for WatchdogConfig {
    fn default() -> WatchdogConfig {
        WatchdogConfig {
            enabled: false,
            timeout_ms: watchdog_timeout_ms(),
        }
    }
}

impl Default for OtaConfig {
    fn default() -> OtaConfig {
        OtaConfig {
            enabled: ota_enabled(),
            channel: ota_channel(),
        }
    }
}
