//! MLB extraction tables — S1 sport lane (see DESIGN.md and the MLB
//! inventory report; the backend oracle is `backend/src/mlb/{types,transform,
//! wire}.rs` plus `backend/src/shared/` and `backend/src/espn/types.rs`).
//!
//! One const path table over the scoreboard body (`$.events[*]…`), one
//! [`Sink`] carrying the semantics, two entry points over it:
//!
//! - [`ListExtractor`] — the games list: every event is validated against the
//!   backend's per-state required-field rules (the same lenient
//!   `parse_events` pass) and yields a [`ListRow`] through a caller
//!   [`ListSink`] the extractor owns and hands back, plus ok/failed counts
//!   (DESIGN.md ruling 13).
//! - [`DetailExtractor`] — one game: every event is validated until the
//!   target is found (ruling 14 — the failure counts are exact precisely
//!   when the 404-vs-502 verdict consumes them: a missing target means
//!   nothing was ever skipped), and only events *after* the found target are
//!   fast-forwarded via [`Directive::SkipElement`]; the target is
//!   transformed and returned as an owned bounded [`Extract`] whose
//!   [`Extract::as_game`] is the borrowed wire-shaped view.
//!
//! Field-order independence is a hard rule (ruling 4): every cross-field
//! input is buffered in per-event scratch and resolved at the event's
//! `leave`, because ESPN emits the state discriminant *after* the payload it
//! discriminates.
//!
//! # The two-tier error model (ruling 1)
//!
//! - **DU tier** (what serde deserialization rejects today): the event is
//!   dropped — the list skips it and counts it `failed`; a detail request
//!   whose target never parses resolves to [`DetailError::Glitched`]
//!   (today's 502), never `NotFound` (today's 404, the firmware's
//!   "game ended" signal).
//! - **Transform tier** (bad color hex, unparseable score/date, not one
//!   home plus one away): the event stays on the list, and the detail
//!   request surfaces [`DetailError::Transform`] (today's 5xx).
//! - **The veto** (its own tier): a live game whose `shortDetail` prefix is
//!   not `Top`/`Mid`/`Bot`/`End` (rain delay, suspension) parses fine and is
//!   *still* excluded — dropped from the list, [`DetailError::NotFound`] on
//!   detail — so the firmware never advertises a live game it cannot fetch.
//!   That is why `testdata/mlb/rain_delay.json` has no wire golden.
//!
//! # Recorded divergences from the backend (all strictly more lenient)
//!
//! Per the inventory's Q1/Q4 recommendation, "required by serde but never
//! consumed" strictness is dropped and recorded here rather than replicated:
//!
//! - `competitions[*]` is scoped to `[0]`; a malformed `competitions[1]`
//!   no longer rejects the event (inventory Q1).
//! - `team.id` presence is not required (Q4; MLB never reads it).
//! - Present-but-malformed *optional* substructures whose only required leaf
//!   is deeper than the table observes (`situation.pitcher` without
//!   `athlete.shortName`, a malformed `probables[1]`, `probables[0]` without
//!   its athlete) degrade to "absent" instead of rejecting the event.
//! - A container of the wrong kind (an object where an array belongs, or
//!   vice versa) is only detected when a scalar sits where the table expects
//!   a container; `records: {}` and friends degrade instead of rejecting.
//! - Duplicate JSON keys are last-wins here; serde rejects the event.
//! - More than [`MAX_LINE_SCORE`] line-score entries clip at accumulation
//!   (pre-sort) rather than at encode (post-sort), with a
//!   [`Quirk::ClippedLineScore`]; the backend clips after sorting.
//!
//! Quirk *timing* also differs where the backend only warns inside the
//! transform it happens to run: the list pass here reports
//! [`Quirk::ClippedLineScore`] while accumulating, and detail reports
//! [`Quirk::UnknownInningHalf`] / [`Quirk::MalformedRecord`] /
//! [`Quirk::WeatherDropped`] only for the target event, matching the
//! backend's call graph.

use core::mem;

use crate::common::{
    CrestPath, Crests, EText, HomeAway, ListRow, ListSink, ListTeam, NoRows, Quirk, Quirks,
    crest_path, linescore_byte, num_i16, num_u8, order_home_away, order_list_teams, parse_hex_rgb,
    parse_record, parse_start_time, saturate_score, set_text, stable_sort_by_key,
};
use crate::path::{ContainerKind, Directive, Error, Pattern, Seg, Sink, StreamMatcher, Value};
use scoreboard_wire::mlb::{AtBat as WireAtBat, Bases, Count, Inning, InningHalf, Weather as WireWeather};
use scoreboard_wire::{
    GameState, LastPlay as WireLastPlay, MAX_LINE_SCORE, Record, TeamColors, TeamState,
    clamp_temperature, mlb as wire_mlb,
};

/// A final team's per-inning runs. Bound: the wire's own `u8` length prefix
/// (`scoreboard_wire::MAX_LINE_SCORE`) — the extract never stores more than
/// the encoder could emit.
pub type LineScore = heapless::Vec<u8, MAX_LINE_SCORE>;

// ---------------------------------------------------------------- extracts

/// One extracted MLB game, domain-shaped (`u32` scores, `i16` temperature —
/// the JSON representation's types; the wire adapter narrows, see
/// [`Extract::as_game`]). String bounds are all [`EText`] (wire cap, ruling
/// 2 — never tighter).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Extract {
    Pregame(Pregame),
    Live(Live),
    Final(Final),
}

impl Extract {
    pub fn game_id(&self) -> &str {
        match self {
            Extract::Pregame(game) => &game.game_id,
            Extract::Live(game) => &game.game_id,
            Extract::Final(game) => &game.game_id,
        }
    }

    /// The crest paths for this game's two teams.
    pub fn crests(&self) -> &Crests {
        match self {
            Extract::Pregame(game) => &game.crests,
            Extract::Live(game) => &game.crests,
            Extract::Final(game) => &game.crests,
        }
    }

    pub fn state(&self) -> GameState {
        match self {
            Extract::Pregame(_) => GameState::Pregame,
            Extract::Live(_) => GameState::Live,
            Extract::Final(_) => GameState::Final,
        }
    }

    /// The borrowed wire-shaped view over the extract's own storage — the
    /// S3 seam. Options stay options: the wire encoder does the zeroing of
    /// unset numeric fields, and the score/temperature narrowing happens
    /// here with the wire's own `saturate_score`/`clamp_temperature`,
    /// exactly where the backend's `mlb/wire.rs` adapter does it.
    pub fn as_game(&self) -> wire_mlb::Game<'_> {
        match self {
            Extract::Pregame(game) => wire_mlb::Game::Pregame(wire_mlb::Pregame {
                game_id: &game.game_id,
                start_time: game.start_time,
                venue: &game.venue,
                weather: game.weather.as_ref().map(|weather| WireWeather {
                    condition: &weather.condition,
                    temperature: clamp_temperature(weather.temperature),
                }),
                away: pregame_team_view(&game.away),
                home: pregame_team_view(&game.home),
            }),
            Extract::Live(game) => wire_mlb::Game::Live(wire_mlb::Live {
                game_id: &game.game_id,
                inning: game.inning,
                count: game.count,
                bases: game.bases,
                away: team_state_view(&game.away),
                home: team_state_view(&game.home),
                at_bat: game.at_bat.as_ref().map(|at_bat| WireAtBat {
                    pitcher: &at_bat.pitcher,
                    batter: &at_bat.batter,
                }),
                last_play: WireLastPlay {
                    id: &game.last_play.id,
                    text: &game.last_play.text,
                },
            }),
            Extract::Final(game) => wire_mlb::Game::Final(wire_mlb::Final {
                game_id: &game.game_id,
                innings_played: game.innings_played,
                away: final_team_view(&game.away),
                home: final_team_view(&game.home),
            }),
        }
    }
}

fn pregame_team_view(team: &PregameTeam) -> wire_mlb::PregameTeam<'_> {
    wire_mlb::PregameTeam {
        abbreviation: &team.abbreviation,
        colors: team.colors,
        record: team.record,
        probable_pitcher: team.probable_pitcher.as_deref(),
    }
}

fn team_state_view(team: &LiveTeam) -> TeamState<'_> {
    TeamState {
        abbreviation: &team.abbreviation,
        score: saturate_score(team.score),
        colors: team.colors,
    }
}

fn final_team_view(team: &FinalTeam) -> scoreboard_wire::FinalTeam<'_> {
    scoreboard_wire::FinalTeam {
        abbreviation: &team.abbreviation,
        score: saturate_score(team.score),
        colors: team.colors,
        line_score: &team.line_score,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pregame {
    pub game_id: EText,
    /// Direct mode's crest sources; outside the wire view by design.
    pub crests: Crests,
    /// Unix epoch seconds, UTC (`parse_start_time`).
    pub start_time: u32,
    pub venue: EText,
    /// Absent when ESPN's weather block is missing or unusable.
    pub weather: Option<Weather>,
    pub away: PregameTeam,
    pub home: PregameTeam,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Weather {
    /// The untrimmed original of whichever member did not parse as a number.
    pub condition: EText,
    /// °F; `i16` like the JSON representation (an April night can be
    /// negative) — the wire adapter clamps to `u8`.
    pub temperature: i16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PregameTeam {
    pub abbreviation: EText,
    pub colors: TeamColors,
    /// Overall season record; absent when ESPN omits or malforms it.
    pub record: Option<Record>,
    pub probable_pitcher: Option<EText>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Live {
    pub game_id: EText,
    /// Direct mode's crest sources; outside the wire view by design.
    pub crests: Crests,
    pub inning: Inning,
    pub count: Count,
    pub bases: Bases,
    pub away: LiveTeam,
    pub home: LiveTeam,
    /// Absent between innings or before an at-bat starts — all-or-nothing:
    /// one side alone yields `None`, exactly like the backend.
    pub at_bat: Option<AtBat>,
    pub last_play: LastPlay,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveTeam {
    pub abbreviation: EText,
    /// `u32` like the JSON representation; the wire adapter saturates.
    pub score: u32,
    pub colors: TeamColors,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtBat {
    pub pitcher: EText,
    pub batter: EText,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastPlay {
    pub id: EText,
    pub text: EText,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Final {
    pub game_id: EText,
    /// Direct mode's crest sources; outside the wire view by design.
    pub crests: Crests,
    pub innings_played: u8,
    pub away: FinalTeam,
    pub home: FinalTeam,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalTeam {
    pub abbreviation: EText,
    pub score: u32,
    pub colors: TeamColors,
    /// Runs per inning, inning 1 first, ordered by the backend's stable
    /// sort on `period` (reproduced here with a `(period, arrival)` key).
    pub line_score: LineScore,
}

// ------------------------------------------------------------------ errors

/// Transform-tier failures, in the backend's own evaluation order per state
/// (the first one hit is the one reported, matching which `AppError` the
/// backend surfaces today).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransformError {
    /// `event.date` failed `parse_start_time` (pregame only).
    Date,
    /// Not exactly one home and one away marker.
    HomeAway,
    /// `competitor.score` failed `str::parse::<u32>` (live/final only).
    Score,
    /// `team.color` / `team.alternateColor` failed `parse_hex_rgb`.
    Color,
}

#[derive(Debug, PartialEq)]
pub enum ListError {
    /// The tokenizer/engine rejected the body — whole scoreboard unusable.
    Stream(Error),
    /// `events` is not an array (`RawScoreboard` would fail — today's 502).
    Events,
}

#[derive(Debug, PartialEq)]
pub enum DetailError {
    /// The tokenizer/engine rejected the body — whole scoreboard unusable.
    Stream(Error),
    /// `events` is not an array (`RawScoreboard` would fail — today's 502).
    Events,
    /// The 404-eligible outcomes: id absent from a clean scoreboard, an
    /// event with no competition, or a live game vetoed on its
    /// `shortDetail` prefix. The firmware treats this as "game ended".
    NotFound,
    /// Id absent but at least one event failed to parse: today's 502 — a
    /// glitched scoreboard must never masquerade as "game ended".
    Glitched,
    /// The target parsed but its transform failed — today's 5xx.
    Transform(TransformError),
}

// -------------------------------------------------------------- path table

use Seg::{AnyIndex, Index, Key};

const P_EVENTS: usize = 0;
const P_EVENT: usize = 1;
const P_ID: usize = 2;
const P_DATE: usize = 3;
const P_WEATHER: usize = 4;
const P_WX_DISPLAY: usize = 5;
const P_WX_CONDITION_ID: usize = 6;
const P_WX_TEMPERATURE: usize = 7;
const P_COMPETITIONS: usize = 8;
const P_COMPETITION0: usize = 9;
const P_STATE: usize = 10;
const P_SHORT_DETAIL: usize = 11;
const P_PERIOD: usize = 12;
const P_VENUE: usize = 13;
const P_VENUE_NAME: usize = 14;
const P_SITUATION: usize = 15;
const P_BALLS: usize = 16;
const P_STRIKES: usize = 17;
const P_OUTS: usize = 18;
const P_ON_FIRST: usize = 19;
const P_ON_SECOND: usize = 20;
const P_ON_THIRD: usize = 21;
const P_PITCHER: usize = 22;
const P_BATTER: usize = 23;
const P_LAST_PLAY_ID: usize = 24;
const P_LAST_PLAY_TEXT: usize = 25;
const P_COMPETITOR: usize = 26;
const P_HOME_AWAY: usize = 27;
const P_SCORE: usize = 28;
const P_ABBREVIATION: usize = 29;
const P_COLOR: usize = 30;
const P_ALT_COLOR: usize = 31;
const P_RECORDS: usize = 32;
const P_RECORD_ENTRY: usize = 33;
const P_RECORD_TYPE: usize = 34;
const P_RECORD_SUMMARY: usize = 35;
const P_PROBABLES: usize = 36;
const P_PROBABLE_NAME: usize = 37;
const P_LINESCORES: usize = 38;
const P_LINESCORE_ENTRY: usize = 39;
const P_LINESCORE_VALUE: usize = 40;
const P_LINESCORE_PERIOD: usize = 41;
/// Direct mode only: the crest artwork the backend used to resolve for the
/// device. Appended rather than filed with the other `team.*` patterns so
/// the indices above keep matching the inventory they were derived from.
const P_TEAM_LOGO: usize = 42;

/// Everything the backend reads, relative to the scoreboard root. The bare
/// container patterns (`weather`, `competitions`, `venue`, `situation`,
/// `records`, `probables`, `linescores`) exist to reproduce serde's
/// presence/typing rules: a scalar where serde wants a container is a
/// DU-tier reject. The status read is competition-level, never `$.status`
/// (inventory §0.2), and `competitions` is scoped to `[0]` (Q1).
static PATHS: &[Pattern] = &[
    /* P_EVENTS          */ &[Key("events")],
    /* P_EVENT           */ &[Key("events"), AnyIndex],
    /* P_ID              */ &[Key("events"), AnyIndex, Key("id")],
    /* P_DATE            */ &[Key("events"), AnyIndex, Key("date")],
    /* P_WEATHER         */ &[Key("events"), AnyIndex, Key("weather")],
    /* P_WX_DISPLAY      */ &[Key("events"), AnyIndex, Key("weather"), Key("displayValue")],
    /* P_WX_CONDITION_ID */ &[Key("events"), AnyIndex, Key("weather"), Key("conditionId")],
    /* P_WX_TEMPERATURE  */ &[Key("events"), AnyIndex, Key("weather"), Key("temperature")],
    /* P_COMPETITIONS    */ &[Key("events"), AnyIndex, Key("competitions")],
    /* P_COMPETITION0    */ &[Key("events"), AnyIndex, Key("competitions"), Index(0)],
    /* P_STATE           */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("status"), Key("type"), Key("state")],
    /* P_SHORT_DETAIL    */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("status"), Key("type"), Key("shortDetail")],
    /* P_PERIOD          */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("status"), Key("period")],
    /* P_VENUE           */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("venue")],
    /* P_VENUE_NAME      */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("venue"), Key("fullName")],
    /* P_SITUATION       */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("situation")],
    /* P_BALLS           */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("situation"), Key("balls")],
    /* P_STRIKES         */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("situation"), Key("strikes")],
    /* P_OUTS            */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("situation"), Key("outs")],
    /* P_ON_FIRST        */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("situation"), Key("onFirst")],
    /* P_ON_SECOND       */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("situation"), Key("onSecond")],
    /* P_ON_THIRD        */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("situation"), Key("onThird")],
    /* P_PITCHER         */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("situation"), Key("pitcher"), Key("athlete"), Key("shortName")],
    /* P_BATTER          */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("situation"), Key("batter"), Key("athlete"), Key("shortName")],
    /* P_LAST_PLAY_ID    */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("situation"), Key("lastPlay"), Key("id")],
    /* P_LAST_PLAY_TEXT  */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("situation"), Key("lastPlay"), Key("text")],
    /* P_COMPETITOR      */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex],
    /* P_HOME_AWAY       */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("homeAway")],
    /* P_SCORE           */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("score")],
    /* P_ABBREVIATION    */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("team"), Key("abbreviation")],
    /* P_COLOR           */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("team"), Key("color")],
    /* P_ALT_COLOR       */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("team"), Key("alternateColor")],
    /* P_RECORDS         */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("records")],
    /* P_RECORD_ENTRY    */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("records"), AnyIndex],
    /* P_RECORD_TYPE     */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("records"), AnyIndex, Key("type")],
    /* P_RECORD_SUMMARY  */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("records"), AnyIndex, Key("summary")],
    /* P_PROBABLES       */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("probables")],
    /* P_PROBABLE_NAME — index 0 only, the backend's `.first()` */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("probables"), Index(0), Key("athlete"), Key("shortName")],
    /* P_LINESCORES      */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("linescores")],
    /* P_LINESCORE_ENTRY */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("linescores"), AnyIndex],
    /* P_LINESCORE_VALUE */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("linescores"), AnyIndex, Key("value")],
    /* P_LINESCORE_PERIOD */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("linescores"), AnyIndex, Key("period")],
    /* P_TEAM_LOGO       */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("team"), Key("logo")],
];

// -------------------------------------------------------------- entry uses

/// `parse_events`' tallies: `ok` counts events that deserialize (including
/// ones that yield no list entry), `failed` counts DU-tier rejects. The
/// 404-vs-502 rule needs `failed` (ruling 13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub ok: u32,
    pub failed: u32,
}

/// Streaming games-list pass. `scratch` is picojson's token buffer: it must
/// hold the longest contiguous string/number token in the body (ESPN link
/// URLs run a few hundred bytes; the corpus harnesses use 16 KiB).
pub struct ListExtractor<'c, 's, L: ListSink, Q: Quirks> {
    matcher: StreamMatcher<'static, 's, MlbSink<'c, L, Q>>,
}

impl<'c, 's, L: ListSink, Q: Quirks> ListExtractor<'c, 's, L, Q> {
    pub fn new(entries: L, quirks: &'c mut Q, scratch: &'s mut [u8]) -> Result<Self, Error> {
        let sink = MlbSink::new(Mode::List, entries, quirks);
        Ok(Self {
            matcher: StreamMatcher::new(PATHS, sink, scratch)?,
        })
    }

    pub fn write(&mut self, chunk: &[u8]) -> Result<(), Error> {
        self.matcher.write(chunk)
    }

    /// The sink back, with the counts. Owning it is what lets one caller-side
    /// type serve every sport (see [`ListSink`]).
    pub fn finish(self) -> Result<(L, Counts), ListError> {
        let sink = self.matcher.finish().map_err(ListError::Stream)?;
        if sink.events_malformed {
            return Err(ListError::Events);
        }
        Ok((
            sink.entries,
            Counts {
                ok: sink.ok,
                failed: sink.failed,
            },
        ))
    }
}

/// Streaming game-detail pass for one target id.
pub struct DetailExtractor<'c, 's, Q: Quirks> {
    matcher: StreamMatcher<'static, 's, MlbSink<'c, NoRows, Q>>,
}

impl<'c, 's, Q: Quirks> DetailExtractor<'c, 's, Q> {
    pub fn new(game_id: &'c str, quirks: &'c mut Q, scratch: &'s mut [u8]) -> Result<Self, Error> {
        let sink = MlbSink::new(Mode::Detail { target: game_id }, NoRows, quirks);
        Ok(Self {
            matcher: StreamMatcher::new(PATHS, sink, scratch)?,
        })
    }

    pub fn write(&mut self, chunk: &[u8]) -> Result<(), Error> {
        self.matcher.write(chunk)
    }

    /// The counts cover events up to and including the target (ruling 14):
    /// events after a found target are skipped and uncounted — fine, the
    /// verdict is already `Found`. When the target is absent, nothing was
    /// ever skipped, so `failed` is exact where the `Glitched`-vs-`NotFound`
    /// verdict consumes it.
    pub fn finish(self) -> Result<(Extract, Counts), DetailError> {
        let sink = self.matcher.finish().map_err(DetailError::Stream)?;
        if sink.events_malformed {
            return Err(DetailError::Events);
        }
        let counts = Counts {
            ok: sink.ok,
            failed: sink.failed,
        };
        match sink.result {
            Some(Ok(extract)) => Ok((extract, counts)),
            Some(Err(Fail::NotFound)) => Err(DetailError::NotFound),
            Some(Err(Fail::Transform(kind))) => Err(DetailError::Transform(kind)),
            None if sink.failed > 0 => Err(DetailError::Glitched),
            None => Err(DetailError::NotFound),
        }
    }
}

// ------------------------------------------------------------- event sink

/// `status.type.state`, strict: any other value is a DU-tier reject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Pre,
    In,
    Post,
}

/// What became of the event currently streaming past.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Fate {
    #[default]
    Scanning,
    /// A DU-tier reject: counts `failed`, everything else ignored.
    Malformed,
    /// Detail mode fast-forwarded an event *after* the found target
    /// (ruling 14) — uncounted, the verdict is already decided.
    Skipped,
}

/// A weather member buffered for the transposition heuristic: the numeric
/// test ran on the full untruncated text (trimmed, `f64` — inventory §2.2)
/// at arrival; the stored text is the untrimmed original.
#[derive(Debug)]
struct Candidate {
    text: EText,
    numeric: bool,
}

#[derive(Debug, Default)]
struct RecordScratch {
    type_present: bool,
    is_total: bool,
    summary_present: bool,
    parsed: Option<(u16, u16)>,
}

#[derive(Debug, Default)]
struct LineScratch {
    value: Option<u8>,
    period: Option<u8>,
}

#[derive(Debug, Default)]
struct CompetitorScratch {
    home_away: Option<HomeAway>,
    /// `team.logo` as a CDN path. No `_present` twin: the backend's games
    /// pipeline never deserializes this field, so nothing about it can fail
    /// an event without breaking parity.
    crest: Option<CrestPath>,
    score_present: bool,
    score: Option<u32>,
    abbreviation: Option<EText>,
    color_present: bool,
    color: Option<u32>,
    alt_present: bool,
    alt_color: Option<u32>,
    /// Outer `Some` once a `type == "total"` entry was seen (first wins,
    /// like the backend's `find`); inner is its summary's parse.
    total_record: Option<Option<(u16, u16)>>,
    probable: Option<EText>,
    /// `(period, clamped value)` in arrival order — the promoted
    /// `stable_sort_by_key` orders them by `period` at finalize, keeping
    /// arrival order for equal periods exactly like the backend's stable
    /// `sort_by_key(period)` (rulings 13/15).
    lines: heapless::Vec<(u8, u8), MAX_LINE_SCORE>,
}

/// Everything buffered for the event in flight. All states accumulate the
/// same union because the discriminant arrives last (inventory §0.1).
#[derive(Debug, Default)]
struct EventScratch {
    fate: Fate,
    id: Option<EText>,
    /// Detail mode: the arrival-time full-text compare against the target
    /// id matched — exact even where the wire-cap copy would truncate.
    id_matched: bool,
    date_present: bool,
    start_time: Option<u32>,
    weather_entered: bool,
    wx_display: Option<Candidate>,
    wx_condition_id: Option<Candidate>,
    wx_temperature: Option<i16>,
    competitions_ok: bool,
    competition0_seen: bool,
    state: Option<State>,
    short_detail_present: bool,
    half: Option<InningHalf>,
    period: Option<u8>,
    venue_entered: bool,
    venue: Option<EText>,
    situation_entered: bool,
    balls: Option<u8>,
    strikes: Option<u8>,
    outs: Option<u8>,
    on_first: Option<bool>,
    on_second: Option<bool>,
    on_third: Option<bool>,
    pitcher: Option<EText>,
    batter: Option<EText>,
    last_play_id: Option<EText>,
    last_play_text: Option<EText>,
    competitor_count: u16,
    comps: [CompetitorScratch; 2],
    record: RecordScratch,
    line: LineScratch,
}

impl EventScratch {
    fn veto(&mut self) {
        self.fate = Fate::Malformed;
    }

    /// Would `EspnEvent<EspnCompetition>` have deserialized? The per-state
    /// required set from the inventory's §1 tables, checked at finalize so
    /// field order never matters.
    fn parses(&self) -> bool {
        if self.id.is_none() || !self.date_present || !self.competitions_ok {
            return false;
        }
        if !self.competition0_seen {
            // An empty `competitions` array deserializes fine; the event
            // just yields no list entry (and NotFound on detail).
            return true;
        }
        let Some(state) = self.state else {
            return false;
        };
        // Deserialized in every state even where discarded (§5.7).
        if !self.short_detail_present || self.period.is_none() {
            return false;
        }
        // `two_competitors` runs inside the DU conversion: exactly 2.
        if self.competitor_count != 2 {
            return false;
        }
        for competitor in &self.comps {
            if competitor.home_away.is_none()
                || !competitor.score_present
                || competitor.abbreviation.is_none()
                || !competitor.color_present
                || !competitor.alt_present
            {
                return false;
            }
        }
        // Optional substructures reject the event when present-but-broken.
        if self.venue_entered && self.venue.is_none() {
            return false;
        }
        if self.situation_entered && !self.situation_complete() {
            return false;
        }
        match state {
            State::Pre => self.venue.is_some(),
            State::In => self.situation_entered,
            State::Post => true,
        }
    }

    /// The full `EspnSituation` required set — including the no-default
    /// bools (§5.3) and `lastPlay` (§5.4), fragile but bug-compatible.
    fn situation_complete(&self) -> bool {
        self.balls.is_some()
            && self.strikes.is_some()
            && self.outs.is_some()
            && self.on_first.is_some()
            && self.on_second.is_some()
            && self.on_third.is_some()
            && self.last_play_id.is_some()
            && self.last_play_text.is_some()
    }
}

/// Which pass is running. The list sink is a sink *field* rather than a mode
/// payload so that detail mode carries a zero-sized [`NoRows`] instead of an
/// unused borrow, and so `Mode` stays `Copy`.
#[derive(Clone, Copy)]
enum Mode<'c> {
    List,
    Detail { target: &'c str },
}

/// Internal detail outcome for the target event.
enum Fail {
    NotFound,
    Transform(TransformError),
}

struct MlbSink<'c, L: ListSink, Q: Quirks> {
    mode: Mode<'c>,
    entries: L,
    quirks: &'c mut Q,
    event: EventScratch,
    ok: u32,
    failed: u32,
    events_malformed: bool,
    result: Option<Result<Extract, Fail>>,
}

impl<'c, L: ListSink, Q: Quirks> MlbSink<'c, L, Q> {
    fn new(mode: Mode<'c>, entries: L, quirks: &'c mut Q) -> Self {
        Self {
            mode,
            entries,
            quirks,
            event: EventScratch::default(),
            ok: 0,
            failed: 0,
            events_malformed: false,
            result: None,
        }
    }

    /// Ruling 14's skip gate. A resolved `result` *is* the found target: the
    /// two were always set together, so there is no second flag to drift.
    fn detail_done(&self) -> bool {
        self.result.is_some()
    }

    fn finish_event(&mut self) {
        let scratch = mem::take(&mut self.event);
        match scratch.fate {
            Fate::Skipped => return,
            Fate::Malformed => {
                self.failed += 1;
                return;
            }
            Fate::Scanning => {}
        }
        if !scratch.parses() {
            self.failed += 1;
            return;
        }
        self.ok += 1;
        match self.mode {
            Mode::List => {
                if !scratch.competition0_seen {
                    return;
                }
                let state = match scratch.state.expect("state was validated by parses()") {
                    State::Pre => GameState::Pregame,
                    State::Post => GameState::Final,
                    State::In => {
                        if scratch.half.is_none() {
                            // The veto: dropped from the list entirely.
                            self.quirks.quirk(Quirk::UnknownInningHalf);
                            return;
                        }
                        GameState::Live
                    }
                };
                let (away, home) = list_teams(&scratch.comps);
                self.entries.row(ListRow {
                    id: scratch.id.as_deref().unwrap_or_default(),
                    state,
                    away,
                    home,
                });
            }
            Mode::Detail { .. } => {
                if self.detail_done() || !scratch.id_matched {
                    return;
                }
                self.result = Some(build_extract(scratch, self.quirks));
            }
        }
    }

    fn finish_record(&mut self, indices: &[u16]) {
        let entry = mem::take(&mut self.event.record);
        if !entry.type_present || !entry.summary_present {
            // `EspnRecord` requires both members.
            self.event.veto();
            return;
        }
        if entry.is_total {
            if let Some(competitor) = self.event.comps.get_mut(competitor_index(indices)) {
                if competitor.total_record.is_none() {
                    competitor.total_record = Some(entry.parsed);
                }
            }
        }
    }

    fn finish_linescore(&mut self, indices: &[u16]) {
        let entry = mem::take(&mut self.event.line);
        let (Some(period), Some(value)) = (entry.period, entry.value) else {
            // `EspnLinescore` requires both members.
            self.event.veto();
            return;
        };
        if let Some(competitor) = self.event.comps.get_mut(competitor_index(indices)) {
            if competitor.lines.push((period, value)).is_err() {
                self.quirks.quirk(Quirk::ClippedLineScore);
            }
        }
    }
}

/// One competitor's list-row extras, straight off the scratch it already
/// buffered for the detail transform.
fn list_team(competitor: &CompetitorScratch) -> ListTeam<'_> {
    ListTeam {
        abbreviation: competitor.abbreviation.as_deref(),
        crest: competitor.crest.as_deref(),
    }
}

/// The list row's `(away, home)` extras. Never a validity gate: `parses()`
/// has already decided the event lists, and unresolvable markers here empty
/// the extras rather than dropping the row.
fn list_teams(comps: &[CompetitorScratch; 2]) -> (ListTeam<'_>, ListTeam<'_>) {
    order_list_teams(
        (comps[0].home_away, list_team(&comps[0])),
        (comps[1].home_away, list_team(&comps[1])),
    )
}

/// The competitor an `AnyIndex`-bound pattern fired under; out-of-range
/// (a third competitor — the event is doomed to the count check anyway)
/// resolves to an index `comps.get_mut` rejects.
fn competitor_index(indices: &[u16]) -> usize {
    indices.get(1).map_or(usize::MAX, |&index| index as usize)
}

impl<L: ListSink, Q: Quirks> Sink for MlbSink<'_, L, Q> {
    fn enter(&mut self, pattern: usize, _indices: &[u16], kind: ContainerKind) -> Directive {
        if pattern == P_EVENTS {
            // `{"events":{…}}` must 502-shape like a scalar shell; only an
            // array here is a scoreboard (engine ContainerKind addition).
            if kind == ContainerKind::Object {
                self.events_malformed = true;
            }
            return Directive::Continue;
        }
        if pattern == P_EVENT {
            self.event = EventScratch::default();
            if self.detail_done() {
                // Ruling 14: only events after the found target skip.
                self.event.fate = Fate::Skipped;
                return Directive::SkipElement;
            }
            return Directive::Continue;
        }
        if self.event.fate != Fate::Scanning {
            return Directive::Continue;
        }
        let event = &mut self.event;
        match pattern {
            P_WEATHER => event.weather_entered = true,
            P_COMPETITIONS => event.competitions_ok = true,
            P_COMPETITION0 => event.competition0_seen = true,
            P_VENUE => event.venue_entered = true,
            P_SITUATION => event.situation_entered = true,
            P_COMPETITOR => event.competitor_count = event.competitor_count.saturating_add(1),
            P_RECORD_ENTRY => event.record = RecordScratch::default(),
            P_LINESCORE_ENTRY => event.line = LineScratch::default(),
            _ => {}
        }
        Directive::Continue
    }

    fn leave(&mut self, pattern: usize, indices: &[u16]) -> Directive {
        match pattern {
            P_EVENT => self.finish_event(),
            P_RECORD_ENTRY if self.event.fate == Fate::Scanning => self.finish_record(indices),
            P_LINESCORE_ENTRY if self.event.fate == Fate::Scanning => {
                self.finish_linescore(indices)
            }
            _ => {}
        }
        Directive::Continue
    }

    fn value(&mut self, pattern: usize, indices: &[u16], value: Value<'_>) -> Directive {
        match pattern {
            // `events` itself a scalar/null: `RawScoreboard` fails — the
            // whole request is unusable, not one event.
            P_EVENTS => {
                self.events_malformed = true;
                return Directive::Continue;
            }
            // A scalar events[i]: one DU-tier reject, like `{}` but cheaper.
            P_EVENT => {
                if !self.detail_done() {
                    self.failed += 1;
                }
                return Directive::Continue;
            }
            _ => {}
        }
        if self.event.fate != Fate::Scanning {
            return Directive::Continue;
        }
        if pattern == P_ID {
            match value {
                Value::Str(text) => {
                    // Ruling 14: no skip on mismatch — every event before
                    // the target is fully validated so the failure counts
                    // stay exact. The compare runs on the full text, before
                    // the wire-cap copy could truncate.
                    if let Mode::Detail { target } = self.mode {
                        self.event.id_matched = text == target;
                    }
                    self.event.id = Some(etext(text));
                }
                _ => self.event.veto(),
            }
            return Directive::Continue;
        }
        let event = &mut self.event;
        match pattern {
            P_DATE => match value {
                Value::Str(text) => {
                    event.date_present = true;
                    // Parsed on the full text at arrival; consumed pregame.
                    event.start_time = parse_start_time(text);
                }
                _ => event.veto(),
            },
            P_WEATHER | P_VENUE | P_SITUATION => match value {
                // `Option<...>` fields: JSON null is a clean None.
                Value::Null => {}
                _ => event.veto(),
            },
            P_WX_DISPLAY | P_WX_CONDITION_ID => match value {
                Value::Str(text) => {
                    let candidate = Candidate {
                        // Numeric test on the trimmed full text, storage
                        // untrimmed — inventory §2.2, exactly.
                        numeric: text.trim().parse::<f64>().is_ok(),
                        text: etext(text),
                    };
                    if pattern == P_WX_DISPLAY {
                        event.wx_display = Some(candidate);
                    } else {
                        event.wx_condition_id = Some(candidate);
                    }
                }
                Value::Null => {}
                _ => event.veto(),
            },
            P_WX_TEMPERATURE => match value {
                // Must be an integer literal in `i16` — a float here kills
                // the event, exactly like serde (inventory §1, Q7).
                Value::Num(raw) => match num_i16(raw) {
                    Some(temperature) => event.wx_temperature = Some(temperature),
                    None => event.veto(),
                },
                Value::Null => {}
                _ => event.veto(),
            },
            // Required containers: a scalar (or null — `#[serde(default)]`
            // never applies to explicit null) rejects the event.
            P_COMPETITIONS | P_COMPETITION0 | P_COMPETITOR | P_RECORDS | P_RECORD_ENTRY
            | P_PROBABLES | P_LINESCORES | P_LINESCORE_ENTRY => event.veto(),
            P_STATE => match value {
                Value::Str("pre") => event.state = Some(State::Pre),
                Value::Str("in") => event.state = Some(State::In),
                Value::Str("post") => event.state = Some(State::Post),
                _ => event.veto(),
            },
            P_SHORT_DETAIL => match value {
                Value::Str(text) => {
                    event.short_detail_present = true;
                    event.half = parse_inning_half(text);
                }
                _ => event.veto(),
            },
            P_PERIOD => match value {
                Value::Num(raw) => match num_u8(raw) {
                    Some(period) => event.period = Some(period),
                    None => event.veto(),
                },
                _ => event.veto(),
            },
            P_VENUE_NAME => match value {
                Value::Str(text) => event.venue = Some(etext(text)),
                _ => event.veto(),
            },
            P_BALLS | P_STRIKES | P_OUTS => match value {
                Value::Num(raw) => match num_u8(raw) {
                    Some(count) => {
                        let slot = match pattern {
                            P_BALLS => &mut event.balls,
                            P_STRIKES => &mut event.strikes,
                            _ => &mut event.outs,
                        };
                        *slot = Some(count);
                    }
                    None => event.veto(),
                },
                _ => event.veto(),
            },
            P_ON_FIRST | P_ON_SECOND | P_ON_THIRD => match value {
                Value::Bool(on) => {
                    let slot = match pattern {
                        P_ON_FIRST => &mut event.on_first,
                        P_ON_SECOND => &mut event.on_second,
                        _ => &mut event.on_third,
                    };
                    *slot = Some(on);
                }
                _ => event.veto(),
            },
            P_PITCHER | P_BATTER | P_LAST_PLAY_ID | P_LAST_PLAY_TEXT => match value {
                Value::Str(text) => {
                    let slot = match pattern {
                        P_PITCHER => &mut event.pitcher,
                        P_BATTER => &mut event.batter,
                        P_LAST_PLAY_ID => &mut event.last_play_id,
                        _ => &mut event.last_play_text,
                    };
                    *slot = Some(etext(text));
                }
                _ => event.veto(),
            },
            P_HOME_AWAY => match value {
                Value::Str(text) => match HomeAway::parse(text) {
                    Some(side) => {
                        if let Some(competitor) = event.comps.get_mut(competitor_index(indices)) {
                            competitor.home_away = Some(side);
                        }
                    }
                    None => event.veto(),
                },
                _ => event.veto(),
            },
            P_SCORE => match value {
                // A JSON *string*; parsed here but only consumed live/final
                // — a pregame "TBD" survives, exactly like the backend.
                Value::Str(text) => {
                    if let Some(competitor) = event.comps.get_mut(competitor_index(indices)) {
                        competitor.score_present = true;
                        competitor.score = text.parse().ok();
                    }
                }
                _ => event.veto(),
            },
            P_ABBREVIATION => match value {
                Value::Str(text) => {
                    if let Some(competitor) = event.comps.get_mut(competitor_index(indices)) {
                        competitor.abbreviation = Some(etext(text));
                    }
                }
                _ => event.veto(),
            },
            // No veto arm: the field is invisible to the backend's parse,
            // so a malformed one must cost the event nothing.
            P_TEAM_LOGO => {
                if let Value::Str(href) = value {
                    if let Some(competitor) = event.comps.get_mut(competitor_index(indices)) {
                        competitor.crest = crest_path(href);
                    }
                }
            }
            P_COLOR | P_ALT_COLOR => match value {
                Value::Str(text) => {
                    if let Some(competitor) = event.comps.get_mut(competitor_index(indices)) {
                        let parsed = parse_hex_rgb(text);
                        if pattern == P_COLOR {
                            competitor.color_present = true;
                            competitor.color = parsed;
                        } else {
                            competitor.alt_present = true;
                            competitor.alt_color = parsed;
                        }
                    }
                }
                _ => event.veto(),
            },
            P_RECORD_TYPE => match value {
                Value::Str(text) => {
                    event.record.type_present = true;
                    // `type == "total"` exactly; `abbreviation` varies and
                    // is deliberately never matched (inventory §2.7).
                    event.record.is_total = text == "total";
                }
                _ => event.veto(),
            },
            P_RECORD_SUMMARY => match value {
                Value::Str(text) => {
                    event.record.summary_present = true;
                    event.record.parsed = parse_record(text);
                }
                _ => event.veto(),
            },
            P_PROBABLE_NAME => match value {
                Value::Str(text) => {
                    if let Some(competitor) = event.comps.get_mut(competitor_index(indices)) {
                        competitor.probable = Some(etext(text));
                    }
                }
                _ => event.veto(),
            },
            P_LINESCORE_VALUE => match value {
                // A JSON float; `f64` then clamp-and-truncate to a byte.
                Value::Num(raw) => match raw.parse::<f64>() {
                    Ok(runs) => event.line.value = Some(linescore_byte(runs)),
                    Err(_) => event.veto(),
                },
                _ => event.veto(),
            },
            P_LINESCORE_PERIOD => match value {
                Value::Num(raw) => match num_u8(raw) {
                    Some(period) => event.line.period = Some(period),
                    None => event.veto(),
                },
                _ => event.veto(),
            },
            _ => {}
        }
        Directive::Continue
    }
}

// --------------------------------------------------- finalize (transform)

/// The target event's transform, in the backend's own evaluation order per
/// state so the first failure reported is the same one it reports.
fn build_extract<Q: Quirks>(scratch: EventScratch, quirks: &mut Q) -> Result<Extract, Fail> {
    if !scratch.competition0_seen {
        // `competitions.into_iter().next().ok_or(GameNotFound)`.
        return Err(Fail::NotFound);
    }
    let state = scratch.state.expect("state was validated by parses()");
    let game_id = scratch.id.expect("id was validated by parses()");
    let period = scratch.period.expect("period was validated by parses()");
    let [first, second] = scratch.comps;

    match state {
        State::Pre => {
            let start_time = scratch
                .start_time
                .ok_or(Fail::Transform(TransformError::Date))?;
            let (home_c, away_c) = ordered(first, second)?;
            let crests = crests(&home_c, &away_c);
            let weather = resolve_weather(
                scratch.weather_entered,
                scratch.wx_display,
                scratch.wx_condition_id,
                scratch.wx_temperature,
                quirks,
            );
            let home = pregame_team(home_c, quirks)?;
            let away = pregame_team(away_c, quirks)?;
            Ok(Extract::Pregame(Pregame {
                game_id,
                crests,
                start_time,
                venue: scratch.venue.expect("venue was validated by parses()"),
                weather,
                away,
                home,
            }))
        }
        State::In => {
            // The veto comes first, before any transform work — a
            // rain-delayed game 404s, it never 502s (inventory §2.1).
            let Some(half) = scratch.half else {
                quirks.quirk(Quirk::UnknownInningHalf);
                return Err(Fail::NotFound);
            };
            let (home_c, away_c) = ordered(first, second)?;
            let crests = crests(&home_c, &away_c);
            let home = live_team(home_c)?;
            let away = live_team(away_c)?;
            let at_bat = match (scratch.pitcher, scratch.batter) {
                // All-or-nothing: one side alone is None (inventory §2.6).
                (Some(pitcher), Some(batter)) => Some(AtBat { pitcher, batter }),
                _ => None,
            };
            Ok(Extract::Live(Live {
                game_id,
                crests,
                inning: Inning {
                    number: period,
                    half,
                },
                count: Count {
                    balls: scratch.balls.expect("situation was validated"),
                    strikes: scratch.strikes.expect("situation was validated"),
                    outs: scratch.outs.expect("situation was validated"),
                },
                bases: Bases {
                    first: scratch.on_first.expect("situation was validated"),
                    second: scratch.on_second.expect("situation was validated"),
                    third: scratch.on_third.expect("situation was validated"),
                },
                away,
                home,
                at_bat,
                last_play: LastPlay {
                    id: scratch.last_play_id.expect("situation was validated"),
                    text: scratch.last_play_text.expect("situation was validated"),
                },
            }))
        }
        State::Post => {
            let (home_c, away_c) = ordered(first, second)?;
            let crests = crests(&home_c, &away_c);
            let home = final_team(home_c)?;
            let away = final_team(away_c)?;
            Ok(Extract::Final(Final {
                game_id,
                crests,
                innings_played: period,
                away,
                home,
            }))
        }
    }
}

/// `(home, away)` by marker, never by index — all corpora happen to send
/// home first, which is a trap, not a rule (inventory §2.5).
fn ordered(
    first: CompetitorScratch,
    second: CompetitorScratch,
) -> Result<(CompetitorScratch, CompetitorScratch), Fail> {
    let first_side = first.home_away.expect("marker was validated by parses()");
    let second_side = second.home_away.expect("marker was validated by parses()");
    order_home_away((first_side, first), (second_side, second))
        .ok_or(Fail::Transform(TransformError::HomeAway))
}

/// Both crests, taken from the `homeAway`-ordered scratch pair.
fn crests(home: &CompetitorScratch, away: &CompetitorScratch) -> Crests {
    Crests {
        away: away.crest.clone(),
        home: home.crest.clone(),
    }
}

fn team_colors(competitor: &CompetitorScratch) -> Result<TeamColors, Fail> {
    let primary = competitor
        .color
        .ok_or(Fail::Transform(TransformError::Color))?;
    let alternate = competitor
        .alt_color
        .ok_or(Fail::Transform(TransformError::Color))?;
    Ok(TeamColors { primary, alternate })
}

fn pregame_team<Q: Quirks>(
    competitor: CompetitorScratch,
    quirks: &mut Q,
) -> Result<PregameTeam, Fail> {
    let colors = team_colors(&competitor)?;
    let record = match competitor.total_record {
        Some(Some((wins, losses))) => Some(Record { wins, losses }),
        Some(None) => {
            quirks.quirk(Quirk::MalformedRecord);
            None
        }
        None => None,
    };
    Ok(PregameTeam {
        abbreviation: competitor
            .abbreviation
            .expect("abbreviation was validated by parses()"),
        colors,
        record,
        probable_pitcher: competitor.probable,
    })
}

fn live_team(competitor: CompetitorScratch) -> Result<LiveTeam, Fail> {
    // Score before colors — `competitor_to_team_state`'s order.
    let score = competitor
        .score
        .ok_or(Fail::Transform(TransformError::Score))?;
    let colors = team_colors(&competitor)?;
    Ok(LiveTeam {
        abbreviation: competitor
            .abbreviation
            .expect("abbreviation was validated by parses()"),
        score,
        colors,
    })
}

fn final_team(competitor: CompetitorScratch) -> Result<FinalTeam, Fail> {
    let score = competitor
        .score
        .ok_or(Fail::Transform(TransformError::Score))?;
    let colors = team_colors(&competitor)?;
    let mut pairs = competitor.lines;
    // The promoted stable sort (ruling 15): equal periods keep arrival
    // order, byte-identical to the backend's stable `sort_by_key(period)`.
    stable_sort_by_key(&mut pairs, |&(period, _)| period);
    let mut line_score = LineScore::new();
    for &(_, runs) in pairs.iter() {
        // Same capacity as the accumulation buffer: cannot overflow.
        let _ = line_score.push(runs);
    }
    Ok(FinalTeam {
        abbreviation: competitor
            .abbreviation
            .expect("abbreviation was validated by parses()"),
        score,
        colors,
        line_score,
    })
}

/// The transposition heuristic (inventory §2.2): the condition is whichever
/// member does *not* parse as a number, `displayValue` winning when both
/// qualify; missing temperature drops the whole block. The quirk fires only
/// when a weather block *exists* and is unusable — an absent block is
/// silent, matching `weather.and_then(normalize_weather)`.
fn resolve_weather<Q: Quirks>(
    entered: bool,
    display: Option<Candidate>,
    condition_id: Option<Candidate>,
    temperature: Option<i16>,
    quirks: &mut Q,
) -> Option<Weather> {
    if !entered {
        return None;
    }
    let pick = |candidate: Option<Candidate>| match candidate {
        Some(Candidate {
            text,
            numeric: false,
        }) => Some(text),
        _ => None,
    };
    let condition = pick(display).or_else(|| pick(condition_id));
    match (condition, temperature) {
        (Some(condition), Some(temperature)) => Some(Weather {
            condition,
            temperature,
        }),
        _ => {
            quirks.quirk(Quirk::WeatherDropped);
            None
        }
    }
}

// ----------------------------------------------------------------- pieces

/// The backend's `parse_inning_half`, minus the warn (the caller decides
/// when the quirk fires — only where the backend would have called this).
fn parse_inning_half(short_detail: &str) -> Option<InningHalf> {
    match short_detail.split_whitespace().next().unwrap_or("") {
        "Top" => Some(InningHalf::Top),
        "Mid" => Some(InningHalf::Middle),
        "Bot" => Some(InningHalf::Bottom),
        "End" => Some(InningHalf::End),
        _ => None,
    }
}

/// Owned wire-bound copy: kept local rather than promoted because it is an
/// *output* constructor over `common::set_text` (ruling 2's truncation) —
/// the scratch strings move into the extract, which `common::WireText`
/// (borrow-only access) cannot do. Compare keys never go through this path
/// (ruling 16): the target-id and `"total"` compares run on the full
/// borrowed token at arrival.
fn etext(text: &str) -> EText {
    let mut owned = EText::new();
    set_text(&mut owned, text);
    owned
}
