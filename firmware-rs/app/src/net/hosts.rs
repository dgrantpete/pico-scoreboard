//! Where the HTTP server finds out whose name is on the request.
//!
//! `main.py` kept the answer on the app object (`app.ap`, read by the `Host`
//! comparison in both route handlers) and rebuilt the set per request. There is
//! no app object here, and the HTTP server (task #10) is a *sibling* of
//! [`crate::net::bringup`] rather than something it calls, so the answer is
//! published once, at the moment provisioning decides it, and read from there.
//!
//! The matching rules and their two deliberate deviations from `main.py` are
//! [`scoreboard_portal::MyHosts`]'s, where they are host-tested.

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

use core::cell::RefCell;

pub use scoreboard_portal::MyHosts;

/// Set exactly once, by `bringup`, before any consumer exists — and then only
/// read. It is behind a lock rather than an atomic because the value is a
/// pair of bounded strings, and behind a lock rather than a `StaticCell`
/// because task #12's `/api/reset-network` will legitimately want to replace it
/// when the device is re-provisioned without a reboot.
static HOSTS: Mutex<CriticalSectionRawMutex, RefCell<Option<MyHosts>>> =
    Mutex::new(RefCell::new(None));

/// Publish the names this device answers to.
pub fn publish(hosts: MyHosts) {
    HOSTS.lock(|slot| *slot.borrow_mut() = Some(hosts));
}

/// Run `f` against the current names, if provisioning has finished.
///
/// A closure rather than a returned clone: the caller wants one boolean out of
/// this — `hosts::with(|h| h.is_mine(header)).unwrap_or(false)` — and copying
/// two bounded strings per HTTP request to get it would be the wrong trade.
/// `None` means provisioning has not finished, which is also when no server is
/// listening.
pub fn with<R>(f: impl FnOnce(&MyHosts) -> R) -> Option<R> {
    HOSTS.lock(|slot| slot.borrow().as_ref().map(f))
}
