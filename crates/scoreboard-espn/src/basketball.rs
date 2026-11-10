//! NBA extraction tables — the S1 sport lane over the shared path engine,
//! grown for the court screens (`firmware-rs/NBA.md` §3 is the normative
//! semantics of everything the `ext=2` groups carry).
//!
//! Two streamed documents feed one payload. The league SCOREBOARD body
//! (`$.events[*]...`, [`PATHS`] + the [`Extractor`] sink) carries the state,
//! the clock, the scores, the leaders, the records and the series; the
//! per-event SUMMARY body ([`SummaryExtractor`], fetched for live games only —
//! D3) carries what the scoreboard cannot: the ledger, the makes, the run and
//! the newest score. Both produce owned extracts and [`Extract::as_game`]
//! combines them into the wire view, degrading to the scoreboard alone when
//! there is no summary. The summary extractor takes a [`SummaryContext`] from
//! the scoreboard extract: a final's or a pregame's body is never read for
//! live facts (nothing on those screens comes from the summary), so the
//! context's state gates the whole pass.
//!
//! The scoreboard half keeps the backend's serde-era semantics, with their
//! rulings:
//!
//! - **Rejection parity (ruling 1).** Event finalize validates the same
//!   required set the backend's serde + DU conversion does, per state: a
//!   pregame event missing `status.displayClock` is dropped even though
//!   nothing reads it. Deserialize-tier failures skip the event and count
//!   in [`ScanStats::failed`]; transform-tier failures (bad hex color,
//!   unparseable score/date, two home markers) are [`DetailOutcome::Rejected`]
//!   — the games list still serves such an event, exactly like the backend,
//!   whose list never runs the transform. The fields added for the court
//!   screens (`season.type`, `status.type.{name,altDetail}`, `series`,
//!   `notes`, `leaders`, `lastPlay.type.id`) have no serde predecessor and
//!   only decorate, so like the betting lines (ruling 17) they never fail an
//!   event: a wrong type reads as absent.
//! - **Field-order independence (ruling 4).** Every matched value is
//!   buffered in a per-event scratch and resolved only when the event
//!   element closes; home/away resolves from the buffered markers via
//!   `order_home_away`, never from array position (every fixture puts
//!   home at index 0 — a trap, not a rule).
//! - **Clock is verbatim.** The colonless sub-minute string is the
//!   firmware's crunch-time signal; no reformatting anywhere.
//! - **Line scores** are stable-sorted by `period` (ruling 13: equal
//!   periods keep arrival order) and clamped per entry with
//!   `linescore_byte`; period gaps are not filled (ruling 7).
//! - **Ok/failed counts (ruling 13)** feed the caller's 404-vs-502 rule: a
//!   glitched scoreboard must never masquerade as "game ended". Detail mode
//!   validates every event until the target is found and skips only after
//!   (ruling 14), so a missing target always comes with exact counts.
//!
//! # The flash (§3.7) and the gift fleet
//!
//! The v2 `last_play` field carries §3.7's filtered play: the scoreboard's
//! `lastPlay` when its type is eligible (it is the newest play there is),
//! else the summary's newest eligible play, else none. Eligible are the
//! stoppages and the news — a full timeout, a challenge or review outcome,
//! a technical or flagrant foul ([`FLASH_TYPES`]); made baskets, free throws,
//! rebounds, substitutions, ordinary fouls and period boundaries are not, nor
//! are the transient first postings of a challenge (213, 277) and the
//! placeholder types (91 "No Shot", 61 "No Turnover"), which ESPN re-types in
//! place under the same id — the re-typed play flashes then, once, if it is
//! eligible. The gift fleet decodes the same field, so its NBA flash follows
//! this filter too — fewer flashes on frozen firmware, not a break.
//! [`Live::last_play`] keeps the unfiltered play for the backend's JSON DTO
//! until phase 6.3 moves that to the filtered one.
//!
//! # The summary tables
//!
//! `header.competitions[0].competitors[*]`: `team.id`, `homeAway`,
//! `fouls.{teamFoulsCurrent,bonusState,foulsToGive}`, `timeoutsRemaining` —
//! the ledger (§3.3) and the one team-id → side mapping every play and
//! boxscore reference resolves through, at finish, whichever order the
//! document arrived in. `plays[*]`: `id`, `sequenceNumber`, `type.id`,
//! `text`, `scoringPlay`, `scoreValue`, `pointsAttempted`, `coordinate.{x,y}`,
//! `team.id`, `awayScore`, `homeScore`, `participants[0].athlete.id` — the
//! run walk (§3.5), the make ring (§3.4), the newest scoring play (§3.6) and
//! the flash candidate (§3.7), each folded as the play closes and never
//! buffered whole. `boxscore.players[*]`: `team.id`, `statistics[0].names[*]`
//! / `keys[*]` (the PTS column, found by name, never by position: the names
//! may stream after the athletes), `statistics[0].athletes[*].{athlete.id,
//! stats[*]}` — the newest scorer's game points (§3.6).
//!
//! # Bounds
//!
//! Every pool is sized from the game and overflow degrades rather than
//! panics: the make ring keeps the newest [`MAX_MAKES`] (the wire's cap, a
//! bounded ring, never every play); each team's boxscore rows fill
//! [`ATHLETES_PER_TEAM`], a nineteenth is a [`Quirk::BoundedOverflow`] and
//! unlisted (points 0); a row keeps [`STAT_COLUMNS`] numeric cells, and a PTS
//! column past them is the same quirk; a third header competitor or player
//! block is a quirk and ignored; the flash text, the scorer's family name,
//! the headline, the series and every other string live at the wire's
//! [`MAX_STRING_BYTES`](scoreboard_wire::MAX_STRING_BYTES) (ruling 2), the
//! compare keys at [`ID_BYTES`] and refuse to truncate (ruling 16).

use crate::common::{
    linescore_byte, num_i16, num_u16, num_u32, num_u8, order_home_away, parse_hex_rgb,
    parse_live_phase, parse_record, parse_start_time, saturate_score, set_text, stable_sort_by_key,
    wire_phase, EText, Exact, HomeAway, LivePhase, Market, OddsScratch, Quirk, Quirks, WireText,
};
use crate::path::{ContainerKind, Directive, Error, Pattern, Seg, Sink, StreamMatcher, Value};
use scoreboard_wire::basketball::{
    Ledger, Make, Records, Run, TeamLedger, MAX_COORDINATE_X, MAX_COORDINATE_Y, MAX_MAKES,
    MIN_COORDINATE_Y, RUN_SHOW,
};
use scoreboard_wire::{self as wire, GameState, Record, Side, TeamColors, MAX_LINE_SCORE};

use Seg::{AnyIndex, Index, Key};

// ---------------------------------------------------------------- bounds

/// ESPN ids are numeric strings: event ids run 9 digits ("401909938"), team
/// ids up to 6, athlete ids up to 7 ("5175737"). As compare keys they refuse
/// to truncate (ruling 16) and an overlong one matches nothing.
pub const ID_BYTES: usize = 24;
/// The longest known `status.type.name` is 18 bytes (STATUS_IN_PROGRESS);
/// an overlong one is an unknown name.
const STATUS_NAME_BYTES: usize = 32;
/// `altDetail` is "OT" or "{n}OT"; anything longer cannot match.
const ALT_DETAIL_BYTES: usize = 8;
/// Boxscore rows per team: fifteen standard contracts and three two-way
/// players are the most a roster can dress. The boxscore lists 11–15.
pub const ATHLETES_PER_TEAM: usize = 18;
/// Numeric cells kept per boxscore row: ESPN's NBA row has fourteen columns
/// (MIN, PTS, FG, …, +/-); PTS is the second.
pub const STAT_COLUMNS: usize = 16;
/// Two teams play. Pools keyed by team hold this many and quirk on a third.
pub const TEAMS: usize = 2;

// ------------------------------------------------------------ ESPN codes

/// `season.type`: 2 is the regular season (preseason and the NBA Cup too), 3
/// the playoffs, 5 the play-in — which is not the playoffs (§3.1).
const SEASON_POSTSEASON: u8 = 3;
/// `status.period` of a regulation final; 5 is the first overtime.
const REGULATION_PERIODS: u8 = 4;
/// A make is a scoring play worth a field goal; free throws attempt 0.
const FIELD_GOAL_ATTEMPT: u8 = 2;
/// ESPN's half-court frame runs 0–50 ft sideline to sideline, with the
/// basket at x = 25 (§2): feet from the centre line is `x − 25`.
const COURT_CENTRE_X: i32 = 25;
/// A coordinate this far outside the court is no coordinate: ESPN's sentinel
/// for "none" is about −2 × 10⁸ on both axes, while real values run 0–50
/// across and −8 to about 70 along (heaves). Anything inside the window is
/// then pulled back to the wire's ranges.
const COORDINATE_WINDOW: core::ops::RangeInclusive<f64> = -100.0..=200.0;
/// Trailing generational suffixes the family-name rule drops (§3.6).
const NAME_SUFFIXES: [&str; 5] = ["Jr.", "Sr.", "II", "III", "IV"];

/// §3.7's eligible play types, pinned from every type in the capture set
/// (`backend/testdata/basketball/nba/PROVENANCE.md`): the full timeout (16), the
/// coach's-challenge outcomes (214 supported, 215 overturned, 216 stands),
/// the ref-initiated review outcomes (278/279/280 likewise), the technicals
/// (29 defensive three seconds, 30 double, 35 technical, 48 flopping) and
/// the flagrant type 1 (32; no type 2 was ever seen). No ejection type
/// exists to add. The one table both documents' plays are judged by.
pub const FLASH_TYPES: [u16; 12] = [16, 214, 215, 216, 278, 279, 280, 29, 30, 35, 48, 32];

/// The three leader categories a final shows, in the wire's row order.
const CATEGORIES: usize = 3;

// ---------------------------------------------------------------- the table

// Pattern indices. Each constant names the row of [`PATHS`] at the same
// position; the sink dispatches on these.
const P_EVENTS: usize = 0;
const P_EVENT: usize = 1;
const P_ID: usize = 2;
const P_DATE: usize = 3;
const P_WEATHER: usize = 4;
const P_WEATHER_DISPLAY: usize = 5;
const P_WEATHER_CONDITION: usize = 6;
const P_WEATHER_TEMP: usize = 7;
const P_COMPETITIONS: usize = 8;
const P_COMPETITION0: usize = 9;
const P_STATE: usize = 10;
const P_DESCRIPTION: usize = 11;
const P_PERIOD: usize = 12;
const P_CLOCK: usize = 13;
const P_VENUE: usize = 14;
const P_VENUE_NAME: usize = 15;
const P_SITUATION: usize = 16;
const P_LAST_PLAY: usize = 17;
const P_LAST_PLAY_ID: usize = 18;
const P_LAST_PLAY_TEXT: usize = 19;
const P_COMPETITOR: usize = 20;
const P_HOME_AWAY: usize = 21;
const P_SCORE: usize = 22;
const P_TEAM_ID: usize = 23;
const P_TEAM_ABBR: usize = 24;
const P_TEAM_COLOR: usize = 25;
const P_TEAM_ALT_COLOR: usize = 26;
const P_TEAM_SHORT_NAME: usize = 27;
const P_RECORDS: usize = 28;
const P_RECORD: usize = 29;
const P_RECORD_TYPE: usize = 30;
const P_RECORD_SUMMARY: usize = 31;
const P_LINESCORES: usize = 32;
const P_LINESCORE: usize = 33;
const P_LINESCORE_VALUE: usize = 34;
const P_LINESCORE_PERIOD: usize = 35;
/// The first sportsbook's closing lines (`common::OddsScratch`), appended
/// rather than filed by subject so the indices above keep matching the
/// inventory they were derived from: the pregame team lines, served as the
/// wire's `ext=1`.
const P_ODDS_SPREAD_AWAY: usize = 36;
const P_ODDS_SPREAD_HOME: usize = 37;
const P_ODDS_MONEYLINE_AWAY: usize = 38;
const P_ODDS_MONEYLINE_HOME: usize = 39;
/// The court screens' fields (NBA.md §3.1, §3.7–3.9), appended likewise.
/// None of them can fail an event.
const P_SEASON_TYPE: usize = 40;
const P_STATUS_NAME: usize = 41;
const P_ALT_DETAIL: usize = 42;
const P_SERIES: usize = 43;
const P_HEADLINE: usize = 44;
const P_LAST_PLAY_TYPE: usize = 45;
const P_LEADER: usize = 46;
const P_LEADER_NAME: usize = 47;
const P_LEADER_VALUE: usize = 48;
const P_LEADER_DISPLAY: usize = 49;
const P_LEADER_ATHLETE: usize = 50;

/// The NBA scoreboard path table. 18 wire-relevant leaves plus the
/// parse-gating-only fields (`team.id`, `weather.*`, `shortDisplayName`) the
/// backend's shared serde structs require to deserialize — rejection parity
/// needs them even though nothing reads their values (ruling 1, inventory §6)
/// — plus the betting lines and the court screens' fields.
///
/// Only `competitions[0]` is consulted, as in the backend
/// (`fetch_game_parts` takes `competitions.into_iter().next()`).
#[rustfmt::skip]
pub static PATHS: &[Pattern] = &[
    // P_EVENTS — a scalar here means the scoreboard shell is malformed
    // (the backend fails the whole response before parse_events).
    &[Key("events")],
    // P_EVENT — one event; enter/leave are the element boundary.
    &[Key("events"), AnyIndex],
    &[Key("events"), AnyIndex, Key("id")],
    &[Key("events"), AnyIndex, Key("date")],
    &[Key("events"), AnyIndex, Key("weather")],
    &[Key("events"), AnyIndex, Key("weather"), Key("displayValue")],
    &[Key("events"), AnyIndex, Key("weather"), Key("conditionId")],
    &[Key("events"), AnyIndex, Key("weather"), Key("temperature")],
    &[Key("events"), AnyIndex, Key("competitions")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0)],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("status"), Key("type"), Key("state")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("status"), Key("type"), Key("description")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("status"), Key("period")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("status"), Key("displayClock")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("venue")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("venue"), Key("fullName")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("situation")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("situation"), Key("lastPlay")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("situation"), Key("lastPlay"), Key("id")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("situation"), Key("lastPlay"), Key("text")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("homeAway")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("score")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("team"), Key("id")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("team"), Key("abbreviation")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("team"), Key("color")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("team"), Key("alternateColor")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("team"), Key("shortDisplayName")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("records")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("records"), AnyIndex],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("records"), AnyIndex, Key("type")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("records"), AnyIndex, Key("summary")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("linescores")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("linescores"), AnyIndex],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("linescores"), AnyIndex, Key("value")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("linescores"), AnyIndex, Key("period")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("odds"), Index(0), Key("pointSpread"), Key("away"), Key("close"), Key("line")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("odds"), Index(0), Key("pointSpread"), Key("home"), Key("close"), Key("line")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("odds"), Index(0), Key("moneyline"), Key("away"), Key("close"), Key("odds")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("odds"), Index(0), Key("moneyline"), Key("home"), Key("close"), Key("odds")],
    &[Key("events"), AnyIndex, Key("season"), Key("type")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("status"), Key("type"), Key("name")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("status"), Key("type"), Key("altDetail")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("series"), Key("summary")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("notes"), Index(0), Key("headline")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("situation"), Key("lastPlay"), Key("type"), Key("id")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("leaders"), AnyIndex],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("leaders"), AnyIndex, Key("name")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("leaders"), AnyIndex, Key("leaders"), Index(0), Key("value")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("leaders"), AnyIndex, Key("leaders"), Index(0), Key("displayValue")],
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("leaders"), AnyIndex, Key("leaders"), Index(0), Key("athlete"), Key("shortName")],
];

// ------------------------------------------------------------- the extract

/// One extracted NBA game: owned, bounded, domain-shaped. String bounds are
/// the wire cap ([`EText`] = 255 bytes, ruling 2), scores stay `u32` like
/// the backend's domain model (the wire view saturates), line scores are
/// already sorted/clamped bytes like `BasketballFinalTeam::line_score`.
#[derive(Debug, Clone, PartialEq)]
pub struct Extract {
    pub game_id: EText,
    /// `season.type` 3: the series line is served and the pregame's second
    /// line carries it (§3.1). The play-in (5) is not the playoffs.
    pub playoffs: bool,
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq)]
#[expect(
    clippy::large_enum_variant,
    reason = "the final carries three leader names; boxing needs alloc and one extract exists per polled game"
)]
pub enum Kind {
    Pregame(Pregame),
    Live(Live),
    Final(Final),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pregame {
    /// Unix epoch seconds, UTC (`parse_start_time`).
    pub start_time: u32,
    pub venue: EText,
    /// The info cycle's second line (§3.8): `notes[0].headline` upper-cased,
    /// plus " - " and `series.summary` in the playoffs; empty when there is
    /// no headline (the shared cycle skips empty lines).
    pub info_secondary: EText,
    pub away: PregameTeam,
    pub home: PregameTeam,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PregameTeam {
    pub abbreviation: EText,
    pub colors: TeamColors,
    /// `None` when ESPN omits or malforms the `type == "total"` record —
    /// a cleared flag bit on the wire, not a zero record.
    pub record: Option<Record>,
    /// The betting line, "+3.5  +140" — both teams or neither
    /// (`OddsScratch::team_lines`). Carried by the wire's `ext=1` only.
    pub line: Option<EText>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Live {
    pub period: u8,
    pub phase: LivePhase,
    /// ESPN's `displayClock`, verbatim — the colonless sub-minute shape is
    /// the device's crunch-time signal and must never be reformatted.
    pub clock: EText,
    pub away: LiveTeam,
    pub home: LiveTeam,
    /// The scoreboard's contribution to the flash: `situation.lastPlay` when
    /// its `type.id` is one of [`FLASH_TYPES`] (§3.7), else `None` — a
    /// rebound or a made basket is not a play to flash, and every consumer
    /// (the wire view and the backend's JSON DTO alike) sees the one filtered
    /// play. Also absent before the opening tip (`situation: {}` is a real
    /// capture).
    pub last_play: Option<LastPlay>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LiveTeam {
    pub abbreviation: EText,
    /// Parsed from ESPN's string score; saturated to `u16` only at the
    /// wire view, like the backend's `saturate_score`.
    pub score: u32,
    pub colors: TeamColors,
}

/// A flash line: the play id the device compares between polls, and its
/// text as ESPN wrote it.
#[derive(Debug, Clone, PartialEq)]
pub struct LastPlay {
    pub id: EText,
    pub text: EText,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Final {
    /// `status.period`: 4 in regulation, more with overtime.
    pub periods_played: u8,
    pub away: FinalTeam,
    pub home: FinalTeam,
    /// The game's leaders in points, rebounds and assists (§3.9): all three
    /// or none, each the higher of the two competitors' with a tie to the
    /// away side.
    pub leaders: Option<Leaders>,
    /// `series.summary`, playoffs only.
    pub series: Option<EText>,
    /// Both sides' `records[type=total]`, together or not at all; a
    /// malformed one is a quirk and drops the pair.
    pub records: Option<Records>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FinalTeam {
    pub abbreviation: EText,
    pub score: u32,
    pub colors: TeamColors,
    /// Sorted by period, clamped to bytes. Capacity is the wire's own
    /// `MAX_LINE_SCORE` (255) so the extract is never the tighter bound.
    pub line_score: heapless::Vec<u8, MAX_LINE_SCORE>,
}

/// The final screen's three rows (§3.9).
#[derive(Debug, Clone, PartialEq)]
pub struct Leaders {
    pub points: Leader,
    pub rebounds: Leader,
    pub assists: Leader,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Leader {
    /// The competitor whose `leaders[]` entry won the category.
    pub side: Side,
    /// `leaders[0].value` saturated to a byte, else `displayValue`.
    pub value: u8,
    /// `leaders[0].athlete.shortName`, "J. Brunson".
    pub name: EText,
}

/// What the summary pass contributes to a live payload (§3.3–3.7), already
/// resolved to sides. Combined with the scoreboard extract by
/// [`Extract::as_game`]; empty when the context was not a live game.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    /// §3.3; `None` when a field was missing on either side or the body was
    /// the one-poll glitch.
    pub ledger: Option<Ledger>,
    /// §3.5; `None` under [`RUN_SHOW`] points.
    pub run: Option<Run>,
    /// The newest [`MAX_MAKES`] made field goals with a mark, oldest first,
    /// §3.4's rotation applied.
    pub makes: heapless::Vec<Make, MAX_MAKES>,
    /// §3.6; `None` when no scoring play with a team was seen.
    pub newest_score: Option<NewestScore>,
    /// The newest eligible play (§3.7), the flash when the scoreboard's own
    /// `lastPlay` is not.
    pub last_play: Option<LastPlay>,
}

impl Summary {
    /// What a context that is not a live game yields: nothing.
    const NONE: Self = Self {
        ledger: None,
        run: None,
        makes: heapless::Vec::new(),
        newest_score: None,
        last_play: None,
    };
}

/// The newest scoring play, free throws included (§3.6).
#[derive(Debug, Clone, PartialEq)]
pub struct NewestScore {
    pub side: Side,
    /// The scorer's PTS from the boxscore; 0 when the player is not listed
    /// yet (the caption then shows the name alone).
    pub scorer_points: u8,
    /// `sequenceNumber`: the news-window key the store compares between
    /// commits.
    pub seq: u32,
    /// The play is also the newest make, so the blink has a mark to blink.
    pub marked: bool,
    /// The family name, upper-cased ("OUBRE"); empty when the text named no
    /// one — no " makes ", or nothing before it.
    pub scorer: EText,
}

/// What the summary extractor needs to know before it reads a byte. Built
/// by [`Extract::summary_context`] from the scoreboard pass, which the
/// backend runs first anyway; it has no other constructor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryContext {
    /// The scoreboard's state. Only a live game has a ledger, a run, marks
    /// to draw and a caption; a final's or a pregame's body is not read.
    state: GameState,
}

impl Extract {
    pub fn state(&self) -> GameState {
        match self.kind {
            Kind::Pregame(_) => GameState::Pregame,
            Kind::Live(_) => GameState::Live,
            Kind::Final(_) => GameState::Final,
        }
    }

    /// What [`SummaryExtractor::new`] needs — the only way to build one.
    pub fn summary_context(&self) -> SummaryContext {
        SummaryContext {
            state: self.state(),
        }
    }

    /// A borrowed wire-shaped view over the extract's own storage — and the
    /// summary's, when the backend has one. Without it the view is the
    /// scoreboard alone (D3's degrade): no ledger, run or newest score, no
    /// makes, and the flash is the scoreboard's `lastPlay` filtered by §3.7.
    pub fn as_game<'a>(&'a self, summary: Option<&'a Summary>) -> wire::basketball::Game<'a> {
        match &self.kind {
            Kind::Pregame(game) => wire::basketball::Game::Pregame(wire::basketball::Pregame {
                game_id: self.game_id.as_str(),
                start_time: game.start_time,
                venue: game.venue.as_str(),
                info_secondary: game.info_secondary.as_str(),
                away: wire_pregame_team(&game.away),
                home: wire_pregame_team(&game.home),
            }),
            Kind::Live(game) => wire::basketball::Game::Live(wire::basketball::Live {
                game_id: self.game_id.as_str(),
                period: game.period,
                phase: wire_phase(game.phase),
                clock: game.clock.as_str(),
                away: wire_live_team(&game.away),
                home: wire_live_team(&game.home),
                // §3.7: the scoreboard's play when it is itself eligible —
                // the newest play there is — else the summary's newest
                // eligible one. The two name a play by the same id, so the
                // device's change detection cannot double-flash. Trailing
                // whitespace goes: the defensive three-seconds technical's
                // text ends in a newline (PROVENANCE), which no font draws.
                last_play: game
                    .last_play
                    .as_ref()
                    .or_else(|| summary.and_then(|s| s.last_play.as_ref()))
                    .map(|play| wire::LastPlay {
                        id: play.id.as_str(),
                        text: play.text.as_str().trim_end(),
                    }),
                ledger: summary.and_then(|s| s.ledger),
                run: summary.and_then(|s| s.run),
                newest_score: summary.and_then(|s| s.newest_score.as_ref()).map(|score| {
                    wire::basketball::NewestScore {
                        side: score.side,
                        scorer_points: score.scorer_points,
                        seq: score.seq,
                        marked: score.marked,
                        scorer: score.scorer.as_str(),
                    }
                }),
                makes: wire::basketball::Makes::new(summary.map_or(&[], |s| s.makes.as_slice())),
            }),
            Kind::Final(game) => wire::basketball::Game::Final(wire::basketball::Final {
                game_id: self.game_id.as_str(),
                periods_played: game.periods_played,
                away: wire_final_team(&game.away),
                home: wire_final_team(&game.home),
                leaders: game
                    .leaders
                    .as_ref()
                    .map(|leaders| wire::basketball::Leaders {
                        points: wire_leader(&leaders.points),
                        rebounds: wire_leader(&leaders.rebounds),
                        assists: wire_leader(&leaders.assists),
                    }),
                series: game.series.as_deref(),
                records: game.records,
            }),
        }
    }
}

fn wire_pregame_team(team: &PregameTeam) -> wire::basketball::PregameTeam<'_> {
    wire::basketball::PregameTeam {
        abbreviation: team.abbreviation.as_str(),
        colors: team.colors,
        record: team.record,
        line: team.line.as_deref(),
    }
}

fn wire_live_team(team: &LiveTeam) -> wire::TeamState<'_> {
    wire::TeamState {
        abbreviation: team.abbreviation.as_str(),
        score: saturate_score(team.score),
        colors: team.colors,
    }
}

fn wire_final_team(team: &FinalTeam) -> wire::FinalTeam<'_> {
    wire::FinalTeam {
        abbreviation: team.abbreviation.as_str(),
        score: saturate_score(team.score),
        colors: team.colors,
        line_score: &team.line_score,
    }
}

fn wire_leader(leader: &Leader) -> wire::basketball::Leader<'_> {
    wire::basketball::Leader {
        side: leader.side,
        value: leader.value,
        name: leader.name.as_str(),
    }
}

// ------------------------------------------------------------ the outcomes

/// Ok/failed event tallies (ruling 13). `failed` mirrors the backend's
/// `parse_events` count: the caller's 404-vs-502 rule needs it. Detail mode
/// validates every event until the target is found (ruling 14), so a missed
/// target means nothing was ever skipped and the counts are exact — only
/// post-target events go uncounted, when the verdict is already `Found`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanStats {
    /// Events that deserialized clean (including ones with an empty
    /// `competitions` array, which list nothing — like the backend).
    pub ok: u32,
    /// Events dropped by the required-field rules (deserialize tier).
    pub failed: u32,
    /// `$.events` was present but not a container — the backend fails the
    /// whole scoreboard response before per-event parsing.
    pub events_malformed: bool,
}

/// A transform-tier failure on the requested game: the backend's hard 5xx,
/// never a skip (inventory §5's two-tier error model).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransformError {
    /// `parse_hex_rgb` failed on a team color.
    Color,
    /// A competitor `score` string did not parse as `u32`.
    Score,
    /// `event.date` did not parse (pregame reads it).
    StartTime,
    /// Two home or two away markers.
    HomeAway,
}

/// What a game-detail run found.
#[expect(
    clippy::large_enum_variant,
    reason = "Found carries the whole bounded extract; boxing needs alloc"
)]
#[derive(Debug)]
pub enum DetailOutcome {
    /// The target deserialized and transformed; encode `as_game()` for the
    /// backend's exact bytes.
    Found(Extract),
    /// The target event exists but a transform-tier field is bad (5xx).
    Rejected(TransformError),
    /// The target event exists with an empty `competitions` array — the
    /// backend serves 404 for it regardless of the failure count.
    NoCompetition,
    /// The target id never matched a clean event. Every event was
    /// validated on the way here (ruling 14), so the verdict is exactly
    /// the backend's `find_event`: 404 when [`ScanStats::failed`] is 0,
    /// the glitched-scoreboard upstream error (502) otherwise.
    NotFound,
}

// ------------------------------------------------------------- the scratch

/// Which wire state an event's competition claims (`status.type.state`,
/// strictly `"pre"` / `"in"` / `"post"` — anything else fails the event).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StateKind {
    Pre,
    In,
    Post,
}

/// Per-event buffer. Everything is stored as it streams past and resolved
/// at the event's `leave` — no field-order assumptions (ruling 4). The
/// `*_seen` / `*_bad` pairs reproduce serde's required/typed field rules:
/// `seen` false at finalize is a missing required field, `bad` true is a
/// present-but-wrong-type one; both drop the event.
#[derive(Default)]
struct EventScratch {
    /// Detail mode fast-forwarded past this event; ignore it entirely.
    skipped: bool,
    /// Detail mode: the (untruncated) id matched the target.
    is_target: bool,
    id: EText,
    id_seen: bool,
    id_bad: bool,
    date_seen: bool,
    date_bad: bool,
    /// Parsed eagerly (the parse is pure); only the pregame arm reads it.
    start_time: Option<u32>,
    /// Any type violation inside the (NBA-unused) weather block: the shared
    /// event shell still fails to deserialize on it (inventory §6).
    weather_bad: bool,
    competitions_seen: bool,
    competitions_bad: bool,
    competition_seen: bool,
    competition_bad: bool,
    state: Option<StateKind>,
    state_bad: bool,
    desc: EText,
    desc_seen: bool,
    desc_bad: bool,
    period: u8,
    period_seen: bool,
    period_bad: bool,
    clock: EText,
    clock_seen: bool,
    clock_bad: bool,
    venue_seen: bool,
    venue_bad: bool,
    venue_name: EText,
    venue_name_seen: bool,
    venue_name_bad: bool,
    situation_seen: bool,
    situation_bad: bool,
    last_play_seen: bool,
    last_play_bad: bool,
    play_id: EText,
    play_id_seen: bool,
    play_id_bad: bool,
    play_text: EText,
    play_text_seen: bool,
    play_text_bad: bool,
    /// `lastPlay.type.id`, §3.7's filter input; absent flashes nothing.
    play_type: Option<u16>,
    season_type: Option<u8>,
    status_name: Exact<STATUS_NAME_BYTES>,
    alt_detail: Exact<ALT_DETAIL_BYTES>,
    series: WireText,
    headline: WireText,
    competitor_count: u16,
    competitor_scalar: bool,
    competitors: [CompetitorScratch; TEAMS],
    entry: EntryScratch,
    leader: LeaderEntry,
    odds: OddsScratch,
}

#[derive(Default)]
struct CompetitorScratch {
    home_away: Option<HomeAway>,
    home_away_bad: bool,
    score_seen: bool,
    score_bad: bool,
    /// Parsed eagerly (pure); read live/final, `None` there is the
    /// transform-tier score error.
    score: Option<u32>,
    team_id_seen: bool,
    team_id_bad: bool,
    abbreviation: EText,
    abbreviation_seen: bool,
    abbreviation_bad: bool,
    color_seen: bool,
    color_bad: bool,
    color: Option<u32>,
    alternate_seen: bool,
    alternate_bad: bool,
    alternate: Option<u32>,
    short_name_bad: bool,
    records_bad: bool,
    /// First `type == "total"` entry: `None` = no such entry,
    /// `Some(None)` = present but not `W-L` (quirk on the pregame and final
    /// arms), `Some(Some(_))` = parsed record.
    total: Option<Option<(u16, u16)>>,
    linescores_bad: bool,
    /// `(period, clamped byte)` in arrival order; stable-sorted at
    /// transform. Capacity is the wire's `MAX_LINE_SCORE`.
    line: heapless::Vec<(u8, u8), MAX_LINE_SCORE>,
    line_clipped: bool,
    /// This competitor's `leaders[name=points|rebounds|assists].leaders[0]`,
    /// the first entry per category; the final arm picks between the two
    /// competitors' (§3.9).
    leaders: [Option<LeaderSlot>; CATEGORIES],
}

/// Scratch for the records/linescores array element in progress. One shared
/// instance suffices: elements never interleave.
#[derive(Default)]
struct EntryScratch {
    record_is_total: bool,
    record_type_seen: bool,
    record_type_bad: bool,
    record_summary: EText,
    record_summary_seen: bool,
    record_summary_bad: bool,
    ls_value: Option<f64>,
    ls_value_bad: bool,
    ls_period: Option<u8>,
    ls_period_bad: bool,
}

/// The `leaders[*]` element in progress: its category and its first
/// leader's value and name, folded at its close.
#[derive(Default)]
struct LeaderEntry {
    category: Option<usize>,
    value: Option<u8>,
    display: Option<u8>,
    name: WireText,
}

/// One category's leader on one competitor.
#[derive(Debug, Clone, PartialEq)]
struct LeaderSlot {
    value: u8,
    name: EText,
}

// ------------------------------------------------------------ the extractor

#[expect(
    clippy::large_enum_variant,
    reason = "Detail holds the found extract in place; boxing needs alloc"
)]
enum Mode<'c> {
    List {
        /// Called once per clean event that has a competition, with the
        /// event id and its state — the games-list payload.
        on_game: &'c mut dyn FnMut(&str, GameState),
    },
    Detail {
        target: &'c str,
        outcome: Option<DetailOutcome>,
    },
}

/// The NBA sink: one implementation, two entry uses. Drive it with
/// `StreamMatcher::new(basketball::PATHS, extractor, scratch)`, feed the body, then
/// [`Extractor::stats`] / [`Extractor::into_detail`] on the finished sink.
pub struct Extractor<'c, Q: Quirks> {
    mode: Mode<'c>,
    quirks: &'c mut Q,
    scratch: EventScratch,
    stats: ScanStats,
}

impl<'c, Q: Quirks> Extractor<'c, Q> {
    /// Games-list mode: every clean event with a competition yields
    /// `(id, state)` — NBA's `list_state` is total, no exclusions.
    pub fn games_list(on_game: &'c mut dyn FnMut(&str, GameState), quirks: &'c mut Q) -> Self {
        Self::new(Mode::List { on_game }, quirks)
    }

    /// Game-detail mode for one event id. Every event is validated and
    /// counted until the target resolves (ruling 14 — the counts must be
    /// exact when the target is missing); after that, remaining events are
    /// fast-forwarded with `Directive::SkipElement` and left uncounted.
    pub fn game_detail(target: &'c str, quirks: &'c mut Q) -> Self {
        Self::new(
            Mode::Detail {
                target,
                outcome: None,
            },
            quirks,
        )
    }

    fn new(mode: Mode<'c>, quirks: &'c mut Q) -> Self {
        Self {
            mode,
            quirks,
            scratch: EventScratch::default(),
            stats: ScanStats::default(),
        }
    }

    pub fn stats(&self) -> ScanStats {
        self.stats
    }

    /// The detail result; `None` when constructed in list mode.
    pub fn into_detail(self) -> Option<DetailOutcome> {
        match self.mode {
            Mode::List { .. } => None,
            Mode::Detail { outcome, .. } => Some(outcome.unwrap_or(DetailOutcome::NotFound)),
        }
    }

    fn finalize_event(&mut self) {
        if self.scratch.skipped {
            return; // reset happens on the next event's enter
        }
        if !deserializes(&self.scratch) {
            self.stats.failed += 1;
            return;
        }
        self.stats.ok += 1;

        let s = &self.scratch;
        // The live phase resolves at DU-parse time in the backend (inside
        // the TryFrom), so the unknown-label quirk fires in both modes; the
        // status name (§3.1) is judged beside it.
        let phase = match s.state {
            Some(StateKind::In) => {
                parse_live_phase(s.desc_seen.then_some(s.desc.as_str()), &mut *self.quirks)
            }
            _ => LivePhase::InProgress,
        };
        if s.state.is_some() && status_name_unknown(&s.status_name) {
            self.quirks.quirk(Quirk::UnknownStatusName);
        }

        match &mut self.mode {
            Mode::List { on_game } => {
                if let Some(state) = s.state {
                    let listed = match state {
                        StateKind::Pre => GameState::Pregame,
                        StateKind::In => GameState::Live,
                        StateKind::Post => GameState::Final,
                    };
                    on_game(s.id.as_str(), listed);
                }
                // A clean event with an empty competitions array lists
                // nothing, exactly like the backend's filter_map.
            }
            Mode::Detail { outcome, .. } => {
                if s.is_target && outcome.is_none() {
                    *outcome = Some(match s.state {
                        None => DetailOutcome::NoCompetition,
                        Some(state) => match transform(s, state, phase, &mut *self.quirks) {
                            Ok(extract) => DetailOutcome::Found(extract),
                            Err(error) => DetailOutcome::Rejected(error),
                        },
                    });
                }
            }
        }
    }

    /// Detail mode with the outcome already decided: everything after is
    /// skipped and uncounted (ruling 14).
    fn target_found(&self) -> bool {
        matches!(
            self.mode,
            Mode::Detail {
                outcome: Some(_),
                ..
            }
        )
    }

    fn competitor(&mut self, indices: &[u16]) -> Option<&mut CompetitorScratch> {
        // Index 1 binds the competitors[*] level. Slots past the second
        // still count toward the exactly-2 rule but store nothing.
        let index = usize::from(*indices.get(1)?);
        self.scratch.competitors.get_mut(index)
    }

    /// A closed `leaders[*]` element: the first complete entry per category
    /// is the competitor's leader there. An entry whose category is not one
    /// of the three (`rating`, the pregame's `pointsPerGame`), or that lacks
    /// a name or a value, is nothing.
    fn fold_leader(&mut self, indices: &[u16]) {
        let entry = core::mem::take(&mut self.scratch.leader);
        let Some(category) = entry.category else {
            return;
        };
        let Some(name) = entry.name.get() else {
            return;
        };
        let Some(value) = entry.value.or(entry.display) else {
            return;
        };
        if let Some(competitor) = self.competitor(indices) {
            let slot = &mut competitor.leaders[category];
            if slot.is_none() {
                *slot = Some(LeaderSlot {
                    value,
                    name: owned(name),
                });
            }
        }
    }
}

impl<Q: Quirks> Sink for Extractor<'_, Q> {
    fn enter(&mut self, pattern: usize, _indices: &[u16], kind: ContainerKind) -> Directive {
        match pattern {
            // `{"events":{…}}` must 502-shape like a scalar shell; only an
            // array here is a scoreboard (engine ContainerKind addition).
            P_EVENTS => {
                if kind == ContainerKind::Object {
                    self.stats.events_malformed = true;
                }
            }
            P_EVENT => {
                if self.target_found() {
                    // Target already resolved: fast-forward the rest,
                    // uncounted (ruling 14).
                    self.scratch.skipped = true;
                    return Directive::SkipElement;
                }
                self.scratch = EventScratch::default();
            }
            P_COMPETITIONS => self.scratch.competitions_seen = true,
            P_COMPETITION0 => self.scratch.competition_seen = true,
            P_VENUE => self.scratch.venue_seen = true,
            P_SITUATION => self.scratch.situation_seen = true,
            P_LAST_PLAY => self.scratch.last_play_seen = true,
            P_COMPETITOR => {
                self.scratch.competitor_count = self.scratch.competitor_count.saturating_add(1);
            }
            P_RECORD => {
                let entry = &mut self.scratch.entry;
                entry.record_is_total = false;
                entry.record_type_seen = false;
                entry.record_type_bad = false;
                entry.record_summary_seen = false;
                entry.record_summary_bad = false;
            }
            P_LINESCORE => {
                let entry = &mut self.scratch.entry;
                entry.ls_value = None;
                entry.ls_value_bad = false;
                entry.ls_period = None;
                entry.ls_period_bad = false;
            }
            P_LEADER => self.scratch.leader = LeaderEntry::default(),
            _ => {}
        }
        Directive::Continue
    }

    fn leave(&mut self, pattern: usize, indices: &[u16]) -> Directive {
        match pattern {
            P_EVENT => self.finalize_event(),
            P_RECORD => {
                let entry = &self.scratch.entry;
                let valid = entry.record_type_seen
                    && !entry.record_type_bad
                    && entry.record_summary_seen
                    && !entry.record_summary_bad;
                let is_total = entry.record_is_total;
                let record =
                    valid.then(|| parse_record(self.scratch.entry.record_summary.as_str()));
                if let Some(competitor) = self.competitor(indices) {
                    if !valid {
                        competitor.records_bad = true;
                    } else if is_total && competitor.total.is_none() {
                        competitor.total = Some(record.unwrap_or(None));
                    }
                }
            }
            P_LINESCORE => {
                let entry = &self.scratch.entry;
                let cell = match (entry.ls_value, entry.ls_period) {
                    (Some(value), Some(period)) if !entry.ls_value_bad && !entry.ls_period_bad => {
                        Some((period, linescore_byte(value)))
                    }
                    _ => None,
                };
                let mut clipped = false;
                if let Some(competitor) = self.competitor(indices) {
                    match cell {
                        Some(pair) => {
                            if competitor.line.push(pair).is_err() && !competitor.line_clipped {
                                competitor.line_clipped = true;
                                clipped = true;
                            }
                        }
                        None => competitor.linescores_bad = true,
                    }
                }
                if clipped {
                    self.quirks.quirk(Quirk::ClippedLineScore);
                }
            }
            P_LEADER => self.fold_leader(indices),
            _ => {}
        }
        Directive::Continue
    }

    fn value(&mut self, pattern: usize, indices: &[u16], value: Value<'_>) -> Directive {
        let s = &mut self.scratch;
        match pattern {
            P_EVENTS => self.stats.events_malformed = true,
            // A scalar where an event object belongs: unparseable event.
            // Scalars never enter, so the post-target skip cannot reach
            // them — gate the count here (nothing counts after Found).
            P_EVENT => {
                if !self.target_found() {
                    self.stats.failed += 1;
                }
            }
            P_ID => match value {
                Value::Str(text) => {
                    set_text(&mut s.id, text);
                    s.id_seen = true;
                    // The target compare runs on the UNTRUNCATED streamed
                    // text and only the boolean survives — the bounded
                    // `s.id` copy is output storage, never a compare key,
                    // so distinct ids sharing a 255-byte prefix can never
                    // silently match (ruling 16; this is the backend's own
                    // unbounded compare, with no false negatives either).
                    // Non-target events are NOT skipped here: pre-target
                    // events validate and count in full (ruling 14) —
                    // skipping starts only after the target resolves, at
                    // the next event's enter.
                    if let Mode::Detail { target, .. } = &self.mode {
                        if text == *target {
                            s.is_target = true;
                        }
                    }
                }
                _ => s.id_bad = true,
            },
            P_DATE => match value {
                Value::Str(text) => {
                    s.date_seen = true;
                    s.start_time = parse_start_time(text);
                }
                _ => s.date_bad = true,
            },
            // The weather block is MLB-only, but the shared event shell
            // still fails on a malformed one — presence checks are all `Option`.
            P_WEATHER => {
                if value != Value::Null {
                    s.weather_bad = true;
                }
            }
            P_WEATHER_DISPLAY | P_WEATHER_CONDITION => {
                if !matches!(value, Value::Str(_) | Value::Null) {
                    s.weather_bad = true;
                }
            }
            P_WEATHER_TEMP => match value {
                Value::Null => {}
                Value::Num(text) if num_i16(text).is_some() => {}
                _ => s.weather_bad = true,
            },
            P_COMPETITIONS => s.competitions_bad = true,
            P_COMPETITION0 => s.competition_bad = true,
            P_STATE => match value {
                Value::Str("pre") => s.state = Some(StateKind::Pre),
                Value::Str("in") => s.state = Some(StateKind::In),
                Value::Str("post") => s.state = Some(StateKind::Post),
                _ => s.state_bad = true,
            },
            P_DESCRIPTION => match value {
                Value::Str(text) => {
                    set_text(&mut s.desc, text);
                    s.desc_seen = true;
                }
                Value::Null => {}
                _ => s.desc_bad = true,
            },
            P_PERIOD => match value {
                Value::Num(text) => match num_u8(text) {
                    Some(period) => {
                        s.period = period;
                        s.period_seen = true;
                    }
                    None => s.period_bad = true,
                },
                _ => s.period_bad = true,
            },
            P_CLOCK => match value {
                Value::Str(text) => {
                    set_text(&mut s.clock, text);
                    s.clock_seen = true;
                }
                _ => s.clock_bad = true,
            },
            P_VENUE => {
                if value != Value::Null {
                    s.venue_bad = true;
                }
            }
            P_VENUE_NAME => match value {
                Value::Str(text) => {
                    set_text(&mut s.venue_name, text);
                    s.venue_name_seen = true;
                }
                _ => s.venue_name_bad = true,
            },
            P_SITUATION => {
                if value != Value::Null {
                    s.situation_bad = true;
                }
            }
            P_LAST_PLAY => {
                if value != Value::Null {
                    s.last_play_bad = true;
                }
            }
            P_LAST_PLAY_ID => match value {
                Value::Str(text) => {
                    set_text(&mut s.play_id, text);
                    s.play_id_seen = true;
                }
                _ => s.play_id_bad = true,
            },
            P_LAST_PLAY_TEXT => match value {
                Value::Str(text) => {
                    set_text(&mut s.play_text, text);
                    s.play_text_seen = true;
                }
                _ => s.play_text_bad = true,
            },
            // The court screens' fields: wrong types read as absent, like
            // the odds (ruling 17) — nothing here has a serde predecessor.
            P_LAST_PLAY_TYPE => s.play_type = code(value),
            P_SEASON_TYPE => {
                if let Value::Num(text) = value {
                    s.season_type = num_u8(text);
                }
            }
            P_STATUS_NAME => {
                if let Value::Str(text) = value {
                    s.status_name.set(text);
                }
            }
            P_ALT_DETAIL => {
                if let Value::Str(text) = value {
                    s.alt_detail.set(text);
                }
            }
            P_SERIES => {
                if let Value::Str(text) = value {
                    s.series.set(text);
                }
            }
            P_HEADLINE => {
                if let Value::Str(text) = value {
                    s.headline.set(text);
                }
            }
            P_LEADER_NAME => {
                if let Value::Str(text) = value {
                    s.leader.category = category(text);
                }
            }
            // `value` is a float ("23.0"); `displayValue` the string ESPN
            // shows ("23"). The float wins when both parse.
            P_LEADER_VALUE => {
                if let Value::Num(text) = value {
                    s.leader.value = text
                        .parse::<f64>()
                        .ok()
                        .filter(|v| v.is_finite())
                        .map(|v| v.clamp(0.0, f64::from(u8::MAX)) as u8);
                }
            }
            P_LEADER_DISPLAY => {
                if let Value::Str(text) = value {
                    s.leader.display = num_u8(text);
                }
            }
            P_LEADER_ATHLETE => {
                if let Value::Str(text) = value {
                    s.leader.name.set(text);
                }
            }
            // A scalar where a competitor object belongs.
            P_COMPETITOR => s.competitor_scalar = true,
            P_HOME_AWAY => {
                let marker = match value {
                    Value::Str(text) => HomeAway::parse(text),
                    _ => None,
                };
                if let Some(competitor) = self.competitor(indices) {
                    match marker {
                        Some(side) => competitor.home_away = Some(side),
                        None => competitor.home_away_bad = true,
                    }
                }
            }
            P_SCORE => {
                if let Some(competitor) = self.competitor(indices) {
                    match value {
                        Value::Str(text) => {
                            competitor.score_seen = true;
                            competitor.score = text.parse().ok();
                        }
                        _ => competitor.score_bad = true,
                    }
                }
            }
            // Odds never fail an event (common.rs): store or ignore.
            P_ODDS_SPREAD_AWAY => s.odds.set(Market::Spread, HomeAway::Away, value),
            P_ODDS_SPREAD_HOME => s.odds.set(Market::Spread, HomeAway::Home, value),
            P_ODDS_MONEYLINE_AWAY => s.odds.set(Market::Moneyline, HomeAway::Away, value),
            P_ODDS_MONEYLINE_HOME => s.odds.set(Market::Moneyline, HomeAway::Home, value),
            P_TEAM_ID => {
                if let Some(competitor) = self.competitor(indices) {
                    match value {
                        Value::Str(_) => competitor.team_id_seen = true,
                        _ => competitor.team_id_bad = true,
                    }
                }
            }
            P_TEAM_ABBR => {
                if let Some(competitor) = self.competitor(indices) {
                    match value {
                        Value::Str(text) => {
                            set_text(&mut competitor.abbreviation, text);
                            competitor.abbreviation_seen = true;
                        }
                        _ => competitor.abbreviation_bad = true,
                    }
                }
            }
            P_TEAM_COLOR => {
                if let Some(competitor) = self.competitor(indices) {
                    match value {
                        Value::Str(text) => {
                            competitor.color_seen = true;
                            competitor.color = parse_hex_rgb(text);
                        }
                        _ => competitor.color_bad = true,
                    }
                }
            }
            P_TEAM_ALT_COLOR => {
                if let Some(competitor) = self.competitor(indices) {
                    match value {
                        Value::Str(text) => {
                            competitor.alternate_seen = true;
                            competitor.alternate = parse_hex_rgb(text);
                        }
                        _ => competitor.alternate_bad = true,
                    }
                }
            }
            P_TEAM_SHORT_NAME => {
                if !matches!(value, Value::Str(_) | Value::Null) {
                    if let Some(competitor) = self.competitor(indices) {
                        competitor.short_name_bad = true;
                    }
                }
            }
            P_RECORDS => {
                // A `records: null` (or any scalar) fails serde's Vec —
                // `#[serde(default)]` only rescues an absent key.
                if let Some(competitor) = self.competitor(indices) {
                    competitor.records_bad = true;
                }
            }
            P_RECORD => {
                if let Some(competitor) = self.competitor(indices) {
                    competitor.records_bad = true;
                }
            }
            P_RECORD_TYPE => match value {
                Value::Str(text) => {
                    s.entry.record_is_total = text == "total";
                    s.entry.record_type_seen = true;
                }
                _ => s.entry.record_type_bad = true,
            },
            P_RECORD_SUMMARY => match value {
                Value::Str(text) => {
                    set_text(&mut s.entry.record_summary, text);
                    s.entry.record_summary_seen = true;
                }
                _ => s.entry.record_summary_bad = true,
            },
            P_LINESCORES => {
                if let Some(competitor) = self.competitor(indices) {
                    competitor.linescores_bad = true;
                }
            }
            P_LINESCORE => {
                if let Some(competitor) = self.competitor(indices) {
                    competitor.linescores_bad = true;
                }
            }
            P_LINESCORE_VALUE => match value {
                // serde_json rejects out-of-range floats, hence the finite
                // filter; NaN cannot appear in JSON text.
                Value::Num(text) => match text.parse::<f64>().ok().filter(|v| v.is_finite()) {
                    Some(parsed) => s.entry.ls_value = Some(parsed),
                    None => s.entry.ls_value_bad = true,
                },
                _ => s.entry.ls_value_bad = true,
            },
            P_LINESCORE_PERIOD => match value {
                Value::Num(text) => match num_u8(text) {
                    Some(period) => s.entry.ls_period = Some(period),
                    None => s.entry.ls_period_bad = true,
                },
                _ => s.entry.ls_period_bad = true,
            },
            _ => {}
        }
        Directive::Continue
    }
}

// ------------------------------------------------------------- the semantics

/// The deserialize tier: mirrors serde's required/typed field rules plus the
/// DU conversion's per-state requirements (`backend/src/basketball/types.rs`).
/// `false` ⇒ the event is skipped and counted failed, exactly where the
/// backend's `parse_events` would drop it.
fn deserializes(s: &EventScratch) -> bool {
    if !s.id_seen || s.id_bad || !s.date_seen || s.date_bad || s.weather_bad {
        return false;
    }
    if !s.competitions_seen || s.competitions_bad || s.competition_bad {
        return false;
    }
    if !s.competition_seen {
        // `competitions: []` deserializes; there is just nothing to list.
        return true;
    }
    let Some(state) = s.state else {
        return false;
    };
    if s.state_bad || s.desc_bad {
        return false;
    }
    // Non-Option for every state: pregame parses-and-discards both.
    if !s.period_seen || s.period_bad || !s.clock_seen || s.clock_bad {
        return false;
    }
    // A present venue must carry fullName in every state; only pregame
    // requires the block itself.
    if s.venue_bad || (s.venue_seen && (!s.venue_name_seen || s.venue_name_bad)) {
        return false;
    }
    if s.situation_bad || s.last_play_bad {
        return false;
    }
    // Within a present lastPlay both id and text are required — all states.
    if s.last_play_seen
        && (!s.play_id_seen || s.play_id_bad || !s.play_text_seen || s.play_text_bad)
    {
        return false;
    }
    if s.competitor_scalar || s.competitor_count != 2 {
        return false;
    }
    for competitor in &s.competitors {
        if competitor.home_away.is_none() || competitor.home_away_bad {
            return false;
        }
        if !competitor.score_seen || competitor.score_bad {
            return false;
        }
        if !competitor.team_id_seen || competitor.team_id_bad {
            return false;
        }
        if !competitor.abbreviation_seen || competitor.abbreviation_bad {
            return false;
        }
        if !competitor.color_seen || competitor.color_bad {
            return false;
        }
        if !competitor.alternate_seen || competitor.alternate_bad {
            return false;
        }
        if competitor.short_name_bad || competitor.records_bad || competitor.linescores_bad {
            return false;
        }
    }
    match state {
        StateKind::Pre => s.venue_seen,
        StateKind::In => s.situation_seen,
        StateKind::Post => true,
    }
}

/// The transform tier (`backend/src/nba/transform.rs`): runs only on the
/// detail target, after the deserialize tier passed. Errors here are the
/// backend's hard 5xx — the games list never reaches this code.
fn transform<Q: Quirks>(
    s: &EventScratch,
    state: StateKind,
    phase: LivePhase,
    quirks: &mut Q,
) -> Result<Extract, TransformError> {
    let [first, second] = &s.competitors;
    let first_marker = first.home_away.ok_or(TransformError::HomeAway)?;
    let second_marker = second.home_away.ok_or(TransformError::HomeAway)?;
    let (home, away) = order_home_away((first_marker, first), (second_marker, second))
        .ok_or(TransformError::HomeAway)?;
    let playoffs = s.season_type == Some(SEASON_POSTSEASON);

    let kind = match state {
        StateKind::Pre => {
            let start_time = s.start_time.ok_or(TransformError::StartTime)?;
            // Home before away: the backend builds its struct in that
            // order, which fixes the malformed-record quirk order.
            let mut home = pregame_team(home, quirks)?;
            let mut away = pregame_team(away, quirks)?;
            if let Some([away_line, home_line]) = s.odds.team_lines() {
                away.line = Some(text_of(&away_line));
                home.line = Some(text_of(&home_line));
            }
            Kind::Pregame(Pregame {
                start_time,
                venue: s.venue_name.clone(),
                info_secondary: info_secondary(playoffs, &s.headline, &s.series),
                away,
                home,
            })
        }
        StateKind::In => {
            let home = live_team(home)?;
            let away = live_team(away)?;
            Kind::Live(Live {
                period: s.period,
                phase,
                clock: s.clock.clone(),
                away,
                home,
                // §3.7's filter, applied where the play is built so no
                // unfiltered copy exists for a consumer to flash by mistake.
                last_play: (s.last_play_seen && s.play_type.is_some_and(flash_eligible)).then(
                    || LastPlay {
                        id: s.play_id.clone(),
                        text: s.play_text.clone(),
                    },
                ),
            })
        }
        StateKind::Post => {
            let home_team = final_team(home)?;
            let away_team = final_team(away)?;
            // §3.1's cross-check: the label comes from the period;
            // `altDetail` only confirms it.
            let actual = if s.alt_detail.seen() {
                // An overlong altDetail can match nothing.
                s.alt_detail.valid().unwrap_or("?")
            } else {
                ""
            };
            if actual != expected_alt_detail(s.period).as_str() {
                quirks.quirk(Quirk::FinalDetailMismatch);
            }
            Kind::Final(Final {
                periods_played: s.period,
                away: away_team,
                home: home_team,
                leaders: leaders(home, away),
                // Playoffs only: a regular-season event never carries one,
                // and the final screen's line is the playoff series.
                series: if playoffs {
                    s.series.get().map(owned)
                } else {
                    None
                },
                records: final_records(home, away, quirks),
            })
        }
    };
    Ok(Extract {
        game_id: s.id.clone(),
        playoffs,
        kind,
    })
}

fn colors(competitor: &CompetitorScratch) -> Result<TeamColors, TransformError> {
    Ok(TeamColors {
        primary: competitor.color.ok_or(TransformError::Color)?,
        alternate: competitor.alternate.ok_or(TransformError::Color)?,
    })
}

/// The `total` record as the pregame reads it: `None` for none, a quirk and
/// `None` for one that is not `W-L` — the backend's `parse_record`,
/// warn-and-drop, never an error.
fn total_record<Q: Quirks>(competitor: &CompetitorScratch, quirks: &mut Q) -> Option<Record> {
    match competitor.total {
        None => None,
        Some(Some((wins, losses))) => Some(Record { wins, losses }),
        Some(None) => {
            quirks.quirk(Quirk::MalformedRecord);
            None
        }
    }
}

fn pregame_team<Q: Quirks>(
    competitor: &CompetitorScratch,
    quirks: &mut Q,
) -> Result<PregameTeam, TransformError> {
    Ok(PregameTeam {
        abbreviation: competitor.abbreviation.clone(),
        colors: colors(competitor)?,
        record: total_record(competitor, quirks),
        line: None,
    })
}

fn text_of(text: &str) -> EText {
    let mut out = EText::new();
    set_text(&mut out, text);
    out
}

/// Owned [`EText`] from an already-wire-truncated source.
fn owned(text: &str) -> EText {
    text_of(text)
}

fn live_team(competitor: &CompetitorScratch) -> Result<LiveTeam, TransformError> {
    Ok(LiveTeam {
        abbreviation: competitor.abbreviation.clone(),
        score: competitor.score.ok_or(TransformError::Score)?,
        colors: colors(competitor)?,
    })
}

fn final_team(competitor: &CompetitorScratch) -> Result<FinalTeam, TransformError> {
    Ok(FinalTeam {
        abbreviation: competitor.abbreviation.clone(),
        score: competitor.score.ok_or(TransformError::Score)?,
        colors: colors(competitor)?,
        line_score: sorted_line(&competitor.line),
    })
}

/// §3.9's records: both sides' totals, home judged first like the pregame;
/// one missing or malformed leaves the pair out.
fn final_records<Q: Quirks>(
    home: &CompetitorScratch,
    away: &CompetitorScratch,
    quirks: &mut Q,
) -> Option<Records> {
    let home = total_record(home, quirks);
    let away = total_record(away, quirks);
    Some(Records {
        away: away?,
        home: home?,
    })
}

/// §3.9's pick per category: the higher `value` of the two competitors'
/// leaders, a tie (or a side without one) to the away leader; all three
/// categories or none.
fn leaders(home: &CompetitorScratch, away: &CompetitorScratch) -> Option<Leaders> {
    let pick = |category: usize| -> Option<Leader> {
        let leader = |side: Side, slot: &LeaderSlot| Leader {
            side,
            value: slot.value,
            name: slot.name.clone(),
        };
        match (&away.leaders[category], &home.leaders[category]) {
            (Some(a), Some(h)) if h.value > a.value => Some(leader(Side::Home, h)),
            (Some(a), _) => Some(leader(Side::Away, a)),
            (None, Some(h)) => Some(leader(Side::Home, h)),
            (None, None) => None,
        }
    };
    Some(Leaders {
        points: pick(0)?,
        rebounds: pick(1)?,
        assists: pick(2)?,
    })
}

/// `leaders[*].name` → the final screen's row; `rating` and the pregame's
/// per-game averages are not rows.
fn category(name: &str) -> Option<usize> {
    match name {
        "points" => Some(0),
        "rebounds" => Some(1),
        "assists" => Some(2),
        _ => None,
    }
}

/// §3.8's second line: the headline upper-cased, then " - " and the series
/// in the playoffs; nothing without a headline.
fn info_secondary(playoffs: bool, headline: &WireText, series: &WireText) -> EText {
    let mut out = EText::new();
    let Some(headline) = headline.get() else {
        return out;
    };
    push_upper(&mut out, headline);
    if let (true, Some(series)) = (playoffs, series.get()) {
        push_upper(&mut out, " - ");
        push_upper(&mut out, series);
    }
    out
}

/// Append `src` upper-cased, char by char with `core`'s mapping (ruling
/// 10), stopping at the first char that no longer fits — a mapping can grow
/// a char, so the cut is where the output fills, on a char boundary.
fn push_upper(dst: &mut EText, src: &str) {
    for ch in src.chars() {
        for upper in ch.to_uppercase() {
            if dst.push(upper).is_err() {
                return;
            }
        }
    }
}

/// The five names ESPN sends for the NBA (§2) plus STATUS_DELAYED, which §2
/// says to assume exists. Absent follows the state silently; anything else
/// is the quirk (§3.1).
fn status_name_unknown(name: &Exact<STATUS_NAME_BYTES>) -> bool {
    if !name.seen() {
        return false;
    }
    !matches!(
        name.valid(),
        Some(
            "STATUS_SCHEDULED"
                | "STATUS_IN_PROGRESS"
                | "STATUS_HALFTIME"
                | "STATUS_END_PERIOD"
                | "STATUS_FINAL"
                | "STATUS_DELAYED"
        )
    )
}

/// The `altDetail` §3.1's final label implies: none through regulation,
/// "OT" after one overtime, "{n}OT" after n.
fn expected_alt_detail(period: u8) -> heapless::String<ALT_DETAIL_BYTES> {
    use core::fmt::Write as _;
    let mut out = heapless::String::new();
    match period.saturating_sub(REGULATION_PERIODS) {
        0 => {}
        1 => {
            let _ = out.push_str("OT");
        }
        // At most "251OT", which fits.
        n => {
            let _ = write!(out, "{n}OT");
        }
    }
    out
}

/// A code field: ESPN sends ids as strings (`"16"`), the engine hands
/// numbers as raw text; both are an unsigned literal that fits.
fn code(value: Value<'_>) -> Option<u16> {
    match value {
        Value::Str(text) | Value::Num(text) => num_u16(text),
        _ => None,
    }
}

/// §3.7's table.
fn flash_eligible(type_id: u16) -> bool {
    FLASH_TYPES.contains(&type_id)
}

/// The backend's `linescore_bytes` ordering: a **stable** sort by period —
/// duplicate periods keep arrival order (ruling 13), gaps are not filled.
fn sorted_line(
    line: &heapless::Vec<(u8, u8), MAX_LINE_SCORE>,
) -> heapless::Vec<u8, MAX_LINE_SCORE> {
    let mut pairs = line.clone();
    stable_sort_by_key(&mut pairs, |&(period, _)| period);
    pairs.iter().map(|&(_, byte)| byte).collect()
}

// ---------------------------------------------------------------- summary

// Summary body. Everything else in its 110–513 KB (the win probability,
// rosters, odds, news, the per-player stat lines past PTS) is skipped by
// construction.
const S_HEADER_COMPETITOR: usize = 0;
const S_HEADER_TEAM_ID: usize = 1;
const S_HEADER_HOME_AWAY: usize = 2;
const S_HEADER_FOULS: usize = 3;
const S_HEADER_BONUS: usize = 4;
const S_HEADER_TO_GIVE: usize = 5;
const S_HEADER_TIMEOUTS: usize = 6;
const S_PLAY: usize = 7;
const S_PLAY_ID: usize = 8;
const S_PLAY_SEQ: usize = 9;
const S_PLAY_TYPE: usize = 10;
const S_PLAY_TEXT: usize = 11;
const S_PLAY_SCORING: usize = 12;
const S_PLAY_SCORE_VALUE: usize = 13;
const S_PLAY_ATTEMPTED: usize = 14;
const S_PLAY_X: usize = 15;
const S_PLAY_Y: usize = 16;
const S_PLAY_TEAM: usize = 17;
const S_PLAY_AWAY_SCORE: usize = 18;
const S_PLAY_HOME_SCORE: usize = 19;
const S_PLAY_SHOOTER: usize = 20;
const S_PLAYERS: usize = 21;
const S_PLAYERS_TEAM_ID: usize = 22;
const S_PLAYERS_NAME: usize = 23;
const S_PLAYERS_KEY: usize = 24;
const S_PLAYERS_ATHLETE: usize = 25;
const S_PLAYERS_ATHLETE_ID: usize = 26;
const S_PLAYERS_STAT: usize = 27;

#[rustfmt::skip]
static SUMMARY_TABLE: &[Pattern] = &[
    /* S_HEADER_COMPETITOR  */ &[Key("header"), Key("competitions"), Index(0), Key("competitors"), AnyIndex],
    /* S_HEADER_TEAM_ID     */ &[Key("header"), Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("team"), Key("id")],
    /* S_HEADER_HOME_AWAY   */ &[Key("header"), Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("homeAway")],
    /* S_HEADER_FOULS       */ &[Key("header"), Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("fouls"), Key("teamFoulsCurrent")],
    /* S_HEADER_BONUS       */ &[Key("header"), Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("fouls"), Key("bonusState")],
    /* S_HEADER_TO_GIVE     */ &[Key("header"), Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("fouls"), Key("foulsToGive")],
    /* S_HEADER_TIMEOUTS    */ &[Key("header"), Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("timeoutsRemaining")],
    /* S_PLAY               */ &[Key("plays"), AnyIndex],
    /* S_PLAY_ID            */ &[Key("plays"), AnyIndex, Key("id")],
    /* S_PLAY_SEQ           */ &[Key("plays"), AnyIndex, Key("sequenceNumber")],
    /* S_PLAY_TYPE          */ &[Key("plays"), AnyIndex, Key("type"), Key("id")],
    /* S_PLAY_TEXT          */ &[Key("plays"), AnyIndex, Key("text")],
    /* S_PLAY_SCORING       */ &[Key("plays"), AnyIndex, Key("scoringPlay")],
    /* S_PLAY_SCORE_VALUE   */ &[Key("plays"), AnyIndex, Key("scoreValue")],
    /* S_PLAY_ATTEMPTED     */ &[Key("plays"), AnyIndex, Key("pointsAttempted")],
    /* S_PLAY_X             */ &[Key("plays"), AnyIndex, Key("coordinate"), Key("x")],
    /* S_PLAY_Y             */ &[Key("plays"), AnyIndex, Key("coordinate"), Key("y")],
    /* S_PLAY_TEAM          */ &[Key("plays"), AnyIndex, Key("team"), Key("id")],
    /* S_PLAY_AWAY_SCORE    */ &[Key("plays"), AnyIndex, Key("awayScore")],
    /* S_PLAY_HOME_SCORE    */ &[Key("plays"), AnyIndex, Key("homeScore")],
    /* S_PLAY_SHOOTER       */ &[Key("plays"), AnyIndex, Key("participants"), Index(0), Key("athlete"), Key("id")],
    /* S_PLAYERS            */ &[Key("boxscore"), Key("players"), AnyIndex],
    /* S_PLAYERS_TEAM_ID    */ &[Key("boxscore"), Key("players"), AnyIndex, Key("team"), Key("id")],
    /* S_PLAYERS_NAME       */ &[Key("boxscore"), Key("players"), AnyIndex, Key("statistics"), Index(0), Key("names"), AnyIndex],
    /* S_PLAYERS_KEY        */ &[Key("boxscore"), Key("players"), AnyIndex, Key("statistics"), Index(0), Key("keys"), AnyIndex],
    /* S_PLAYERS_ATHLETE    */ &[Key("boxscore"), Key("players"), AnyIndex, Key("statistics"), Index(0), Key("athletes"), AnyIndex],
    /* S_PLAYERS_ATHLETE_ID */ &[Key("boxscore"), Key("players"), AnyIndex, Key("statistics"), Index(0), Key("athletes"), AnyIndex, Key("athlete"), Key("id")],
    /* S_PLAYERS_STAT       */ &[Key("boxscore"), Key("players"), AnyIndex, Key("statistics"), Index(0), Key("athletes"), AnyIndex, Key("stats"), AnyIndex],
];

/// Side `0 = away, 1 = home`, the index every per-side pair below uses.
fn slot(side: HomeAway) -> usize {
    match side {
        HomeAway::Away => 0,
        HomeAway::Home => 1,
    }
}

/// A team id against the two header competitors', home compared first.
fn side_of(team_id: &Exact<ID_BYTES>, home: Option<&str>, away: Option<&str>) -> Option<Side> {
    let id = team_id.valid()?;
    if Some(id) == home {
        Some(Side::Home)
    } else if Some(id) == away {
        Some(Side::Away)
    } else {
        None
    }
}

/// A small count (fouls, timeouts, points): an unsigned literal, saturated
/// to the byte the wire carries. Anything else is absent.
fn count(text: &str) -> Option<u8> {
    num_u32(text).map(|n| n.min(u32::from(u8::MAX)) as u8)
}

/// A coordinate in feet: a finite number inside [`COORDINATE_WINDOW`],
/// rounded half away from zero by hand (`f64::round` is std-only). The
/// sentinel and anything unparseable is no coordinate.
fn feet(value: Value<'_>) -> Option<i32> {
    let Value::Num(text) = value else {
        return None;
    };
    let value: f64 = text
        .parse()
        .ok()
        .filter(|v: &f64| v.is_finite() && COORDINATE_WINDOW.contains(v))?;
    Some(if value >= 0.0 {
        (value + 0.5) as i32
    } else {
        (value - 0.5) as i32
    })
}

/// §3.6's family name: the text up to " makes " is the player, minus a
/// trailing generational suffix; the last token, upper-cased. Empty when
/// there is no " makes " or nothing before it (a placeholder's "makes
/// 2-foot shot", a goaltending award).
fn family_name(text: &str) -> EText {
    let mut out = EText::new();
    let Some(at) = text.find(" makes ") else {
        return out;
    };
    let mut tokens = text[..at].split_whitespace().rev();
    let Some(mut last) = tokens.next() else {
        return out;
    };
    if NAME_SUFFIXES.contains(&last) {
        if let Some(before) = tokens.next() {
            last = before;
        }
    }
    push_upper(&mut out, last);
    out
}

/// One `header.competitions[0].competitors[*]`: the team-id → side mapping
/// and the side's ledger column (§3.3).
#[derive(Debug, Clone, Default)]
struct HeaderTeam {
    team_id: Exact<ID_BYTES>,
    home_away: Option<HomeAway>,
    fouls: Option<u8>,
    /// `bonusState == "DOUBLE"`; `None` when the field is not a string.
    in_bonus: Option<bool>,
    to_give: Option<u8>,
    timeouts: Option<u8>,
}

/// One boxscore row: a player and the numeric cells of his stat line; a
/// cell that is not a plain count ("5-12", "+3") is `None`.
#[derive(Debug, Clone, Default)]
struct AthleteRow {
    active: bool,
    id: Exact<ID_BYTES>,
    stats: heapless::Vec<Option<u8>, STAT_COLUMNS>,
}

/// One `boxscore.players[*]` block: a team, where its PTS column is, and
/// its rows. The column is found by name, never position (ruling 4), and
/// `names`/`keys` may stream after the athletes, so the rows keep every cell
/// until finish.
#[derive(Debug, Clone, Default)]
struct TeamBox {
    team_id: Exact<ID_BYTES>,
    pts_by_name: Option<u16>,
    pts_by_key: Option<u16>,
    athletes: heapless::Vec<AthleteRow, ATHLETES_PER_TEAM>,
    /// A nineteenth row arrived; the listing is incomplete.
    overflowed: bool,
    row: AthleteRow,
}

impl TeamBox {
    fn pts_column(&self) -> Option<u16> {
        self.pts_by_name.or(self.pts_by_key)
    }
}

/// The `plays[*]` element in progress, folded at its close.
#[derive(Debug, Clone, Default)]
struct PlayScratch {
    active: bool,
    id: WireText,
    seq: Option<u32>,
    type_id: Option<u16>,
    text: WireText,
    scoring: bool,
    score_value: Option<u8>,
    attempted: Option<u8>,
    x: Option<i32>,
    y: Option<i32>,
    team_id: Exact<ID_BYTES>,
    away_score: Option<u32>,
    home_score: Option<u32>,
    shooter: Exact<ID_BYTES>,
}

/// A make awaiting its side (§3.4); `x` is feet from the centre line before
/// the home rotation, which needs the side.
#[derive(Debug, Clone)]
struct MakeRec {
    team_id: Exact<ID_BYTES>,
    three: bool,
    x: i8,
    y: i8,
}

/// The newest scoring play awaiting its side and its scorer's points (§3.6).
#[derive(Debug, Clone)]
struct NewestRec {
    team_id: Exact<ID_BYTES>,
    seq: u32,
    scorer: EText,
    shooter: Exact<ID_BYTES>,
    marked: bool,
}

/// A per-team pool: the two blocks a game has, the one in progress, and
/// whether a third ever arrived.
#[derive(Debug, Clone, Default)]
struct Pool<T> {
    items: heapless::Vec<T, TEAMS>,
    current: T,
    open: bool,
    overflowed: bool,
}

impl<T: Default> Pool<T> {
    fn open(&mut self) {
        self.current = T::default();
        self.open = true;
    }

    fn current(&mut self) -> Option<&mut T> {
        self.open.then_some(&mut self.current)
    }

    /// Close the block in progress; `true` the first time one is dropped.
    fn close(&mut self) -> bool {
        if !self.open {
            return false;
        }
        self.open = false;
        let item = core::mem::take(&mut self.current);
        if self.items.push(item).is_err() && !self.overflowed {
            self.overflowed = true;
            return true;
        }
        false
    }
}

struct SummarySink<Q: Quirks> {
    /// The context said live; otherwise every callback is a no-op and the
    /// summary is empty.
    live: bool,
    quirks: Q,
    play: PlayScratch,
    /// The running score before the play in hand (§3.5); 0-0 before the
    /// first.
    prev: (u32, u32),
    /// The side and points of the current run, whatever its length.
    run: Option<(Side, u8)>,
    makes: heapless::Vec<MakeRec, MAX_MAKES>,
    newest: Option<NewestRec>,
    /// A made field goal got no mark (§3.4's quirk, once at finish).
    unplaced: bool,
    flash: Option<LastPlay>,
    header: Pool<HeaderTeam>,
    players: Pool<TeamBox>,
}

impl<Q: Quirks> SummarySink<Q> {
    fn new(context: SummaryContext, quirks: Q) -> Self {
        Self {
            live: context.state == GameState::Live,
            quirks,
            play: PlayScratch::default(),
            prev: (0, 0),
            run: None,
            makes: heapless::Vec::new(),
            newest: None,
            unplaced: false,
            flash: None,
            header: Pool::default(),
            players: Pool::default(),
        }
    }

    /// §3.4–3.7 over one play, in document order (chronological; the
    /// `sequenceNumber`s are posting order and may not be).
    fn fold_play(&mut self) {
        let play = &mut self.play;
        if !play.active {
            return;
        }
        play.active = false;

        // §3.5: a play whose running score differs from the previous one is
        // a scoring step for the side that rose alone. Both rising, or a
        // side falling (ESPN posts stale rows), is a correction: the run
        // ends and none starts. A play without both scores is no step.
        if let (Some(away), Some(home)) = (play.away_score, play.home_score) {
            let (prev_away, prev_home) = self.prev;
            if (away, home) != (prev_away, prev_home) {
                let step = if away > prev_away && home == prev_home {
                    Some((Side::Away, away - prev_away))
                } else if home > prev_home && away == prev_away {
                    Some((Side::Home, home - prev_home))
                } else {
                    None
                };
                match step {
                    Some((side, points)) => {
                        let points = u8::try_from(points).unwrap_or(u8::MAX);
                        self.run = Some(match self.run {
                            Some((running, sum)) if running == side => {
                                (side, sum.saturating_add(points))
                            }
                            _ => (side, points),
                        });
                    }
                    None => {
                        self.run = None;
                        self.quirks.quirk(Quirk::RunCorrection);
                    }
                }
                self.prev = (away, home);
            }
        }

        // §3.4: a make is a scoring play worth a field goal, with a team and
        // a coordinate; a free throw attempts 0 and carries the sentinel.
        let has_team = play.team_id.seen();
        let field_goal = play.scoring && play.attempted.is_some_and(|n| n >= FIELD_GOAL_ATTEMPT);
        let mut marked = false;
        if field_goal {
            match (has_team, play.x.zip(play.y)) {
                (true, Some((x, y))) => {
                    if self.makes.is_full() {
                        self.makes.remove(0);
                    }
                    // Both clamps land inside i8.
                    let _ = self.makes.push(MakeRec {
                        team_id: play.team_id.clone(),
                        three: play.score_value == Some(3),
                        x: (x - COURT_CENTRE_X)
                            .clamp(-i32::from(MAX_COORDINATE_X), i32::from(MAX_COORDINATE_X))
                            as i8,
                        y: y.clamp(i32::from(MIN_COORDINATE_Y), i32::from(MAX_COORDINATE_Y)) as i8,
                    });
                    marked = true;
                }
                _ => self.unplaced = true,
            }
        }

        // §3.6: the newest scoring play with a team, free throws included.
        if play.scoring && has_team {
            self.newest = Some(NewestRec {
                team_id: play.team_id.clone(),
                seq: play.seq.unwrap_or(0),
                scorer: family_name(play.text.as_str()),
                shooter: play.shooter.clone(),
                marked,
            });
        }

        // §3.7: the newest eligible play, its text as ESPN wrote it (the
        // wire view trims it beside the scoreboard's).
        if play.type_id.is_some_and(flash_eligible) {
            if let (Some(id), Some(text)) = (play.id.get(), play.text.get()) {
                self.flash = Some(LastPlay {
                    id: owned(id),
                    text: owned(text),
                });
            }
        }
    }

    fn fold_athlete(&mut self) {
        let Some(team) = self.players.current() else {
            return;
        };
        if !team.row.active {
            return;
        }
        let row = core::mem::take(&mut team.row);
        if team.athletes.push(row).is_err() {
            team.overflowed = true;
        }
    }

    /// Everything that waited for the team mapping (ruling 4), resolved.
    fn resolve(self) -> SummaryReport<Q> {
        let Self {
            live,
            mut quirks,
            run,
            makes: recs,
            newest,
            mut unplaced,
            flash,
            header,
            players,
            ..
        } = self;
        if !live {
            return SummaryReport {
                summary: Summary::NONE,
                quirks,
            };
        }
        let mut sides: [Option<&HeaderTeam>; TEAMS] = [None, None];
        for team in &header.items {
            if let Some(marker) = team.home_away {
                let k = slot(marker);
                if sides[k].is_none() {
                    sides[k] = Some(team);
                }
            }
        }
        let [away_team, home_team] = sides;
        let home = home_team.and_then(|team| team.team_id.valid());
        let away = away_team.and_then(|team| team.team_id.valid());

        let mut makes = heapless::Vec::new();
        for rec in &recs {
            match side_of(&rec.team_id, home, away) {
                Some(side) => {
                    // §3.4's rotation: home's x is negated, a 180° turn about
                    // the centre, so the two sports project the same way.
                    let x = match side {
                        Side::Away => rec.x,
                        Side::Home => -rec.x,
                    };
                    let _ = makes.push(Make {
                        side,
                        three: rec.three,
                        x,
                        y: rec.y,
                    });
                }
                None => unplaced = true,
            }
        }
        if unplaced {
            quirks.quirk(Quirk::UnplacedMake);
        }

        let newest_score = newest.and_then(|rec| {
            let side = side_of(&rec.team_id, home, away)?;
            Some(NewestScore {
                side,
                scorer_points: points_of(&players.items, &rec.shooter, &mut quirks),
                seq: rec.seq,
                marked: rec.marked,
                scorer: rec.scorer,
            })
        });

        let ledger = match (away_team, home_team) {
            (Some(away), Some(home)) => ledger(away, home, &mut quirks),
            _ => None,
        };
        // A run shorter than RUN_SHOW is not a run on the wire (§3.5).
        let run =
            run.and_then(|(side, points)| (points >= RUN_SHOW).then_some(Run { side, points }));

        SummaryReport {
            summary: Summary {
                ledger,
                run,
                makes,
                newest_score,
                last_play: flash,
            },
            quirks,
        }
    }
}

/// §3.3: both columns whole, or nothing. The glitch body — `foulsToGive` 0
/// beside fewer than two team fouls on either side, a penalty no rule of
/// the game produces — is the one-poll reset, a quirk and no ledger; two
/// teams out of timeouts and both in the penalty is a real end-of-game
/// state and stands.
fn ledger<Q: Quirks>(away: &HeaderTeam, home: &HeaderTeam, quirks: &mut Q) -> Option<Ledger> {
    let column = |team: &HeaderTeam| -> Option<(TeamLedger, u8)> {
        Some((
            TeamLedger {
                fouls: team.fouls?,
                in_bonus: team.in_bonus?,
                timeouts: team.timeouts?,
            },
            team.to_give?,
        ))
    };
    let (away, away_to_give) = column(away)?;
    let (home, home_to_give) = column(home)?;
    let impossible = |column: &TeamLedger, to_give: u8| to_give == 0 && column.fouls < 2;
    if impossible(&away, away_to_give) || impossible(&home, home_to_give) {
        quirks.quirk(Quirk::LedgerGlitch);
        return None;
    }
    Some(Ledger { away, home })
}

/// §3.6's points: the shooter's PTS cell in whichever team block lists him;
/// 0 when no block does yet, when the row has no such cell, or when the
/// block never named a PTS column. A PTS column past the cells a row keeps
/// is a bounded overflow.
fn points_of<Q: Quirks>(teams: &[TeamBox], shooter: &Exact<ID_BYTES>, quirks: &mut Q) -> u8 {
    let Some(id) = shooter.valid() else {
        return 0;
    };
    for team in teams {
        let Some(row) = team.athletes.iter().find(|row| row.id.valid() == Some(id)) else {
            continue;
        };
        let Some(column) = team.pts_column() else {
            return 0;
        };
        if usize::from(column) >= STAT_COLUMNS {
            quirks.quirk(Quirk::BoundedOverflow);
            return 0;
        }
        return row
            .stats
            .get(usize::from(column))
            .copied()
            .flatten()
            .unwrap_or(0);
    }
    0
}

impl<Q: Quirks> Sink for SummarySink<Q> {
    fn value(&mut self, pattern: usize, indices: &[u16], value: Value<'_>) -> Directive {
        if !self.live {
            return Directive::Continue;
        }
        let play = &mut self.play;
        match pattern {
            S_PLAY_ID => {
                if let Value::Str(text) = value {
                    play.id.set(text);
                }
            }
            // A string in the corpus ("630"); a number is accepted too.
            S_PLAY_SEQ => {
                if let Value::Str(text) | Value::Num(text) = value {
                    play.seq = num_u32(text);
                }
            }
            S_PLAY_TYPE => play.type_id = code(value),
            S_PLAY_TEXT => {
                if let Value::Str(text) = value {
                    play.text.set(text);
                }
            }
            S_PLAY_SCORING => {
                if let Value::Bool(flag) = value {
                    play.scoring = flag;
                }
            }
            S_PLAY_SCORE_VALUE => {
                if let Value::Num(text) = value {
                    play.score_value = num_u8(text);
                }
            }
            S_PLAY_ATTEMPTED => {
                if let Value::Num(text) = value {
                    play.attempted = num_u8(text);
                }
            }
            S_PLAY_X => play.x = feet(value),
            S_PLAY_Y => play.y = feet(value),
            S_PLAY_TEAM => {
                if let Value::Str(text) = value {
                    play.team_id.set(text);
                }
            }
            S_PLAY_AWAY_SCORE => {
                if let Value::Num(text) = value {
                    play.away_score = num_u32(text);
                }
            }
            S_PLAY_HOME_SCORE => {
                if let Value::Num(text) = value {
                    play.home_score = num_u32(text);
                }
            }
            S_PLAY_SHOOTER => {
                if let Value::Str(text) = value {
                    play.shooter.set(text);
                }
            }
            S_HEADER_TEAM_ID => {
                if let (Value::Str(text), Some(team)) = (value, self.header.current()) {
                    team.team_id.set(text);
                }
            }
            S_HEADER_HOME_AWAY => {
                if let (Value::Str(text), Some(team)) = (value, self.header.current()) {
                    team.home_away = HomeAway::parse(text);
                }
            }
            S_HEADER_FOULS => {
                if let (Value::Num(text), Some(team)) = (value, self.header.current()) {
                    team.fouls = count(text);
                }
            }
            S_HEADER_BONUS => {
                if let (Value::Str(text), Some(team)) = (value, self.header.current()) {
                    team.in_bonus = Some(text == "DOUBLE");
                }
            }
            S_HEADER_TO_GIVE => {
                if let (Value::Num(text), Some(team)) = (value, self.header.current()) {
                    team.to_give = count(text);
                }
            }
            S_HEADER_TIMEOUTS => {
                if let (Value::Num(text), Some(team)) = (value, self.header.current()) {
                    team.timeouts = count(text);
                }
            }
            S_PLAYERS_TEAM_ID => {
                if let (Value::Str(text), Some(team)) = (value, self.players.current()) {
                    team.team_id.set(text);
                }
            }
            // The innermost index is the column.
            S_PLAYERS_NAME => {
                if let (Value::Str("PTS"), Some(team), Some(column)) =
                    (value, self.players.current(), indices.last())
                {
                    team.pts_by_name = Some(*column);
                }
            }
            S_PLAYERS_KEY => {
                if let (Value::Str("points"), Some(team), Some(column)) =
                    (value, self.players.current(), indices.last())
                {
                    team.pts_by_key = Some(*column);
                }
            }
            S_PLAYERS_ATHLETE_ID => {
                if let (Value::Str(text), Some(team)) = (value, self.players.current()) {
                    team.row.id.set(text);
                }
            }
            // Cells stream in column order; one past the cap is dropped and
            // noticed only if it is the PTS column (`points_of`).
            S_PLAYERS_STAT => {
                if let (Some(team), Some(column)) = (self.players.current(), indices.last()) {
                    if usize::from(*column) == team.row.stats.len() {
                        let cell = match value {
                            Value::Str(text) | Value::Num(text) => count(text),
                            _ => None,
                        };
                        let _ = team.row.stats.push(cell);
                    }
                }
            }
            _ => {}
        }
        Directive::Continue
    }

    fn enter(&mut self, pattern: usize, _indices: &[u16], _kind: ContainerKind) -> Directive {
        if !self.live {
            return Directive::Continue;
        }
        match pattern {
            S_PLAY => {
                self.play = PlayScratch {
                    active: true,
                    ..PlayScratch::default()
                };
            }
            S_HEADER_COMPETITOR => self.header.open(),
            S_PLAYERS => self.players.open(),
            S_PLAYERS_ATHLETE => {
                if let Some(team) = self.players.current() {
                    team.row = AthleteRow {
                        active: true,
                        ..AthleteRow::default()
                    };
                }
            }
            _ => {}
        }
        Directive::Continue
    }

    fn leave(&mut self, pattern: usize, _indices: &[u16]) -> Directive {
        if !self.live {
            return Directive::Continue;
        }
        let overflowed = match pattern {
            S_PLAY => {
                self.fold_play();
                false
            }
            S_HEADER_COMPETITOR => self.header.close(),
            S_PLAYERS_ATHLETE => {
                self.fold_athlete();
                false
            }
            S_PLAYERS => {
                let rows_overflowed = self.players.current().is_some_and(|team| team.overflowed);
                self.players.close() || rows_overflowed
            }
            _ => false,
        };
        if overflowed {
            self.quirks.quirk(Quirk::BoundedOverflow);
        }
        Directive::Continue
    }
}

/// What the summary pass produced, with the quirk receiver handed back.
/// Nothing in a summary fails the game: a malformed region degrades the
/// fields that depended on it and the rest stands.
#[derive(Debug)]
pub struct SummaryReport<Q> {
    pub summary: Summary,
    pub quirks: Q,
}

/// Streams one per-event summary body (110 KB at the tip, 513 KB at the end
/// of a long game) down to §3.3–3.7's facts, keeping only what those rules
/// need (the module docs list the bounds).
pub struct SummaryExtractor<'scratch, Q: Quirks> {
    matcher: StreamMatcher<'static, 'scratch, SummarySink<Q>>,
}

impl<'scratch, Q: Quirks> SummaryExtractor<'scratch, Q> {
    /// `context` comes from [`Extract::summary_context`] on the scoreboard
    /// extract of the same game; `scratch` must hold the longest contiguous
    /// string token (play texts and link URLs — kilobytes).
    pub fn new(
        context: SummaryContext,
        quirks: Q,
        scratch: &'scratch mut [u8],
    ) -> Result<Self, Error> {
        Ok(Self {
            matcher: StreamMatcher::new(SUMMARY_TABLE, SummarySink::new(context, quirks), scratch)?,
        })
    }

    pub fn write(&mut self, chunk: &[u8]) -> Result<(), Error> {
        self.matcher.write(chunk)
    }

    /// Finish the document; only the stream itself can fail.
    pub fn finish(self) -> Result<SummaryReport<Q>, Error> {
        Ok(self.matcher.finish()?.resolve())
    }
}
