//! The device configuration, and the two things `/api/config` does with it.
//!
//! Port of `scoreboard/config.py`. That module is a deep-merge of the stored
//! `/config.json` over a defaults dict, plus a pile of defensive accessors, and
//! its governing promise is that **it never raises**: `Config()` is constructed
//! at import time in `main.py`, so a corrupt or hand-edited file must not be
//! able to brick a boot. Everything here keeps that promise, by a different
//! mechanism — see [`DeviceConfig::from_json`].
//!
//! # Why this is a crate
//!
//! Three things in here are decisions about bytes with no hardware in them:
//! the merge, the cadence invariant, and the JSON shape the settings SPA reads.
//! All three are exactly what a firmware bug hides in, so they live where they
//! can be tested on a desktop (SPEC §2's crate-boundary rule) and the firmware
//! keeps only the flash writes and the driver calls.
//!
//! The shape is not ours to redesign: `frontend/src/lib/api/types.ts` and the
//! settings page read these key names as they ship today, and the parity
//! release replaces the firmware under an unchanged SPA.
//!
//! # The merge is serde's, not a dict walk
//!
//! `config.py:_deep_merge(_DEFAULTS, stored)` recursively overlaid two dicts.
//! Here every field carries `#[serde(default = "...")]` naming the same default
//! `_DEFAULTS` held, so deserializing a partial document *is* the deep merge —
//! absent keys take the default, present ones win, at every level. A stored
//! config written by an older firmware that lacked a key reads correctly, which
//! is the property the dict walk existed for.
//!
//! # Where the invariant lives
//!
//! `poll_interval_seconds < game_rotation_seconds`, strictly, so the inner poll
//! for the current game fires at least once before rotation advances the index.
//! `config.py` checked it in two places with subtly different rules;
//! [`DeviceConfig::apply`] checks it once, against the *merged* result, so a
//! jointly-valid pair cannot be rejected for arriving in the wrong key order.

#![no_std]
#![forbid(unsafe_code)]

use heapless::{String, Vec};
use serde::{Deserialize, Deserializer, Serialize};

pub use scoreboard_log::Level as LogLevel;

mod defaults;
mod patch;

pub use patch::{Applied, ConfigPatch, ParseError};

/// SSIDs are at most 32 bytes (IEEE 802.11), and the device name doubles as the
/// AP's SSID, so both cap here — the same limit
/// [`scoreboard_portal::MyHosts`](../scoreboard_portal/struct.MyHosts.html)
/// truncates to.
pub const MAX_SSID: usize = 32;
/// A WPA2 passphrase is 8–63 characters; a raw PSK is 64 hex digits.
pub const MAX_PASSWORD: usize = 64;
/// The backend base URL. `https://pico-scoreboard-backend.fly.dev` and room to
/// spare for a staging host or a LAN address with a port.
pub const MAX_URL: usize = 128;
/// The `X-Api-Key` header value.
pub const MAX_API_KEY: usize = 64;
/// An ESPN league slug — `college-football` and `fifa.world` are the long ones.
pub const MAX_LEAGUE: usize = 24;
/// Leagues configurable per sport. Football has two slugs today and soccer
/// four; eight leaves room without making the config struct a budget problem.
pub const MAX_LEAGUES: usize = 8;
/// A screen-variant letter (`"A"`, `"B"`, `"C"`).
pub const MAX_VARIANT: usize = 4;
/// An OTA channel name: `stable` or `dev`, with room for one more.
pub const MAX_CHANNEL: usize = 8;

/// The RP2350's hardware watchdog tops out near 8.3 s; the floor keeps the
/// feeder (timeout / 4) under roughly twice a second. `config.py`'s
/// `_WDT_TIMEOUT_MIN_MS` / `_MAX_MS`.
pub const WATCHDOG_TIMEOUT_MIN_MS: u32 = 2_000;
pub const WATCHDOG_TIMEOUT_MAX_MS: u32 = 8_300;

/// `poll_interval_seconds >= game_rotation_seconds`.
///
/// `config.py`'s `CadenceError`. The only way a `PUT /api/config` is rejected,
/// and the reason `/api/config` can answer `400 {"error": "invalid_cadence"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CadenceError {
    pub poll_interval_seconds: u32,
    pub game_rotation_seconds: u32,
}

/// The whole merged configuration — `config.raw`, which is what `GET
/// /api/config` returns verbatim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceConfig {
    #[serde(default)]
    pub network: NetworkConfig,
    #[serde(default)]
    pub api: ApiConfig,
    #[serde(default)]
    pub display: DisplayConfig,
    #[serde(default)]
    pub colors: ColorsConfig,
    #[serde(default)]
    pub sports: SportsConfig,
    #[serde(default)]
    pub log: LogConfig,
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub watchdog: WatchdogConfig,
    #[serde(default)]
    pub ota: OtaConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetworkConfig {
    #[serde(default)]
    pub ssid: String<MAX_SSID>,
    #[serde(default)]
    pub password: String<MAX_PASSWORD>,
    #[serde(default = "defaults::device_name")]
    pub device_name: String<MAX_SSID>,
    #[serde(default = "defaults::connect_timeout_seconds")]
    pub connect_timeout_seconds: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiConfig {
    #[serde(default)]
    pub url: String<MAX_URL>,
    #[serde(default)]
    pub key: String<MAX_API_KEY>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayConfig {
    #[serde(default = "defaults::brightness")]
    pub brightness: u8,
    #[serde(default = "defaults::poll_interval_seconds")]
    pub poll_interval_seconds: u32,
    #[serde(default = "defaults::game_rotation_seconds")]
    pub game_rotation_seconds: u32,
    #[serde(default = "defaults::data_frequency_khz")]
    pub data_frequency_khz: u32,
    #[serde(default = "defaults::target_refresh_rate")]
    pub target_refresh_rate: f64,
    #[serde(default)]
    pub gamma: GammaConfig,
    #[serde(default)]
    pub blanking_time_ns: u32,
    #[serde(default)]
    pub variants: VariantsConfig,
    #[serde(default = "defaults::show_dividers")]
    pub show_dividers: bool,
    #[serde(default = "defaults::scroll_speed")]
    pub scroll_speed_px_per_sec: i32,
}

/// `{"type": "srgb" | "power" | "none", "value": 2.2}`.
///
/// `value` is absent for everything but `power`, which is how `_DEFAULTS`
/// spells it and therefore what the SPA's settings form round-trips.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GammaConfig {
    #[serde(rename = "type", default)]
    pub kind: GammaKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GammaKind {
    #[default]
    Srgb,
    Power,
    None,
}

/// Unknown spellings fall back to sRGB rather than failing the whole document.
///
/// `config.py`'s `gamma` accessor did the same with an `if/elif/else`, and the
/// reason matters more here than there: serde would otherwise reject the entire
/// `PUT` — or, on a stored config, the entire boot configuration — over one
/// unrecognised word.
impl<'de> Deserialize<'de> for GammaKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<GammaKind, D::Error> {
        let name = <&str>::deserialize(deserializer)?;
        Ok(match name {
            "power" => GammaKind::Power,
            "none" => GammaKind::None,
            _ => GammaKind::Srgb,
        })
    }
}

impl GammaConfig {
    /// The exponent a [`GammaKind::Power`] curve uses.
    ///
    /// `raw.get("value", 2.2)` — a `power` gamma with no `value` is 2.2, not a
    /// rejection. Meaningless for the other two kinds, which is why this is a
    /// separate accessor rather than a field.
    pub fn power_exponent(&self) -> f64 {
        self.value.unwrap_or(2.2)
    }
}

/// The configured design per sport × screen.
///
/// Only the four keys `_DEFAULTS` carries are modelled. `screen_geometry`
/// registers more names than these (the pregame and single-design live
/// screens), and it already ignores both unknown keys and unknown letters — so
/// an unmodelled key in a stored config selected nothing before and selects
/// nothing now. What changes is that it no longer round-trips back out of `GET
/// /api/config`; PARITY.md records that.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VariantsConfig {
    #[serde(default = "defaults::variant_c")]
    pub mlb_final: String<MAX_VARIANT>,
    #[serde(default = "defaults::variant_c")]
    pub nba_final: String<MAX_VARIANT>,
    #[serde(default = "defaults::variant_c")]
    pub football_final: String<MAX_VARIANT>,
    #[serde(default = "defaults::variant_a")]
    pub soccer_live: String<MAX_VARIANT>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorsConfig {
    #[serde(default = "defaults::white")]
    pub primary: Rgb,
    #[serde(default = "defaults::gray")]
    pub secondary: Rgb,
    #[serde(default = "defaults::yellow")]
    pub accent: Rgb,
    #[serde(default = "defaults::green")]
    pub clock_normal: Rgb,
    #[serde(default = "defaults::red")]
    pub clock_warning: Rgb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgb {
    #[serde(default)]
    pub r: u8,
    #[serde(default)]
    pub g: u8,
    #[serde(default)]
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r, g, b }
    }
}

impl From<Rgb> for scoreboard_model::Rgb888 {
    fn from(color: Rgb) -> scoreboard_model::Rgb888 {
        scoreboard_model::Rgb888::new(color.r, color.g, color.b)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SportsConfig {
    #[serde(default = "defaults::mlb")]
    pub mlb: SportToggle,
    #[serde(default)]
    pub nba: SportToggle,
    #[serde(default)]
    pub football: SportLeagues,
    #[serde(default)]
    pub soccer: SportLeagues,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SportToggle {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SportLeagues {
    #[serde(default)]
    pub leagues: Vec<String<MAX_LEAGUE>, MAX_LEAGUES>,
}

impl SportLeagues {
    /// The slugs worth polling: empty strings dropped.
    ///
    /// `config.py`'s `[s for s in leagues if isinstance(s, str) and s]`. The
    /// type system covers the `isinstance` half; the emptiness filter is real,
    /// because the SPA's league editor can leave a blank row behind.
    pub fn active(&self) -> impl Iterator<Item = &str> {
        self.leagues
            .iter()
            .map(String::as_str)
            .filter(|slug| !slug.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogConfig {
    #[serde(default = "defaults::log_level")]
    pub level: String<8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "defaults::cache_max_age_seconds")]
    pub cache_max_age_seconds: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchdogConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "defaults::watchdog_timeout_ms")]
    pub timeout_ms: u32,
}

/// Not `Copy`, unlike its neighbours: [`channel`](OtaConfig::channel) is a
/// string, and it is one rather than an enum because it reaches the device
/// through `PUT /api/config` — a value the firmware does not recognise has to
/// read as "the conservative channel" rather than as a rejected request that
/// leaves the settings page unable to save anything else either.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OtaConfig {
    #[serde(default = "defaults::ota_enabled")]
    pub enabled: bool,
    /// Which published artifact this device follows: `stable` or `dev`.
    ///
    /// SPEC §8's replacement for `ota.py`'s `/ota_dev` marker file — but only
    /// half of it. The marker did two jobs, and the more important one (do not
    /// roll a locally-built image back to the published one) is now a property
    /// of the image rather than a setting: see
    /// [`scoreboard_ota::decide`](https://docs.rs). This field is the other
    /// job, pinning a bench unit to the staging artifact so the whole update
    /// path can be exercised without publishing to the units that are gifts.
    #[serde(default = "defaults::ota_channel")]
    pub channel: String<MAX_CHANNEL>,
}

impl DeviceConfig {
    /// The `_DEFAULTS` dict, as a value.
    pub fn new() -> DeviceConfig {
        DeviceConfig {
            network: NetworkConfig::default(),
            api: ApiConfig::default(),
            display: DisplayConfig::default(),
            colors: ColorsConfig::default(),
            sports: SportsConfig::default(),
            log: LogConfig::default(),
            server: ServerConfig::default(),
            watchdog: WatchdogConfig::default(),
            ota: OtaConfig::default(),
        }
    }

    /// Read a stored configuration document, **never failing**.
    ///
    /// `config.py`'s `_load`: a corrupt file falls back to defaults with a
    /// logged complaint, because `Config()` runs at import time and a raise
    /// here is a device that will not boot. The second return value is what to
    /// complain about — `None` when the document was clean.
    ///
    /// Two recoveries, both `config.py`'s:
    ///
    /// - **Unparseable** → the whole document is discarded for defaults.
    /// - **Parseable but with an invalid cadence** → *only* the two cadence
    ///   keys reset to their defaults, so one bad edit does not throw away a
    ///   working SSID.
    pub fn from_json(document: &[u8]) -> (DeviceConfig, Option<LoadComplaint>) {
        let Ok((mut config, _)) = serde_json_core::from_slice::<DeviceConfig>(document) else {
            return (DeviceConfig::new(), Some(LoadComplaint::Unparseable));
        };
        if let Err(error) = config.check_cadence() {
            let defaults = DisplayConfig::default();
            config.display.poll_interval_seconds = defaults.poll_interval_seconds;
            config.display.game_rotation_seconds = defaults.game_rotation_seconds;
            return (config, Some(LoadComplaint::InvalidCadence(error)));
        }
        (config, None)
    }

    /// Serialize into a caller-owned buffer, returning the byte count.
    ///
    /// SPEC §7.3: responses are serialized into a buffer the caller owns, so
    /// nothing here needs an allocator or a `static` scratch.
    pub fn to_json(&self, out: &mut [u8]) -> Result<usize, SerializeError> {
        serde_json_core::to_slice(self, out).map_err(|_| SerializeError)
    }

    fn check_cadence(&self) -> Result<(), CadenceError> {
        check_cadence(
            self.display.poll_interval_seconds,
            self.display.game_rotation_seconds,
        )
    }

    /// The log level, already parsed. `config.py` cached this as a plain int
    /// because it is read before every log statement.
    pub fn log_level(&self) -> LogLevel {
        LogLevel::from_name(&self.log.level)
    }

    /// The watchdog timeout, clamped to what the RP2350 can actually arm.
    pub fn watchdog_timeout_ms(&self) -> u32 {
        self.watchdog
            .timeout_ms
            .clamp(WATCHDOG_TIMEOUT_MIN_MS, WATCHDOG_TIMEOUT_MAX_MS)
    }

    /// The renderer's view of this config: variants, dividers, scroll speed.
    ///
    /// The scroll speed degrades to the default if it is not one of the smooth
    /// set, and an unknown variant letter leaves that screen's selection alone
    /// — both inside `RenderSettings`, which is where `screen_geometry`'s
    /// validation lived.
    pub fn render_settings(&self) -> scoreboard_render::RenderSettings {
        let mut settings = scoreboard_render::RenderSettings::new();
        let variants = &self.display.variants;
        settings.apply_variant("mlb_final", &variants.mlb_final);
        settings.apply_variant("nba_final", &variants.nba_final);
        settings.apply_variant("football_final", &variants.football_final);
        settings.apply_variant("soccer_live", &variants.soccer_live);
        settings.show_dividers = self.display.show_dividers;
        settings.set_scroll_speed(self.display.scroll_speed_px_per_sec);
        settings
    }

    /// The UI colors, as the model carries them.
    pub fn ui_colors(&self) -> scoreboard_model::UiColors {
        scoreboard_model::UiColors {
            primary: self.colors.primary.into(),
            secondary: self.colors.secondary.into(),
            accent: self.colors.accent.into(),
            clock_normal: self.colors.clock_normal.into(),
            clock_warning: self.colors.clock_warning.into(),
        }
    }
}

impl Default for DeviceConfig {
    fn default() -> DeviceConfig {
        DeviceConfig::new()
    }
}

/// What was wrong with a stored configuration document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadComplaint {
    /// Not valid JSON, or not shaped like a configuration at all. Everything
    /// fell back to defaults.
    Unparseable,
    /// Parsed, but `poll_interval >= game_rotation`. Only those two keys reset.
    InvalidCadence(CadenceError),
}

/// The response buffer was too small.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SerializeError;

/// The invariant, in one place.
pub fn check_cadence(
    poll_interval_seconds: u32,
    game_rotation_seconds: u32,
) -> Result<(), CadenceError> {
    if poll_interval_seconds >= game_rotation_seconds {
        return Err(CadenceError {
            poll_interval_seconds,
            game_rotation_seconds,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests;
