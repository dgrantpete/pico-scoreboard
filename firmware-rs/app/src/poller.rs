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

use embassy_futures::select::{Either, select};
use embassy_net::Stack;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Instant, Timer};
use scoreboard_config::DeviceConfig;
use scoreboard_model::feed::LeagueId;
use scoreboard_model::poll::{
    self, FailureTracker, PollError, SkipKind, SkipMachine, SkipVerdict,
};
use scoreboard_model::slate::MAX_SOURCES;
use scoreboard_model::snapshot::Millis;
use scoreboard_model::store::Logos;
use scoreboard_model::{GameFeed, Mode, Publisher, Slate, Sport, Store, WireFeed};

use crate::logos::CrestDirectory;
use crate::net::api_client::{ApiClient, Etag, ResponseBuffer, base_url, url};
use crate::net::timesync;
use crate::settings;

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
}

/// Task #12's presses land here, and so does `PUT /api/config`.
///
/// Four deep: a command is consumed between ticks, and no producer sends more
/// than one per user action. A full channel means the poller is inside a
/// request, and `try_send`'s failure is the right answer — a press that could
/// not be delivered is a press that would have been rejected anyway.
static COMMANDS: Channel<CriticalSectionRawMutex, Command, 4> = Channel::new();

/// Send a command, dropping it if the queue is full. Never blocks, so an HTTP
/// handler can call it.
pub fn command(command: Command) {
    if COMMANDS.try_send(command).is_err() {
        defmt::warn!("poller: command queue full, dropped one");
    }
}

// ---------------------------------------------------------------------------
// Liveness, for task #12
// ---------------------------------------------------------------------------

use core::sync::atomic::{AtomicU32, Ordering};

/// Consecutive failed ticks. Zero means the last tick succeeded.
static FAILURE_STREAK: AtomicU32 = AtomicU32::new(0);
/// Uptime seconds at the last successful tick; [`NEVER`] until the first one.
static LAST_SUCCESS_S: AtomicU32 = AtomicU32::new(NEVER);

/// `LAST_SUCCESS_S` before any poll has succeeded. Not zero: zero is a real
/// uptime, and "succeeded at boot" is the opposite of what this means.
pub const NEVER: u32 = u32::MAX;

/// What the poller knows about the network, for the health gate task #12's
/// watchdog feeder needs.
///
/// BACKLOG 69: the bench unit fell off the Wi-Fi overnight and kept rendering,
/// with no link-down event to notice — embassy-net's IPv4 configuration stayed
/// up because the association never formally dropped. The poller is the only
/// thing in the firmware that finds out, because it is the only thing that
/// talks to anything.
///
/// **The gate #12 should use** is `since_success_s > 3 × poll_interval` OR
/// `streak >= MAX_FAILURES`, not `streak > 0`: a single failed poll is a
/// backend restart, and a scoreboard that reboots itself over one is worse than
/// one showing a stale score. Both halves are needed — the streak alone cannot
/// distinguish a poller that is failing from one that has stopped ticking at
/// all, and a task that has stopped is precisely what a watchdog is for.
#[derive(Debug, Clone, Copy)]
pub struct Health {
    pub streak: u32,
    /// Seconds since the last successful poll, or `None` if there has never
    /// been one — a device that has not reached the backend since boot.
    pub since_success_s: Option<u32>,
}

pub fn health() -> Health {
    let last = LAST_SUCCESS_S.load(Ordering::Relaxed);
    Health {
        streak: FAILURE_STREAK.load(Ordering::Relaxed),
        since_success_s: (last != NEVER)
            .then(|| (Instant::now().as_secs() as u32).saturating_sub(last)),
    }
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
    failures: FailureTracker,
    skips: SkipMachine,
    /// Per-source, parallel to [`Slate::sources`]. `None` until a list refresh
    /// has returned one.
    etags: [Option<Etag>; MAX_SOURCES],
    /// When the rotation last advanced. `None` until the first list refresh,
    /// which is what `poller.py` used to mean "this is the first tick".
    last_rotation_ms: Option<Millis>,
    /// When to fetch the backend clock again. Starts in the past, so the first
    /// tick syncs before it commits anything — `main.py` ran the sync in the
    /// boot sequence, ahead of the poller, for the same reason.
    next_time_sync: Instant,
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
        failures: FailureTracker::new(),
        skips: SkipMachine::new(),
        etags: [const { None }; MAX_SOURCES],
        last_rotation_ms: None,
        next_time_sync: Instant::now(),
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
        let now = Instant::now().as_millis();
        match poller.tick(&cadence, now).await {
            Ok(()) => {
                if let Some(streak) = poller.failures.record_success() {
                    // ERROR level, as `poller.py:362-365` had it: a recovery is
                    // the line you go looking for after the fact, so it has to
                    // survive the same filter the failures did.
                    crate::error!("poll: recovered after {} failed polls", streak);
                }
                FAILURE_STREAK.store(0, Ordering::Relaxed);
                LAST_SUCCESS_S.store(Instant::now().as_secs() as u32, Ordering::Relaxed);
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

        // The interruptible sleep. `asyncio.wait_for(self._wake.wait(),
        // poll_interval_seconds)` — a command arriving mid-tick is already in
        // the channel and returns from this immediately.
        if let Either::Second(command) =
            select(Timer::after(cadence.poll_interval), COMMANDS.receive()).await
        {
            poller.apply(command);
        }
    }
}

impl Poller {
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
    async fn refresh_lists(&mut self, cadence: &Cadence, initial: bool) -> Result<(), PollError> {
        let Poller {
            client,
            slate,
            etags,
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
    async fn poll_current(&mut self, cadence: &Cadence) -> Result<(), PollError> {
        let Some((league, game_id)) = self.slate.current() else {
            return Ok(());
        };
        // Cloned so the slate is free for the rest of the tick; the commit
        // needs the league's display name, which is 32 bytes.
        let league = league.clone();
        let detail_url = url(
            &cadence.base_url,
            format_args!("/{}/games/{}", league.key.as_str(), game_id),
        )?;

        let Poller {
            client,
            store,
            publisher,
            crests,
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

    fn apply(&mut self, command: Command) {
        match command {
            Command::ColorsChanged => {
                if let Some(colors) = settings::take_ui_colors() {
                    self.store.set_ui_colors(colors);
                    self.publish();
                    crate::debug!("poll: ui colours applied");
                }
            }
        }
    }

    /// A press, once task #12 has one to deliver.
    ///
    /// The accept/reject decision lives here rather than in the input task
    /// because it is about the poller's state, and `poller.py`'s `skip()` made
    /// it in the same place. What #12 adds is the [`Command`] variant that
    /// calls this; the machine underneath is host-tested in `scoreboard-model`.
    #[expect(
        dead_code,
        reason = "task #12 owns the button loop; declaring the seam before its caller exists, as `ringlog::set_wall_clock` did for this task"
    )]
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

