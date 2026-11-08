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
//! `frontend/src/lib/api/types.ts` and the settings page read these key names,
//! so the two move together: a key added or dropped here is added or dropped
//! there in the same change, and the SPA ships embedded in the same image.
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
use scoreboard_model::feed::{LEAGUES, RegisteredLeague};
use scoreboard_model::slate::MAX_SOURCES;
use scoreboard_model::{LeagueId, Sport};
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
    /// The game-description scroll speed. A document stored before 2026-11-08
    /// also carries `variants` and `show_dividers` beside it; both keys went
    /// with the layout variants, so serde skips them like any other unknown
    /// field and `GET /api/config` no longer echoes them.
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

/// Fields in rotation and menu order (NHL.md D6), which is also the order `GET
/// /api/config` lists them in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SportsConfig {
    #[serde(default = "defaults::mlb")]
    pub mlb: SportToggle,
    #[serde(default)]
    pub nba: SportToggle,
    /// The key's presence in `GET /api/config` is how the settings page knows
    /// this firmware has hockey (NHL.md D7); the frozen MicroPython never
    /// sends it.
    #[serde(default = "defaults::nhl")]
    pub nhl: SportToggle,
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

impl SportToggle {
    /// Switch on or off, returning whether that was a change.
    fn set(&mut self, enabled: bool) -> bool {
        let changed = self.enabled != enabled;
        self.enabled = enabled;
        changed
    }
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

    fn contains(&self, slug: &str) -> bool {
        self.leagues.iter().any(|listed| listed == slug)
    }

    /// Switch one slug on or off the way the settings page edits the list —
    /// on appends, off removes — returning whether the list changed. Every
    /// other slug keeps its place, so a league the firmware does not know by
    /// name rides along untouched.
    ///
    /// A list already holding [`MAX_LEAGUES`] slugs cannot take another; only a
    /// hand-written config gets there, and the answer is "nothing changed"
    /// rather than a dropped slug elsewhere.
    fn set(&mut self, slug: &str, enabled: bool) -> bool {
        if self.contains(slug) == enabled {
            return false;
        }
        if !enabled {
            self.leagues.retain(|listed| listed != slug);
            return true;
        }
        let Ok(slug) = String::try_from(slug) else {
            return false;
        };
        self.leagues.push(slug).is_ok()
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

    /// The renderer's view of this config: the scroll speed.
    ///
    /// It degrades to the default if it is not one of the smooth set — inside
    /// `RenderSettings`, which is where `screen_geometry`'s validation lived.
    pub fn render_settings(&self) -> scoreboard_render::RenderSettings {
        let mut settings = scoreboard_render::RenderSettings::new();
        settings.set_scroll_speed(self.display.scroll_speed_px_per_sec);
        settings
    }

    /// The configured leagues as poll sources, in rotation and menu order —
    /// NHL.md D6: MLB, NBA, NHL, the football leagues, the soccer leagues.
    ///
    /// `poller.py:223-239`'s `sources_from_config`, here rather than in the
    /// app so the order is host-tested. The app reads it at poller start and
    /// again whenever the `sports` section is written ([`Applied::sources`]),
    /// which is what lets a settings save or the device's league menu take
    /// effect without a reboot — `api_routes.py` never rebuilt the sources, so
    /// on MicroPython it always took one.
    pub fn league_sources(&self) -> LeagueSources {
        let mut out = LeagueSources {
            sources: Vec::new(),
            dropped: 0,
        };
        let sports = &self.sports;
        let toggles = [
            (sports.mlb, Sport::Mlb, "mlb"),
            (sports.nba, Sport::Nba, "nba"),
            (sports.nhl, Sport::Hockey, "nhl"),
        ];
        for (toggle, sport, slug) in toggles {
            if toggle.enabled {
                out.push(LeagueId::from_slug(sport, slug));
            }
        }
        for slug in sports.football.active() {
            out.push(LeagueId::from_slug(Sport::Football, slug));
        }
        for slug in sports.soccer.active() {
            out.push(LeagueId::from_slug(Sport::Soccer, slug));
        }
        out
    }

    /// Which registered leagues are on, one bit per row of
    /// [`scoreboard_model::feed::LEAGUES`]: bit `i` is set when league `i`
    /// is configured — the `enabled` toggle for MLB, NBA and the NHL, or the
    /// slug's membership of the football or soccer list.
    ///
    /// This is the device's league menu's view of the configuration: nine
    /// checkboxes in registry order, which is also rotation order. The menu
    /// reads the mask when it opens and hands a new one to
    /// [`DeviceConfig::set_enabled_leagues`] when it closes, so the two sides of
    /// the `sports` section — the settings page's switches and the panel's
    /// checkboxes — edit the same bits and neither can drift from the other.
    /// Unregistered slugs are not in the mask; they are preserved by `set` and
    /// still polled, they just have no checkbox.
    pub fn enabled_leagues(&self) -> u16 {
        let mut mask = 0u16;
        for (index, league) in LEAGUES.iter().enumerate() {
            if self.league_enabled(league) {
                mask |= 1 << index;
            }
        }
        mask
    }

    /// Switch every registered league on or off to match `mask`, the inverse
    /// of [`DeviceConfig::enabled_leagues`]. Returns whether anything changed,
    /// which is the caller's cue to write flash.
    ///
    /// A list slug turning on is appended, as the settings page appends it, so
    /// a config edited from both ends keeps the page's order; turning off
    /// removes the one slug and nothing else, so a hand-written league the
    /// firmware does not know stays listed and polled.
    pub fn set_enabled_leagues(&mut self, mask: u16) -> bool {
        let mut changed = false;
        for (index, league) in LEAGUES.iter().enumerate() {
            let enabled = mask & (1 << index) != 0;
            let sports = &mut self.sports;
            changed |= match league.sport {
                Sport::Mlb => sports.mlb.set(enabled),
                Sport::Nba => sports.nba.set(enabled),
                // One hockey league, one toggle; a second row needs a config
                // shape first, which `every_registered_league_fits_the_slate`
                // insists on (NHL.md D5).
                Sport::Hockey => sports.nhl.set(enabled),
                Sport::Football => sports.football.set(league.slug, enabled),
                Sport::Soccer => sports.soccer.set(league.slug, enabled),
            };
        }
        changed
    }

    fn league_enabled(&self, league: &RegisteredLeague) -> bool {
        match league.sport {
            Sport::Mlb => self.sports.mlb.enabled,
            Sport::Nba => self.sports.nba.enabled,
            Sport::Hockey => self.sports.nhl.enabled,
            Sport::Football => self.sports.football.contains(league.slug),
            Sport::Soccer => self.sports.soccer.contains(league.slug),
        }
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

/// The configured leagues, as [`DeviceConfig::league_sources`] builds them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeagueSources {
    pub sources: Vec<LeagueId, MAX_SOURCES>,
    /// Configured leagues past [`MAX_SOURCES`], which are not polled.
    ///
    /// A count rather than an `expect`, because this is reachable: every
    /// registered league fits at once, but `PUT /api/config` takes
    /// unregistered slugs too (up to [`MAX_LEAGUES`] per sport, so a league
    /// the backend adds needs no release), and a panic at poller start would
    /// be a boot loop. Zero for anything the settings page can produce; the
    /// app logs it otherwise.
    pub dropped: usize,
}

impl LeagueSources {
    fn push(&mut self, league: LeagueId) {
        if self.sources.push(league).is_err() {
            self.dropped += 1;
        }
    }
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

pub mod timezone;
