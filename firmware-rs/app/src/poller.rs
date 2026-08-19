//! The poll loop: core 0's one owner of the display state.
//!
//! Port of `poller.py`'s `GamePoller.run` / `_tick`. What the rotation *is* —
//! live-first ordering, the league filter, the stale-clock guard, the
//! view-identity rule — belongs to `scoreboard-model` and is host-tested there;
//! what is here is the I/O around it and the order the two are done in.
//!
//! # Who owns the display state, and why it is a task rather than a lock
//!
//! [`Store`], [`Slate`] and the [`Publisher`] are **owned by this task**, as
//! locals. Everything else that wants to change what is on the panel sends a
//! [`Command`]. There is no mutex, and that is a decision worth its paragraph,
//! because the obvious alternative — `Mutex<StoreCtx>` behind
//! `embassy_sync` — would have worked too.
//!
//! Three things decided it:
//!
//! - **`poller.py`'s skip machine is only correct because nothing else can be
//!   running.** Its own docstring says so: the flags are plain booleans because
//!   the poll loop and both button hooks share one asyncio loop and can only
//!   interleave at `await` points. That argument survives the port — every task
//!   that would touch the store runs on core 0's single executor — but it is an
//!   argument about *where tasks were spawned*, and it fails silently the day
//!   one moves to core 1 or to an interrupt executor. Single ownership makes
//!   the same guarantee structurally: [`SkipMachine`] is a `&mut` local, and a
//!   second writer does not compile.
//! - **A mutex would have to be held across the commit, and the commit sits
//!   between two awaits.** `_poll_current` fetches a detail, then fetches
//!   crests, then commits — the decoded game borrows the receive buffer for
//!   that whole span. Holding a lock across it is exactly the "no borrows
//!   across `.await`" rule this codebase keeps elsewhere
//!   ([`crate::config`]'s docs); not holding it means the commit and the
//!   publish are separately locked, and a button press landing between them
//!   sees a state no tick ever produced.
//! - **Task #12 joins by adding a variant, not by re-architecting.** A button
//!   press becomes `COMMANDS.send(...)`; the arm/reject decision stays here,
//!   with the state it is about, which is also where `poller.py` had it.
//!
//! The cost, stated plainly: a command is applied *between* ticks rather than
//! the instant it arrives. `skip()` set the spinner toast at press time and
//! woke the loop; here the press is delivered when the in-flight request
//! finishes, so the spinner can lag a press by one request. Bench-measured that
//! is 60–300 ms, and the 15 s request timeout is its ceiling. Closing the gap
//! would mean giving the input task its own path to the store, which is the
//! thing this design is buying.
//!
//! # What is not here
//!
//! No exponential backoff, and none in `poller.py` either: the sleep is always
//! `poll_interval_seconds`. Nothing else in the firmware retries at all, so a
//! backend that comes back is on the panel within one interval.
//!
//! # The crest warmer is a phase of this loop too
//!
//! After a tick commits, and before the sleep it has earned, the loop spends up
//! to [`WARM_FETCHES`] requests filling crest slots for games the rotation has
//! not reached yet ([`Poller::warm_crests`]). It is here, rather than in a task
//! of its own, for the reason every other phase is: **there is one API client
//! and it takes `&mut self`**, so a second fetcher would need a second
//! connection, and a second connection would need its own socket, its own
//! buffer and a rule for which of the two a button press interrupts. Inside the
//! loop it inherits the answers — one request in flight, the store untouched,
//! and a command channel that is already checked between fetches.
//!
//! What it buys is the burst: a board that has been idle has its crests in the
//! pool, so a skip, a league-skip or a mashed button paints the next game
//! immediately instead of waiting out two logo fetches. What it costs, honestly
//! stated, is that a press arriving mid-warm waits for one logo — the same
//! bound as a press arriving mid-tick, and smaller than one. **That cost does
//! not scale with [`WARM_FETCHES`]**, because the channel is checked between
//! every fetch rather than after the batch; the constant's own docs say what it
//! does decide, which is how fast a cold board converges.
//!
//! It also stops at the window's deadline, so however many fetches it is
//! allowed and however short `poll_interval_seconds` is set, the next tick is
//! never late because of it.
//!
//! It cannot affect anything else in this file. A warm fetch happens outside
//! [`Poller::tick`], so it cannot reach [`FailureTracker`]; a failed one is a
//! debug line and a retry on a later window, and [`Health`] never hears about
//! it. That is structural rather than careful: `record_failure` is only called
//! on `tick`'s return value, and the warmer has none.
//!
//! # The OTA check is a phase of this loop, not a task of its own
//!
//! [`crate::ota`]'s module docs carry the argument; the consequence here is one
//! branch at the top of the loop. While an update runs, this loop is not
//! polling — which is what stops a game commit painting over the progress bar,
//! and is the same arrangement `main.py` got for free from a synchronous
//! download freezing its event loop.

use embassy_futures::select::{Either, select};
use embassy_net::Stack;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Instant, Timer};
use scoreboard_config::DeviceConfig;
use scoreboard_input::button::Press;
use scoreboard_input::menu::{Action, Button as MenuButton, MenuController};
use scoreboard_model::feed::LeagueId;
use scoreboard_model::poll::{
    self, FailureTracker, Health, PollError, SkipKind, SkipMachine, SkipVerdict,
};
use scoreboard_model::prefetch::WarmIndex;
#[cfg(not(feature = "direct"))]
use scoreboard_model::prefetch::Step;
use scoreboard_model::slate::MAX_SOURCES;
#[cfg(not(feature = "direct"))]
use scoreboard_model::snapshot::ABBR;
#[cfg(not(feature = "direct"))]
use scoreboard_model::snapshot::GAME_ID;
use scoreboard_model::snapshot::Millis;
#[cfg(not(feature = "direct"))]
use scoreboard_model::store::Logos;
#[cfg(not(feature = "direct"))]
use scoreboard_model::text::{Text, set_plain};
#[cfg(not(feature = "direct"))]
use scoreboard_model::{GameFeed, WireFeed};
use scoreboard_model::{Mode, Publisher, Slate, Sport, Store};

#[cfg(not(feature = "direct"))]
use crate::logos::Warm;
use crate::logos::{CrestDirectory, WARM_GAMES};
#[cfg(not(feature = "direct"))]
use crate::net::api_client::{Etag, url};
use crate::net::api_client::{ApiClient, ResponseBuffer, base_url};
use crate::net::timesync;
use crate::settings;

/// The fetch phases' direct-feed twins (SPEC §14): same names, same
/// signatures, fed from ESPN instead of the backend. Selected per-function
/// rather than by swapping the whole poller, because everything that is not
/// a fetch — the rotation, the skip machine, the commit — must not fork.
#[cfg(feature = "direct")]
mod direct;

/// What the poller can be asked to do from outside.
///
/// Deliberately small. Everything the running configuration already answers —
/// the poll interval, the rotation period, the backend URL — is read from
/// [`crate::config`] each tick rather than pushed, so a `PUT /api/config`
/// changing them needs no message at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// New UI colours are waiting in [`settings::take_ui_colors`].
    ///
    /// They reach the panel *in the snapshot*, so they only move when something
    /// commits — and on an idle scoreboard the next commit is up to
    /// `poll_interval_seconds` away, on a screen the render loop is skipping.
    /// `api_routes.py` wrote them into a module the renderers read directly and
    /// they appeared on the next frame; this is what keeps that promise.
    ColorsChanged,
    /// A button press, already debounced and folded into short or long by
    /// [`crate::inputs`]. Where it goes is [`MenuController`]'s decision, not
    /// the input task's — see its module docs.
    Press(MenuButton, Press),
}

/// Presses land here, and so does `PUT /api/config`.
///
/// Four deep: a command is consumed between ticks, and no producer sends more
/// than one per user action. A full channel means the poller is inside a
/// request, and `try_send`'s failure is the right answer — a press that could
/// not be delivered is a press that would have been rejected anyway.
///
/// The depth is also what makes a **burst** of presses advance the rotation
/// exactly once, together with the skip machine: the first press arms a skip,
/// and every press that arrives before that skip's tick completes is *rejected*
/// rather than queued (`SkipMachine::request`). Four slots is enough that a
/// human mashing the button at ~10 Hz never overflows the channel between
/// ticks, and if one ever did, the dropped press would have been rejected on
/// arrival anyway.
static COMMANDS: Channel<CriticalSectionRawMutex, Command, 4> = Channel::new();

/// Requests the crest warmer may make in one idle window.
///
/// **This number is not the latency bound, and reading it as one is the mistake
/// to avoid.** The warmer checks the command channel between every fetch, so a
/// button press waits for whatever single request is in flight — one 1.1 KB
/// logo — and it waits exactly that long whether this is 2 or 60. The bound is
/// the check, not the count.
///
/// What the number actually decides is how fast a cold board converges, and it
/// is set against that. A 15-game MLB slate is 15 probes and 30 crests; at six
/// per 30 s window that is warm in four to six minutes from boot, which is
/// inside the first sitting. At two it took three times as long — slower than
/// the rotation warms the pool by simply visiting games, which made the warmer
/// close to pointless on a board being watched.
///
/// Six rather than more is politeness to a backend that is ours and small:
/// roughly 7 KB and a few seconds of a window that is otherwise idle. The
/// warmer also stops at the window's deadline, so this can never push the next
/// poll late however short `poll_interval_seconds` is configured.
const WARM_FETCHES: usize = 6;

/// Send a command, dropping it if the queue is full. Never blocks, so an HTTP
/// handler can call it.
pub fn command(command: Command) {
    if COMMANDS.try_send(command).is_err() {
        defmt::warn!("poller: command queue full, dropped one");
    }
}

// ---------------------------------------------------------------------------
// Liveness, for the watchdog's health gate
// ---------------------------------------------------------------------------

use core::sync::atomic::{AtomicU32, Ordering};

/// Consecutive failed ticks. Zero means the last tick succeeded.
static FAILURE_STREAK: AtomicU32 = AtomicU32::new(0);
/// Uptime seconds at the last successful tick; [`NEVER`] until the first one.
static LAST_SUCCESS_S: AtomicU32 = AtomicU32::new(NEVER);

/// `LAST_SUCCESS_S` before any poll has succeeded. Not zero: zero is a real
/// uptime, and "succeeded at boot" is the opposite of what this means.
pub const NEVER: u32 = u32::MAX;

/// Assemble what the poller knows about the network.
///
/// Two clocks from two places, and the split is the point:
/// [`LAST_SUCCESS_S`] is the *backend's* — a tick that fetched, decoded and
/// committed — and belongs to this loop, while the *link's* is stamped by
/// [`api_client`](crate::net::api_client) on any HTTP answer at all. What each
/// is allowed to conclude is [`Health`]'s documentation, and the gate that
/// reads them is [`poll::gate`].
pub fn health() -> Health {
    let now = Instant::now().as_secs() as u32;
    let last_success = LAST_SUCCESS_S.load(Ordering::Relaxed);
    Health {
        streak: FAILURE_STREAK.load(Ordering::Relaxed),
        since_success_s: (last_success != NEVER).then(|| now.saturating_sub(last_success)),
        since_answer_s: last_answer_uptime_s().map(|at| now.saturating_sub(at)),
    }
}

/// The link's last answer, whichever client heard it. A `direct` build's data
/// plane is `net::espn` and its OTA plane is still `api_client`, so the
/// *newer* of the two clocks is the honest reading — merging is what keeps a
/// device that only ever talks to ESPN from looking permanently silent to
/// [`poll::gate`], which resets on silence.
fn last_answer_uptime_s() -> Option<u32> {
    let backend = crate::net::api_client::last_answer_uptime_s();
    #[cfg(feature = "direct")]
    let answer = match (backend, crate::net::espn::last_answer_uptime_s()) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
    #[cfg(not(feature = "direct"))]
    let answer = backend;
    answer
}

// ---------------------------------------------------------------------------
// Sources
// ---------------------------------------------------------------------------

/// The configured leagues, in poll order: MLB, NBA, football leagues, soccer
/// leagues. `sources_from_config` (`poller.py:223-239`).
///
/// Read once, at task start. `api_routes.py` re-applied colours and display
/// settings live but never rebuilt the poller's sources, so changing which
/// leagues are polled has always taken a reboot; [`Slate::set_sources`] drops
/// every cached game anyway, which is why it is not a thing to do casually.
fn sources_from_config(config: &DeviceConfig) -> heapless::Vec<LeagueId, MAX_SOURCES> {
    let mut sources = heapless::Vec::new();
    if config.sports.mlb.enabled {
        let _ = sources.push(LeagueId::from_slug(Sport::Mlb, "mlb"));
    }
    if config.sports.nba.enabled {
        let _ = sources.push(LeagueId::from_slug(Sport::Nba, "nba"));
    }
    for slug in config.sports.football.active() {
        let _ = sources.push(LeagueId::from_slug(Sport::Football, slug));
    }
    for slug in config.sports.soccer.active() {
        let _ = sources.push(LeagueId::from_slug(Sport::Soccer, slug));
    }
    sources
}

/// The cadence, re-read every tick so a `PUT /api/config` takes effect without
/// a reboot — which is what `poller.py` got for free by holding the `Config`
/// object the API route mutated.
struct Cadence {
    poll_interval: Duration,
    rotation_ms: Millis,
    base_url: heapless::String<{ crate::net::api_client::URL_BYTES }>,
}

fn cadence() -> Cadence {
    crate::config::with(|config| Cadence {
        poll_interval: Duration::from_secs(config.display.poll_interval_seconds as u64),
        rotation_ms: config.display.game_rotation_seconds as Millis * 1_000,
        base_url: base_url(&config.api.url),
    })
}

/// An error, in the words the panel would use. For log lines, which want the
/// same vocabulary the error screen shows so the two can be compared.
pub fn describe(error: &PollError) -> DescribedError {
    DescribedError(poll::friendly(error))
}

pub struct DescribedError(poll::Friendly);

/// Whether a command wants the poll loop to tick now or to finish its sleep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wake {
    Now,
    Later,
}

impl defmt::Format for DescribedError {
    fn format(&self, formatter: defmt::Formatter<'_>) {
        defmt::write!(
            formatter,
            "{=str}: {=str}",
            self.0.kind.as_str(),
            self.0.detail.as_str()
        )
    }
}

impl core::fmt::Display for DescribedError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "{}: {}", self.0.kind, self.0.detail)
    }
}

// ---------------------------------------------------------------------------
// The task
// ---------------------------------------------------------------------------

/// Everything the loop carries between ticks.
///
/// A struct rather than a pile of locals so the phases below can take exactly
/// the pieces they touch — which is also what keeps the receive buffer's
/// borrows disjoint from the slate's.
struct Poller {
    store: &'static mut Store,
    publisher: Publisher<'static>,
    client: ApiClient,
    slate: Slate,
    crests: CrestDirectory,
    /// Who is playing in each game the loop has heard of, so the warmer asks
    /// the backend once per game rather than once per idle window. Filled by
    /// every commit for free — see [`Poller::poll_current`].
    warm: WarmIndex<WARM_GAMES>,
    failures: FailureTracker,
    skips: SkipMachine,
    /// Per-source, parallel to [`Slate::sources`]. `None` until a list refresh
    /// has returned one. Wire only: ESPN serves no ETag worth a conditional
    /// request, so the direct build has nothing to store.
    #[cfg(not(feature = "direct"))]
    etags: [Option<Etag>; MAX_SOURCES],
    /// The direct feed's client, scratches and crest-path index, in one field
    /// so the wire build carries none of it. See [`direct::DirectState`].
    #[cfg(feature = "direct")]
    direct: direct::DirectState,
    /// When the rotation last advanced. `None` until the first list refresh,
    /// which is what `poller.py` used to mean "this is the first tick".
    last_rotation_ms: Option<Millis>,
    /// When to fetch the backend clock again. Starts in the past, so the first
    /// tick syncs before it commits anything — `main.py` ran the sync in the
    /// boot sequence, ahead of the poller, for the same reason.
    next_time_sync: Instant,
    /// The league-select session. Lives here because it mutates the `Slate` and
    /// the `Store`, and this task owns both — `menu.py`'s controller had the
    /// same two references for the same reason.
    menu: MenuController,
    /// When the next update check is due. A local for the same reason
    /// everything else here is one: the check borrows this task's client,
    /// buffer, store and publisher, so its schedule belongs to the task that
    /// runs it.
    ota: crate::ota::Schedule,
    buffer: ResponseBuffer,
}

/// The poll loop. Station mode only, as `main.py`'s task table has it.
#[embassy_executor::task]
pub async fn run(
    store: &'static mut Store,
    publisher: Publisher<'static>,
    stack: Stack<'static>,
) -> ! {
    let sources = crate::config::with(sources_from_config);
    let mut poller = Poller {
        store,
        publisher,
        client: ApiClient::new(stack),
        slate: Slate::new(),
        crests: CrestDirectory::new(),
        warm: WarmIndex::new(),
        failures: FailureTracker::new(),
        skips: SkipMachine::new(),
        #[cfg(not(feature = "direct"))]
        etags: [const { None }; MAX_SOURCES],
        #[cfg(feature = "direct")]
        direct: direct::DirectState::new(stack),
        last_rotation_ms: None,
        next_time_sync: Instant::now(),
        menu: MenuController::new(),
        ota: crate::ota::Schedule::new(),
        buffer: [0; poll::RESPONSE_BYTES],
    };
    poller.slate.set_sources(&sources);

    if sources.is_empty() {
        // Not an error: `sources_from_config` returns nothing when every sport
        // is switched off, and the panel's answer to that is the same as its
        // answer to a day with no games.
        crate::error!("poll: no sports enabled, nothing to poll");
    }
    for source in &sources {
        crate::debug!("poll: source {}", source.key.as_str());
    }

    loop {
        let cadence = cadence();

        // Before the tick, not after: an update that installs never returns
        // from here, and doing it first means the panel goes from a live game
        // to the progress bar rather than showing one more stale score first.
        if poller.ota.due(Instant::now()) {
            poller.check_for_update(&cadence).await;
        }

        // Also before the tick, and from this frame on purpose: the TLS
        // handshake must run over the shallowest stack the loop has — see
        // `direct::Poller::pre_connect`.
        #[cfg(feature = "direct")]
        poller.pre_connect().await;

        let now = Instant::now().as_millis();
        let outcome = poller.tick(&cadence, now).await;
        // Fixed here, before the warmer runs: the poll interval has always been
        // measured from the end of the tick, and warming spends the front of
        // that window rather than adding to it. A board that warms therefore
        // polls at exactly the cadence a board that does not warm polls at.
        let deadline = Instant::now() + cadence.poll_interval;
        match outcome {
            Ok(()) => {
                if let Some(streak) = poller.failures.record_success() {
                    // ERROR level, as `poller.py:362-365` had it: a recovery is
                    // the line you go looking for after the fact, so it has to
                    // survive the same filter the failures did.
                    crate::error!("poll: recovered after {} failed polls", streak);
                }
                FAILURE_STREAK.store(0, Ordering::Relaxed);
                LAST_SUCCESS_S.store(Instant::now().as_secs() as u32, Ordering::Relaxed);
                // Only after a tick that worked. Warming through an outage
                // would spend every game's give-up count on the outage, and
                // leave the whole slate marked unwarmable once it ended.
                poller.warm_crests(&cadence, deadline).await;
            }
            Err(error) => {
                let failure = poller.failures.record_failure(now, &error);
                crate::error!(
                    "poll: poll failed ({}/{}): {}",
                    failure.streak,
                    poll::MAX_FAILURES,
                    describe(&error)
                );
                FAILURE_STREAK.store(failure.streak, Ordering::Relaxed);
                if let Some(screen) = failure.screen {
                    screen.commit(poller.store);
                    poller.publisher.publish(poller.store.snapshot());
                }
            }
        }

        poller.sleep_until(deadline).await;
    }
}

impl Poller {
    /// Run one OTA check, lending it everything it needs.
    ///
    /// The borrows are split out of `self` by hand because the check needs four
    /// disjoint fields at once and `&mut self` would lend all of them together.
    async fn check_for_update(&mut self, cadence: &Cadence) {
        let Poller {
            client,
            store,
            publisher,
            buffer,
            ota,
            ..
        } = self;
        crate::ota::check(
            crate::ota::Context {
                client,
                buffer,
                base_url: cadence.base_url.as_str(),
                store,
                publisher,
            },
            ota,
        )
        .await;
    }

    /// One tick. `poller.py:394-437`, including the `finally`.
    async fn tick(&mut self, cadence: &Cadence, now: Millis) -> Result<(), PollError> {
        // Before anything commits: a pregame card built without the offset
        // shows no start time, and the first tick is the one most likely to
        // build one.
        if Instant::now() >= self.next_time_sync {
            let delay =
                timesync::sync(&mut self.client, &mut self.buffer, &cadence.base_url).await;
            self.next_time_sync = Instant::now() + delay;
        }

        let skip = self.skips.consume();
        let result = self.tick_inner(cadence, now, skip).await;
        // Every exit path — success, empty slate, 404, or a failed request —
        // releases the spinner the consumed skip was holding, so the toast's
        // lifetime is exactly the work it announced.
        if self.skips.finish() {
            self.store.clear_toast_if_sticky(Instant::now().as_millis());
            self.publish();
        }
        result
    }

    async fn tick_inner(
        &mut self,
        cadence: &Cadence,
        now: Millis,
        skip: Option<SkipKind>,
    ) -> Result<(), PollError> {
        let rotation_due = self
            .last_rotation_ms
            .is_some_and(|last| now.saturating_sub(last) >= cadence.rotation_ms);

        // Animation restamps are the store's, under the view-identity rule, so
        // rotating needs no flag here.
        if self.last_rotation_ms.is_none() {
            self.refresh_lists(cadence, true).await?;
            self.last_rotation_ms = Some(now);
        } else if skip == Some(SkipKind::League) {
            self.refresh_lists(cadence, false).await?;
            self.slate.advance_league();
            self.last_rotation_ms = Some(now);
        } else if skip == Some(SkipKind::Game) || (rotation_due && !self.slate.locked()) {
            self.refresh_lists(cadence, false).await?;
            self.slate.advance();
            self.last_rotation_ms = Some(now);
        }

        if self.slate.is_empty() {
            // A non-empty slate always yields at least one rotation entry, so
            // only a genuinely empty merged slate reaches here.
            //
            // `poller.py` committed this every tick. Committing only on the
            // transition is the same screen and one fewer redraw: `no_games` is
            // a static mode, so an unconditional commit would wake core 1 out
            // of its skip once a poll interval to draw the identical frame.
            if self.store.mode() != Mode::NoGames {
                self.store.set_mode(Mode::NoGames);
                self.publish();
            }
            return Ok(());
        }

        self.poll_current(cadence).await
    }

    /// Refresh every source's list and rebuild the rotation.
    ///
    /// **A single source failing keeps its cached slate** — a dead league feed
    /// must not blank the others — and the tick only counts as failed when
    /// every source failed (`poller.py:439-475`).
    #[cfg(not(feature = "direct"))]
    async fn refresh_lists(&mut self, cadence: &Cadence, initial: bool) -> Result<(), PollError> {
        let Poller {
            client,
            slate,
            etags,
            warm,
            buffer,
            ..
        } = self;

        let mut failures = 0usize;
        let mut last_error = None;
        let sources = slate.sources().len();
        for (index, etag_slot) in etags.iter_mut().enumerate().take(sources) {
            let key = slate.sources()[index].key.clone();
            let request = url(&cadence.base_url, format_args!("/{}/games", key.as_str()));
            let outcome = match request {
                Ok(url) => {
                    // The conditional request goes out on every refresh but the
                    // first. The stored value is the header verbatim, quotes
                    // included: the backend compares strings.
                    let conditional = if initial { None } else { etag_slot.as_deref() };
                    client.game_list(&url, conditional, &mut buffer[..]).await
                }
                Err(error) => Err(error),
            };

            match outcome {
                Ok(fetched) if fetched.status == 304 => {
                    // The cached slate stands. Worth a ring line of its own on
                    // top of the request log: a 304 is the ETag *working*, and
                    // the alternative reading of the same evidence — a list
                    // that never changes because the backend is stuck — looks
                    // identical from the panel.
                    crate::debug!("poll: {} list unchanged (304)", key.as_str());
                }
                Ok(fetched) => {
                    let etag = fetched.etag;
                    let mut update = slate.update_source(index as u8);
                    match WireFeed.list(fetched.body, &mut update) {
                        Ok(()) => *etag_slot = etag,
                        Err(error) => {
                            // The source's entries are whatever decoded before
                            // the error; not storing the ETag is what makes the
                            // next refresh unconditional and self-healing.
                            failures += 1;
                            *etag_slot = None;
                            let error = PollError::Decode(error);
                            crate::error!(
                                "poll: {} list did not decode: {}",
                                key.as_str(),
                                describe(&error)
                            );
                            last_error = Some(error);
                        }
                    }
                }
                Err(error) => {
                    failures += 1;
                    crate::error!(
                        "poll: {} list refresh failed, keeping cached slate: {}",
                        key.as_str(),
                        describe(&error)
                    );
                    last_error = Some(error);
                }
            }
        }

        if sources > 0 && failures == sources {
            return Err(last_error.expect("a failure recorded an error"));
        }
        slate.rebuild();
        // The warmer's records are keyed to games, so the games that just left
        // the day take theirs with them. Games that only left the *rotation* —
        // every pregame, the moment one game goes live — keep theirs, which is
        // what stops a first pitch costing a re-probe of the whole slate.
        warm.prune(slate);
        // `poller.py:472-475`, minus the merged-slate total: `Slate` counts the
        // rotation, and the difference between the two numbers was only ever
        // "how many games the live-first rule filtered out", which the rotation
        // length says more directly.
        crate::debug!(
            "poll: lists refreshed, sources {}, rotation {}",
            sources,
            slate.len()
        );
        Ok(())
    }

    /// Re-fetch the game on screen and commit it.
    ///
    /// Every tick, **including static pre- and post-game screens**: that
    /// standing re-poll is what lets a pregame card notice its own pre→live
    /// flip mid-view rather than waiting for the next rotation. It does not
    /// flicker, because the store restamps the animation clock only when the
    /// displayed `(mode, game id)` changes.
    #[cfg(not(feature = "direct"))]
    async fn poll_current(&mut self, cadence: &Cadence) -> Result<(), PollError> {
        let Some(entry) = self.slate.current() else {
            return Ok(());
        };
        // Copied out so the slate is free for the rest of the tick; the commit
        // needs the league's display name, which is 32 bytes, and the warmer's
        // index is keyed by the other two.
        let source = entry.source;
        let league = entry.league.clone();
        let mut game_id = Text::<GAME_ID>::new();
        set_plain(&mut game_id, entry.id);
        let detail_url = url(
            &cadence.base_url,
            format_args!("/{}/games/{}", league.key.as_str(), game_id.as_str()),
        )?;

        let Poller {
            client,
            store,
            publisher,
            crests,
            warm,
            buffer,
            ..
        } = self;
        // A decoded game and a crest are live at the same time, so the one
        // receive buffer is split for this phase. `split_at_mut` is what proves
        // the crest fetch cannot overwrite the game being committed — the rule
        // `api_client.py` stated in a docstring ("the parse must not await").
        let (detail_buffer, logo_buffer) = buffer.split_at_mut(poll::DETAIL_BYTES);

        let Some(payload) = client.game_detail(&detail_url, detail_buffer).await? else {
            // A 404 means the game left today's scoreboard between the list
            // refresh and this fetch. Skip the slot; the next rotation picks up
            // a fresh list.
            defmt::debug!("poll: {=str} is gone (404)", detail_url.as_str());
            return Ok(());
        };
        let detail = WireFeed
            .detail(league.sport, payload)
            .map_err(PollError::Decode)?;

        // Abbreviations are present on every state of every sport, which is
        // what lets crests be fetched without knowing which kind of game this
        // is.
        let (away, home) = detail.abbreviations();
        // The warmer's index, filled for free. This tick has just decoded who
        // is playing, and the only other way for the warmer to learn it is a
        // request of its own — so a game the rotation has shown is a game the
        // warmer never has to probe.
        warm.learned(source, game_id.as_str(), away, home);
        let base = cadence.base_url.as_str();
        let key = league.key.as_str();
        let logos = Logos {
            away: crests.get(base, key, away, client, &mut *logo_buffer).await,
            home: crests.get(base, key, home, client, &mut *logo_buffer).await,
        };

        // The config→snapshot seam: UI colours are model state, so whoever owns
        // the snapshot applies them, before its next commit.
        if let Some(colors) = settings::take_ui_colors() {
            store.set_ui_colors(colors);
        }
        store.commit_detail(
            &league,
            &detail,
            logos,
            Instant::now().as_millis(),
            timesync::local_clock(),
        );
        publisher.publish(store.snapshot());
        // After the publish: these two slots are now the ones core 1 draws
        // from, and must not be evicted until a later commit replaces them.
        crests.hold(logos);
        Ok(())
    }

    /// Spend the front of the idle window filling crest slots the rotation has
    /// not needed yet.
    ///
    /// Two decisions carry this, and both are about not being able to do harm:
    ///
    /// * **It never evicts** ([`CrestDirectory::prefetch`]). A warmed crest is
    ///   a guess, and a guess that displaced a crest the rotation was about to
    ///   draw would make the board slower in exactly the case the pool
    ///   expansion exists to fix. So a full pool ends the warming, for good —
    ///   which is also the right answer for a slate with more teams than the
    ///   pool holds.
    /// * **It gives up on what it cannot have.** A game whose detail or crest
    ///   keeps failing is retried on later windows and then left alone
    ///   ([`scoreboard_model::prefetch`]), so one dead team never becomes the
    ///   thing the warmer proposes forever while the rest of the slate stays
    ///   cold.
    ///
    /// What it is *not* is a second poller. It commits nothing, publishes
    /// nothing and touches neither the store nor the failure tracker; the only
    /// state it changes is the crest pool and its own index.
    ///
    /// `deadline` is the moment the next tick is due. Stopping there is what
    /// keeps the poll cadence exactly what it would be on a board that does no
    /// warming at all.
    #[cfg(not(feature = "direct"))]
    async fn warm_crests(&mut self, cadence: &Cadence, deadline: Instant) {
        for _ in 0..WARM_FETCHES {
            // Both checks between fetches, not only before the first. The
            // command check is the latency bound — a press that landed while a
            // logo was in flight is answered when that one lands, not when the
            // batch ends — and the deadline is what makes "the warmer spends
            // the front of the window" true rather than approximately true.
            // Nothing stops `poll_interval_seconds` being configured at 1 s,
            // and six fetches would not fit in that.
            if !COMMANDS.is_empty() || Instant::now() >= deadline {
                return;
            }
            let Poller {
                client,
                slate,
                crests,
                warm,
                buffer,
                ..
            } = self;
            let Some(step) = warm.next(slate, |league, abbreviation| {
                crests.holds(league, abbreviation)
            }) else {
                return;
            };

            let (Step::Probe { position } | Step::Crest { position, .. }) = step;
            let Some(entry) = slate.at(position) else {
                return;
            };
            let source = entry.source;
            let sport = entry.league.sport;
            let league_key = entry.league.key.clone();
            let mut game_id = Text::<GAME_ID>::new();
            set_plain(&mut game_id, entry.id);

            match step {
                Step::Probe { .. } => {
                    let teams = Self::probe(
                        client,
                        buffer,
                        cadence.base_url.as_str(),
                        league_key.as_str(),
                        sport,
                        game_id.as_str(),
                    )
                    .await;
                    match teams {
                        Some((away, home)) => {
                            warm.learned(source, game_id.as_str(), away.as_str(), home.as_str())
                        }
                        None => warm.missed(source, game_id.as_str()),
                    }
                }
                Step::Crest { abbreviation, .. } => {
                    let outcome = crests
                        .prefetch(
                            cadence.base_url.as_str(),
                            league_key.as_str(),
                            abbreviation.as_str(),
                            client,
                            &mut buffer[..],
                        )
                        .await;
                    match outcome {
                        Warm::Cached => {}
                        // Nothing later in this window, or any window, can find
                        // room either.
                        Warm::Full => return,
                        Warm::Failed => warm.missed(source, game_id.as_str()),
                    }
                }
            }
        }
    }

    /// Fetch a game's detail for the two abbreviations and throw the rest away.
    ///
    /// The one request the warmer would rather not make, and the reason
    /// [`WarmIndex`] exists: the games list carries ids and states only, so
    /// there is no other way to learn who is playing in a game the rotation has
    /// not reached. The answer is remembered, so this is paid once per game and
    /// not at all for the games the board has already shown.
    ///
    /// Every failure here is a debug line, not an error: nothing is waiting on
    /// this, a 404 is an ordinary game that left today's scoreboard, and a
    /// transport failure has already been logged by whatever the *tick* did
    /// next.
    #[cfg(not(feature = "direct"))]
    async fn probe(
        client: &mut ApiClient,
        buffer: &mut ResponseBuffer,
        base: &str,
        league_key: &str,
        sport: Sport,
        game_id: &str,
    ) -> Option<(Text<ABBR>, Text<ABBR>)> {
        let url = url(base, format_args!("/{league_key}/games/{game_id}")).ok()?;
        let payload = match client.game_detail(&url, &mut buffer[..]).await {
            Ok(Some(payload)) => payload,
            Ok(None) => {
                crate::debug!("warm: {} is gone (404)", url.as_str());
                return None;
            }
            Err(error) => {
                crate::debug!("warm: {} failed, {}", url.as_str(), describe(&error));
                return None;
            }
        };
        let detail = match WireFeed.detail(sport, payload) {
            Ok(detail) => detail,
            Err(error) => {
                crate::debug!(
                    "warm: {} did not decode: {}",
                    url.as_str(),
                    describe(&PollError::Decode(error))
                );
                return None;
            }
        };
        let (away, home) = detail.abbreviations();
        let mut teams = (Text::new(), Text::new());
        set_plain(&mut teams.0, away);
        set_plain(&mut teams.1, home);
        Some(teams)
    }

    /// The interruptible sleep between ticks.
    ///
    /// `asyncio.wait_for(self._wake.wait(), poll_interval_seconds)`, plus one
    /// thing MicroPython did not need. Two rules:
    ///
    /// * **Only some commands wake the loop.** `skip()` and `skip_league()` set
    ///   `self._wake` because the whole point is to advance *now*;
    ///   `toggle_lock()` deliberately did not, because a lock changes nothing
    ///   that a tick would show. A menu keystroke is the same — it repaints
    ///   through the store and needs no poll — so applying it and going back to
    ///   sleep is both cheaper and closer to the original.
    /// * **An open menu caps the sleep.** `menu.py` checked its 10 s inactivity
    ///   timeout from the 50 ms button loop, a task that ran regardless. Here
    ///   the controller lives with its state's owner, and this task sleeps for a
    ///   poll interval — 30 s by default, three times the timeout. Without the
    ///   cap a user who walked away would leave the menu on the panel until the
    ///   next poll.
    ///
    /// The deadline is passed in rather than derived here because the warmer
    /// runs between the tick and this, and spends part of the same window.
    async fn sleep_until(&mut self, deadline: Instant) {
        loop {
            // Drained before the select, not only inside it. On a short poll
            // interval the warmer can have consumed the whole window, and an
            // expired timer wins the race below every time — so without this a
            // press delivered during warming would wait out another whole tick.
            while let Ok(command) = COMMANDS.try_receive() {
                if self.apply(command) == Wake::Now {
                    return;
                }
            }
            let wake_at = match self.menu.deadline_ms() {
                Some(menu_ms) => deadline.min(Instant::from_millis(menu_ms)),
                None => deadline,
            };
            match select(Timer::at(wake_at), COMMANDS.receive()).await {
                Either::First(()) => {
                    let now = Instant::now();
                    if self.menu_timeout(now.as_millis()) || now >= deadline {
                        return;
                    }
                }
                Either::Second(command) => {
                    if self.apply(command) == Wake::Now {
                        return;
                    }
                    if Instant::now() >= deadline {
                        return;
                    }
                }
            }
        }
    }

    /// Apply the menu's inactivity timeout. Returns whether the loop should
    /// tick now.
    fn menu_timeout(&mut self, now_ms: Millis) -> bool {
        let action = self.menu.check_timeout(&mut self.slate, self.store, now_ms);
        if action == Action::FilterApplied {
            crate::debug!("menu: input timeout, filter applied");
            self.publish();
            return true;
        }
        false
    }

    fn apply(&mut self, command: Command) -> Wake {
        match command {
            Command::ColorsChanged => {
                if let Some(colors) = settings::take_ui_colors() {
                    self.store.set_ui_colors(colors);
                    self.publish();
                    crate::debug!("poll: ui colours applied");
                }
                Wake::Later
            }
            Command::Press(button, press) => self.route(button, press),
        }
    }

    /// Hand a press to the menu controller and act on what it hands back.
    ///
    /// The controller is the single dispatch point for both buttons whether or
    /// not the menu is open — `menu.py`'s arrangement, and the reason the
    /// open/closed question is asked in exactly one place.
    fn route(&mut self, button: MenuButton, press: Press) -> Wake {
        let now = Instant::now().as_millis();
        match self.menu.press(button, press, &mut self.slate, self.store, now) {
            // `Handled` means the controller staged a menu change into the
            // store — open, cursor, toggle, a timeout restamp, or a close that
            // left the filter unchanged. None of it reaches the panel until
            // the store crosses the channel, so publish unconditionally: the
            // one arm that didn't left the menu running invisibly while every
            // button drove it (found on the first hardware unit with buttons).
            Action::Handled => {
                self.publish();
                Wake::Later
            }
            Action::Skip => self.skip(SkipKind::Game),
            Action::SkipLeague => self.skip(SkipKind::League),
            Action::ToggleLock => {
                let locked = self.slate.toggle_lock();
                // Non-sticky, deliberately: a lock toast fired mid-skip has to
                // survive the skip tick's `clear_toast_if_sticky` teardown.
                self.store.set_toast(
                    "",
                    if locked {
                        scoreboard_model::ToastKind::Lock
                    } else {
                        scoreboard_model::ToastKind::Unlock
                    },
                    false,
                    now,
                );
                self.publish();
                crate::debug!("poll: rotation lock {}", locked);
                // `toggle_lock` did not set `self._wake`: the current game keeps
                // polling either way and there is nothing for a tick to do.
                Wake::Later
            }
            Action::FilterApplied => {
                crate::debug!("menu: filter applied, {} in rotation", self.slate.len());
                self.publish();
                // `poller.py:353` woke the loop here so the board moves off a
                // filtered-out game within a tick rather than a poll interval.
                Wake::Now
            }
        }
    }

    /// A press that asks the rotation to advance.
    ///
    /// The accept/reject decision lives here rather than in the input task
    /// because it is about the poller's state, and `poller.py`'s `skip()` made
    /// it in the same place; the machine underneath is host-tested in
    /// `scoreboard-model`. A rejected press does **not** wake the loop — there
    /// is no work to do, and waking would turn a burst of presses into a burst
    /// of polls.
    fn skip(&mut self, kind: SkipKind) -> Wake {
        match self.press(kind) {
            SkipVerdict::Armed => Wake::Now,
            SkipVerdict::Rejected => Wake::Later,
        }
    }

    fn press(&mut self, kind: SkipKind) -> SkipVerdict {
        let now = Instant::now().as_millis();
        let verdict = self.skips.request(kind);
        match verdict {
            // The sticky spinner is owned by the tick that consumes this, and
            // torn down on every path out of it.
            SkipVerdict::Armed => {
                self.store
                    .set_toast("", scoreboard_model::ToastKind::Spinner, true, now)
            }
            // Rejected, not queued: dim the visible toast one cycle so the
            // press is acknowledged without advancing the rotation twice.
            SkipVerdict::Rejected => self.store.pulse_toast(now),
        }
        self.publish();
        verdict
    }

    fn publish(&mut self) {
        self.publisher.publish(self.store.snapshot());
    }
}

