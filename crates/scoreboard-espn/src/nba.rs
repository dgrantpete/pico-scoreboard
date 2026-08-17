//! NBA extraction tables — the S1 sport lane over the shared path engine.
//!
//! One const path table over the ESPN scoreboard body (`$.events[*]...`),
//! one [`Extractor`] sink with the backend's semantics, and one owned
//! bounded [`Extract`] whose [`Extract::as_game`] view feeds
//! `scoreboard_wire::nba::encode` byte-identically to the backend's
//! `types.rs → transform.rs → wire.rs` pipeline (DESIGN.md parity contract;
//! field inventory in the S1 NBA report, sources
//! `backend/src/nba/{types,transform,wire}.rs`).
//!
//! Semantics reproduced here, with their rulings:
//!
//! - **Rejection parity (ruling 1).** Event finalize validates the same
//!   required set the backend's serde + DU conversion does, per state: a
//!   pregame event missing `status.displayClock` is dropped even though
//!   nothing reads it. Deserialize-tier failures skip the event and count
//!   in [`ScanStats::failed`]; transform-tier failures (bad hex color,
//!   unparseable score/date, two home markers) are [`DetailOutcome::Rejected`]
//!   — the games list still serves such an event, exactly like the backend,
//!   whose list never runs the transform.
//! - **Field-order independence (ruling 4).** Every matched value is
//!   buffered in a per-event scratch and resolved only when the event
//!   element closes; home/away resolves from the buffered markers via
//!   `order_home_away`, never from array position (all seven fixtures put
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

use crate::common::{
    EText, HomeAway, LivePhase, Quirk, Quirks, linescore_byte, num_i16, num_u8, order_home_away,
    parse_hex_rgb, parse_live_phase, parse_record, parse_start_time, saturate_score, set_text,
    stable_sort_by_key, wire_phase,
};
use crate::path::{ContainerKind, Directive, Pattern, Seg, Sink, Value};
use scoreboard_wire::{self as wire, GameState, MAX_LINE_SCORE, Record, TeamColors};

use Seg::{AnyIndex, Index, Key};

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

/// The NBA scoreboard path table. 18 wire-relevant leaves plus the
/// parse-gating-only fields (`team.id`, `weather.*`, `shortDisplayName`) the
/// backend's shared serde structs require to deserialize — rejection parity
/// needs them even though nothing reads their values (ruling 1, inventory §6).
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
];

// ------------------------------------------------------------- the extract

/// One extracted NBA game: owned, bounded, domain-shaped. String bounds are
/// the wire cap ([`EText`] = 255 bytes, ruling 2), scores stay `u32` like
/// the backend's domain model (the wire view saturates), line scores are
/// already sorted/clamped bytes like `NbaFinalTeam::line_score`.
#[derive(Debug, Clone)]
pub struct Extract {
    pub game_id: EText,
    pub kind: Kind,
}

#[expect(
    clippy::large_enum_variant,
    reason = "bounded owned variants are the design; boxing needs alloc"
)]
#[derive(Debug, Clone)]
pub enum Kind {
    Pregame(Pregame),
    Live(Live),
    Final(Final),
}

#[derive(Debug, Clone)]
pub struct Pregame {
    /// Unix epoch seconds, UTC (`parse_start_time`).
    pub start_time: u32,
    pub venue: EText,
    pub away: PregameTeam,
    pub home: PregameTeam,
}

#[derive(Debug, Clone)]
pub struct PregameTeam {
    pub abbreviation: EText,
    pub colors: TeamColors,
    /// `None` when ESPN omits or malforms the `type == "total"` record —
    /// a cleared flag bit on the wire, not a zero record.
    pub record: Option<Record>,
}

#[derive(Debug, Clone)]
pub struct Live {
    pub period: u8,
    pub phase: LivePhase,
    /// ESPN's `displayClock`, verbatim — the colonless sub-minute shape is
    /// the device's crunch-time signal and must never be reformatted.
    pub clock: EText,
    pub away: LiveTeam,
    pub home: LiveTeam,
    /// Absent before the opening tip (`situation: {}` is a real capture).
    pub last_play: Option<LastPlay>,
}

#[derive(Debug, Clone)]
pub struct LiveTeam {
    pub abbreviation: EText,
    /// Parsed from ESPN's string score; saturated to `u16` only at the
    /// wire view, like the backend's `saturate_score`.
    pub score: u32,
    pub colors: TeamColors,
}

#[derive(Debug, Clone)]
pub struct LastPlay {
    pub id: EText,
    pub text: EText,
}

#[derive(Debug, Clone)]
pub struct Final {
    /// `status.period`: 4 in regulation, more with overtime.
    pub periods_played: u8,
    pub away: FinalTeam,
    pub home: FinalTeam,
}

#[derive(Debug, Clone)]
pub struct FinalTeam {
    pub abbreviation: EText,
    pub score: u32,
    pub colors: TeamColors,
    /// Sorted by period, clamped to bytes. Capacity is the wire's own
    /// `MAX_LINE_SCORE` (255) so the extract is never the tighter bound.
    pub line_score: heapless::Vec<u8, MAX_LINE_SCORE>,
}

impl Extract {
    /// Borrowed wire-shaped view over the extract's own storage; encoding
    /// it with `scoreboard_wire::nba::encode` is the parity gate.
    pub fn as_game(&self) -> wire::nba::Game<'_> {
        match &self.kind {
            Kind::Pregame(game) => wire::nba::Game::Pregame(wire::nba::Pregame {
                game_id: self.game_id.as_str(),
                start_time: game.start_time,
                venue: game.venue.as_str(),
                away: wire_pregame_team(&game.away),
                home: wire_pregame_team(&game.home),
            }),
            Kind::Live(game) => wire::nba::Game::Live(wire::nba::Live {
                game_id: self.game_id.as_str(),
                period: game.period,
                phase: wire_phase(game.phase),
                clock: game.clock.as_str(),
                away: wire_live_team(&game.away),
                home: wire_live_team(&game.home),
                last_play: game.last_play.as_ref().map(|play| wire::LastPlay {
                    id: play.id.as_str(),
                    text: play.text.as_str(),
                }),
            }),
            Kind::Final(game) => wire::nba::Game::Final(wire::nba::Final {
                game_id: self.game_id.as_str(),
                periods_played: game.periods_played,
                away: wire_final_team(&game.away),
                home: wire_final_team(&game.home),
            }),
        }
    }
}

fn wire_pregame_team(team: &PregameTeam) -> wire::nba::PregameTeam<'_> {
    wire::nba::PregameTeam {
        abbreviation: team.abbreviation.as_str(),
        colors: team.colors,
        record: team.record,
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
    competitor_count: u16,
    competitor_scalar: bool,
    competitors: [CompetitorScratch; 2],
    entry: EntryScratch,
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
    /// `Some(None)` = present but not `W-L` (quirk on the pregame arm),
    /// `Some(Some(_))` = parsed record.
    total: Option<Option<(u16, u16)>>,
    linescores_bad: bool,
    /// `(period, clamped byte)` in arrival order; stable-sorted at
    /// transform. Capacity is the wire's `MAX_LINE_SCORE`.
    line: heapless::Vec<(u8, u8), MAX_LINE_SCORE>,
    line_clipped: bool,
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
/// `StreamMatcher::new(nba::PATHS, extractor, scratch)`, feed the body, then
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
        // the TryFrom), so the unknown-label quirk fires in both modes.
        let phase = match s.state {
            Some(StateKind::In) => {
                parse_live_phase(s.desc_seen.then_some(s.desc.as_str()), &mut *self.quirks)
            }
            _ => LivePhase::InProgress,
        };

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
                let record = valid.then(|| parse_record(self.scratch.entry.record_summary.as_str()));
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
/// DU conversion's per-state requirements (`backend/src/nba/types.rs`).
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

    let kind = match state {
        StateKind::Pre => {
            let start_time = s.start_time.ok_or(TransformError::StartTime)?;
            // Home before away: the backend builds its struct in that
            // order, which fixes the malformed-record quirk order.
            let home = pregame_team(home, quirks)?;
            let away = pregame_team(away, quirks)?;
            Kind::Pregame(Pregame {
                start_time,
                venue: s.venue_name.clone(),
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
                last_play: s.last_play_seen.then(|| LastPlay {
                    id: s.play_id.clone(),
                    text: s.play_text.clone(),
                }),
            })
        }
        StateKind::Post => {
            let home = final_team(home)?;
            let away = final_team(away)?;
            Kind::Final(Final {
                periods_played: s.period,
                away,
                home,
            })
        }
    };
    Ok(Extract {
        game_id: s.id.clone(),
        kind,
    })
}

fn colors(competitor: &CompetitorScratch) -> Result<TeamColors, TransformError> {
    Ok(TeamColors {
        primary: competitor.color.ok_or(TransformError::Color)?,
        alternate: competitor.alternate.ok_or(TransformError::Color)?,
    })
}

fn pregame_team<Q: Quirks>(
    competitor: &CompetitorScratch,
    quirks: &mut Q,
) -> Result<PregameTeam, TransformError> {
    let record = match competitor.total {
        None => None,
        Some(Some((wins, losses))) => Some(Record { wins, losses }),
        Some(None) => {
            // A `total` entry that is not `W-L`: warn-and-drop, never an
            // error — the backend's `parse_record`.
            quirks.quirk(Quirk::MalformedRecord);
            None
        }
    };
    Ok(PregameTeam {
        abbreviation: competitor.abbreviation.clone(),
        colors: colors(competitor)?,
        record,
    })
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

/// The backend's `linescore_bytes` ordering: a **stable** sort by period —
/// duplicate periods keep arrival order (ruling 13), gaps are not filled.
fn sorted_line(
    line: &heapless::Vec<(u8, u8), MAX_LINE_SCORE>,
) -> heapless::Vec<u8, MAX_LINE_SCORE> {
    let mut pairs = line.clone();
    stable_sort_by_key(&mut pairs, |&(period, _)| period);
    pairs.iter().map(|&(_, byte)| byte).collect()
}
