//! What provisioning decided, published for `/api/status` to read.
//!
//! `main.get_network_status` reconstructed this per request by interrogating
//! `app.ap` and `app.wlan` — live interface objects it could ask "are you
//! active?", "what is your address?". embassy-net has no equivalent to ask: the
//! link's address is on the `Stack`, but *why* the device is in setup mode, and
//! which SSID it failed to join, are facts only [`crate::net::wifi::provision`]
//! ever knew. So they are recorded at the moment they are decided, next to
//! [`crate::net::hosts`] and for the same reason.

use core::cell::RefCell;

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use heapless::String;
use scoreboard_model::SetupReason;

/// The outcome of provisioning, in the shape `/api/status` reports it.
#[derive(Debug, Clone)]
pub enum NetStatus {
    Station {
        ip: String<15>,
        device_name: String<32>,
        /// The network that was joined. Reported by `/api/status` only in AP
        /// mode, but recorded in both so `reset-network` has one thing to
        /// clear.
        configured_ssid: String<32>,
    },
    Ap {
        reason: SetupReason,
        ap_ip: String<15>,
        ap_ssid: String<32>,
        configured_ssid: String<32>,
    },
}

static STATUS: Mutex<CriticalSectionRawMutex, RefCell<Option<NetStatus>>> =
    Mutex::new(RefCell::new(None));

pub fn publish(status: NetStatus) {
    STATUS.lock(|slot| *slot.borrow_mut() = Some(status));
}

/// The published status, or `None` before provisioning has finished — which is
/// the `mode: "unknown"` shape.
pub fn read() -> Option<NetStatus> {
    STATUS.lock(|slot| slot.borrow().clone())
}

/// Forget the credentials this device was provisioned with.
///
/// `POST /api/reset-network`'s runtime half. `api_routes.py` cleared
/// `network.ssid` and `network.password` in the config and left the live
/// connection alone — the device kept running on the network it had joined and
/// entered setup mode at the *next* boot. That is the behaviour, and it is
/// deliberate: dropping the link would take the settings page away from the
/// browser mid-request, before the response reached it.
///
/// So this clears only what the status reports, not the link.
pub fn clear_credentials() {
    STATUS.lock(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(NetStatus::Station {
            configured_ssid, ..
        }) = slot.as_mut()
        {
            configured_ssid.clear();
        }
    });
}
