//! The OTA client: poll, download, verify, arm, confirm.
//!
//! SPEC §8, and the port of `ota.py` + `main.py`'s `ota_check_task`. The
//! decisions — what the manifest means, whether to install, when to give up,
//! what percent to draw — are [`scoreboard_ota`]'s, where a desktop runs them.
//! What is here is the flash, the socket, and the order.
//!
//! # It is not a task, and that is the design
//!
//! `main.py` ran the OTA check in its own asyncio task, which worked because
//! its synchronous download froze the whole event loop — the poller could not
//! interleave even if it wanted to. There is no equivalent accident here, and
//! two things make a separate task actively wrong:
//!
//! * **The `Store` has one owner.** [`crate::poller`] holds it, the
//!   [`Publisher`] and every snapshot decision as task locals, on purpose (its
//!   module docs argue the case). A second task drawing the updating screen
//!   would need a lock around the thing that deliberately has none.
//! * **The poller would paint over it.** An update takes minutes. A poll loop
//!   running underneath would commit a game every 30 s and the progress bar
//!   would flicker between a score and a percentage.
//!
//! So the update *is a phase of the poll loop*: [`check`] borrows the poller's
//! client, buffer, store and publisher, and while it runs the poller is not
//! polling. That is exactly the shape MicroPython had, arrived at deliberately
//! rather than by blocking the executor.
//!
//! Two things fall out of it that are worth stating. The socket budget in
//! [`crate::net`] no longer needs a slot for OTA — the poller's is free while
//! this runs. And the receive buffer needs no companion: [`check`] splits the
//! poller's existing [`poll::RESPONSE_BYTES`] into a header half and a chunk
//! half, which is the "unioned where phases can't overlap (OTA vs. poll)" that
//! SPEC §11's budget table anticipated. The download costs **zero** additional
//! RAM.
//!
//! # The three writes, in the order that survives a power cut
//!
//! 1. **The attempt record, before the download.** A device reset mid-download
//!    must come back having already counted the attempt, or the count never
//!    reaches its limit and a permanently-failing image is retried forever.
//! 2. **DFU, during the download.** Interrupted at any point, the next attempt
//!    simply overwrites it; nothing reads DFU until it is marked.
//! 3. **The swap request, last, and only after the signature verified.** From
//!    that instant the bootloader will swap on the next boot, so it is the only
//!    write that must not happen early.
//!
//! # The screen
//!
//! Percent changes only ([`Progress`]), because every commit crosses the core
//! boundary and wakes core 1 to redraw. The countdown before the reset is
//! `main.py`'s 5→1 and exists so somebody watching sees why the panel is about
//! to go dark for a minute.

mod install;
mod key;

use embassy_time::{Duration, Instant};
use scoreboard_model::{Publisher, Store};
use scoreboard_ota::{Channel, Decision};

pub use install::{confirm, read_boot_state};
/// The trust root. Reached only by the boot-integrated arm — a standalone image
/// has no DFU partition and nothing to verify — but the module is compiled in
/// both, so `key.rs` and its rotation instructions stay reachable either way.
#[cfg(feature = "link-boot-integrated")]
pub(crate) use key::PUBLIC_KEY;

use crate::net::api_client::{ApiClient, ResponseBuffer};
use install::{fetch_manifest, install};

/// This image's identity, stamped in by `build.rs`.
///
/// `"dev"` unless `tools/build.py publish-fw` built it — see
/// [`scoreboard_ota::decide::Decision::DevBuild`] for what that buys.
pub const VERSION: &str = env!("FW_VERSION");

/// How long between checks when the last one was fine. `main.py`'s daily tick.
const HEALTHY_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// And after one that failed. Also `main.py`'s: a transient network problem
/// should not cost a day.
const RETRY_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// Boot traffic settles before the first check, so an update does not compete
/// with provisioning, the first poll and somebody opening the settings page.
/// `main.py` waited the same two minutes.
const SETTLE_DELAY: Duration = Duration::from_secs(120);

/// The pre-reset countdown, in seconds. `main.py`'s 5→1.
#[cfg_attr(
    not(feature = "link-boot-integrated"),
    allow(dead_code, reason = "reached only through the OTA install path, which needs a bootloader")
)]
const COUNTDOWN_SECONDS: u8 = 5;

/// A whole download, ceiling. Generous on purpose: the watchdog's silence gate
/// is the thing that actually catches a *stalled* transfer (this feeds the
/// link's liveness clock per chunk, so a slow-but-moving download keeps the
/// device alive), and this only has to stop a connection that is neither
/// progressing nor erroring from holding the poll loop forever.
#[cfg_attr(
    not(feature = "link-boot-integrated"),
    allow(dead_code, reason = "reached only through the OTA install path, which needs a bootloader")
)]
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(240);

/// What the poller carries between ticks: when to check next.
pub struct Schedule {
    next: Instant,
}

impl Schedule {
    pub fn new() -> Schedule {
        Schedule {
            next: Instant::now() + SETTLE_DELAY,
        }
    }

    pub fn due(&self, now: Instant) -> bool {
        now >= self.next || requested()
    }

    fn arm(&mut self, interval: Duration) {
        self.next = Instant::now() + interval;
    }
}

impl Default for Schedule {
    fn default() -> Schedule {
        Schedule::new()
    }
}

// ---------------------------------------------------------------------------
// The on-demand seam: POST /api/check-update
// ---------------------------------------------------------------------------

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;

/// `POST /api/check-update` asks here; the poll loop answers.
///
/// A [`Signal`] each way rather than a channel: there is one requester (an HTTP
/// handler) and one responder (the poll loop), and a second request arriving
/// while the first is in flight should coalesce into the same check rather than
/// queue a second one. `main.py` was emphatic about this for the same reason —
/// its `request_ota_check` only ever *signalled* the task, so "the
/// download/reboot lifecycle stays single-owner".
static REQUESTED: Signal<CriticalSectionRawMutex, ()> = Signal::new();
static ANSWERED: Signal<CriticalSectionRawMutex, Answer> = Signal::new();

/// What the handler tells the settings page.
///
/// The `status` strings are the SPA's contract
/// (`frontend/src/lib/api/types.ts`), and the version rides along because the
/// page shows it.
#[derive(Debug, Clone, Copy)]
pub struct Answer {
    pub status: &'static str,
    pub message: Option<&'static str>,
}

impl Answer {
    const fn of(status: &'static str) -> Answer {
        Answer {
            status,
            message: None,
        }
    }
}

/// Ask the poll loop to check now. Returns immediately.
pub fn request_check() {
    REQUESTED.signal(());
}

fn requested() -> bool {
    REQUESTED.signaled()
}

/// Wait for the answer to a [`request_check`]. The handler bounds this itself.
pub async fn wait_for_answer() -> Answer {
    ANSWERED.wait().await
}

// ---------------------------------------------------------------------------
// What /api/status reports
// ---------------------------------------------------------------------------

use core::sync::atomic::{AtomicU8, Ordering};

/// Coarse state, for `GET /api/status`. One byte, because the settings page
/// polls it every few seconds while an update runs and the alternative is a
/// lock around a string.
static STATE: AtomicU8 = AtomicU8::new(State::Idle as u8);
static PERCENT: AtomicU8 = AtomicU8::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    Idle = 0,
    Checking = 1,
    Downloading = 2,
    Verifying = 3,
    /// Armed: the bootloader swaps on the next boot.
    Restarting = 4,
    /// This boot is a trial that has not yet earned `mark_booted`.
    Trial = 5,
    /// The bootloader rolled an image back to get here.
    RolledBack = 6,
}

impl State {
    pub const fn as_str(self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::Checking => "checking",
            State::Downloading => "downloading",
            State::Verifying => "verifying",
            State::Restarting => "restarting",
            State::Trial => "trial",
            State::RolledBack => "rolled_back",
        }
    }

    fn from_byte(byte: u8) -> State {
        match byte {
            1 => State::Checking,
            2 => State::Downloading,
            3 => State::Verifying,
            4 => State::Restarting,
            5 => State::Trial,
            6 => State::RolledBack,
            _ => State::Idle,
        }
    }
}

pub fn set_state(state: State) {
    STATE.store(state as u8, Ordering::Relaxed);
}

#[cfg_attr(
    not(feature = "link-boot-integrated"),
    allow(dead_code, reason = "reached only through the OTA install path, which needs a bootloader")
)]
pub(crate) fn set_percent(percent: u8) {
    PERCENT.store(percent, Ordering::Relaxed);
}

/// Whether this boot still owes the bootloader a `mark_booted`.
///
/// True after a swap *and* after a revert: both leave the state machine
/// somewhere the next update cannot start from, and only `mark_booted` puts it
/// back to `Boot`. [`crate::supervise`] asks, gates, and calls [`confirm`].
pub fn needs_confirm() -> bool {
    matches!(
        State::from_byte(STATE.load(Ordering::Relaxed)),
        State::Trial | State::RolledBack
    )
}

/// `(state, percent)` for `/api/status`.
pub fn status() -> (State, u8) {
    (
        State::from_byte(STATE.load(Ordering::Relaxed)),
        PERCENT.load(Ordering::Relaxed),
    )
}

// ---------------------------------------------------------------------------
// The check
// ---------------------------------------------------------------------------

/// Everything [`check`] borrows from the poll loop.
pub struct Context<'a> {
    pub client: &'a mut ApiClient,
    pub buffer: &'a mut ResponseBuffer,
    pub base_url: &'a str,
    pub store: &'a mut Store,
    pub publisher: &'a mut Publisher<'static>,
}

/// Run one update check, and everything it leads to.
///
/// Returns only when the device is staying on this image; an installed update
/// resets the chip from inside and never comes back.
pub async fn check(cx: Context<'_>, schedule: &mut Schedule) {
    // What to paint when the check ends. Trial and RolledBack are the health
    // gate's to clear — only `confirm` may — and a check that runs during an
    // unconfirmed boot must resume them, not stomp them: the settle-delay
    // check landing mid-trial repainted an armed, unconfirmed image as `idle`
    // for the back half of its trial window (drill day 2026-08-16, step 6),
    // which is exactly the state an operator must be able to see.
    let resume = match status().0 {
        State::Trial => State::Trial,
        State::RolledBack => State::RolledBack,
        _ => State::Idle,
    };
    REQUESTED.reset();
    let answer = run(cx).await;
    // Re-arm before answering, so a handler that immediately asks again cannot
    // land between the two and be dropped.
    schedule.arm(if answer.status == "error" {
        RETRY_INTERVAL
    } else {
        HEALTHY_INTERVAL
    });
    // Unless the health gate confirmed while this check was in flight — its
    // `Idle` must win, or the resumed Trial would have no one left to clear it.
    if CONFIRMED_THIS_BOOT.load(Ordering::Relaxed) {
        set_state(State::Idle);
    } else {
        set_state(resume);
    }
    ANSWERED.signal(answer);
}

/// Latched by [`confirm`] so a concurrent [`check`] cannot resurrect a
/// Trial/RolledBack status the gate has already cleared.
static CONFIRMED_THIS_BOOT: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

pub(crate) fn record_confirmed() {
    CONFIRMED_THIS_BOOT.store(true, Ordering::Relaxed);
}

async fn run(cx: Context<'_>) -> Answer {
    let Context {
        client,
        buffer,
        base_url,
        store,
        publisher,
    } = cx;

    let (enabled, channel, api_key) = crate::config::with(|config| {
        (
            config.ota.enabled,
            Channel::from_config(&config.ota.channel),
            config.api.key.clone(),
        )
    });

    // Cheap refusals first, before a socket is opened: a device with updates
    // switched off, or a locally-built image, has no business asking.
    if !enabled {
        crate::debug!("ota: checks disabled by configuration");
        return Answer::of("disabled");
    }
    if scoreboard_ota::is_dev_build(VERSION) {
        crate::debug!("ota: this is a dev build; a check would roll it back");
        return Answer::of("dev_deploy");
    }

    set_state(State::Checking);
    let manifest = match fetch_manifest(client, buffer, base_url, &api_key, channel).await {
        Ok(manifest) => manifest,
        Err(message) => {
            crate::error!("ota: manifest check failed: {}", message);
            return Answer {
                status: "error",
                message: Some(message),
            };
        }
    };

    let record = crate::storage::load_ota_attempt();
    let local = scoreboard_ota::Local {
        running: VERSION,
        enabled,
        record: record.as_ref(),
    };
    match scoreboard_ota::decide(&manifest, &local) {
        Decision::Install => {}
        Decision::Current => {
            crate::debug!("ota: already running {}", VERSION);
            return Answer::of("current");
        }
        Decision::Blocked { reverted, attempts } => {
            let message = scoreboard_ota::decide::blocked_message(reverted);
            crate::error!(
                "ota: {} is blocked after {} attempt(s): {}",
                manifest.version.as_str(),
                attempts,
                message
            );
            return Answer {
                status: "error",
                message: Some(message),
            };
        }
        // Both re-checked above, before the fetch; unreachable here.
        Decision::Disabled => return Answer::of("disabled"),
        Decision::DevBuild => return Answer::of("dev_deploy"),
    }

    install(
        client, buffer, base_url, &api_key, channel, store, publisher, &manifest, record,
    )
    .await
}
