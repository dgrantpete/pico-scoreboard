//! FOOTBALL extraction tables — NFL + college football (S1 sport lane).
//!
//! One const path table over the ESPN scoreboard body (`$.events[*]…`)
//! drives both entry points:
//!
//! - [`ListExtractor`] — the games list: `(game_id, state)` per displayable
//!   event plus ok/failed counts. The list is not a cheaper parse: every
//!   event runs the full deserialize-tier validation, exactly like the
//!   backend's `parse_events` (DESIGN.md rulings 1 and 13).
//! - [`DetailExtractor`] — one target game: full extraction into a
//!   [`GameExtract`] whose [`GameExtract::as_game`] is a borrowed
//!   [`scoreboard_wire::football::Game`] view, byte-identical to the
//!   backend pipeline over the fixture corpus.
//!
//! `is_college` is a call parameter of the detail extractor, never a table
//! row (ruling 8): the only NFL/NCAAF difference is the pregame rank line,
//! gated in the transform.
//!
//! Two-tier error model (ruling 1): deserialize-tier violations (missing
//! `displayClock`, a pregame without a venue, ≠2 competitors, a wrongly
//! typed leaf) mark the event failed exactly where the backend's DU
//! conversion skips it; transform-tier failures (bad color hex,
//! unparseable score/date, two `home` markers) surface as [`ExtractError`]
//! from [`DetailExtractor::finish`] — the backend adapter maps them to
//! today's 5xx.
//!
//! Skip policy: the detail extractor validates every event until the
//! target is found (the 404-vs-502 rule needs an exact failure count when
//! the target is absent — ruling 13), then fast-forwards the remainder of
//! the document with [`Directive::SkipElement`]. Skipping non-target
//! events *before* the target is found would undercount failures and turn
//! a glitched scoreboard into a spurious "game ended".
//!
//! Body-scope rejection: a `$.events` that is a scalar or null fails the
//! backend's whole-body deserialize before any event, so both extractors
//! surface [`FootballError::MalformedEvents`] instead of a clean empty
//! slate / absent target. An `events` OBJECT is the documented residue —
//! see the variant's doc.

use core::fmt::Write as _;

use crate::common::{
    EText, Exact, HomeAway, Quirk, Quirks, linescore_byte, num_i16, num_u8, num_u16,
    order_home_away, parse_hex_rgb, parse_live_phase, parse_record, parse_start_time,
    saturate_score, set_text, stable_sort_by_key, wire_phase,
};
use crate::path::{self, ContainerKind, Directive, Pattern, Seg, Sink, StreamMatcher, Value};
use scoreboard_wire::football as wire;
use scoreboard_wire::{GameState, MAX_LINE_SCORE, Record, Side, TeamColors};

use Seg::{AnyIndex, Key};

// ---------------------------------------------------------------- bounds

/// ESPN ids (event and team) are numeric strings, ≤ 10 digits in every
/// sampled league (football corpus max 9 bytes). These are compare keys,
/// never wire strings, so overflow marks the field invalid instead of
/// truncating (ruling 16, soccer's convention): a silently-truncated key
/// could false-match the possession side or serve the wrong game as the
/// detail target, while a refused key degrades safely and visibly
/// (situation dropped / target absent).
const ID_BYTES: usize = 24;

// --------------------------------------------------------------- the table
//
// Pattern indices; PATHS below is ordered identically. Paths mirror the
// backend's serde DTOs (`backend/src/football/types.rs` +
// `backend/src/espn/types.rs`): only `competitions[0]` is *extracted*, but
// every competition is *validated* (serde deserializes the whole
// `Vec<EspnCompetition>`, so a bad second competition fails the event) —
// hence `AnyIndex` at the competitions level with an index-0 guard at each
// store.

const EVENT: usize = 0;
const EVENT_ID: usize = 1;
const EVENT_DATE: usize = 2;
const WEATHER: usize = 3;
const WEATHER_DISPLAY: usize = 4;
const WEATHER_CONDITION: usize = 5;
const WEATHER_TEMP: usize = 6;
const COMPETITIONS: usize = 7;
const COMPETITION: usize = 8;
const STATE: usize = 9;
const DESCRIPTION: usize = 10;
const PERIOD: usize = 11;
const CLOCK: usize = 12;
const VENUE: usize = 13;
const VENUE_NAME: usize = 14;
const SITUATION: usize = 15;
const SIT_DOWN: usize = 16;
const SIT_DISTANCE: usize = 17;
const SIT_YARD_LINE: usize = 18;
const SIT_HOME_TIMEOUTS: usize = 19;
const SIT_AWAY_TIMEOUTS: usize = 20;
const SIT_RED_ZONE: usize = 21;
const SIT_POSSESSION: usize = 22;
const LAST_PLAY: usize = 23;
const LAST_PLAY_ID: usize = 24;
const LAST_PLAY_TEXT: usize = 25;
const COMPETITORS: usize = 26;
const COMPETITOR: usize = 27;
const HOME_AWAY: usize = 28;
const SCORE: usize = 29;
const TEAM_ID: usize = 30;
const TEAM_ABBR: usize = 31;
const TEAM_COLOR: usize = 32;
const TEAM_ALT_COLOR: usize = 33;
const TEAM_SDN: usize = 34;
const RECORDS: usize = 35;
const RECORD: usize = 36;
const RECORD_TYPE: usize = 37;
const RECORD_SUMMARY: usize = 38;
const LINESCORES: usize = 39;
const LINESCORE: usize = 40;
const LINESCORE_VALUE: usize = 41;
const LINESCORE_PERIOD: usize = 42;
const CURATED: usize = 43;
const CURATED_CURRENT: usize = 44;
/// Bare `$.events` container probe (appended late — index order is free of
/// meaning here since a length-1 pattern never shares a node with the
/// length-2+ rows): catches a scoreboard shell whose `events` is a scalar
/// or null, the backend's whole-body deserialize failure.
const EVENTS: usize = 45;
const PATTERN_COUNT: usize = 46;

static PATHS: [Pattern; PATTERN_COUNT] = [
    /* 00 EVENT            */ &[Key("events"), AnyIndex],
    /* 01 EVENT_ID         */ &[Key("events"), AnyIndex, Key("id")],
    /* 02 EVENT_DATE       */ &[Key("events"), AnyIndex, Key("date")],
    /* 03 WEATHER          */ &[Key("events"), AnyIndex, Key("weather")],
    /* 04 WEATHER_DISPLAY  */ &[Key("events"), AnyIndex, Key("weather"), Key("displayValue")],
    /* 05 WEATHER_CONDITION*/ &[Key("events"), AnyIndex, Key("weather"), Key("conditionId")],
    /* 06 WEATHER_TEMP     */ &[Key("events"), AnyIndex, Key("weather"), Key("temperature")],
    /* 07 COMPETITIONS     */ &[Key("events"), AnyIndex, Key("competitions")],
    /* 08 COMPETITION      */ &[Key("events"), AnyIndex, Key("competitions"), AnyIndex],
    /* 09 STATE            */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("status"), Key("type"), Key("state")],
    /* 10 DESCRIPTION      */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("status"), Key("type"), Key("description")],
    /* 11 PERIOD           */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("status"), Key("period")],
    /* 12 CLOCK            */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("status"), Key("displayClock")],
    /* 13 VENUE            */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("venue")],
    /* 14 VENUE_NAME       */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("venue"), Key("fullName")],
    /* 15 SITUATION        */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("situation")],
    /* 16 SIT_DOWN         */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("situation"), Key("down")],
    /* 17 SIT_DISTANCE     */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("situation"), Key("distance")],
    /* 18 SIT_YARD_LINE    */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("situation"), Key("yardLine")],
    /* 19 SIT_HOME_TIMEOUTS*/
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("situation"), Key("homeTimeouts")],
    /* 20 SIT_AWAY_TIMEOUTS*/
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("situation"), Key("awayTimeouts")],
    /* 21 SIT_RED_ZONE     */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("situation"), Key("isRedZone")],
    /* 22 SIT_POSSESSION   */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("situation"), Key("possession")],
    /* 23 LAST_PLAY        */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("situation"), Key("lastPlay")],
    /* 24 LAST_PLAY_ID     */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("situation"), Key("lastPlay"), Key("id")],
    /* 25 LAST_PLAY_TEXT   */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("situation"), Key("lastPlay"), Key("text")],
    /* 26 COMPETITORS      */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("competitors")],
    /* 27 COMPETITOR       */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("competitors"), AnyIndex],
    /* 28 HOME_AWAY        */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("competitors"), AnyIndex, Key("homeAway")],
    /* 29 SCORE            */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("competitors"), AnyIndex, Key("score")],
    /* 30 TEAM_ID          */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("competitors"), AnyIndex, Key("team"), Key("id")],
    /* 31 TEAM_ABBR        */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("competitors"), AnyIndex, Key("team"), Key("abbreviation")],
    /* 32 TEAM_COLOR       */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("competitors"), AnyIndex, Key("team"), Key("color")],
    /* 33 TEAM_ALT_COLOR   */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("competitors"), AnyIndex, Key("team"), Key("alternateColor")],
    /* 34 TEAM_SDN         */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("competitors"), AnyIndex, Key("team"), Key("shortDisplayName")],
    /* 35 RECORDS          */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("competitors"), AnyIndex, Key("records")],
    /* 36 RECORD           */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("competitors"), AnyIndex, Key("records"), AnyIndex],
    /* 37 RECORD_TYPE      */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("competitors"), AnyIndex, Key("records"), AnyIndex, Key("type")],
    /* 38 RECORD_SUMMARY   */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("competitors"), AnyIndex, Key("records"), AnyIndex, Key("summary")],
    /* 39 LINESCORES       */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("competitors"), AnyIndex, Key("linescores")],
    /* 40 LINESCORE        */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("competitors"), AnyIndex, Key("linescores"), AnyIndex],
    /* 41 LINESCORE_VALUE  */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("competitors"), AnyIndex, Key("linescores"), AnyIndex, Key("value")],
    /* 42 LINESCORE_PERIOD */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("competitors"), AnyIndex, Key("linescores"), AnyIndex, Key("period")],
    /* 43 CURATED          */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("competitors"), AnyIndex, Key("curatedRank")],
    /* 44 CURATED_CURRENT  */
    &[Key("events"), AnyIndex, Key("competitions"), AnyIndex, Key("competitors"), AnyIndex, Key("curatedRank"), Key("current")],
    /* 45 EVENTS           */ &[Key("events")],
];

// ----------------------------------------------------------- public types

/// Backend `parse_events` counters (ruling 13): `failed` drives the
/// 404-vs-502 decision when the requested game is absent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// Events that passed the deserialize-tier validation.
    pub ok: usize,
    /// Events the backend's lenient parse would have skipped with a warn.
    pub failed: usize,
}

/// Transform-tier failures — the backend adapter maps these to today's
/// 5xx (`AppError::InvalidTeamColor` / `EspnDeserialize`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractError {
    /// `order_home_away` found two homes or two aways.
    HomeAwayConflict,
    /// `team.color` / `team.alternateColor` failed `parse_hex_rgb`.
    BadColor,
    /// A live/final `score` string failed `parse::<u32>()`.
    BadScore,
    /// A pregame `event.date` failed `parse_start_time`.
    BadStartTime,
}

/// Everything `finish` can fail with.
#[derive(Debug, PartialEq)]
pub enum FootballError {
    /// Tokenizer / engine failure — the body was not well-formed JSON.
    Stream(path::Error),
    /// The scoreboard shell was malformed (`events` present but a scalar
    /// or null) — the backend's whole-body deserialize failure, a 502
    /// before any event, for BOTH detail and list (ruling 13's
    /// glitch-vs-ended rule at body scope). Residue: an `events` OBJECT
    /// is invisible at the sink API (its members arrive as unmatched
    /// keys), so `{"events":{…}}` parses like the legal empty scoreboard
    /// `{"events":[]}` — same limitation in all four lanes; flagging
    /// "no elements seen" instead would 502 every real no-games day.
    MalformedEvents,
    /// Transform-tier failure on the target event.
    Extract(ExtractError),
}

impl From<path::Error> for FootballError {
    fn from(e: path::Error) -> Self {
        FootballError::Stream(e)
    }
}

/// What the detail extraction found. The caller's mapping mirrors the
/// backend handler: `Found` → 200, `NoCompetitions` → 404 (even with
/// failures — the id *was* on the board), `Absent` → 502 when
/// `counts.failed > 0`, else 404.
// A `GameExtract` cannot be boxed (no_std, no alloc) and is moved exactly
// once, out of `finish` — the size difference is the point of the type.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum DetailOutcome {
    Found(GameExtract),
    /// The target event parsed clean but carried no competitions.
    NoCompetitions,
    /// The target id was not among the cleanly parsed events.
    Absent,
}

/// Result of a detail extraction.
#[derive(Debug)]
pub struct DetailReport<Q> {
    pub outcome: DetailOutcome,
    pub counts: Counts,
    pub quirks: Q,
}

/// Result of a list extraction.
#[derive(Debug)]
pub struct ListReport<E, Q> {
    pub entries: E,
    pub counts: Counts,
    pub quirks: Q,
}

/// Receives finalized games-list entries in event order. `game_id` borrows
/// the extractor's scratch and is only valid for the duration of the call
/// (bounded at the wire's 255-byte string cap, same truncation as encode).
pub trait ListEntries {
    fn entry(&mut self, game_id: &str, state: GameState);
}

/// The detail extractor's stand-in — detail mode emits no list entries.
#[derive(Debug, Default)]
pub struct NoEntries;

impl ListEntries for NoEntries {
    fn entry(&mut self, _game_id: &str, _state: GameState) {}
}

// --------------------------------------------------------- the extract

/// One extracted football game, post-transform (domain-shaped). Strings
/// are bound at the wire cap and truncated with the wire's own
/// `truncate_utf8` (ruling 2), so [`Self::as_game`] + encode is
/// byte-identical to the backend pipeline. Options stay `Option` all the
/// way to the encoder, which zeroes absent blocks and their flag bits.
#[derive(Debug)]
pub struct GameExtract {
    state: GameState,
    game_id: EText,
    /// Pregame only — unix epoch seconds.
    start_time: u32,
    /// Pregame only.
    venue: EText,
    /// Live `period` / final `periods_played`; parsed (and discarded) even
    /// for pregame, as the backend does.
    period: u8,
    /// Live only.
    phase: scoreboard_wire::LivePhase,
    /// Live only.
    clock: EText,
    situation: Option<wire::Situation>,
    timeouts: Option<wire::Timeouts>,
    last_play: bool,
    last_play_id: EText,
    last_play_text: EText,
    away: TeamExtract,
    home: TeamExtract,
}

#[derive(Debug, Default)]
struct TeamExtract {
    abbreviation: EText,
    /// Live/final only — pregame requires the string's presence but never
    /// parses it (backend parity: `parse_score` is not called pregame).
    score: u16,
    primary: u32,
    alternate: u32,
    record: Option<Record>,
    rank: bool,
    rank_line: EText,
    /// Sorted line-score bytes; bound = the wire's `u8`-length cap.
    line_score: heapless::Vec<u8, MAX_LINE_SCORE>,
}

impl TeamExtract {
    fn colors(&self) -> TeamColors {
        TeamColors {
            primary: self.primary,
            alternate: self.alternate,
        }
    }

    fn pregame(&self) -> wire::PregameTeam<'_> {
        wire::PregameTeam {
            abbreviation: &self.abbreviation,
            colors: self.colors(),
            record: self.record,
            rank_line: self.rank.then_some(self.rank_line.as_str()),
        }
    }

    fn live(&self) -> scoreboard_wire::TeamState<'_> {
        scoreboard_wire::TeamState {
            abbreviation: &self.abbreviation,
            score: self.score,
            colors: self.colors(),
        }
    }

    fn final_team(&self) -> scoreboard_wire::FinalTeam<'_> {
        scoreboard_wire::FinalTeam {
            abbreviation: &self.abbreviation,
            score: self.score,
            colors: self.colors(),
            line_score: &self.line_score,
        }
    }
}

impl GameExtract {
    fn new(state: GameState) -> Self {
        Self {
            state,
            game_id: EText::new(),
            start_time: 0,
            venue: EText::new(),
            period: 0,
            phase: scoreboard_wire::LivePhase::InProgress,
            clock: EText::new(),
            situation: None,
            timeouts: None,
            last_play: false,
            last_play_id: EText::new(),
            last_play_text: EText::new(),
            away: TeamExtract::default(),
            home: TeamExtract::default(),
        }
    }

    pub fn state(&self) -> GameState {
        self.state
    }

    pub fn game_id(&self) -> &str {
        &self.game_id
    }

    /// A borrowed wire-shaped view over the extract's own storage — the S3
    /// seam (DESIGN.md): encode this with `scoreboard_wire::football::encode`.
    pub fn as_game(&self) -> wire::Game<'_> {
        match self.state {
            GameState::Pregame => wire::Game::Pregame(wire::Pregame {
                game_id: &self.game_id,
                start_time: self.start_time,
                venue: &self.venue,
                away: self.away.pregame(),
                home: self.home.pregame(),
            }),
            GameState::Live => wire::Game::Live(wire::Live {
                game_id: &self.game_id,
                period: self.period,
                phase: self.phase,
                clock: &self.clock,
                away: self.away.live(),
                home: self.home.live(),
                situation: self.situation,
                timeouts: self.timeouts,
                last_play: self.last_play.then(|| scoreboard_wire::LastPlay {
                    id: &self.last_play_id,
                    text: &self.last_play_text,
                }),
            }),
            GameState::Final => wire::Game::Final(wire::Final {
                game_id: &self.game_id,
                periods_played: self.period,
                away: self.away.final_team(),
                home: self.home.final_team(),
            }),
        }
    }
}

// ------------------------------------------------------------- scratch

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum St {
    Pre,
    In,
    Post,
}

impl St {
    fn game_state(self) -> GameState {
        match self {
            St::Pre => GameState::Pregame,
            St::In => GameState::Live,
            St::Post => GameState::Final,
        }
    }
}

/// Per-competitor deserialize-tier presence bits, reset at each
/// `competitors[k]` enter. Values are stored separately (guarded to
/// competition 0, competitor < 2); these bits validate *every* competitor.
#[derive(Debug, Default)]
struct CompetitorCheck {
    home_away: bool,
    score: bool,
    team_id: bool,
    abbreviation: bool,
    color: bool,
    alternate_color: bool,
    curated_entered: bool,
    curated_current: bool,
    /// Current `records[j]` element.
    record_type: bool,
    record_summary: bool,
    record_is_total: bool,
    /// Current `linescores[j]` element.
    line_value: Option<u8>,
    line_period: Option<u8>,
}

/// Per-competition deserialize-tier state, reset at each
/// `competitions[i]` enter — validation runs for every competition, not
/// just the extracted first one.
#[derive(Debug, Default)]
struct CompCheck {
    state: Option<St>,
    period: bool,
    clock: bool,
    venue_entered: bool,
    venue_name: bool,
    competitors_entered: bool,
    competitor_count: u16,
    competitor: CompetitorCheck,
    /// Current `situation.lastPlay` element.
    last_play_id: bool,
    last_play_text: bool,
}

/// Extracted (pre-transform) values from `competitions[0]` — writes are
/// guarded to competition index 0.
#[derive(Debug)]
struct Comp0Data {
    state: Option<St>,
    period: u8,
    description: bool,
    description_text: EText,
    clock: EText,
    venue: EText,
    /// ESPN's `-1` sentinel family: absent and `-1` are the same thing.
    down: i16,
    distance: i16,
    yard_line: i16,
    home_timeouts: i16,
    away_timeouts: i16,
    red_zone: bool,
    /// Compare key (ruling 16): overflow invalidates instead of truncating.
    possession: Exact<ID_BYTES>,
    last_play: bool,
    last_play_id: EText,
    last_play_text: EText,
    competitors: [Comp0Competitor; 2],
}

impl Default for Comp0Data {
    fn default() -> Self {
        Self {
            state: None,
            period: 0,
            description: false,
            description_text: EText::new(),
            clock: EText::new(),
            venue: EText::new(),
            down: -1,
            distance: -1,
            yard_line: -1,
            home_timeouts: -1,
            away_timeouts: -1,
            red_zone: false,
            possession: Exact::default(),
            last_play: false,
            last_play_id: EText::new(),
            last_play_text: EText::new(),
            competitors: [Comp0Competitor::default(), Comp0Competitor::default()],
        }
    }
}

#[derive(Debug, Default)]
struct Comp0Competitor {
    home_away: Option<HomeAway>,
    score: EText,
    /// Compared as a *string* against `situation.possession` — never
    /// parsed, so `"012" != "12"` (backend parity). Compare key
    /// (ruling 16): overflow invalidates instead of truncating.
    team_id: Exact<ID_BYTES>,
    abbreviation: EText,
    color: EText,
    alternate_color: EText,
    short_display_name: bool,
    short_display_name_text: EText,
    /// First `records[]` entry with `type == "total"` (`.find` semantics).
    total_summary: bool,
    total_summary_text: EText,
    /// Current record element's summary, paired with its `type` at element
    /// leave — order-independent within the object (ruling 4).
    record_scratch: EText,
    /// `(clamped value, period)` pairs in arrival order; stably sorted by
    /// period at finalize. Bound = the wire's line-score cap; overflow
    /// drops the tail with a [`Quirk::ClippedLineScore`].
    linescores: heapless::Vec<(u8, u8), MAX_LINE_SCORE>,
    linescores_clipped: bool,
    rank: Option<u16>,
}

#[derive(Debug, Default)]
struct EventScratch {
    invalid: bool,
    is_target: bool,
    id_seen: bool,
    id: EText,
    date_seen: bool,
    date: EText,
    competitions_entered: bool,
    comp_count: u16,
    check: CompCheck,
    comp0: Comp0Data,
}

impl EventScratch {
    fn reset(&mut self) {
        *self = Self::default();
    }
}

// ----------------------------------------------------------------- sink

struct DetailCfg {
    /// Compare key (ruling 16): an overflowed target id matches nothing —
    /// the extractor reports `Absent` rather than serving a prefix match.
    target: Exact<ID_BYTES>,
    is_college: bool,
}

/// Internal outcome state while streaming.
// Same justification as `DetailOutcome`: no alloc, moved once.
#[allow(clippy::large_enum_variant)]
enum DState {
    Searching,
    NoCompetitions,
    Found(GameExtract),
    Failed(ExtractError),
}

struct FootballSink<E: ListEntries, Q: Quirks> {
    entries: E,
    quirks: Q,
    counts: Counts,
    /// `Some` = detail mode; `None` = list mode.
    detail: Option<DetailCfg>,
    dstate: DState,
    /// Set once the detail outcome is decided: every later callback
    /// answers `SkipElement` and nothing is counted (the counts are only
    /// consumed when the target is absent, in which case nothing was
    /// skipped).
    done: bool,
    /// `$.events` was present but a scalar/null — the whole body is
    /// unusable ([`FootballError::MalformedEvents`]), not one event.
    events_malformed: bool,
    ev: EventScratch,
}

impl<E: ListEntries, Q: Quirks> FootballSink<E, Q> {
    fn new(entries: E, quirks: Q, detail: Option<DetailCfg>) -> Self {
        Self {
            entries,
            quirks,
            counts: Counts::default(),
            detail,
            dstate: DState::Searching,
            done: false,
            events_malformed: false,
            ev: EventScratch::default(),
        }
    }

    fn invalid(&mut self) {
        self.ev.invalid = true;
    }

    /// Store guard: extraction reads only `competitions[0]`.
    fn comp0(idx: &[u16]) -> bool {
        idx[1] == 0
    }

    /// Store guard for competitor-level values: competition 0, first two
    /// competitors (a third fails validation regardless).
    fn slot(idx: &[u16]) -> Option<usize> {
        (idx[1] == 0 && idx[2] < 2).then_some(idx[2] as usize)
    }

    fn on_value(&mut self, pattern: usize, idx: &[u16], v: Value<'_>) {
        match pattern {
            // `$.events` itself a scalar/null: `RawScoreboard` fails —
            // the whole request is unusable, not one event.
            EVENTS => self.events_malformed = true,
            // A scalar where an event object belongs: the backend's
            // per-event deserialize fails, the event is skipped + counted.
            EVENT => self.counts.failed += 1,
            EVENT_ID => match v {
                Value::Str(s) => {
                    self.ev.id_seen = true;
                    set_text(&mut self.ev.id, s);
                    if let Some(cfg) = &self.detail {
                        if cfg.target.valid() == Some(s) {
                            self.ev.is_target = true;
                        }
                    }
                }
                _ => self.invalid(),
            },
            EVENT_DATE => match v {
                Value::Str(s) => {
                    self.ev.date_seen = true;
                    set_text(&mut self.ev.date, s);
                }
                _ => self.invalid(),
            },
            // `weather` is consumed by nothing, but its shape still gates
            // the event's deserialize (`Option<EspnWeather>`).
            WEATHER | LAST_PLAY | CURATED | VENUE => {
                if v != Value::Null {
                    self.invalid();
                }
            }
            WEATHER_DISPLAY | WEATHER_CONDITION => {
                if !matches!(v, Value::Str(_) | Value::Null) {
                    self.invalid();
                }
            }
            WEATHER_TEMP => match v {
                Value::Num(n) if num_i16(n).is_some() => {}
                Value::Null => {}
                _ => self.invalid(),
            },
            // Required containers: a scalar (including null) fails serde.
            COMPETITIONS | COMPETITORS | RECORDS | LINESCORES | SITUATION | COMPETITION
            | COMPETITOR | RECORD | LINESCORE => self.invalid(),
            STATE => match v {
                Value::Str("pre") => self.ev.check.state = Some(St::Pre),
                Value::Str("in") => self.ev.check.state = Some(St::In),
                Value::Str("post") => self.ev.check.state = Some(St::Post),
                _ => self.invalid(),
            },
            DESCRIPTION => match v {
                Value::Str(s) => {
                    if Self::comp0(idx) {
                        self.ev.comp0.description = true;
                        set_text(&mut self.ev.comp0.description_text, s);
                    }
                }
                Value::Null => {}
                _ => self.invalid(),
            },
            PERIOD => match v {
                Value::Num(n) => match num_u8(n) {
                    Some(p) => {
                        self.ev.check.period = true;
                        if Self::comp0(idx) {
                            self.ev.comp0.period = p;
                        }
                    }
                    None => self.invalid(),
                },
                _ => self.invalid(),
            },
            CLOCK => match v {
                Value::Str(s) => {
                    self.ev.check.clock = true;
                    if Self::comp0(idx) {
                        set_text(&mut self.ev.comp0.clock, s);
                    }
                }
                _ => self.invalid(),
            },
            VENUE_NAME => match v {
                Value::Str(s) => {
                    self.ev.check.venue_name = true;
                    if Self::comp0(idx) {
                        set_text(&mut self.ev.comp0.venue, s);
                    }
                }
                _ => self.invalid(),
            },
            SIT_DOWN | SIT_DISTANCE | SIT_YARD_LINE | SIT_HOME_TIMEOUTS | SIT_AWAY_TIMEOUTS => {
                match v {
                    Value::Num(n) => match num_i16(n) {
                        Some(value) => {
                            if Self::comp0(idx) {
                                let c = &mut self.ev.comp0;
                                match pattern {
                                    SIT_DOWN => c.down = value,
                                    SIT_DISTANCE => c.distance = value,
                                    SIT_YARD_LINE => c.yard_line = value,
                                    SIT_HOME_TIMEOUTS => c.home_timeouts = value,
                                    _ => c.away_timeouts = value,
                                }
                            }
                        }
                        None => self.invalid(),
                    },
                    _ => self.invalid(),
                }
            }
            SIT_RED_ZONE => match v {
                Value::Bool(b) => {
                    if Self::comp0(idx) {
                        self.ev.comp0.red_zone = b;
                    }
                }
                _ => self.invalid(),
            },
            SIT_POSSESSION => match v {
                Value::Str(s) => {
                    if Self::comp0(idx) {
                        self.ev.comp0.possession.set(s);
                    }
                }
                Value::Null => {}
                _ => self.invalid(),
            },
            LAST_PLAY_ID => match v {
                Value::Str(s) => {
                    self.ev.check.last_play_id = true;
                    if Self::comp0(idx) {
                        set_text(&mut self.ev.comp0.last_play_id, s);
                    }
                }
                _ => self.invalid(),
            },
            LAST_PLAY_TEXT => match v {
                Value::Str(s) => {
                    self.ev.check.last_play_text = true;
                    if Self::comp0(idx) {
                        set_text(&mut self.ev.comp0.last_play_text, s);
                    }
                }
                _ => self.invalid(),
            },
            HOME_AWAY => match v {
                Value::Str(s) => match HomeAway::parse(s) {
                    Some(marker) => {
                        self.ev.check.competitor.home_away = true;
                        if let Some(k) = Self::slot(idx) {
                            self.ev.comp0.competitors[k].home_away = Some(marker);
                        }
                    }
                    None => self.invalid(),
                },
                _ => self.invalid(),
            },
            SCORE => match v {
                Value::Str(s) => {
                    self.ev.check.competitor.score = true;
                    if let Some(k) = Self::slot(idx) {
                        set_text(&mut self.ev.comp0.competitors[k].score, s);
                    }
                }
                _ => self.invalid(),
            },
            TEAM_ID => match v {
                Value::Str(s) => {
                    self.ev.check.competitor.team_id = true;
                    if let Some(k) = Self::slot(idx) {
                        self.ev.comp0.competitors[k].team_id.set(s);
                    }
                }
                _ => self.invalid(),
            },
            TEAM_ABBR | TEAM_COLOR | TEAM_ALT_COLOR => match v {
                Value::Str(s) => {
                    let check = &mut self.ev.check.competitor;
                    match pattern {
                        TEAM_ABBR => check.abbreviation = true,
                        TEAM_COLOR => check.color = true,
                        _ => check.alternate_color = true,
                    }
                    if let Some(k) = Self::slot(idx) {
                        let c = &mut self.ev.comp0.competitors[k];
                        let dst = match pattern {
                            TEAM_ABBR => &mut c.abbreviation,
                            TEAM_COLOR => &mut c.color,
                            _ => &mut c.alternate_color,
                        };
                        set_text(dst, s);
                    }
                }
                _ => self.invalid(),
            },
            TEAM_SDN => match v {
                Value::Str(s) => {
                    if let Some(k) = Self::slot(idx) {
                        let c = &mut self.ev.comp0.competitors[k];
                        c.short_display_name = true;
                        set_text(&mut c.short_display_name_text, s);
                    }
                }
                Value::Null => {}
                _ => self.invalid(),
            },
            RECORD_TYPE => match v {
                Value::Str(s) => {
                    self.ev.check.competitor.record_type = true;
                    if s == "total" {
                        self.ev.check.competitor.record_is_total = true;
                    }
                }
                _ => self.invalid(),
            },
            RECORD_SUMMARY => match v {
                Value::Str(s) => {
                    self.ev.check.competitor.record_summary = true;
                    if let Some(k) = Self::slot(idx) {
                        set_text(&mut self.ev.comp0.competitors[k].record_scratch, s);
                    }
                }
                _ => self.invalid(),
            },
            LINESCORE_VALUE => match v {
                // `f64` in serde — every JSON number is acceptable.
                Value::Num(n) => match n.parse::<f64>() {
                    Ok(value) => {
                        self.ev.check.competitor.line_value = Some(linescore_byte(value));
                    }
                    Err(_) => self.invalid(),
                },
                _ => self.invalid(),
            },
            LINESCORE_PERIOD => match v {
                Value::Num(n) => match num_u8(n) {
                    Some(p) => self.ev.check.competitor.line_period = Some(p),
                    None => self.invalid(),
                },
                _ => self.invalid(),
            },
            CURATED_CURRENT => match v {
                Value::Num(n) => match num_u16(n) {
                    Some(rank) => {
                        self.ev.check.competitor.curated_current = true;
                        if let Some(k) = Self::slot(idx) {
                            self.ev.comp0.competitors[k].rank = Some(rank);
                        }
                    }
                    None => self.invalid(),
                },
                _ => self.invalid(),
            },
            _ => {}
        }
    }

    fn on_enter(&mut self, pattern: usize, _idx: &[u16]) {
        match pattern {
            // Container is fine — but the engine cannot tell an object
            // from an array at enter, and an object's members arrive as
            // unmatched keys, so an events OBJECT is the documented
            // residue: it scans like the legal empty scoreboard `[]`.
            EVENTS => {}
            EVENT => self.ev.reset(),
            COMPETITIONS => self.ev.competitions_entered = true,
            COMPETITION => {
                self.ev.comp_count = self.ev.comp_count.saturating_add(1);
                self.ev.check = CompCheck::default();
            }
            VENUE => self.ev.check.venue_entered = true,
            COMPETITORS => self.ev.check.competitors_entered = true,
            COMPETITOR => {
                self.ev.check.competitor_count = self.ev.check.competitor_count.saturating_add(1);
                self.ev.check.competitor = CompetitorCheck::default();
            }
            LAST_PLAY => {
                self.ev.check.last_play_id = false;
                self.ev.check.last_play_text = false;
            }
            RECORD => {
                let c = &mut self.ev.check.competitor;
                c.record_type = false;
                c.record_summary = false;
                c.record_is_total = false;
            }
            LINESCORE => {
                let c = &mut self.ev.check.competitor;
                c.line_value = None;
                c.line_period = None;
            }
            CURATED => self.ev.check.competitor.curated_entered = true,
            WEATHER | SITUATION | RECORDS | LINESCORES => {}
            // A container where a scalar belongs fails serde.
            _ => self.invalid(),
        }
    }

    fn on_leave(&mut self, pattern: usize, idx: &[u16]) -> Directive {
        match pattern {
            EVENT => return self.finish_event(),
            COMPETITION => self.finish_competition(idx),
            COMPETITOR => self.finish_competitor(idx),
            RECORD => self.finish_record(idx),
            LINESCORE => self.finish_linescore(idx),
            LAST_PLAY => {
                if !(self.ev.check.last_play_id && self.ev.check.last_play_text) {
                    self.invalid();
                } else if Self::comp0(idx) {
                    self.ev.comp0.last_play = true;
                }
            }
            _ => {}
        }
        Directive::Continue
    }

    fn finish_record(&mut self, idx: &[u16]) {
        let check = &self.ev.check.competitor;
        if !(check.record_type && check.record_summary) {
            self.invalid();
            return;
        }
        if check.record_is_total {
            if let Some(k) = Self::slot(idx) {
                let c = &mut self.ev.comp0.competitors[k];
                // `.find` semantics: the first total wins.
                if !c.total_summary {
                    c.total_summary = true;
                    let scratch = c.record_scratch.clone();
                    set_text(&mut c.total_summary_text, &scratch);
                }
            }
        }
    }

    fn finish_linescore(&mut self, idx: &[u16]) {
        let check = &self.ev.check.competitor;
        let (Some(value), Some(period)) = (check.line_value, check.line_period) else {
            self.invalid();
            return;
        };
        if let Some(k) = Self::slot(idx) {
            let c = &mut self.ev.comp0.competitors[k];
            if c.linescores.push((value, period)).is_err() && !c.linescores_clipped {
                c.linescores_clipped = true;
                self.quirks.quirk(Quirk::ClippedLineScore);
            }
        }
    }

    fn finish_competitor(&mut self, _idx: &[u16]) {
        let c = &self.ev.check.competitor;
        let complete = c.home_away
            && c.score
            && c.team_id
            && c.abbreviation
            && c.color
            && c.alternate_color
            // `curatedRank: {}` fails serde (`current` is required).
            && (!c.curated_entered || c.curated_current);
        if !complete {
            self.invalid();
        }
    }

    fn finish_competition(&mut self, idx: &[u16]) {
        let check = &self.ev.check;
        let state = check.state;
        // A venue object without `fullName` fails serde in every state.
        let venue_ok = !check.venue_entered || check.venue_name;
        // `period` and `displayClock` are required in all three states,
        // even where their values go unread (inventory §5.12).
        let mut ok = check.state.is_some()
            && check.period
            && check.clock
            && venue_ok
            && check.competitors_entered
            && check.competitor_count == 2;
        // The pregame arm additionally demands a venue (DU conversion).
        if check.state == Some(St::Pre) && !(check.venue_entered && check.venue_name) {
            ok = false;
        }
        if !ok {
            self.invalid();
        }
        if Self::comp0(idx) {
            self.ev.comp0.state = state;
        }
    }

    fn finish_event(&mut self) -> Directive {
        let valid = !self.ev.invalid
            && self.ev.id_seen
            && self.ev.date_seen
            && self.ev.competitions_entered;
        if !valid {
            self.counts.failed += 1;
            return Directive::Continue;
        }
        self.counts.ok += 1;

        match &self.detail {
            None => {
                if self.ev.comp_count > 0 {
                    if let Some(state) = self.ev.comp0.state {
                        self.entries.entry(&self.ev.id, state.game_state());
                    }
                }
                Directive::Continue
            }
            Some(cfg) => {
                if !self.ev.is_target {
                    return Directive::Continue;
                }
                self.done = true;
                self.dstate = if self.ev.comp_count == 0 {
                    DState::NoCompetitions
                } else {
                    let is_college = cfg.is_college;
                    match transform(&self.ev, is_college, &mut self.quirks) {
                        Ok(game) => DState::Found(game),
                        Err(e) => DState::Failed(e),
                    }
                };
                // Fast-forward the rest of the document; the `done` guard
                // keeps the remaining events out of the counts.
                Directive::SkipElement
            }
        }
    }
}

impl<E: ListEntries, Q: Quirks> Sink for FootballSink<E, Q> {
    fn value(&mut self, pattern: usize, indices: &[u16], value: Value<'_>) -> Directive {
        if self.done {
            return Directive::SkipElement;
        }
        self.on_value(pattern, indices, value);
        Directive::Continue
    }

    fn enter(&mut self, pattern: usize, indices: &[u16], kind: ContainerKind) -> Directive {
        if self.done {
            return Directive::SkipElement;
        }
        // `{"events":{…}}` must 502-shape like a scalar shell; only an
        // array here is a scoreboard (engine ContainerKind addition).
        if pattern == EVENTS && kind == ContainerKind::Object {
            self.events_malformed = true;
            return Directive::Continue;
        }
        self.on_enter(pattern, indices);
        Directive::Continue
    }

    fn leave(&mut self, pattern: usize, indices: &[u16]) -> Directive {
        if self.done {
            return Directive::Continue;
        }
        self.on_leave(pattern, indices)
    }
}

// -------------------------------------------------------- the transform

/// The backend's transform tier, run once over the target event at its
/// close (ruling 4: cross-field logic resolves at finalize from buffered
/// values, never from emission order). Error order mirrors the backend's:
/// pregame parses the date before ordering; live/final order first, then
/// per-team home-before-away, score-before-colors.
fn transform(
    ev: &EventScratch,
    is_college: bool,
    quirks: &mut impl Quirks,
) -> Result<GameExtract, ExtractError> {
    let d = &ev.comp0;
    // Validity guarantees both markers parsed and state present.
    let state = d.state.expect("validated competition has a state");

    let mut g = GameExtract::new(state.game_state());
    set_text(&mut g.game_id, &ev.id);
    g.period = d.period;

    if state == St::Pre {
        g.start_time = parse_start_time(&ev.date).ok_or(ExtractError::BadStartTime)?;
    }

    let marker = |k: usize| d.competitors[k].home_away.expect("validated marker");
    let (home_k, away_k) =
        order_home_away((marker(0), 0usize), (marker(1), 1usize)).ok_or(ExtractError::HomeAwayConflict)?;

    match state {
        St::Pre => {
            set_text(&mut g.venue, &d.venue);
            fill_pregame_team(&mut g.home, &d.competitors[home_k], is_college, quirks)?;
            fill_pregame_team(&mut g.away, &d.competitors[away_k], is_college, quirks)?;
        }
        St::In => {
            set_text(&mut g.clock, &d.clock);
            g.phase = wire_phase(parse_live_phase(
                d.description.then_some(d.description_text.as_str()),
                quirks,
            ));
            let home_id = d.competitors[home_k].team_id.valid();
            let away_id = d.competitors[away_k].team_id.valid();
            g.situation = validate_situation(d, home_id, away_id, quirks);
            g.timeouts = parse_timeouts(d);
            if d.last_play {
                g.last_play = true;
                set_text(&mut g.last_play_id, &d.last_play_id);
                set_text(&mut g.last_play_text, &d.last_play_text);
            }
            fill_scored_team(&mut g.home, &d.competitors[home_k])?;
            fill_scored_team(&mut g.away, &d.competitors[away_k])?;
        }
        St::Post => {
            fill_scored_team(&mut g.home, &d.competitors[home_k])?;
            fill_scored_team(&mut g.away, &d.competitors[away_k])?;
            fill_line_score(&mut g.home, &d.competitors[home_k]);
            fill_line_score(&mut g.away, &d.competitors[away_k]);
        }
    }
    Ok(g)
}

fn fill_pregame_team(
    team: &mut TeamExtract,
    c: &Comp0Competitor,
    is_college: bool,
    quirks: &mut impl Quirks,
) -> Result<(), ExtractError> {
    set_text(&mut team.abbreviation, &c.abbreviation);
    fill_colors(team, c)?;
    team.record = if c.total_summary {
        match parse_record(&c.total_summary_text) {
            Some((wins, losses)) => Some(Record { wins, losses }),
            None => {
                quirks.quirk(Quirk::MalformedRecord);
                None
            }
        }
    } else {
        None
    };
    // The four rank-line gates (backend `rank_line`): pros never rank,
    // 99 is ESPN's unranked sentinel, and an absent or empty short
    // display name leaves nothing to uppercase.
    if is_college {
        if let Some(rank) = c.rank {
            if rank != 99 && c.short_display_name && !c.short_display_name_text.is_empty() {
                team.rank = true;
                build_rank_line(&mut team.rank_line, rank, &c.short_display_name_text);
            }
        }
    }
    Ok(())
}

fn fill_scored_team(team: &mut TeamExtract, c: &Comp0Competitor) -> Result<(), ExtractError> {
    set_text(&mut team.abbreviation, &c.abbreviation);
    let score: u32 = c.score.parse().map_err(|_| ExtractError::BadScore)?;
    team.score = saturate_score(score);
    fill_colors(team, c)
}

fn fill_colors(team: &mut TeamExtract, c: &Comp0Competitor) -> Result<(), ExtractError> {
    team.primary = parse_hex_rgb(&c.color).ok_or(ExtractError::BadColor)?;
    team.alternate = parse_hex_rgb(&c.alternate_color).ok_or(ExtractError::BadColor)?;
    Ok(())
}

fn fill_line_score(team: &mut TeamExtract, c: &Comp0Competitor) {
    let mut pairs = c.linescores.clone();
    // Explicit key: sort by the ESPN period, stability keeping arrival
    // order for duplicates (ruling 13).
    stable_sort_by_key(&mut pairs, |&(_, period)| period);
    team.line_score.clear();
    for (value, _) in &pairs {
        // Capacity is identical to the source vec's, so this cannot fail.
        let _ = team.line_score.push(*value);
    }
}

/// The backend's `validate_situation`, in its exact order: down 1..=4
/// (`-1` is the ordinary between-plays sentinel and stays silent), then
/// yardLine 0..=100, then possession resolved by string comparison — home
/// first — against the two team ids. All-or-nothing; `distance` alone is
/// clamped, never validated. [`Quirk::SituationDropped`] fires exactly
/// where the backend warns: a glitched down (anything but `-1`), an
/// out-of-range yardLine, or a possession id — absent included — that
/// resolves to neither competitor. Ids are compare keys with `Exact`
/// semantics (ruling 16): an overflowed possession or team id lands in
/// the unresolvable arm rather than prefix-matching a side.
fn validate_situation(
    d: &Comp0Data,
    home_id: Option<&str>,
    away_id: Option<&str>,
    quirks: &mut impl Quirks,
) -> Option<wire::Situation> {
    if !(1..=4).contains(&d.down) {
        if d.down != -1 {
            quirks.quirk(Quirk::SituationDropped);
        }
        return None;
    }
    if !(0..=100).contains(&d.yard_line) {
        quirks.quirk(Quirk::SituationDropped);
        return None;
    }
    let possession = match d.possession.valid() {
        Some(id) if Some(id) == home_id => Side::Home,
        Some(id) if Some(id) == away_id => Side::Away,
        _ => {
            quirks.quirk(Quirk::SituationDropped);
            return None;
        }
    };
    Some(wire::Situation {
        down: d.down as u8,
        distance: d.distance.clamp(0, u8::MAX as i16) as u8,
        yard_line: d.yard_line as u8,
        possession,
        red_zone: d.red_zone,
    })
}

/// The backend's `parse_timeouts`: independent of the situation, and
/// all-or-nothing — either side negative (absent) drops both.
fn parse_timeouts(d: &Comp0Data) -> Option<wire::Timeouts> {
    if d.away_timeouts < 0 || d.home_timeouts < 0 {
        return None;
    }
    Some(wire::Timeouts {
        away: d.away_timeouts.clamp(0, u8::MAX as i16) as u8,
        home: d.home_timeouts.clamp(0, u8::MAX as i16) as u8,
    })
}

/// `format!("#{} {}", rank, name.to_uppercase())` in `no_std`: `core`'s
/// `char::to_uppercase` per char (full Unicode mapping — ruling 10;
/// `make_ascii_uppercase` would diverge on any non-ASCII name, and `ß`
/// grows to `SS`). Stops at the first char that no longer fits, which is
/// exactly the wire's truncate-at-a-char-boundary prefix of the backend's
/// unbounded string.
fn build_rank_line(dst: &mut EText, rank: u16, name: &str) {
    dst.clear();
    // "#" + at most 5 digits + " " always fits in 255.
    let _ = write!(dst, "#{rank} ");
    'chars: for c in name.chars() {
        for upper in c.to_uppercase() {
            if dst.push(upper).is_err() {
                break 'chars;
            }
        }
    }
}

// -------------------------------------------------------- entry points

/// Streaming extractor for `GET /football/{league}/games/{game_id}`:
/// feed the scoreboard body in chunks, finish into a [`DetailReport`].
pub struct DetailExtractor<'s, Q: Quirks> {
    matcher: StreamMatcher<'static, 's, FootballSink<NoEntries, Q>>,
}

impl<'s, Q: Quirks> DetailExtractor<'s, Q> {
    /// `scratch` must hold the longest contiguous string/number token in
    /// the body (see [`StreamMatcher::new`]). `game_id` is a compare key
    /// (ruling 16): past [`ID_BYTES`] it diverges from the backend's
    /// unbounded compare by *refusing* — it matches nothing and the
    /// extraction reports `Absent` — never by prefix-matching a different
    /// game (no real ESPN id approaches the bound).
    pub fn new(
        game_id: &str,
        is_college: bool,
        quirks: Q,
        scratch: &'s mut [u8],
    ) -> Result<Self, path::Error> {
        let mut target = Exact::default();
        target.set(game_id);
        let sink = FootballSink::new(
            NoEntries,
            quirks,
            Some(DetailCfg { target, is_college }),
        );
        Ok(Self {
            matcher: StreamMatcher::new(&PATHS, sink, scratch)?,
        })
    }

    pub fn write(&mut self, chunk: &[u8]) -> Result<(), path::Error> {
        self.matcher.write(chunk)
    }

    pub fn finish(self) -> Result<DetailReport<Q>, FootballError> {
        let sink = self.matcher.finish()?;
        if sink.events_malformed {
            return Err(FootballError::MalformedEvents);
        }
        let outcome = match sink.dstate {
            DState::Found(game) => DetailOutcome::Found(game),
            DState::NoCompetitions => DetailOutcome::NoCompetitions,
            DState::Searching => DetailOutcome::Absent,
            DState::Failed(e) => return Err(FootballError::Extract(e)),
        };
        Ok(DetailReport {
            outcome,
            counts: sink.counts,
            quirks: sink.quirks,
        })
    }
}

/// Streaming extractor for `GET /football/{league}/games`: entries are
/// delivered to `E` in event order; the counts carry the backend's
/// lenient-parse ok/failed tally.
pub struct ListExtractor<'s, E: ListEntries, Q: Quirks> {
    matcher: StreamMatcher<'static, 's, FootballSink<E, Q>>,
}

impl<'s, E: ListEntries, Q: Quirks> ListExtractor<'s, E, Q> {
    pub fn new(entries: E, quirks: Q, scratch: &'s mut [u8]) -> Result<Self, path::Error> {
        let sink = FootballSink::new(entries, quirks, None);
        Ok(Self {
            matcher: StreamMatcher::new(&PATHS, sink, scratch)?,
        })
    }

    pub fn write(&mut self, chunk: &[u8]) -> Result<(), path::Error> {
        self.matcher.write(chunk)
    }

    pub fn finish(self) -> Result<ListReport<E, Q>, FootballError> {
        let sink = self.matcher.finish()?;
        if sink.events_malformed {
            return Err(FootballError::MalformedEvents);
        }
        Ok(ListReport {
            entries: sink.entries,
            counts: sink.counts,
            quirks: sink.quirks,
        })
    }
}
