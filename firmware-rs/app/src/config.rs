//! The running configuration: one owner, reached through a lock.
//!
//! [`scoreboard_config::DeviceConfig`] is the value and every decision about
//! its shape; this is where the device keeps *the* one, in the same place and
//! for the same reason [`crate::net::hosts`] and [`crate::ringlog`] keep theirs.
//!
//! `main.py` built a `Config` at import time and passed the object to whoever
//! needed it — the poller, the display init, `create_api`. Threading a `&mut`
//! through picoserve's handlers is not available (a handler gets `&State`), and
//! the readers are on both cores and in several tasks, so it is a static behind
//! a `CriticalSectionRawMutex`.
//!
//! **The lock is never held across an `await`.** Both accessors take a closure
//! that must return; anything that needs to do I/O with the result copies what
//! it needs out first. `PUT /api/config` is the case that matters — it builds
//! the whole live-apply message inside [`with_mut`] and sends it outside, so
//! the critical section covers a struct copy and nothing else.
//!
//! # Where the stored configuration comes from, and where `dev.toml` fits
//!
//! [`crate::storage`], which is the whole answer for a provisioned device. The
//! bench seam survives underneath it with **one** precedence rule:
//!
//! > `dev.toml` seeds the configuration only when the storage region holds no
//! > document at all.
//!
//! Not "when a field is empty", which is the tempting version and is wrong in a
//! way that would waste an afternoon: `POST /api/reset-network` clears the SSID
//! and saves, so a field-level fallback would refill it from the build at the
//! next boot and the device would never enter setup mode on the bench. Once
//! anything has been written — the first `PUT /api/config`, or a provisioning
//! save — the build's values are out of the picture for good, which is also
//! what makes "delete `dev.toml` and reboot" a meaningful test that storage
//! alone can bring the device up.

use core::cell::RefCell;

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use scoreboard_config::{CadenceError, DeviceConfig, LoadComplaint};

static CONFIG: Mutex<CriticalSectionRawMutex, RefCell<Option<DeviceConfig>>> =
    Mutex::new(RefCell::new(None));

/// Read the stored configuration and install it as the running one.
///
/// Called from `main` **before core 1 starts**, which is what makes the flash
/// read free — see [`crate::storage::install`]. Never fails: a corrupt document
/// falls back to defaults with a logged complaint, which is `config.py:_load`'s
/// promise that a hand-edited file cannot brick a boot.
pub fn load() -> DeviceConfig {
    let stored = crate::storage::load_config();
    let mut config = stored.config;

    match stored.complaint {
        Some(LoadComplaint::Unparseable) => crate::error!(
            "config: the stored configuration did not parse; every value is a default"
        ),
        Some(LoadComplaint::InvalidCadence(error)) => crate::error!(
            "config: stored poll {} s is not under rotation {} s; both keys reset to defaults",
            error.poll_interval_seconds,
            error.game_rotation_seconds
        ),
        None if stored.present => crate::debug!("config: loaded from storage"),
        None => {
            // A device on its first Rust boot. On a probe-flashed bench image
            // the gitignored `dev.toml` fills in what a settings page would
            // have; in CI, and on a real device out of the box, every one of
            // these is empty and the result is the un-provisioned path — no
            // SSID, so `net::wifi` goes straight to AP setup mode, and no
            // backend URL, so the first poll would fail as `Network error /
            // api url is not valid`. That is the honest state of a device
            // nobody has configured yet.
            seed_from_build(&mut config);
            crate::debug!("config: no stored document; defaults plus the build's dev seed");
        }
    }

    CONFIG.lock(|slot| *slot.borrow_mut() = Some(config.clone()));
    crate::ringlog::set_level(config.log_level());
    config
}

/// Copy `build.rs`'s `dev.toml` values in. See the module docs for when.
fn seed_from_build(config: &mut DeviceConfig) {
    let _ = config.network.ssid.push_str(env!("DEV_WIFI_SSID"));
    let _ = config.network.password.push_str(env!("DEV_WIFI_PASSWORD"));
    if !env!("DEV_DEVICE_NAME").is_empty() {
        config.network.device_name.clear();
        let _ = config.network.device_name.push_str(env!("DEV_DEVICE_NAME"));
    }
    if let Ok(seconds) = env!("DEV_CONNECT_TIMEOUT_SECONDS").parse::<u32>() {
        config.network.connect_timeout_seconds = seconds;
    }
    let _ = config.api.url.push_str(env!("DEV_API_URL"));
}

/// Write the running configuration to flash.
///
/// **One flash write per call**, and every caller is a place that has just
/// finished a whole batch of changes — see [`crate::storage::save_config`] for
/// why that matters and `config.py`'s `update_many` for where the rule comes
/// from. Costs the panel a frame; BUDGET.md carries the measurement.
pub fn persist() -> bool {
    with(crate::storage::save_config)
}

/// Read something out of the configuration.
///
/// Panics if called before [`load`], which is a boot-ordering bug rather than a
/// runtime condition — the server does not start until provisioning has run,
/// and provisioning runs after `load`.
pub fn with<R>(f: impl FnOnce(&DeviceConfig) -> R) -> R {
    CONFIG.lock(|slot| {
        let slot = slot.borrow();
        let config = slot
            .as_ref()
            .expect("configuration read before config::load");
        f(config)
    })
}

/// Change the configuration, or reject the change.
///
/// The closure's `Err` is returned untouched and **the configuration is left
/// exactly as it was** — which is not this function's doing but
/// [`DeviceConfig::apply`]'s, and is worth saying twice because the `&mut` here
/// would otherwise let a half-applied patch escape.
pub fn with_mut<T, E>(
    f: impl FnOnce(&mut DeviceConfig) -> Result<T, E>,
) -> Result<T, E> {
    CONFIG.lock(|slot| {
        let mut slot = slot.borrow_mut();
        let config = slot
            .as_mut()
            .expect("configuration written before config::load");
        f(config)
    })
}

/// The type `with_mut`'s callers reject a `PUT` with.
pub type Rejection = CadenceError;
