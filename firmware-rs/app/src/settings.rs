//! The seam a config change crosses to reach the things that draw.
//!
//! `PUT /api/config` runs on core 0. Four of the things it changes live on core
//! 1: the renderer's [`RenderSettings`] and the panel driver's data clock,
//! refresh rate, gamma and blanking time. `api_routes.py` could simply call
//! `update_display_gamma(config)` because MicroPython's driver was a global
//! object either core could reach; here core 1 owns the driver by value, which
//! is the property that makes the render loop's aliasing story work, so the
//! change has to be *sent*.
//!
//! # Why a `Signal` and not an atomic
//!
//! [`crate::display_core1::BRIGHTNESS`] is an atomic because it moves at 5 Hz
//! and a dropped update is the next update. This moves once per config save,
//! carries an `f64` and an enum, and every one of them matters — so it is a
//! `Signal`, which holds exactly one pending value, overwrites it if a second
//! `PUT` lands first, and costs nothing when idle. Core 1 drains it at the top
//! of a frame with `try_take`, so the render loop never waits on core 0.
//!
//! # Why it carries the flags too
//!
//! Applying all four driver parameters on every message would be simpler, and
//! wrong in a visible way: rebuilding the gamma LUT and restamping the timing
//! stream are things you can see on the panel, and `api_routes.py` deliberately
//! only re-applied what the request body named. [`scoreboard_config::Applied`]
//! is that answer, computed where the patch is merged, and it rides along so
//! core 1 can make the same distinction.
//!
//! # The colours go the other way
//!
//! UI colours are not a core-1 setting — they are model state, carried in the
//! snapshot and read by renderers out of it. So they cross a different seam:
//! core 0's snapshot producer picks them up with [`take_ui_colors`] before its
//! next commit. Today that producer is `demo::feed`; task #11's poller inherits
//! the same call.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use hub75::gamma::Gamma;
use scoreboard_config::{Applied, DeviceConfig, GammaKind};
use scoreboard_model::UiColors;
use scoreboard_render::RenderSettings;

/// Everything a config change asks core 1 to do.
#[derive(Debug, Clone, Copy)]
pub struct DisplayUpdate {
    /// Which of the fields below to actually apply.
    pub applied: Applied,
    pub render: RenderSettings,
    pub data_clock_hz: u32,
    pub target_refresh_rate_hz: f64,
    pub gamma: Gamma,
    pub blanking_time_ns: u32,
}

impl DisplayUpdate {
    /// The update `config` describes, running the hooks `applied` names.
    pub fn new(config: &DeviceConfig, applied: Applied) -> DisplayUpdate {
        DisplayUpdate {
            applied,
            render: config.render_settings(),
            data_clock_hz: config.display.data_frequency_khz.saturating_mul(1_000),
            target_refresh_rate_hz: config.display.target_refresh_rate,
            gamma: match config.display.gamma.kind {
                GammaKind::Srgb => Gamma::Srgb,
                GammaKind::None => Gamma::Identity,
                GammaKind::Power => Gamma::Power(config.display.gamma.power_exponent()),
            },
            blanking_time_ns: config.display.blanking_time_ns,
        }
    }

    /// The whole configuration, every hook on. What the boot sends once, before
    /// anything has been changed — core 1 starts from `RenderSettings::new()`
    /// and the driver from `Config::defaults`, neither of which knows what is
    /// stored.
    pub fn boot(config: &DeviceConfig) -> DisplayUpdate {
        DisplayUpdate::new(
            config,
            Applied {
                ui_colors: false,
                render_settings: true,
                data_clock: true,
                refresh_rate: true,
                gamma: true,
                blanking_time: true,
                log_level: false,
            },
        )
    }
}

static DISPLAY: Signal<CriticalSectionRawMutex, DisplayUpdate> = Signal::new();
static PENDING_UI_COLORS: Signal<CriticalSectionRawMutex, UiColors> = Signal::new();

/// Send a display change to core 1. Overwrites any update it has not yet taken.
pub fn publish_display(update: DisplayUpdate) {
    DISPLAY.signal(update);
}

/// Core 1's end: the pending update, or `None`. Never blocks.
pub fn take_display() -> Option<DisplayUpdate> {
    DISPLAY.try_take()
}

/// Hand new UI colours to whoever owns the snapshot.
pub fn publish_ui_colors(colors: UiColors) {
    PENDING_UI_COLORS.signal(colors);
}

/// The snapshot producer's end. Call before building a commit; `None` means
/// nothing changed.
pub fn take_ui_colors() -> Option<UiColors> {
    PENDING_UI_COLORS.try_take()
}
