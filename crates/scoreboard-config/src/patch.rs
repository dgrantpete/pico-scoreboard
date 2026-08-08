//! The partial update `PUT /api/config` applies.
//!
//! `config.py:update_many` took a `{section: {key: value}}` dict, validated the
//! cadence pair *as it would exist after the merge*, then wrote every key it
//! recognised in one flash write. The shape here is the same operation with the
//! dict replaced by a struct of `Option`s: absent means "leave it alone",
//! present means "set it to this".
//!
//! # What the caller gets back
//!
//! `api_routes.py` decided what to re-apply by asking which keys were in the
//! request body — `if 'colors' in data`, `if 'gamma' in data['display']`. That
//! test does not survive into a struct, so [`DeviceConfig::apply`] returns
//! [`Applied`], which records the same answer: which live-apply hooks this
//! particular request needs. The firmware runs exactly those, so a `PUT` that
//! only changes the SSID does not rebuild the gamma LUT or restamp the panel's
//! timing — which matters, because those calls are visible on the panel.

use heapless::{String, Vec};
use serde::Deserialize;

use crate::{
    CadenceError, DeviceConfig, GammaConfig, MAX_API_KEY, MAX_CHANNEL, MAX_LEAGUE, MAX_LEAGUES,
    MAX_PASSWORD, MAX_SSID, MAX_URL, MAX_VARIANT, Rgb, check_cadence,
};

/// Which live-apply hooks a [`DeviceConfig::apply`] needs run.
///
/// One bool per thing `api_routes.py` re-applied, and it is a *set*, not a
/// diff: a request that sets `show_dividers` to the value it already had still
/// reports it, exactly as `if 'show_dividers' in data['display']` did. Chasing
/// no-op writes would be a behaviour change, and the hooks are all idempotent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Applied {
    /// `update_ui_colors` — the snapshot's UI palette.
    pub ui_colors: bool,
    /// `update_screen_variants` / `update_show_dividers` / `update_scroll_speed`
    /// — everything the renderer reads out of `RenderSettings`. One flag,
    /// because they travel to core 1 in one message.
    pub render_settings: bool,
    /// `update_display_frequency` — the PIO data clock.
    pub data_clock: bool,
    /// `update_display_refresh_rate`.
    pub refresh_rate: bool,
    /// `update_display_gamma` — rebuilds the 256-entry LUT.
    pub gamma: bool,
    /// `update_display_blanking_time`.
    pub blanking_time: bool,
    /// `logger.set_level`, which `config.py` pushed rather than `api_routes.py`.
    pub log_level: bool,
}

impl Applied {
    /// Whether anything core 1 owns changed — the render settings or any of the
    /// four driver parameters. The firmware sends one message when this is
    /// true and none when it is false.
    pub fn touches_core1(&self) -> bool {
        self.render_settings
            || self.data_clock
            || self.refresh_rate
            || self.gamma
            || self.blanking_time
    }

    pub fn any(&self) -> bool {
        self.ui_colors || self.log_level || self.touches_core1()
    }
}

/// A partial configuration document.
///
/// Unknown keys are ignored rather than rejected — serde's default behaviour,
/// and `update_many`'s (`if section not in self._data ... continue`). A field
/// present but of the wrong *type* is the one place the two differ: MicroPython
/// stored whatever it was given and let the defensive accessors cope, while
/// this rejects the request with a parse error. PARITY.md records it; a `400`
/// on `{"display": {"brightness": "bright"}}` is the better answer.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ConfigPatch {
    pub network: Option<NetworkPatch>,
    pub api: Option<ApiPatch>,
    pub display: Option<DisplayPatch>,
    pub colors: Option<ColorsPatch>,
    pub sports: Option<SportsPatch>,
    pub log: Option<LogPatch>,
    pub server: Option<ServerPatch>,
    pub watchdog: Option<WatchdogPatch>,
    pub ota: Option<OtaPatch>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct NetworkPatch {
    pub ssid: Option<String<MAX_SSID>>,
    pub password: Option<String<MAX_PASSWORD>>,
    pub device_name: Option<String<MAX_SSID>>,
    pub connect_timeout_seconds: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ApiPatch {
    pub url: Option<String<MAX_URL>>,
    pub key: Option<String<MAX_API_KEY>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DisplayPatch {
    pub brightness: Option<u8>,
    pub poll_interval_seconds: Option<u32>,
    pub game_rotation_seconds: Option<u32>,
    pub data_frequency_khz: Option<u32>,
    pub target_refresh_rate: Option<f64>,
    pub gamma: Option<GammaConfig>,
    pub blanking_time_ns: Option<u32>,
    pub variants: Option<VariantsPatch>,
    pub show_dividers: Option<bool>,
    pub scroll_speed_px_per_sec: Option<i32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct VariantsPatch {
    pub mlb_final: Option<String<MAX_VARIANT>>,
    pub nba_final: Option<String<MAX_VARIANT>>,
    pub football_final: Option<String<MAX_VARIANT>>,
    pub soccer_live: Option<String<MAX_VARIANT>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ColorsPatch {
    pub primary: Option<Rgb>,
    pub secondary: Option<Rgb>,
    pub accent: Option<Rgb>,
    pub clock_normal: Option<Rgb>,
    pub clock_warning: Option<Rgb>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SportsPatch {
    pub mlb: Option<TogglePatch>,
    pub nba: Option<TogglePatch>,
    pub football: Option<LeaguesPatch>,
    pub soccer: Option<LeaguesPatch>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TogglePatch {
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LeaguesPatch {
    pub leagues: Option<Vec<String<MAX_LEAGUE>, MAX_LEAGUES>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LogPatch {
    pub level: Option<String<8>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ServerPatch {
    pub cache_max_age_seconds: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct WatchdogPatch {
    pub enabled: Option<bool>,
    pub timeout_ms: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct OtaPatch {
    pub enabled: Option<bool>,
    pub channel: Option<String<MAX_CHANNEL>>,
}

impl ConfigPatch {
    /// Parse a `PUT /api/config` body.
    ///
    /// An empty body is an empty patch, not an error: `api_routes.py` returned
    /// `config.raw` unchanged when `request.json` was `None`.
    pub fn from_json(body: &[u8]) -> Result<ConfigPatch, ParseError> {
        if body.iter().all(u8::is_ascii_whitespace) {
            return Ok(ConfigPatch::default());
        }
        serde_json_core::from_slice::<ConfigPatch>(body)
            .map(|(patch, _)| patch)
            .map_err(|_| ParseError)
    }
}

/// The body was not a configuration document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseError;

impl DeviceConfig {
    /// Merge `patch` in, or reject the whole thing.
    ///
    /// **Nothing is written unless everything validates.** The cadence pair is
    /// checked against the merged result *before* any field moves, so a request
    /// that would leave the device in a state where rotation outruns polling
    /// changes nothing at all — including the keys in it that were fine.
    /// `update_many` had the same all-or-nothing shape for the same reason,
    /// and it is what lets the route answer `400` and still be telling the
    /// truth when it echoes the config back.
    pub fn apply(&mut self, patch: &ConfigPatch) -> Result<Applied, CadenceError> {
        if let Some(display) = &patch.display {
            check_cadence(
                display
                    .poll_interval_seconds
                    .unwrap_or(self.display.poll_interval_seconds),
                display
                    .game_rotation_seconds
                    .unwrap_or(self.display.game_rotation_seconds),
            )?;
        }

        let mut applied = Applied::default();

        if let Some(network) = &patch.network {
            set(&mut self.network.ssid, &network.ssid);
            set(&mut self.network.password, &network.password);
            set(&mut self.network.device_name, &network.device_name);
            set(
                &mut self.network.connect_timeout_seconds,
                &network.connect_timeout_seconds,
            );
        }

        if let Some(api) = &patch.api {
            set(&mut self.api.url, &api.url);
            set(&mut self.api.key, &api.key);
        }

        if let Some(display) = &patch.display {
            set(&mut self.display.brightness, &display.brightness);
            set(
                &mut self.display.poll_interval_seconds,
                &display.poll_interval_seconds,
            );
            set(
                &mut self.display.game_rotation_seconds,
                &display.game_rotation_seconds,
            );
            if set(
                &mut self.display.data_frequency_khz,
                &display.data_frequency_khz,
            ) {
                applied.data_clock = true;
            }
            if set(
                &mut self.display.target_refresh_rate,
                &display.target_refresh_rate,
            ) {
                applied.refresh_rate = true;
            }
            if set(&mut self.display.gamma, &display.gamma) {
                applied.gamma = true;
            }
            if set(&mut self.display.blanking_time_ns, &display.blanking_time_ns) {
                applied.blanking_time = true;
            }
            if let Some(variants) = &display.variants {
                set(&mut self.display.variants.mlb_final, &variants.mlb_final);
                set(&mut self.display.variants.nba_final, &variants.nba_final);
                set(
                    &mut self.display.variants.football_final,
                    &variants.football_final,
                );
                set(&mut self.display.variants.soccer_live, &variants.soccer_live);
                applied.render_settings = true;
            }
            if set(&mut self.display.show_dividers, &display.show_dividers) {
                applied.render_settings = true;
            }
            if set(
                &mut self.display.scroll_speed_px_per_sec,
                &display.scroll_speed_px_per_sec,
            ) {
                applied.render_settings = true;
            }
        }

        if let Some(colors) = &patch.colors {
            set(&mut self.colors.primary, &colors.primary);
            set(&mut self.colors.secondary, &colors.secondary);
            set(&mut self.colors.accent, &colors.accent);
            set(&mut self.colors.clock_normal, &colors.clock_normal);
            set(&mut self.colors.clock_warning, &colors.clock_warning);
            applied.ui_colors = true;
        }

        if let Some(sports) = &patch.sports {
            if let Some(mlb) = &sports.mlb {
                set(&mut self.sports.mlb.enabled, &mlb.enabled);
            }
            if let Some(nba) = &sports.nba {
                set(&mut self.sports.nba.enabled, &nba.enabled);
            }
            if let Some(football) = &sports.football {
                set(&mut self.sports.football.leagues, &football.leagues);
            }
            if let Some(soccer) = &sports.soccer {
                set(&mut self.sports.soccer.leagues, &soccer.leagues);
            }
        }

        if let Some(log) = &patch.log
            && set(&mut self.log.level, &log.level)
        {
            applied.log_level = true;
        }

        if let Some(server) = &patch.server {
            set(
                &mut self.server.cache_max_age_seconds,
                &server.cache_max_age_seconds,
            );
        }

        if let Some(watchdog) = &patch.watchdog {
            set(&mut self.watchdog.enabled, &watchdog.enabled);
            set(&mut self.watchdog.timeout_ms, &watchdog.timeout_ms);
        }

        if let Some(ota) = &patch.ota {
            set(&mut self.ota.enabled, &ota.enabled);
            set(&mut self.ota.channel, &ota.channel);
        }

        Ok(applied)
    }
}

/// Write `patch` into `field` if it is present. Returns whether it was — which
/// is the `'key' in data` test the live-apply decisions are made on.
fn set<T: Clone>(field: &mut T, patch: &Option<T>) -> bool {
    match patch {
        Some(value) => {
            *field = value.clone();
            true
        }
        None => false,
    }
}
