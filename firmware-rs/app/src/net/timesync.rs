//! The wall clock: one `GET /time`, and what depends on having had one.
//!
//! Port of `main.py:453-488`. MicroPython set `machine.RTC()` from the reply
//! and returned the UTC offset for the poller to hold; there is no RTC here and
//! SPEC §7.4 says so — the epoch and the offset are anchored against
//! `embassy_time::Instant`, which is the only monotonic clock the firmware has
//! and the one every other deadline already rides.
//!
//! # `None` is not `Some(0)`
//!
//! The distinction `main.py` was explicit about, and the reason this stores two
//! values instead of one: a device that has never synced omits a pregame card's
//! start time entirely rather than showing one computed in the wrong timezone.
//! UTC itself is a legitimate answer, and the backend serves it to anyone whose
//! request carries no timezone. [`local_clock`] carries the difference into
//! [`LocalClock`], which is where the model acts on it.
//!
//! Both values therefore need an *unset* state that is not a plausible reading,
//! and neither can use zero: [`UNSYNCED`] for the epoch, [`OFFSET_UNKNOWN`] for
//! the offset. The epoch has had one since Phase 3; the offset did not until
//! Phase S, because until then the epoch and the offset arrived in the same
//! response and having one implied having the other. SNTP broke that — it
//! carries UTC and no timezone — and the two are now separately sourced, which
//! is what [`local_clock`]'s chain describes.
//!
//! # Why this is a phase of the poll loop and not a task of its own
//!
//! It wants the same things the poller has: an [`ApiClient`], a receive buffer,
//! and a TCP socket. Giving it its own would put a second request in flight
//! against a client whose entire design — one buffer, one connection, one
//! caller — exists to make that impossible, and would cost a second socket out
//! of `net`'s budget for one request a day. `main.py` ran the sync inline in
//! the boot sequence for the same reason: it is a step, not a service.

use core::sync::atomic::{AtomicI32, AtomicU32, Ordering};

use embassy_time::{Duration, Instant};
use scoreboard_model::poll::PollError;
use scoreboard_model::sports::LocalClock;

use crate::net::api_client::ApiClient;
#[cfg(not(feature = "direct"))]
use crate::net::api_client::url;

/// Between successful syncs.
///
/// `main.py` synced once at boot and never again, so a device up for a month
/// drifted by whatever its crystal drifted by. A day costs one request and
/// removes the drift; it is the same request either way.
pub const RESYNC: Duration = Duration::from_secs(24 * 60 * 60);

/// After a failed sync. The hour `ota_check_task` waits after a failed check —
/// an unreachable backend is one condition, not two.
pub const RETRY: Duration = Duration::from_secs(60 * 60);

/// Unix seconds at the device's epoch (`Instant` zero), or [`UNSYNCED`].
static EPOCH_AT_BOOT: AtomicU32 = AtomicU32::new(UNSYNCED);

/// The backend's GeoIP answer, or [`OFFSET_UNKNOWN`] until there has been one.
///
/// **Only the backend fetch path writes this.** SNTP carries UTC and no
/// timezone, so a `direct` device leaves it at the sentinel forever and the
/// offset comes from [`crate::timezone`] instead — see [`local_clock`] for the
/// chain that arranges both.
static UTC_OFFSET_S: AtomicI32 = AtomicI32::new(OFFSET_UNKNOWN);

/// `EPOCH_AT_BOOT` before a successful sync. Zero would be a real, if absurd,
/// answer — 1970 — and this has to be distinguishable from one.
const UNSYNCED: u32 = 0;

/// [`UTC_OFFSET_S`] before anything has written a real offset.
///
/// **Zero cannot mean this**, which is the whole reason a sentinel exists: UTC
/// is a legitimate offset that the backend genuinely serves, so a device
/// holding `0` has to be distinguishable from one that has been told nothing.
/// Initialising to `0` instead made a `direct` device — which has no backend to
/// tell it anything — claim to be in UTC from its first SNTP sync, rendering
/// every pregame time in the wrong timezone rather than omitting it. That is
/// exactly what the module docs' "`None` is not `Some(0)`" section exists to
/// prevent, and it was found in review rather than on a panel.
///
/// `i32::MIN` is unreachable as a real value by a wide margin: offsets live
/// within ±14 h, which is ±50,400.
const OFFSET_UNKNOWN: i32 = i32::MIN;

/// The device's notion of local time, for the pregame start-time line.
///
/// # The offset chain, in order
///
/// 1. **The manual override**, if somebody set one. A fixed offset for a board
///    that lives somewhere other than the browser that configured it.
/// 2. **The browser-seeded schedule**, flipping at its stored DST transition.
/// 3. **The backend's GeoIP answer**, which is a guess from the request's
///    source address and is therefore the weakest evidence here — a browser in
///    the same household beats it. A `direct` build has no backend and never
///    reaches this rung.
/// 4. **`None`**, and the model omits the start time.
///
/// Rungs 1 and 2 are [`crate::timezone`]'s and resolved inside
/// [`offset_seconds_at`](crate::timezone::offset_seconds_at); this function
/// owns only the fall-through to 3 and 4. Note that the epoch gates everything:
/// without a clock there is no instant at which to evaluate a schedule, so an
/// unsynced device answers `None` no matter what it has been seeded with.
pub fn local_clock() -> LocalClock {
    let epoch = EPOCH_AT_BOOT.load(Ordering::Relaxed);
    if epoch == UNSYNCED {
        return LocalClock {
            now_epoch_s: 0,
            utc_offset_s: None,
        };
    }
    let now_epoch_s = epoch.saturating_add(Instant::now().as_secs() as u32);
    LocalClock {
        now_epoch_s,
        utc_offset_s: crate::timezone::offset_seconds_at(now_epoch_s).or_else(geoip_offset),
    }
}

/// Rung 3: what the backend's `/time` last said, if it has ever said anything.
fn geoip_offset() -> Option<i32> {
    let offset = UTC_OFFSET_S.load(Ordering::Relaxed);
    (offset != OFFSET_UNKNOWN).then_some(offset)
}

/// Fetch the clock and anchor it. Returns how long until the next attempt.
///
/// A failure is logged and nothing else: it must not count against the poll's
/// failure streak, because `main.py` returned `None` from a failed sync and
/// carried on. The visible consequence is a pregame card with no start time,
/// which is the designed answer to not knowing the time.
pub async fn sync(client: &mut ApiClient, buffer: &mut [u8], base: &str) -> Duration {
    match fetch(client, buffer, base).await {
        Ok(()) => RESYNC,
        Err(error) => {
            crate::error!("time: sync failed, {}", crate::poller::describe(&error));
            RETRY
        }
    }
}

#[cfg(not(feature = "direct"))]
async fn fetch(client: &mut ApiClient, buffer: &mut [u8], base: &str) -> Result<(), PollError> {
    let endpoint = url(base, format_args!("/time"))?;
    let time = client.time(&endpoint, buffer).await?;
    anchor(time.unix_seconds);
    UTC_OFFSET_S.store(time.utc_offset_s, Ordering::Relaxed);
    crate::debug!(
        "time: synced, unix {}, utc offset {} s",
        time.unix_seconds,
        time.utc_offset_s
    );
    Ok(())
}

/// The same phase with no backend behind it: the epoch comes from the NTP pool
/// (S3-DESIGN decision 7) and the offset does not come at all.
///
/// The signature is the backend path's, unchanged, because the call site is the
/// poller's and the poller belongs to another lane — `buffer` and `base` are a
/// receive buffer and a base URL, and SNTP wants neither.
///
/// **`UTC_OFFSET_S` is deliberately not written here, and no offset is
/// invented.** NTP carries UTC and no timezone, so there is nothing honest to
/// put in it; the offset comes from [`crate::timezone`]'s browser-seeded
/// schedule, under its own storage key and on its own cadence.
///
/// Which leaves one thing open, and it is not this module's to close:
/// [`local_clock`] gates the offset on the *epoch*, so with `UTC_OFFSET_S`
/// still initialised to `0` a `direct` device answers `Some(0)` — "I am in
/// UTC" — from its first sync, having been told nothing of the sort. That is
/// exactly the failure the module docs' "`None` is not `Some(0)`" section
/// exists to prevent, and the fix is an unset sentinel on `UTC_OFFSET_S`
/// (`i32::MIN`; real offsets are within ±14 h) so that `local_clock` reports
/// `None` until something writes a real one. It belongs with the code that
/// writes the offset, which is the timezone lane's.
#[cfg(feature = "direct")]
async fn fetch(client: &mut ApiClient, _buffer: &mut [u8], _base: &str) -> Result<(), PollError> {
    let unix_seconds = super::sntp::epoch(client.stack()).await?;
    anchor(unix_seconds);
    crate::debug!(
        "time: synced from {}, unix {}",
        super::sntp::POOL_HOST,
        unix_seconds
    );
    Ok(())
}

/// Pin the Unix second to [`Instant`] zero.
///
/// Shared by both fetch paths rather than written twice, so the two can never
/// disagree about what a synced clock means — the arithmetic below is the whole
/// of SPEC §7.4's "there is no RTC here".
fn anchor(unix_seconds: u32) {
    let uptime = Instant::now().as_secs() as u32;
    EPOCH_AT_BOOT.store(
        unix_seconds.saturating_sub(uptime).max(1),
        Ordering::Relaxed,
    );
    // The ring log's stamps become real from here on. Entries already recorded
    // keep the boot-relative ones they were written with — rewriting them would
    // move a client's `?since=` cursor onto a different entry.
    crate::ringlog::set_wall_clock(unix_seconds);
}
