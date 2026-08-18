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
static UTC_OFFSET_S: AtomicI32 = AtomicI32::new(0);

/// `EPOCH_AT_BOOT` before a successful sync. Zero would be a real, if absurd,
/// answer — 1970 — and this has to be distinguishable from one.
const UNSYNCED: u32 = 0;

/// The device's notion of local time, for the pregame start-time line.
///
/// Before the first successful sync `utc_offset_s` is `None`, which is what
/// makes the model omit the time rather than show a wrong one.
pub fn local_clock() -> LocalClock {
    let epoch = EPOCH_AT_BOOT.load(Ordering::Relaxed);
    if epoch == UNSYNCED {
        return LocalClock {
            now_epoch_s: 0,
            utc_offset_s: None,
        };
    }
    LocalClock {
        now_epoch_s: epoch.saturating_add(Instant::now().as_secs() as u32),
        utc_offset_s: Some(UTC_OFFSET_S.load(Ordering::Relaxed)),
    }
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
