//! The poll loop's direct-feed arms: the same tick, fed from ESPN.
//!
//! Wire mode's fetch phases ask the backend for pre-shaped payloads;
//! these ask ESPN for the raw scoreboard and stream it through
//! `scoreboard-direct`'s extractors instead. Everything that is not a fetch —
//! the rotation, the skip machine, the commit, the crest pool's cross-core
//! rules — is `poller.rs`'s and identical in both builds; the functions here
//! carry the same names and signatures as their wire twins so the loop cannot
//! tell which build it is.
//!
//! # What replaces what
//!
//! - A list refresh fetches `{base}/{league key}/scoreboard` per source and
//!   streams it through [`ListStream`]; there is no conditional request,
//!   because ESPN's edge does not serve the backend's ETags. The rows carry
//!   team identity (S3-DESIGN decision 8), which feeds the warm index and the
//!   [`PathIndex`] as a side effect of a pass the loop already pays for.
//! - A detail poll fetches the *same scoreboard body* and streams it through
//!   [`DetailStream`] targeted at the current game — ESPN has no per-game
//!   scoreboard endpoint, and the summary endpoint is a different (richer,
//!   heavier) document that only soccer commentary needs. List and detail are
//!   two fetches rather than one shared body for now: sequential is the
//!   simpler shape decision 3 already prices for soccer, a refresh tick costs
//!   one extra ~1 s fetch at the 16 KB window, and folding the two into one
//!   body is a measured optimization for after the field trial, not before.
//! - The warmer's probe is gone (decision 8). A game whose list row carried
//!   no identity is [`WarmIndex::missed`] on the spot: re-fetching a 300 KB
//!   body to re-read a row that was already empty would spend seconds to
//!   learn nothing, and the game still gets its crests the ordinary way when
//!   the rotation shows it.
//!
//! # Errors wear the backend's status codes
//!
//! The panel's vocabulary is [`PollError`], and proxy mode taught it that a
//! glitched upstream is an HTTP 502 from the backend. Direct mode keeps the
//! contract: an extraction failure or a glitched verdict surfaces as
//! `PollError::http(502, ...)` with a short code naming the tier, so the
//! error screen reads the same on both builds and the ring log says which
//! layer gave up. The full detail is logged before the fold — the fold is
//! lossy on purpose, the log is not.

use embassy_net::Stack;
use embassy_time::Instant;
use scoreboard_direct::{
    CommentaryStream, DetailStream, Error as FeedError, Feed, ListStream, Outcome, Quirks,
    SummaryOutcome,
};
use scoreboard_espn::common::Quirk;
use scoreboard_espn::{ListRow, ListSink as RowSink};
// The model's slate sink — same name, different altitude: `RowSink` receives
// ESPN rows here, and this trait is how the slate's update handle receives
// the `(state, id)` half of each one.
use scoreboard_model::feed::ListSink as _;
use scoreboard_model::feed::LeagueId;
use scoreboard_model::poll::PollError;
use scoreboard_model::prefetch::{Step, WarmIndex};
use scoreboard_model::snapshot::{ABBR, GAME_ID};
use scoreboard_model::store::Logos;
use scoreboard_model::text::{Text, set_plain};
use static_cell::ConstStaticCell;

use super::{Cadence, Poller};
use crate::logos::WARM_GAMES;
use crate::net::espn::{EspnClient, Fetched};
use crate::net::timesync;
use crate::settings;

/// ESPN's site API root, ending exactly where [`LeagueId::key`] begins —
/// decision 2's "no new registry": the key *is* the path segment.
///
/// Overridable at build time so the replay rig can front the same image with
/// the TLS-terminated mock (decision 13, `SPIKE_TERMINATOR`'s pattern); a
/// shipped image is built without the override and there is no runtime switch
/// to leave on by accident.
const BASE: &str = match option_env!("SCOREBOARD_ESPN_BASE") {
    Some(base) => base,
    None => "https://site.api.espn.com/apis/site/v2/sports",
};

/// Longest URL this module builds: the base, a league key, and
/// `/summary?event=` plus a game id. Generous rather than tight — the
/// override base may carry an address and a port.
const URL_BYTES: usize = 192;

/// picojson token scratch for the streams, carved from the front of the PNG
/// decoder's loaned window (see [`DirectState::decode`]). 16 KiB is the
/// bench's proven size against every corpus body (S1 validation); the
/// backend uses 64 KiB only because it can. The list pass borrows the same
/// carve — its measured need is 210–464 B (`scoreboard-direct`'s
/// `list_scratch.rs` pins 2 KiB), and should the one-body optimization ever
/// run list and detail streams together, the 32 KB window carves both
/// disjointly with room to spare.
const EXTRACT_SCRATCH_BYTES: usize = 16 * 1024;

const _: () = assert!(
    EXTRACT_SCRATCH_BYTES <= png_stream::WINDOW_BYTES,
    "the token scratch is a loan from the PNG window and must fit in it"
);

/// The PNG decoder's window and tables, initialized IN PLACE.
///
/// Not `StaticCell<Scratch>` + `init(Scratch::new())`: a by-value `Scratch`
/// is ~60 KB and `init`'s argument materializes in the constructing frame —
/// on this build that was a stack spike into a 97 KB budget, and the first
/// direct image boot-looped on the resulting STKOF HardFault before its
/// first poll. `Scratch::init_at` writes the slot field by field instead;
/// its doc carries the argument.
static DECODE: ConstStaticCell<core::mem::MaybeUninit<png_stream::Scratch>> =
    ConstStaticCell::new(core::mem::MaybeUninit::uninit());

/// Everything the direct build adds to the poller, in one field so the
/// `Poller` struct grows one `#[cfg]` rather than five.
///
/// There is deliberately no separate JSON scratch here: the streams borrow
/// [`png_stream::Scratch::loan_window`] instead — 32 KB of bytes that carry
/// nothing between decodes, on a device where extraction and crest decoding
/// never overlap by the poller's own sequencing. The borrow checker enforces
/// the never-overlap: a stream holding the loan pins `decode` until it
/// finishes.
pub(super) struct DirectState {
    pub espn: EspnClient,
    pub paths: PathIndex,
    pub decode: &'static mut png_stream::Scratch,
}

impl DirectState {
    /// Panics on a second call, transitively: the decode scratch and the
    /// client's own statics are all take-once, and there is one poller.
    pub fn new(stack: Stack<'static>) -> DirectState {
        DirectState {
            espn: EspnClient::new(stack),
            paths: PathIndex::new(),
            decode: png_stream::Scratch::init_at(DECODE.take()),
        }
    }
}

/// Crest CDN paths by `(league key, abbreviation)`, filled by every list and
/// detail pass, read by the warmer.
///
/// The app's rather than the model's (S3-DESIGN decision 9): `WarmIndex`
/// stays exactly as wire builds know it, and this rides beside it. Bounded at
/// the crest pool's own size — remembering more paths than the pool can hold
/// sprites would let the index outrun the thing it exists to fill. Insert or
/// update, skip when full: entries are re-offered by every refresh, so a
/// stale entry is overwritten within one list pass and a skipped one gets its
/// chance when pruning-by-rotation frees a slot. Abbreviations compare
/// case-insensitively for the reason the pool's key is lowercased — ESPN's
/// casing is not stable across endpoints.
pub(super) struct PathIndex {
    entries: heapless::Vec<PathEntry, { crate::logos::SLOTS }>,
}

struct PathEntry {
    league: heapless::String<{ scoreboard_model::feed::LEAGUE_KEY }>,
    abbreviation: Text<ABBR>,
    path: scoreboard_direct::CrestPath,
}

impl PathIndex {
    fn new() -> PathIndex {
        PathIndex {
            entries: heapless::Vec::new(),
        }
    }

    fn remember(&mut self, league_key: &str, abbreviation: &str, path: &str) {
        if abbreviation.is_empty() {
            return;
        }
        if let Some(entry) = self.find_mut(league_key, abbreviation) {
            if entry.path.as_str() != path {
                entry.path.clear();
                let _ = entry.path.push_str(path);
            }
            return;
        }
        let mut entry = PathEntry {
            league: heapless::String::new(),
            abbreviation: Text::new(),
            path: scoreboard_direct::CrestPath::new(),
        };
        if entry.league.push_str(league_key).is_err() || entry.path.push_str(path).is_err() {
            return;
        }
        set_plain(&mut entry.abbreviation, abbreviation);
        let _ = self.entries.push(entry);
    }

    pub fn path(&self, league_key: &str, abbreviation: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| {
                entry.league.as_str() == league_key
                    && entry
                        .abbreviation
                        .as_str()
                        .eq_ignore_ascii_case(abbreviation)
            })
            .map(|entry| entry.path.as_str())
    }

    fn find_mut(&mut self, league_key: &str, abbreviation: &str) -> Option<&mut PathEntry> {
        self.entries.iter_mut().find(|entry| {
            entry.league.as_str() == league_key
                && entry
                    .abbreviation
                    .as_str()
                    .eq_ignore_ascii_case(abbreviation)
        })
    }
}

/// Counts quirks instead of formatting them: `Quirk` is a host-crate enum
/// with no `defmt::Format`, and one ring line per anomaly on a 99-event
/// Saturday would spend the whole ring saying the same thing. The count goes
/// out with the per-source log line; a device that shows a rising number is
/// the cue to re-run the host sweep, which prints them properly.
struct CountQuirks(u32);

impl Quirks for CountQuirks {
    fn quirk(&mut self, _quirk: Quirk) {
        self.0 = self.0.saturating_add(1);
    }
}

/// `{BASE}/{league key}/scoreboard`.
fn scoreboard_url(league_key: &str) -> Result<heapless::String<URL_BYTES>, PollError> {
    build_url(format_args!("{BASE}/{league_key}/scoreboard"))
}

/// `{BASE}/{league key}/summary?event={game id}` — soccer commentary's second
/// body, and nothing else's.
fn summary_url(
    league_key: &str,
    game_id: &str,
) -> Result<heapless::String<URL_BYTES>, PollError> {
    build_url(format_args!(
        "{BASE}/{league_key}/summary?event={game_id}"
    ))
}

fn build_url(args: core::fmt::Arguments<'_>) -> Result<heapless::String<URL_BYTES>, PollError> {
    let mut url = heapless::String::new();
    core::fmt::Write::write_fmt(&mut url, args)
        .map_err(|_| PollError::Transport(scoreboard_model::poll::Transport::BadUrl))?;
    Ok(url)
}

/// The lossy fold from extraction failures onto the panel's vocabulary — the
/// backend's own status for the same conditions, per the module docs. The
/// lossless half is the caller's log line, written before calling this.
fn feed_error(error: &FeedError) -> PollError {
    PollError::http(
        502,
        match error {
            FeedError::Stream(_) => "espn_stream",
            FeedError::MalformedBody => "espn_malformed",
            FeedError::Transform(_) => "espn_transform",
        },
    )
}

/// One list pass's sink: rows into the slate, identity into the warm index
/// and the path index — the three consumers of a pass the poller already
/// pays for, fed in one traversal.
struct SlateRows<'a> {
    update: scoreboard_model::slate::SourceUpdate<'a>,
    warm: &'a mut WarmIndex<WARM_GAMES>,
    paths: &'a mut PathIndex,
    source: u8,
    league_key: &'a str,
    /// The slate said stop. The extractor cannot be stopped mid-body — and
    /// should not be, the identity side effects are still wanted — so rows
    /// past capacity feed everything except the slate.
    slate_full: bool,
}

impl RowSink for SlateRows<'_> {
    fn row(&mut self, row: ListRow<'_>) {
        if !self.slate_full {
            self.slate_full = !self.update.entry(row.state, row.id);
        }
        if let (Some(away), Some(home)) = (row.away.abbreviation, row.home.abbreviation) {
            self.warm.learned(self.source, row.id, away, home);
            if let Some(path) = row.away.crest {
                self.paths.remember(self.league_key, away, path);
            }
            if let Some(path) = row.home.crest {
                self.paths.remember(self.league_key, home, path);
            }
        }
    }
}

impl Poller {
    /// Refresh every source's list and rebuild the rotation — the wire twin's
    /// contract, kept clause for clause: a single source failing keeps its
    /// cached slate, and the tick only counts as failed when every source
    /// failed.
    ///
    /// `initial` is the wire twin's conditional-request flag. There is no
    /// conditional request here — ESPN serves no ETag worth storing — so it
    /// is accepted and unused to keep the call site build-agnostic.
    pub(super) async fn refresh_lists(
        &mut self,
        _cadence: &Cadence,
        _initial: bool,
    ) -> Result<(), PollError> {
        let Poller {
            slate,
            warm,
            direct,
            ..
        } = self;

        let mut failures = 0usize;
        let mut last_error = None;
        let sources = slate.sources().len();
        for index in 0..sources {
            let league = slate.sources()[index].clone();
            let key = league.key.as_str();
            let outcome = fetch_list(direct, slate, warm, &league, index as u8).await;
            match outcome {
                Ok(quirk_count) => {
                    if quirk_count > 0 {
                        crate::debug!("poll: {} list ok, {} quirks", key, quirk_count);
                    }
                }
                Err(error) => {
                    failures += 1;
                    crate::error!(
                        "poll: {} list refresh failed, keeping cached slate: {}",
                        key,
                        super::describe(&error)
                    );
                    last_error = Some(error);
                }
            }
        }

        if sources > 0 && failures == sources {
            return Err(last_error.expect("a failure recorded an error"));
        }
        self.slate.rebuild();
        // Same pruning rule and the same reasoning as the wire twin: games
        // that left the day take their warm records with them.
        self.warm.prune(&self.slate);
        crate::debug!(
            "poll: lists refreshed, sources {}, rotation {}",
            sources,
            self.slate.len()
        );
        Ok(())
    }

    /// Re-fetch the game on screen and commit it — the wire twin's contract
    /// over the direct fetch path. Every tick, including static screens, for
    /// the same pre→live reason.
    pub(super) async fn poll_current(&mut self, _cadence: &Cadence) -> Result<(), PollError> {
        let Some(entry) = self.slate.current() else {
            return Ok(());
        };
        let source = entry.source;
        let league = entry.league.clone();
        let mut game_id = Text::<GAME_ID>::new();
        set_plain(&mut game_id, entry.id);

        let Poller {
            store,
            publisher,
            crests,
            warm,
            direct,
            ..
        } = self;

        let report = fetch_detail(direct, &league, game_id.as_str()).await?;
        let mut extract = match report {
            Outcome::Found(extract) => extract,
            Outcome::NotFound => {
                // The wire twin's 404 arm: the game left today's scoreboard
                // between the list refresh and this fetch. Skip the slot; the
                // next rotation picks up a fresh list.
                crate::debug!("poll: {} is gone (not on the board)", game_id.as_str());
                return Ok(());
            }
            Outcome::Glitched => {
                // The backend's 502 for the same evidence: the target is
                // absent AND events failed to parse, so "gone" cannot be
                // concluded. Retry, never "game ended".
                return Err(PollError::http(502, "upstream_glitched"));
            }
        };

        if extract.wants_commentary() {
            let commentary = fetch_commentary(direct, &league, game_id.as_str()).await;
            extract.set_commentary(commentary);
        }

        // Copied out before the crest awaits: `crests()` borrows the extract,
        // and the commit below wants the extract back. Paths refresh the
        // index too — the detail body is the freshest evidence there is.
        let (away_path, home_path) = {
            let crest_paths = extract.crests();
            (crest_paths.away.clone(), crest_paths.home.clone())
        };
        let (mut away, mut home) = (Text::<ABBR>::new(), Text::<ABBR>::new());
        {
            let detail = extract.detail();
            let (away_str, home_str) = detail.abbreviations();
            set_plain(&mut away, away_str);
            set_plain(&mut home, home_str);
        }
        warm.learned(source, game_id.as_str(), away.as_str(), home.as_str());
        if let Some(path) = away_path.as_deref() {
            direct.paths.remember(league.key.as_str(), away.as_str(), path);
        }
        if let Some(path) = home_path.as_deref() {
            direct.paths.remember(league.key.as_str(), home.as_str(), path);
        }

        let key = league.key.as_str();
        let logos = Logos {
            away: crests
                .get_direct(
                    key,
                    away.as_str(),
                    away_path.as_deref(),
                    &mut direct.espn,
                    direct.decode,
                )
                .await,
            home: crests
                .get_direct(
                    key,
                    home.as_str(),
                    home_path.as_deref(),
                    &mut direct.espn,
                    direct.decode,
                )
                .await,
        };

        // From here down this is the wire twin, line for line: the
        // config→snapshot seam, the commit, the publish, and the hold.
        if let Some(colors) = settings::take_ui_colors() {
            store.set_ui_colors(colors);
        }
        store.commit_detail(
            &league,
            &extract.detail(),
            logos,
            Instant::now().as_millis(),
            timesync::local_clock(),
        );
        publisher.publish(store.snapshot());
        crests.hold(logos);
        Ok(())
    }

    /// The warmer, with the probe arm folded away (S3-DESIGN decision 8): a
    /// game the list pass could not identify is marked missed on the spot,
    /// because the only fetch that could answer is the 300 KB one the
    /// decision exists to delete. Crest steps read the [`PathIndex`]; a
    /// missing path is the same verdict. Loop skeleton, deadline discipline
    /// and never-evict rule are the wire twin's, unchanged.
    pub(super) async fn warm_crests(&mut self, _cadence: &Cadence, deadline: Instant) {
        for _ in 0..super::WARM_FETCHES {
            if !super::COMMANDS.is_empty() || Instant::now() >= deadline {
                return;
            }
            let Poller {
                slate,
                crests,
                warm,
                direct,
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
            let league_key = entry.league.key.clone();
            let mut game_id = Text::<GAME_ID>::new();
            set_plain(&mut game_id, entry.id);

            match step {
                Step::Probe { .. } => {
                    crate::debug!(
                        "warm: {} has no list identity, giving it up",
                        game_id.as_str()
                    );
                    warm.missed(source, game_id.as_str());
                }
                Step::Crest { abbreviation, .. } => {
                    let path = direct.paths.path(league_key.as_str(), abbreviation.as_str());
                    let outcome = crests
                        .prefetch_direct(
                            league_key.as_str(),
                            abbreviation.as_str(),
                            path,
                            &mut direct.espn,
                            direct.decode,
                        )
                        .await;
                    match outcome {
                        crate::logos::Warm::Cached => {}
                        crate::logos::Warm::Full => return,
                        crate::logos::Warm::Failed => warm.missed(source, game_id.as_str()),
                    }
                }
            }
        }
    }
}

/// One source's list fetch and stream, split from the loop so the borrow of
/// the slate's update handle ends before the next iteration reads sources.
/// Returns the quirk count — the rows were delivered through the sink's side
/// effects, which is the whole reason the sink exists.
async fn fetch_list(
    direct: &mut DirectState,
    slate: &mut scoreboard_model::Slate,
    warm: &mut WarmIndex<WARM_GAMES>,
    league: &LeagueId,
    source: u8,
) -> Result<u32, PollError> {
    let url = scoreboard_url(league.key.as_str())?;
    let feed = Feed::from_league(league);
    let DirectState {
        espn,
        paths,
        decode,
    } = direct;
    let scratch = &mut decode.loan_window()[..EXTRACT_SCRATCH_BYTES];
    let mut quirks = CountQuirks(0);
    let sink = SlateRows {
        update: slate.update_source(source),
        warm,
        paths,
        source,
        league_key: league.key.as_str(),
        slate_full: false,
    };
    let mut stream =
        ListStream::new(feed, sink, &mut quirks, scratch).map_err(|error| feed_error(&error))?;

    let mut stream_error = None;
    let fetched = espn
        .fetch(url.as_str(), &mut |chunk| match stream.write(chunk) {
            Ok(()) => true,
            Err(error) => {
                stream_error = Some(error);
                false
            }
        })
        .await;

    match (fetched, stream_error) {
        (_, Some(error)) => {
            crate::error!("poll: {} list did not extract", league.key.as_str());
            Err(feed_error(&error))
        }
        (Err(error), None) => Err(error),
        (Ok(Fetched::NotFound), None) => {
            // A scoreboard *endpoint* 404 is a server anomaly, not a game
            // event — there is no target here to be gone. It fails the source
            // like any other bad answer.
            Err(PollError::http(404, "scoreboard_missing"))
        }
        (Ok(Fetched::Complete), None) => {
            let report = stream.finish().map_err(|error| {
                crate::error!("poll: {} list body malformed", league.key.as_str());
                feed_error(&error)
            })?;
            if report.counts.failed > 0 {
                crate::debug!(
                    "poll: {} list ok {} failed {}",
                    league.key.as_str(),
                    report.counts.ok,
                    report.counts.failed
                );
            }
            Ok(quirks.0)
        }
    }
}

/// The current game's detail: one scoreboard body streamed at the target id.
async fn fetch_detail(
    direct: &mut DirectState,
    league: &LeagueId,
    game_id: &str,
) -> Result<Outcome, PollError> {
    let url = scoreboard_url(league.key.as_str())?;
    let feed = Feed::from_league(league);
    let DirectState { espn, decode, .. } = direct;
    let scratch = &mut decode.loan_window()[..EXTRACT_SCRATCH_BYTES];
    let mut quirks = CountQuirks(0);
    let mut stream =
        DetailStream::new(feed, game_id, &mut quirks, scratch).map_err(|error| feed_error(&error))?;

    let mut stream_error = None;
    let fetched = espn
        .fetch(url.as_str(), &mut |chunk| match stream.write(chunk) {
            Ok(()) => true,
            Err(error) => {
                stream_error = Some(error);
                false
            }
        })
        .await;

    match (fetched, stream_error) {
        (_, Some(error)) => {
            crate::error!("poll: {} detail did not extract", game_id);
            Err(feed_error(&error))
        }
        (Err(error), None) => Err(error),
        (Ok(Fetched::NotFound), None) => Err(PollError::http(404, "scoreboard_missing")),
        (Ok(Fetched::Complete), None) => {
            let report = stream.finish().map_err(|error| {
                crate::error!("poll: {} detail body malformed", game_id);
                feed_error(&error)
            })?;
            if quirks.0 > 0 {
                crate::debug!("poll: {} detail, {} quirks", game_id, quirks.0);
            }
            Ok(report.outcome)
        }
    }
}

/// The soccer summary pass, best-effort by contract: every failure — fetch,
/// stream, malformed body — degrades to `None` with a debug line, because
/// commentary is polish and the game is not (the seam crate's rule, and the
/// backend's before it).
async fn fetch_commentary(
    direct: &mut DirectState,
    league: &LeagueId,
    game_id: &str,
) -> Option<scoreboard_direct::CommentaryExtract> {
    let url = match summary_url(league.key.as_str(), game_id) {
        Ok(url) => url,
        Err(_) => return None,
    };
    let DirectState { espn, decode, .. } = direct;
    let scratch = &mut decode.loan_window()[..EXTRACT_SCRATCH_BYTES];
    let mut stream = match CommentaryStream::new(scratch) {
        Ok(stream) => stream,
        Err(_) => return None,
    };

    let mut failed = false;
    let fetched = espn
        .fetch(url.as_str(), &mut |chunk| match stream.write(chunk) {
            Ok(()) => true,
            Err(_) => {
                failed = true;
                false
            }
        })
        .await;

    let outcome = match (fetched, failed) {
        (Ok(Fetched::Complete), false) => stream.finish().ok(),
        _ => None,
    };
    match outcome {
        Some(SummaryOutcome {
            commentary: Some(commentary),
            ..
        }) => Some(commentary),
        _ => {
            crate::debug!("poll: {} summary gave no commentary", game_id);
            None
        }
    }
}
