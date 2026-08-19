#![no_std]
//! The direct feed seam (Phase S3): one ESPN scoreboard body streamed in, one
//! [`GameDetail`] view handed out.
//!
//! # What this crate is, and what it deliberately is not
//!
//! The display half of the firmware consumes
//! [`scoreboard_model::feed::GameDetail`] — *borrowed* `scoreboard-wire`
//! structs that `Store` copies out of. Wire mode produces those by decoding a
//! packed payload; direct mode produces them by stream-extracting a 300–450 KB
//! ESPN body into owned bounded storage and borrowing *that*. `Store`, the
//! snapshot channel, core 1 and every renderer are untouched by construction.
//!
//! The owned bounded storage already exists: `scoreboard-espn`'s per-sport
//! extract structs (`mlb::Extract`, `nba::Extract`, `football::GameExtract`,
//! `soccer::SoccerExtract`) are exactly that, and each already exposes an
//! `as_game()` borrowed wire-shaped view — the seam S1's DESIGN.md named and
//! S1's tests pin byte-for-byte against all 33 committed wire goldens. This
//! crate therefore does **not** re-declare those structs or re-express the
//! backend's adapter mappings over them; a second implementation of a
//! transform whose whole point is having exactly one would be the drift
//! `scoreboard-wire` and `scoreboard-espn` were both built to prevent.
//!
//! What is genuinely missing between `scoreboard-espn` and `Store`, and what
//! this crate supplies, is:
//!
//! - **Sport dispatch into `GameDetail`** — [`DirectExtract`], the owned
//!   sport-tagged union whose [`DirectExtract::detail`] is the seam. The
//!   poller holds one; the display stack cannot tell it from wire mode.
//! - **One streaming detail API over four divergent sport shapes** —
//!   [`DetailStream`]. The four S1 sport lanes landed with four different
//!   extractor surfaces (borrowed vs owned quirk sinks, three spellings of the
//!   ok/failed tally, four transform-error enums, a bare `Sink` the caller
//!   must wrap in a `StreamMatcher` themselves). The poller should not learn
//!   any of that; the friction is absorbed here, once.
//! - **One verdict vocabulary** — [`Outcome`] and [`Error`], folded to the
//!   backend's own 404-vs-502 rule so the device's "game ended" signal means
//!   the same thing it does in proxy mode.
//! - **The soccer commentary seam** — [`CommentaryStream`], the second body a
//!   live soccer poll needs (see below).
//! - **One crest accessor** — [`DirectExtract::crests`]. Three sport lanes
//!   expose `crests()` and NBA exposes a bare field; the poller sees one
//!   method. Crest paths ride the extract but stay out of the wire view, so
//!   they cannot affect parity.
//!
//! # Soccer commentary is two bodies, not one
//!
//! `scoreboard_wire::soccer::Live` carries `commentary`, but ESPN's scoreboard
//! body does not: commentary lives on the per-event *summary* endpoint. The
//! shape is therefore a two-pass merge, identical to the backend's
//! `soccer/handler.rs`: extract the game from the scoreboard body, and *if it
//! is live*, stream the summary through [`CommentaryStream`] and attach the
//! result with [`DirectExtract::set_commentary`]. It is best-effort by
//! construction — a failed or malformed summary degrades to no commentary
//! rather than failing the poll, because commentary is polish and the game is
//! not. The committed soccer goldens encode the no-commentary shape, which is
//! why the parity gate exercises exactly that path.
//!
//! # The games list is not here, and that is a finding
//!
//! A sport-uniform *list* stream cannot be built over today's
//! `scoreboard-espn` surface without either `unsafe` or a duplicated
//! transform, and the blocker is structural rather than a matter of effort:
//! `mlb::ListExtractor` borrows its sink (`&'c mut L where L: mlb::ListSink`),
//! so the sink must be a caller-owned *local type*, while `nba::Extractor`'s
//! list mode borrows a callback (`&'c mut dyn FnMut(&str, GameState)`), so its
//! sink must be a caller-owned *closure*. Rust's orphan rules forbid
//! implementing `mlb::ListSink` for a closure type, and `FnMut` is not
//! implementable on stable — one object cannot be both, and a stream that owns
//! either one and hands a `&mut` to an extractor it also owns is
//! self-referential. Wrapping the two in one API therefore means either two
//! caller-owned objects (half of them dead on every call) or re-implementing a
//! sport's transform. Neither earns its keep when the poller's alternative is
//! the `match sport` it already has to write.
//!
//! The fix belongs one crate up: the S1 checklist already carries an open
//! API-unification pass ("shared Counts/TransformError/Report shapes") driven
//! by the backend adapter's friction report. The concrete ask this lane adds
//! is that the list extractors converge on **owning** their sink and handing
//! it back at `finish` — the shape `football::ListExtractor` already has —
//! after which a uniform `ListStream` here is about thirty lines. Until then
//! the poller calls the four list extractors directly; the vocabulary they
//! report in ([`GameState`] and the ids) is already shared.
//!
//! # Bounds, allocation, and the target
//!
//! `no_std`, no `alloc`, no `unsafe`. Every byte of extract storage is
//! `scoreboard-espn`'s, bound at the wire's own 255-byte string cap (its
//! DESIGN.md ruling 2) — deliberately *looser* than
//! `scoreboard_model::text::Text`, whose tighter corpus-measured bounds apply
//! downstream at `Store` copy-in and are unchanged by this crate. The
//! picojson token scratch is caller-supplied: it must hold the longest
//! contiguous string or number token in the body (the backend uses 64 KiB, the
//! on-silicon bench 16 KiB).

mod commentary;
mod detail;
mod extract;

pub use commentary::CommentaryStream;
pub use detail::{DetailReport, DetailStream, Outcome};
pub use extract::DirectExtract;

pub use scoreboard_espn::common::{
    CDN_ORIGIN, CrestPath, CrestUrl, Crests, Quirk, Quirks, crest_url,
};
pub use scoreboard_espn::soccer::{CommentaryExtract, SummaryOutcome};
pub use scoreboard_model::feed::{GameDetail, LeagueId, Sport};
pub use scoreboard_wire::GameState;

use scoreboard_espn::path;

/// Which extraction to run over a scoreboard body.
///
/// A sport plus the one per-league input the extraction actually takes.
/// `college` rides the variant rather than a bare `bool` parameter because it
/// is meaningless for the other three sports: it gates the pregame rank line
/// and nothing else (`scoreboard-espn` DESIGN.md ruling 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feed {
    Mlb,
    Nba,
    Football { college: bool },
    Soccer,
}

impl Feed {
    /// The feed a polled league selects.
    ///
    /// `LeagueId.key` is ESPN's own path segment, so the college test is the
    /// backend's `FootballLeague::from_path` test on the same slug — one
    /// string compare, no second registry to drift (S3-DESIGN decision 2).
    pub fn from_league(league: &LeagueId) -> Self {
        match league.sport {
            Sport::Mlb => Feed::Mlb,
            Sport::Nba => Feed::Nba,
            Sport::Football => Feed::Football {
                college: league.key.as_str() == "football/college-football",
            },
            Sport::Soccer => Feed::Soccer,
        }
    }

    pub fn sport(self) -> Sport {
        match self {
            Feed::Mlb => Sport::Mlb,
            Feed::Nba => Sport::Nba,
            Feed::Football { .. } => Sport::Football,
            Feed::Soccer => Sport::Soccer,
        }
    }
}

/// The backend's `parse_events` tallies, unified across the four lanes (which
/// spell them `u32`, `usize` and `u16`). `failed` is what separates a glitched
/// scoreboard from a finished game — see [`Outcome`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// Events that passed the deserialize-tier validation.
    pub ok: u32,
    /// Events the backend's lenient parse would have dropped with a warn.
    pub failed: u32,
}

/// A transform-tier failure on the target event: the field was present and
/// well-typed but did not convert. The backend serves 5xx for these; the
/// device retries rather than treating the game as gone.
///
/// The four sport lanes each declare their own spelling of this set
/// (`Date`/`BadStartTime`/`StartTime`, `Sides`/`HomeAway`/`HomeAwayConflict`);
/// the sets themselves are identical, so they fold here without loss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransformError {
    /// `event.date` failed the start-time parse (pregame reads it).
    StartTime,
    /// The `homeAway` markers were not exactly one home and one away.
    HomeAway,
    /// A competitor `score` string failed its `u32` parse.
    Score,
    /// `team.color` / `team.alternateColor` failed the hex parse.
    Color,
}

/// Everything a stream can fail with. Absence of the target is *not* here —
/// that is a verdict, not an error (see [`Outcome`]).
#[derive(Debug, PartialEq)]
pub enum Error {
    /// The tokenizer or path engine rejected the body: malformed JSON, or a
    /// document deeper than the engine's bounded frame stack.
    Stream(path::Error),
    /// `$.events` was present but not an array. The backend's whole-body
    /// deserialize fails here before any event, so this is a glitched
    /// upstream (502) and never an empty slate.
    MalformedBody,
    /// The target event parsed but failed the transform tier.
    Transform(TransformError),
}

impl From<path::Error> for Error {
    fn from(error: path::Error) -> Self {
        Error::Stream(error)
    }
}

/// Passes a borrowed quirk receiver to the two sport lanes that take theirs by
/// value, so [`DetailStream`] can offer one `&mut Q` signature for all four.
///
/// A local newtype rather than `impl Quirks for &mut Q`: that impl would be a
/// foreign trait on a foreign type and the orphan rules forbid it.
pub(crate) struct ByRef<'a, Q: ?Sized>(pub(crate) &'a mut Q);

impl<Q: Quirks + ?Sized> Quirks for ByRef<'_, Q> {
    fn quirk(&mut self, quirk: Quirk) {
        self.0.quirk(quirk);
    }
}
