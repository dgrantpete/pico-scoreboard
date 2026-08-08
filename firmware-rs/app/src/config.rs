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
//! # Where the stored configuration comes from
//!
//! Nowhere, yet. This starts at `_DEFAULTS` on every boot, so a `PUT` changes
//! the running device and does not survive a reset. Task #12 fills two seams:
//! [`load`] here, and `http::routes::persist` there.

use core::cell::RefCell;

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use scoreboard_config::{CadenceError, DeviceConfig};

static CONFIG: Mutex<CriticalSectionRawMutex, RefCell<Option<DeviceConfig>>> =
    Mutex::new(RefCell::new(None));

/// Install the boot configuration.
///
/// Task #12 replaces the body with a read of the storage region, which is why
/// this takes the document rather than building it: `DeviceConfig::from_json`
/// already handles a corrupt or partial one, and its complaint is the thing
/// worth logging at boot.
pub fn load() -> DeviceConfig {
    // No storage yet, so there is no document to parse and defaults are the
    // whole answer. When #12 lands, the read goes here and the complaint it
    // returns gets logged exactly as `config.py:_load` logged it.
    let config = DeviceConfig::new();
    CONFIG.lock(|slot| *slot.borrow_mut() = Some(config.clone()));
    crate::ringlog::set_level(config.log_level());
    config
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
