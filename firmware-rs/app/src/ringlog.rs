//! The device's own log, and the clock it stamps entries with.
//!
//! [`scoreboard_log::Ring`] holds the entries and decides everything about
//! their shape; this module is the firmware's half — the `static`, the lock,
//! and the two macros that write to defmt and the ring from one call site.
//!
//! # Two channels, one call site
//!
//! SPEC §9 splits logging in two. **defmt over RTT** is the development
//! channel: it needs a probe attached, it costs almost nothing because the
//! strings stay on the host, and it is what you read while the device is on the
//! bench. **The ring** is the deployed channel: it is what `/api/logs` serves
//! and therefore the only way to find out what a unit in someone else's living
//! room has been doing.
//!
//! Every [`error!`](crate::error) and [`debug!`](crate::debug) writes to both,
//! which is the point — a line that only reached the probe is a line the owner
//! of a gift unit cannot see, and remembering to write two statements is a
//! discipline that lasts about a week.
//!
//! **Format arguments are evaluated twice**, once per channel, so they must be
//! plain values rather than calls with side effects. And the format string has
//! to be legal for both: plain `{}` holes work in defmt and in `core::fmt`
//! alike, while defmt's typed hints (`{=u8}`) do not compile as `core::fmt`.
//! Both macros are used with plain `{}` throughout.
//!
//! # The clock
//!
//! Entries carry seconds. Until task #11's time sync lands there is no wall
//! clock, so [`now_seconds`] returns seconds since boot and the settings SPA
//! renders them as `+123s` — it already has that branch, for the same reason,
//! because MicroPython's `time.time()` before an RTC sync was equally
//! fictional. [`set_wall_clock`] is the one-line seam that makes them real.

use core::cell::RefCell;
use core::sync::atomic::{AtomicU32, Ordering};

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::Instant;
use scoreboard_log::{Rendered, Ring};

/// The ring itself.
///
/// A `CriticalSectionRawMutex` because both cores may log — core 1's render
/// loop is expected to stay on defmt, but nothing structural stops a panic path
/// or a future task from recording here, and a lock that is wrong across cores
/// is the kind of bug that corrupts a diagnostic surface exactly when it is
/// needed. Holding it across a whole read is also what lets `Ring` skip
/// `logger.py`'s torn-wrap guard.
///
/// Never held across an `await`: every function here locks, does bounded work,
/// and returns.
static RING: Mutex<CriticalSectionRawMutex, RefCell<Ring>> =
    Mutex::new(RefCell::new(Ring::new()));

/// Unix seconds at boot, or 0 while unknown. See the module docs.
static WALL_CLOCK_EPOCH: AtomicU32 = AtomicU32::new(0);

/// Seconds for a log entry's timestamp.
pub fn now_seconds() -> u32 {
    let uptime = Instant::now().as_secs() as u32;
    WALL_CLOCK_EPOCH.load(Ordering::Relaxed).saturating_add(uptime)
}

/// Anchor the wall clock: `unix_seconds` is the time *now*, with the device
/// `Instant::now()` seconds into its uptime.
///
/// Unused until task #11 calls it — the `allow` is the seam being declared
/// before its caller exists, not dead code left behind.
///
/// Task #11's time sync calls this. Entries recorded before it keep the
/// boot-relative stamps they were written with — rewriting history in the ring
/// would make a client's `?since=` cursor point at a different entry than it
/// did a moment ago.
#[allow(dead_code, reason = "task #11's time sync is the only caller")]
pub fn set_wall_clock(unix_seconds: u32) {
    let uptime = Instant::now().as_secs() as u32;
    WALL_CLOCK_EPOCH.store(unix_seconds.saturating_sub(uptime), Ordering::Relaxed);
}

/// Record a message. Prefer the [`error!`](crate::error) / [`debug!`](crate::debug)
/// macros, which also reach defmt.
pub fn record(level: Level, args: core::fmt::Arguments) {
    let ts = now_seconds();
    RING.lock(|ring| ring.borrow_mut().record_fmt(level, ts, args));
}

/// Set the filter from `config.json`'s `log.level`.
pub fn set_level(level: Level) {
    RING.lock(|ring| ring.borrow_mut().set_level(level));
}

/// `(entries held, newest sequence number)` — what `/api/status` reports where
/// MicroPython reported free flash for the log file.
pub fn stats() -> (u32, u32) {
    RING.lock(|ring| {
        let ring = ring.borrow();
        (ring.since(0).count() as u32, ring.latest_seq())
    })
}

/// Render one NDJSON chunk of entries newer than `after`.
///
/// The lock is taken and released inside this call, so the caller can write the
/// chunk to a socket — an `await` — without holding it. That is why `/api/logs`
/// streams in passes rather than snapshotting the whole ring.
pub fn render_ndjson_since(after: u32, out: &mut [u8]) -> Rendered {
    RING.lock(|ring| ring.borrow().render_ndjson_since(after, out))
}

/// Write to defmt and the ring at ERROR.
#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {{
        defmt::error!($($arg)*);
        $crate::ringlog::record($crate::ringlog::Level::Error, format_args!($($arg)*));
    }};
}

/// Write to defmt and the ring at DEBUG.
///
/// defmt's own level for these is `info`: defmt `debug` is compiled out at the
/// `DEFMT_LOG=info` the bench runs at, and a line that reaches the ring but not
/// the probe would make the two channels disagree about what happened.
#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {{
        defmt::info!($($arg)*);
        $crate::ringlog::record($crate::ringlog::Level::Debug, format_args!($($arg)*));
    }};
}

pub use scoreboard_log::Level;

