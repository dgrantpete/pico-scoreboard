//! HOCKEY extraction tables — the NHL lane (firmware-rs/NHL.md §3 is the
//! normative semantics; this module implements it over the shared engine).
//!
//! Two streamed documents feed one payload. The league SCOREBOARD body
//! (`{"events":[…]}`, [`GameExtractor`] / [`ListExtractor`]) carries the
//! state, the clock, the scores, the stars and the shots identity; the
//! per-event SUMMARY body ([`SummaryExtractor`], fetched for live and final
//! games — D3) carries what the scoreboard cannot: strength, the goals, the
//! flash, the shootout and the reliable shots. Both produce owned extracts
//! and [`Extract::as_game`] combines them into the wire view, degrading to
//! the scoreboard alone when the summary fetch failed. The summary extractor
//! takes a [`SummaryContext`] from the scoreboard extract because it must know
//! the season kind *while streaming* — a period-5 scoring play is a goal in
//! the playoffs and a shootout attempt in the regular season, and ruling 4
//! forbids waiting for the summary header to say which.
//!
//! # Required fields
//!
//! Hockey has no serde predecessor, so there is no rejection parity to keep
//! (ruling 1); the rule is written here once. An event is **dropped**
//! (counted failed, exactly as the other lanes count a deserialize failure)
//! when a field the screens cannot do without is missing or has the wrong
//! type:
//!
//! - every event: `id` (string), `date` (string), `competitions` (array);
//! - with a competition: `status.type.state` (one of `pre`/`in`/`post`),
//!   exactly two `competitors`, each with `homeAway`, `team.id`,
//!   `team.abbreviation` and `team.color` (strings);
//! - live: `status.period` (number) and `status.displayClock` (string);
//! - live and final: each competitor's `score` (string);
//! - every state: a `linescores[*]` entry, when there is one, carries a
//!   numeric `value` — skipping a bad entry would shift every later period
//!   left, so the NBA lane's rule applies and the event is dropped.
//!
//! An event whose `competitions` array is empty is clean but unservable
//! ([`GameOutcome::NoCompetition`]). A found target is **rejected**
//! ([`TransformError`], the backend's 5xx) when a required value fails to
//! parse: a color that is not six hex digits, a score that is not a `u32`, a
//! pregame `date` that is not a start time, two home or two away markers.
//! `team.alternateColor` is the one color that may be absent: ESPN sent none
//! for the NHL through 2026-10-14 (0/484 at design time, 19 of the 20
//! fixtures), then one for every team from 2026-10-14 23:44 UTC — a field
//! the feed went a whole season without is not one to drop games over, so an
//! absent alternate reads as the primary. A string that is not six hex
//! digits is the Color rejection, like the primary's; any other type reads
//! as absent, like every optional field (the other lanes drop the event
//! there — a serde-era rule hockey has no reason to keep). Everything else
//! degrades: a wrong type on an optional field reads as absent, an unknown
//! status name follows its state with a quirk, a malformed record is a quirk
//! and no record, and nothing in the summary can fail the game.
//!
//! # The summary tables
//!
//! `plays[*]`: `id`, `type.{id,penaltyType,penaltyMinutes}`, `text`,
//! `period.number`, `clock.displayValue`, `team.id`, `scoringPlay`,
//! `participants[*].{type,athlete.id,athlete.shortName}`, `coordinate.{x,y}`,
//! `strength.id`, `shotInfo.id`. `onIce[*]`: `teamId`, `entries[*].athleteid`.
//! `boxscore.players[*]`: `team.id`, `statistics[*].name`,
//! `statistics[*].athletes[*].athlete.id` (the goalie group, and every id
//! any group lists — §3.4's listed players).
//! `boxscore.teams[*]`: `team.id`, `statistics[*].{name,displayValue}`
//! (`shotsTotal`). `header.competitions[0].competitors[*]`: `team.id`,
//! `homeAway` — the one team-id → side mapping every other summary reference
//! resolves through, at finish, whichever order the document arrived in.
//!
//! # Bounds
//!
//! Every pool is sized from the game, and overflow degrades rather than
//! panics: goals keep the newest [`MAX_GOALS`] (the wire's cap, §3.5);
//! shootout attempts keep the first [`ATTEMPT_MAX`] in sequence with a
//! [`Quirk::BoundedOverflow`] past it; each attempt remembers
//! [`ATTEMPT_ATHLETES`] participant ids for the goalie fallback, extra ones
//! dropped silently; each `onIce` side stores [`ON_ICE_IDS`] athlete ids
//! (more entries than that are insane whether or not a goalie is among
//! them); each team's goalie group holds [`GOALIES_PER_TEAM`] ids, a fifth
//! is a quirk; each team's listed players fill [`LISTED_PER_TEAM`] — past
//! it the relief-goalie inference is suspended for that team with a quirk,
//! because an unlisted id proves nothing once the listing is incomplete, and
//! a real skater must never read as unlisted; a third `onIce` block, player
//! group, boxscore team or header competitor is a quirk and ignored. Line
//! scores use the wire's own [`MAX_LINE_SCORE`]. The games list holds
//! [`LIST_MAX`].

use core::fmt::Write as _;

use scoreboard_wire::hockey::{
    Coordinate, Final as WireFinal, Game as WireGame, Goal, GoalTag, Goals, Live as WireLive,
    Pregame as WirePregame, PregameTeam as WirePregameTeam, Record, Shootout, ShootoutAttempts,
    Shots, Star as WireStar, Strength, StrengthKind, MAX_COORDINATE_X, MAX_COORDINATE_Y, MAX_GOALS,
    MAX_SHOOTOUT_ATTEMPTS,
};
use scoreboard_wire::{
    FinalTeam as WireFinalTeam, GameState, LastPlay as WireLastPlay, Side, TeamColors, TeamState,
    MAX_LINE_SCORE,
};

use crate::common::{
    compose, crest_path, linescore_byte, num_u16, num_u8, order_home_away, parse_hex_rgb,
    parse_start_time, saturate_score, wire_phase, CrestPath, Crests, EText, Exact, HomeAway,
    IgnoreQuirks, LivePhase, Quirk, Quirks, WireText,
};
use crate::path::{ContainerKind, Directive, Error, Pattern, Seg, Sink, StreamMatcher, Value};

use Seg::{AnyIndex, Index, Key};

// ---------------------------------------------------------------- bounds

/// ESPN ids are numeric strings: event ids run 9 digits ("401892505"), team
/// ids up to 6 ("129764"), athlete ids up to 7 ("5188366"). As compare keys
/// they refuse to truncate (ruling 16) and an overlong one matches nothing.
pub const ID_BYTES: usize = 24;
/// Scores parse to `u32` (10 digits); margin for a sign or leading zeros.
const SCORE_BYTES: usize = 16;
/// A valid color is at most 7 bytes (`#RRGGBB`); longer fails the parse.
const COLOR_BYTES: usize = 8;
/// Valid `event.date` forms are at most 20 bytes.
const DATE_BYTES: usize = 32;
/// The longest known `status.type.name` is 18 bytes (STATUS_IN_PROGRESS);
/// an overlong one is an unknown name.
const STATUS_NAME_BYTES: usize = 32;
/// `altDetail` is "OT", "SO" or "{n}OT"; anything longer cannot match.
const ALT_DETAIL_BYTES: usize = 8;
/// The NHL plays at most 16 games a night; 2× margin. Overflow is reported
/// through [`GamesList::overflowed`].
pub const LIST_MAX: usize = 32;
/// Shootout attempts kept in sequence: 20 rounds, the longest shootout in
/// NHL history (FLA–WSH, 2014-12-16). Past it the markers are frozen anyway
/// (§3.8 keeps the first 16 per side); only the next shooter could go stale.
pub const ATTEMPT_MAX: usize = 40;
/// Participant ids remembered per attempt for the goalie fallback. An attempt
/// lists a shooter and a goalie, or a scorer alone; a fourth is unheard of.
pub const ATTEMPT_ATHLETES: usize = 3;
/// Athlete ids stored per `onIce` side. A full unit is six (five skaters and
/// the goalie); seven entries read as six skaters only if the goalie is among
/// them, which needs all seven ids; eight or more are insane regardless, so
/// the goalie check never needs more than this.
pub const ON_ICE_IDS: usize = 8;
/// Goalie ids per team: two dress, three with an emergency backup.
pub const GOALIES_PER_TEAM: usize = 4;
/// Listed players per team, every `boxscore.players` group taken together
/// and deduplicated (the skaters group repeats the forwards and defenses):
/// a dressed roster is 20, with margin for scratches the boxscore carries.
/// Past it the set is incomplete, so "not listed" no longer means "not a
/// skater" and §3.4's relief-goalie inference is suspended for that team
/// (with a quirk) — a real skater must never read as the goalie, which
/// would hide a real empty net.
pub const LISTED_PER_TEAM: usize = 32;
/// Two teams play. Pools keyed by team hold this many and quirk on a third.
pub const TEAMS: usize = 2;

/// An ESPN athlete id the scoreboard hands to the summary pass (§3.4's
/// probable starting goalie), in the ids' own compare-key storage.
pub type AthleteId = heapless::String<ID_BYTES>;

// ------------------------------------------------------------ ESPN codes

/// Play types (`plays[*].type.id`, `situation.lastPlay.type.id`).
const TYPE_GOAL: u16 = 505;
const TYPE_SHOT: u16 = 506;
const TYPE_MISSED: u16 = 507;
/// 2025-26's generic penalty; 2026-27 uses the infraction as the type.
const TYPE_PENALTY_2025: u16 = 509;
/// Every 2026-27 penalty is first posted under this type and rewritten to the
/// real infraction under the same play id 10–30 s later (§2).
const TYPE_PLACEHOLDER_PENALTY: u16 = 143;
const TYPE_SHOOTOUT_END: u16 = 521;
const TYPE_FAILED_ATTEMPT: u16 = 2010;
/// The rows that end a pending penalty (§3.4 step 2b): play resumed at a
/// face-off, a period boundary, a goal. 3003 is a boundary the spec lists;
/// the corpus has none.
const TYPE_FACE_OFF: u16 = 502;
const TYPE_PERIOD_START: u16 = 518;
const TYPE_PERIOD_END: u16 = 519;
const TYPE_GAME_END: u16 = 522;
const TYPE_PERIOD_BOUNDARY: u16 = 3003;
/// `strength.id`, relative to the play's team; `shotInfo.id` shares 903.
const STRENGTH_POWER_PLAY: u16 = 702;
const STRENGTH_SHORT_HANDED: u16 = 703;
const STRENGTH_EMPTY_NET: u16 = 903;
const SHOT_INFO_EMPTY_NET: u16 = 903;
/// `season.type`: 2 is the regular season (preseason too), 3 the playoffs.
const SEASON_POSTSEASON: u8 = 3;
/// Period 5 and up is the shootout — in the regular season only (§3.1).
const FIRST_SHOOTOUT_PERIOD: u8 = 5;
const LAST_REGULATION_PERIOD: u8 = 3;
const FIRST_OVERTIME_PERIOD: u8 = 4;
/// A full unit skates five; a pulled goalie's unit is six attackers.
const FULL_SKATERS: u8 = 5;
const PULLED_UNIT: u8 = 6;

// ---------------------------------------------------------------- extracts

/// One transformed hockey scoreboard event — owned, bounded, domain-shaped.
/// The backend adapter builds its DTO from these fields; the wire view is
/// [`Extract::as_game`], with or without a [`Summary`].
#[derive(Debug, Clone, PartialEq)]
pub struct Extract {
    pub game_id: EText,
    /// Direct mode's crest sources; outside the wire view by design.
    pub crests: Crests,
    /// `season.type` 3 — what tells a regular-season shootout from a playoff
    /// second overtime at period 5 (§3.1).
    pub playoffs: bool,
    /// Each side's probable starting goalie by `athlete.id`, `[away, home]`:
    /// the summary pass's input to §3.4 (the goalie the boxscore group lacks
    /// at puck drop and after a change), outside the wire view.
    pub probable_goalies: [Option<AthleteId>; TEAMS],
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq)]
// The variants intentionally differ in size and boxing is unavailable
// (no_std, no alloc); one extract exists per polled game.
#[allow(clippy::large_enum_variant)]
pub enum Kind {
    Pregame(Pregame),
    Live(Live),
    Final(Final),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pregame {
    /// Unix epoch seconds UTC, from `event.date`.
    pub start_time: u32,
    pub venue: EText,
    /// `notes[0].headline` + " - " + `series.summary` in the playoffs, else
    /// empty (§3.9). A playoff event missing one half carries the other.
    pub info_secondary: EText,
    pub away: PregameTeam,
    pub home: PregameTeam,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PregameTeam {
    pub abbreviation: EText,
    pub colors: TeamColors,
    /// W-L-OTL from the first `records[]` entry of type `total` or `ytd`;
    /// `None` when there is none or it is malformed (quirk).
    pub record: Option<Record>,
    /// The probable starting goalie's `shortName` (`probables[]`).
    pub goalie: Option<EText>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Live {
    /// ESPN's raw period: 1–3, 4 the first overtime, 5 a regular-season
    /// shootout or a playoff second overtime, and on. Never 0 under
    /// STATUS_DELAYED — that is a [`Kind::Pregame`] (§3.1).
    pub period: u8,
    /// `EndOfPeriod` for STATUS_END_PERIOD, the intermission; else in play.
    pub phase: LivePhase,
    /// `displayClock` verbatim, never extrapolated (§3.2).
    pub clock: EText,
    pub away: LiveTeam,
    pub home: LiveTeam,
    /// The scoreboard identity `saves(other) + goals(side)` (§3.3 source 2);
    /// absent until both teams have game statistics.
    pub shots: Option<Shots>,
    /// `situation.lastPlay`, filtered to goals and eligible penalties (§3.6)
    /// — the flash when there is no summary.
    pub last_play: Option<LastPlay>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LiveTeam {
    pub abbreviation: EText,
    /// Parsed from ESPN's string; saturated to `u16` at the wire view.
    pub score: u32,
    pub colors: TeamColors,
}

/// A flash line: the play id the device compares between polls, and its
/// text, already prefixed `GOAL! ` for a goal (§3.6).
#[derive(Debug, Clone, PartialEq)]
pub struct LastPlay {
    pub id: EText,
    pub text: EText,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Final {
    /// `status.period`: 3, 4 after overtime, 5 after a shootout or a second
    /// playoff overtime.
    pub periods_played: u8,
    pub away: FinalTeam,
    pub home: FinalTeam,
    /// The scoreboard identity, except when both teams report `saves` "0" —
    /// the post-final quirk — which counts as missing (§3.3).
    pub shots: Option<Shots>,
    /// `featuredAthletes` first/second/thirdStar, all three or none (§3.10).
    pub stars: Option<[Star; 3]>,
    /// `series.summary`, playoffs only.
    pub series: Option<EText>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FinalTeam {
    pub abbreviation: EText,
    pub score: u32,
    pub colors: TeamColors,
    /// `linescores[*].value` in ESPN's order, clamped to bytes: one entry per
    /// period played, each overtime its own, the shootout a fifth of 1/0.
    /// The model collapses the overtimes (§3.10).
    pub line_score: heapless::Vec<u8, MAX_LINE_SCORE>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Star {
    pub name: EText,
    /// From the entry's `team.id`; `None` when it matched neither team.
    pub side: Option<Side>,
}

/// What the summary pass contributes to a live or final payload (§3.4–3.8),
/// already resolved to sides. Combined with the scoreboard extract by
/// [`Extract::as_game`].
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    /// `boxscore.teams[].statistics[name=shotsTotal]`, both teams or neither
    /// (§3.3 source 1).
    pub shots: Option<Shots>,
    /// §3.4; `None` when the context asked for none (a final, a live
    /// shootout) or nothing could be derived.
    pub strength: Option<Strength>,
    /// The newest [`MAX_GOALS`] goals outside the shootout, oldest first,
    /// coordinates normalized so away attacks +x (§3.5).
    pub goals: heapless::Vec<Goal, MAX_GOALS>,
    /// The scorer of the newest goal kept; empty when there is none.
    pub last_scorer: EText,
    /// The newest eligible goal or penalty play (§3.6).
    pub last_play: Option<LastPlay>,
    /// Present once any shootout-period play was seen (§3.8).
    pub shootout: Option<Shootout>,
}

/// What the summary extractor needs to know about the game before it reads
/// a byte. Built by [`Extract::summary_context`] from the scoreboard pass,
/// which the backend runs first anyway — the same shape as football's
/// `is_college` call parameter (ruling 8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryContext {
    /// `season.type` 3: period 5 is a second overtime, never the shootout.
    playoffs: bool,
    /// The scoreboard's state. Strength is derived for a live game only: a
    /// final's `onIce` is a stale unit (or the shootout's goalies) and
    /// nothing on the final screen reads it.
    state: GameState,
    /// The scoreboard says the shootout is running (regular season, live,
    /// period ≥ 5): `onIce` holds only the goalies and §3.4 derives no
    /// strength at all — not even a quirk.
    live_shootout: bool,
    /// The intermission: `onIce` freezes the last unit through the break,
    /// an extra attacker included, so a pulled reading is not believed
    /// there (§3.4 step 3).
    end_of_period: bool,
    /// Each side's probable starting goalie — a known goalie before the
    /// boxscore group lists anyone and after a change it has not caught up
    /// with (§3.4 step 1).
    probable_goalies: [Option<AthleteId>; TEAMS],
    /// `status.period`: the pull test reads it (§3.4 step 2a — the
    /// shorthanded trailing pull needs period ≥ 3, overtime its own count).
    period: u8,
    /// The scoreboard's scores, `[away, home]`: a goalie-less unit is
    /// believed pulled only when its team trails or a penalty is pending
    /// against the other (§3.4 step 2b).
    scores: [u32; TEAMS],
}

impl Extract {
    /// The crest paths for this game's two teams.
    pub fn crests(&self) -> &Crests {
        &self.crests
    }

    pub fn state(&self) -> GameState {
        match self.kind {
            Kind::Pregame(_) => GameState::Pregame,
            Kind::Live(_) => GameState::Live,
            Kind::Final(_) => GameState::Final,
        }
    }

    /// Regular season, live, period ≥ 5: the shootout is on (§3.1).
    fn live_shootout(&self) -> bool {
        matches!(&self.kind, Kind::Live(live) if !self.playoffs && live.period >= FIRST_SHOOTOUT_PERIOD)
    }

    /// What [`SummaryExtractor::new`] needs — the only way to build one.
    pub fn summary_context(&self) -> SummaryContext {
        let (period, scores) = match &self.kind {
            Kind::Live(live) => (live.period, [live.away.score, live.home.score]),
            Kind::Final(game) => (game.periods_played, [game.away.score, game.home.score]),
            Kind::Pregame(_) => (0, [0, 0]),
        };
        SummaryContext {
            playoffs: self.playoffs,
            state: self.state(),
            live_shootout: self.live_shootout(),
            end_of_period: matches!(&self.kind, Kind::Live(live) if live.phase == LivePhase::EndOfPeriod),
            probable_goalies: self.probable_goalies.clone(),
            period,
            scores,
        }
    }

    /// A borrowed wire-shaped view over the extract's own storage — and the
    /// summary's, when the backend has one. Without it the view is the
    /// scoreboard alone (D3's degrade): no strength, no goals, no shootout
    /// tracker, the identity shots, the scoreboard's flash.
    pub fn as_game<'a>(&'a self, summary: Option<&'a Summary>) -> WireGame<'a> {
        match &self.kind {
            Kind::Pregame(game) => WireGame::Pregame(WirePregame {
                game_id: self.game_id.as_str(),
                start_time: game.start_time,
                venue: game.venue.as_str(),
                info_secondary: game.info_secondary.as_str(),
                playoffs: self.playoffs,
                away: wire_pregame_team(&game.away),
                home: wire_pregame_team(&game.home),
            }),
            Kind::Live(game) => {
                let shootout_live = self.live_shootout();
                // ESPN's live score often already carries the shootout
                // winner's +1 (§2); the game score stays the tied one (§3.8).
                let (away_score, home_score) = if shootout_live {
                    let tied = game.away.score.min(game.home.score);
                    (tied, tied)
                } else {
                    (game.away.score, game.home.score)
                };
                WireGame::Live(WireLive {
                    game_id: self.game_id.as_str(),
                    period: game.period,
                    phase: wire_phase(game.phase),
                    clock: game.clock.as_str(),
                    playoffs: self.playoffs,
                    away: TeamState {
                        abbreviation: game.away.abbreviation.as_str(),
                        score: saturate_score(away_score),
                        colors: game.away.colors,
                    },
                    home: TeamState {
                        abbreviation: game.home.abbreviation.as_str(),
                        score: saturate_score(home_score),
                        colors: game.home.colors,
                    },
                    shots: summary.and_then(|s| s.shots).or(game.shots),
                    strength: if shootout_live {
                        None
                    } else {
                        summary.and_then(|s| s.strength)
                    },
                    goals: Goals::new(summary.map_or(&[], |s| s.goals.as_slice())),
                    last_scorer: summary.map_or("", |s| s.last_scorer.as_str()),
                    // Present while the shootout runs — with no attempts yet
                    // when the summary has none — and never without the
                    // summary that tracks it.
                    shootout: if shootout_live {
                        summary.map(|s| s.shootout.unwrap_or(NO_SHOOTOUT))
                    } else {
                        None
                    },
                    // One source per payload: the summary's newest eligible
                    // play when there is a summary, else the scoreboard's.
                    last_play: match summary {
                        Some(s) => s.last_play.as_ref(),
                        None => game.last_play.as_ref(),
                    }
                    .map(|play| WireLastPlay {
                        id: play.id.as_str(),
                        text: play.text.as_str(),
                    }),
                })
            }
            Kind::Final(game) => WireGame::Final(WireFinal {
                game_id: self.game_id.as_str(),
                periods_played: game.periods_played,
                playoffs: self.playoffs,
                away: wire_final_team(&game.away),
                home: wire_final_team(&game.home),
                shots: summary.and_then(|s| s.shots).or(game.shots),
                stars: game.stars.as_ref().map(|stars| {
                    [
                        wire_star(&stars[0]),
                        wire_star(&stars[1]),
                        wire_star(&stars[2]),
                    ]
                }),
                series: game.series.as_deref(),
            }),
        }
    }
}

/// A shootout the scoreboard reports and the summary has no plays for yet.
const NO_SHOOTOUT: Shootout = Shootout {
    away: ShootoutAttempts {
        taken: 0,
        scored: 0,
    },
    home: ShootoutAttempts {
        taken: 0,
        scored: 0,
    },
    next_shooter: None,
};

fn wire_pregame_team(team: &PregameTeam) -> WirePregameTeam<'_> {
    WirePregameTeam {
        abbreviation: team.abbreviation.as_str(),
        colors: team.colors,
        record: team.record,
        goalie: team.goalie.as_deref(),
    }
}

fn wire_final_team(team: &FinalTeam) -> WireFinalTeam<'_> {
    WireFinalTeam {
        abbreviation: team.abbreviation.as_str(),
        score: saturate_score(team.score),
        colors: team.colors,
        line_score: &team.line_score,
    }
}

fn wire_star(star: &Star) -> WireStar<'_> {
    WireStar {
        name: star.name.as_str(),
        side: star.side,
    }
}

// --------------------------------------------------------------- semantics

/// Side `0 = away, 1 = home`, the index every per-side pair below uses.
fn slot(side: Side) -> usize {
    match side {
        Side::Away => 0,
        Side::Home => 1,
    }
}

fn other(side: Side) -> Side {
    match side {
        Side::Away => Side::Home,
        Side::Home => Side::Away,
    }
}

/// Owned [`EText`] from an already-wire-truncated source.
fn owned(text: &str) -> EText {
    let mut out = EText::new();
    let _ = out.push_str(text);
    out
}

/// A team id against the two competitors', home compared first.
fn side_of(
    team_id: &Exact<ID_BYTES>,
    home_id: Option<&str>,
    away_id: Option<&str>,
) -> Option<Side> {
    let id = team_id.valid()?;
    if Some(id) == home_id {
        Some(Side::Home)
    } else if Some(id) == away_id {
        Some(Side::Away)
    } else {
        None
    }
}

/// Two ids are the same only when both are valid and equal — an unseen or
/// overflowed key equals nothing, not even another unseen one.
fn same_id(a: &Exact<ID_BYTES>, b: &Exact<ID_BYTES>) -> bool {
    matches!((a.valid(), b.valid()), (Some(a), Some(b)) if a == b)
}

/// Hockey's record is W-L-OTL, always (§2): exactly three `u16` fields.
pub fn parse_record_otl(summary: &str) -> Option<Record> {
    let mut parts = summary.split('-');
    let record = Record {
        wins: parts.next()?.parse().ok()?,
        losses: parts.next()?.parse().ok()?,
        ot_losses: parts.next()?.parse().ok()?,
    };
    if parts.next().is_some() {
        return None;
    }
    Some(record)
}

/// `clock.displayValue` is elapsed `M:SS` (`"12:49"`); the goal strip shows
/// seconds elapsed (§3.2).
fn elapsed_seconds(text: &str) -> Option<u16> {
    let (minutes, seconds) = text.split_once(':')?;
    let (minutes, seconds) = (num_u16(minutes)?, num_u16(seconds)?);
    if seconds >= 60 {
        return None;
    }
    minutes.checked_mul(60)?.checked_add(seconds)
}

/// A code field: ESPN sends ids as strings (`"505"`), the engine hands
/// numbers as raw text; both are an unsigned literal that fits.
fn code(value: Value<'_>) -> Option<u16> {
    match value {
        Value::Str(text) | Value::Num(text) => num_u16(text),
        _ => None,
    }
}

/// A coordinate in feet from center ice, rounded and pulled back to the
/// boards so the extract equals what the wire encodes.
fn coordinate_value(value: Value<'_>, limit: i8) -> Option<i8> {
    let Value::Num(text) = value else {
        return None;
    };
    let value: f64 = text.parse().ok().filter(|v: &f64| v.is_finite())?;
    // Round half away from zero by hand: `f64::round` is std-only. The `as`
    // cast saturates, so no value can overflow on the way to the clamp.
    let rounded = if value >= 0.0 {
        (value + 0.5) as i32
    } else {
        (value - 0.5) as i32
    };
    Some(rounded.clamp(-i32::from(limit), i32::from(limit)) as i8)
}

/// §3.5's normalization: away attacks +x. Away plots at `(|x|, y·sign(x))`,
/// home at `(−|x|, −y·sign(x))` — a 180° rotation when the team attacked −x,
/// which keeps handedness. `sign(0)` is +1: a goal from the center line has
/// no end to infer, so it is left unrotated rather than flattened onto the
/// axis.
fn normalize(side: Side, x: i8, y: i8) -> Coordinate {
    // Both are within ±100 / ±43 by `coordinate_value`, so neither `abs` nor
    // the negation can overflow.
    let sign: i8 = if x < 0 { -1 } else { 1 };
    let (ax, sy) = (x.abs(), y.saturating_mul(sign));
    match side {
        Side::Away => Coordinate { x: ax, y: sy },
        Side::Home => Coordinate { x: -ax, y: -sy },
    }
}

/// A penalty play: the 2026-27 shape carries `penaltyType`; 2025-26 was type
/// 509; the 143 placeholder is a penalty's first appearance (§3.4, §3.6).
fn is_penalty(type_id: Option<u16>, penalty_type: bool) -> bool {
    penalty_type || matches!(type_id, Some(TYPE_PENALTY_2025 | TYPE_PLACEHOLDER_PENALTY))
}

/// §3.6's eligibility: `Some(true)` for a goal — one of §3.5's, so a shootout
/// attempt is not one and callers pass `scoring` already gated — `Some(false)`
/// for a penalty that is neither the placeholder nor a minutes-less
/// penalty-shot call, `None` for everything else (hits, faceoffs, stoppages,
/// shots).
fn flash_kind(
    type_id: Option<u16>,
    scoring: bool,
    penalty_type: bool,
    penalty_minutes: bool,
) -> Option<bool> {
    if scoring {
        return Some(true);
    }
    let eligible_penalty = is_penalty(type_id, penalty_type)
        && penalty_minutes
        && type_id != Some(TYPE_PLACEHOLDER_PENALTY);
    eligible_penalty.then_some(false)
}

/// ESPN's play text, prefixed for a goal; truncated where the wire would.
fn flash_text(goal: bool, text: &str) -> EText {
    let mut out = EText::new();
    if goal {
        compose(&mut out, &["GOAL! ", text]);
    } else {
        compose(&mut out, &[text]);
    }
    out
}

/// §3.5's tag: ENG from either empty-net marker, then the strength codes; a
/// 902 penalty-shot goal is a plain GOAL.
fn goal_tag(strength: Option<u16>, shot_info: Option<u16>) -> GoalTag {
    if shot_info == Some(SHOT_INFO_EMPTY_NET) || strength == Some(STRENGTH_EMPTY_NET) {
        GoalTag::EmptyNet
    } else {
        match strength {
            Some(STRENGTH_POWER_PLAY) => GoalTag::PowerPlay,
            Some(STRENGTH_SHORT_HANDED) => GoalTag::ShortHanded,
            _ => GoalTag::Goal,
        }
    }
}

/// The five names ESPN sends (§2). `Absent` follows the state silently;
/// `Unknown` is a quirk and follows it too (§3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusName {
    Scheduled,
    InProgress,
    EndPeriod,
    Final,
    Delayed,
    Unknown,
    Absent,
}

fn status_name(name: &Exact<STATUS_NAME_BYTES>) -> StatusName {
    if !name.seen() {
        return StatusName::Absent;
    }
    match name.valid() {
        Some("STATUS_SCHEDULED") => StatusName::Scheduled,
        Some("STATUS_IN_PROGRESS") => StatusName::InProgress,
        Some("STATUS_END_PERIOD") => StatusName::EndPeriod,
        Some("STATUS_FINAL") => StatusName::Final,
        Some("STATUS_DELAYED") => StatusName::Delayed,
        _ => StatusName::Unknown,
    }
}

/// §3.1's dispatch: the state, except STATUS_DELAYED at period 0, which is a
/// pregame (the puck has not dropped and the competitors are still
/// pregame-shaped); DELAYED at period ≥ 1 is live. The intermission is
/// STATUS_END_PERIOD.
fn resolve_state<Q: Quirks>(
    state: GameState,
    name: &Exact<STATUS_NAME_BYTES>,
    period: u8,
    quirks: &mut Q,
) -> (GameState, LivePhase) {
    let name = status_name(name);
    if name == StatusName::Unknown {
        quirks.quirk(Quirk::UnknownStatusName);
    }
    match state {
        GameState::Live if name == StatusName::Delayed && period == 0 => {
            (GameState::Pregame, LivePhase::InProgress)
        }
        GameState::Live => {
            let phase = if name == StatusName::EndPeriod {
                LivePhase::EndOfPeriod
            } else {
                LivePhase::InProgress
            };
            (GameState::Live, phase)
        }
        other => (other, LivePhase::InProgress),
    }
}

/// The `altDetail` §3.1's final label implies: none through regulation,
/// "OT" at 4, then "SO" in the regular season or "{n-3}OT" in the playoffs.
fn expected_alt_detail(period: u8, playoffs: bool) -> heapless::String<ALT_DETAIL_BYTES> {
    let mut out = heapless::String::new();
    match period {
        0..=3 => {}
        4 => {
            let _ = out.push_str("OT");
        }
        n if playoffs => {
            // At most "252OT", which fits.
            let _ = write!(out, "{}OT", n - 3);
        }
        _ => {
            let _ = out.push_str("SO");
        }
    }
    out
}

/// §3.3 source 2: `sog(side) = saves(other) + goals(side)`, both teams'
/// statistics present. On a final where both teams report `saves` 0 — the
/// post-final quirk — the identity counts as missing.
fn identity_shots(
    away: &CompetitorScratch,
    home: &CompetitorScratch,
    final_: bool,
) -> Option<Shots> {
    let (away_saves, away_goals) = (away.saves?, away.goals?);
    let (home_saves, home_goals) = (home.saves?, home.goals?);
    if final_ && away_saves == 0 && home_saves == 0 {
        return None;
    }
    let saturate = |shots: u32| shots.min(u32::from(u8::MAX)) as u8;
    Some(Shots {
        away: saturate(home_saves.saturating_add(away_goals)),
        home: saturate(away_saves.saturating_add(home_goals)),
    })
}

// -------------------------------------------------------------- path tables

// Scoreboard body, relative to the document root. `competitions[0]` only, as
// in every lane; everything unlisted (odds, broadcasts, leaders, headlines…)
// is skipped by construction.
const P_EVENTS: usize = 0;
const P_EVENT: usize = 1;
const P_ID: usize = 2;
const P_DATE: usize = 3;
const P_SEASON_TYPE: usize = 4;
const P_COMPS: usize = 5;
const P_COMP0: usize = 6;
const P_STATE: usize = 7;
const P_STATUS_NAME: usize = 8;
const P_ALT_DETAIL: usize = 9;
const P_PERIOD: usize = 10;
const P_CLOCK: usize = 11;
const P_FEATURED: usize = 12;
const P_FEATURED_NAME: usize = 13;
const P_FEATURED_ATHLETE: usize = 14;
const P_FEATURED_TEAM: usize = 15;
const P_VENUE_NAME: usize = 16;
const P_LAST_PLAY: usize = 17;
const P_LAST_PLAY_ID: usize = 18;
const P_LAST_PLAY_TEXT: usize = 19;
const P_LAST_PLAY_TYPE: usize = 20;
const P_LAST_PLAY_PENALTY_TYPE: usize = 21;
const P_LAST_PLAY_PENALTY_MINUTES: usize = 22;
const P_SERIES: usize = 23;
const P_HEADLINE: usize = 24;
const P_COMPETITOR: usize = 25;
const P_HOME_AWAY: usize = 26;
const P_SCORE: usize = 27;
const P_TEAM_ID: usize = 28;
const P_TEAM_ABBR: usize = 29;
const P_TEAM_COLOR: usize = 30;
const P_TEAM_ALT: usize = 31;
const P_TEAM_LOGO: usize = 32;
const P_RECORD: usize = 33;
const P_RECORD_TYPE: usize = 34;
const P_RECORD_SUMMARY: usize = 35;
const P_LINESCORE: usize = 36;
const P_LINESCORE_VALUE: usize = 37;
const P_STAT: usize = 38;
const P_STAT_NAME: usize = 39;
const P_STAT_VALUE: usize = 40;
const P_PROBABLE: usize = 41;
const P_PROBABLE_NAME: usize = 42;
const P_PROBABLE_ATHLETE: usize = 43;
const P_PROBABLE_ATHLETE_ID: usize = 44;

#[rustfmt::skip]
static SCOREBOARD_TABLE: &[Pattern] = &[
    /* P_EVENTS                   */ &[Key("events")],
    /* P_EVENT                    */ &[Key("events"), AnyIndex],
    /* P_ID                       */ &[Key("events"), AnyIndex, Key("id")],
    /* P_DATE                     */ &[Key("events"), AnyIndex, Key("date")],
    /* P_SEASON_TYPE              */ &[Key("events"), AnyIndex, Key("season"), Key("type")],
    /* P_COMPS                    */ &[Key("events"), AnyIndex, Key("competitions")],
    /* P_COMP0                    */ &[Key("events"), AnyIndex, Key("competitions"), Index(0)],
    /* P_STATE                    */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("status"), Key("type"), Key("state")],
    /* P_STATUS_NAME              */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("status"), Key("type"), Key("name")],
    /* P_ALT_DETAIL               */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("status"), Key("type"), Key("altDetail")],
    /* P_PERIOD                   */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("status"), Key("period")],
    /* P_CLOCK                    */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("status"), Key("displayClock")],
    /* P_FEATURED                 */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("status"), Key("featuredAthletes"), AnyIndex],
    /* P_FEATURED_NAME            */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("status"), Key("featuredAthletes"), AnyIndex, Key("name")],
    /* P_FEATURED_ATHLETE         */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("status"), Key("featuredAthletes"), AnyIndex, Key("athlete"), Key("shortName")],
    /* P_FEATURED_TEAM            */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("status"), Key("featuredAthletes"), AnyIndex, Key("team"), Key("id")],
    /* P_VENUE_NAME               */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("venue"), Key("fullName")],
    /* P_LAST_PLAY                */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("situation"), Key("lastPlay")],
    /* P_LAST_PLAY_ID             */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("situation"), Key("lastPlay"), Key("id")],
    /* P_LAST_PLAY_TEXT           */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("situation"), Key("lastPlay"), Key("text")],
    /* P_LAST_PLAY_TYPE           */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("situation"), Key("lastPlay"), Key("type"), Key("id")],
    /* P_LAST_PLAY_PENALTY_TYPE   */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("situation"), Key("lastPlay"), Key("type"), Key("penaltyType")],
    /* P_LAST_PLAY_PENALTY_MINUTES*/ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("situation"), Key("lastPlay"), Key("type"), Key("penaltyMinutes")],
    /* P_SERIES                   */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("series"), Key("summary")],
    /* P_HEADLINE                 */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("notes"), Index(0), Key("headline")],
    /* P_COMPETITOR               */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex],
    /* P_HOME_AWAY                */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("homeAway")],
    /* P_SCORE                    */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("score")],
    /* P_TEAM_ID                  */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("team"), Key("id")],
    /* P_TEAM_ABBR                */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("team"), Key("abbreviation")],
    /* P_TEAM_COLOR               */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("team"), Key("color")],
    /* P_TEAM_ALT                 */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("team"), Key("alternateColor")],
    /* P_TEAM_LOGO                */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("team"), Key("logo")],
    /* P_RECORD                   */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("records"), AnyIndex],
    /* P_RECORD_TYPE              */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("records"), AnyIndex, Key("type")],
    /* P_RECORD_SUMMARY           */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("records"), AnyIndex, Key("summary")],
    /* P_LINESCORE                */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("linescores"), AnyIndex],
    /* P_LINESCORE_VALUE          */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("linescores"), AnyIndex, Key("value")],
    /* P_STAT                     */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("statistics"), AnyIndex],
    /* P_STAT_NAME                */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("statistics"), AnyIndex, Key("name")],
    /* P_STAT_VALUE               */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("statistics"), AnyIndex, Key("displayValue")],
    /* P_PROBABLE                 */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("probables"), AnyIndex],
    /* P_PROBABLE_NAME            */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("probables"), AnyIndex, Key("name")],
    /* P_PROBABLE_ATHLETE         */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("probables"), AnyIndex, Key("athlete"), Key("shortName")],
    /* P_PROBABLE_ATHLETE_ID      */ &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("probables"), AnyIndex, Key("athlete"), Key("id")],
];

// Summary body. Everything else in its 85–630 KB (rosters, standings, odds,
// news, the per-player stat lines) is skipped by construction.
const S_PLAY: usize = 0;
const S_PLAY_ID: usize = 1;
const S_PLAY_TYPE: usize = 2;
const S_PLAY_PENALTY_TYPE: usize = 3;
const S_PLAY_PENALTY_MINUTES: usize = 4;
const S_PLAY_TEXT: usize = 5;
const S_PLAY_PERIOD: usize = 6;
const S_PLAY_CLOCK: usize = 7;
const S_PLAY_TEAM: usize = 8;
const S_PLAY_SCORING: usize = 9;
const S_PARTICIPANT: usize = 10;
const S_PARTICIPANT_TYPE: usize = 11;
const S_PARTICIPANT_ID: usize = 12;
const S_PARTICIPANT_NAME: usize = 13;
const S_PLAY_X: usize = 14;
const S_PLAY_Y: usize = 15;
const S_PLAY_STRENGTH: usize = 16;
const S_PLAY_SHOT_INFO: usize = 17;
const S_ON_ICE_TEAM: usize = 18;
const S_ON_ICE_TEAM_ID: usize = 19;
const S_ON_ICE_ENTRY: usize = 20;
const S_ON_ICE_ATHLETE: usize = 21;
const S_PLAYERS: usize = 22;
const S_PLAYERS_TEAM_ID: usize = 23;
const S_PLAYERS_GROUP: usize = 24;
const S_PLAYERS_GROUP_NAME: usize = 25;
const S_PLAYERS_ATHLETE_ID: usize = 26;
const S_BOX_TEAM: usize = 27;
const S_BOX_TEAM_ID: usize = 28;
const S_BOX_STAT: usize = 29;
const S_BOX_STAT_NAME: usize = 30;
const S_BOX_STAT_VALUE: usize = 31;
const S_HEADER_COMPETITOR: usize = 32;
const S_HEADER_TEAM_ID: usize = 33;
const S_HEADER_HOME_AWAY: usize = 34;

#[rustfmt::skip]
static SUMMARY_TABLE: &[Pattern] = &[
    /* S_PLAY                 */ &[Key("plays"), AnyIndex],
    /* S_PLAY_ID              */ &[Key("plays"), AnyIndex, Key("id")],
    /* S_PLAY_TYPE            */ &[Key("plays"), AnyIndex, Key("type"), Key("id")],
    /* S_PLAY_PENALTY_TYPE    */ &[Key("plays"), AnyIndex, Key("type"), Key("penaltyType")],
    /* S_PLAY_PENALTY_MINUTES */ &[Key("plays"), AnyIndex, Key("type"), Key("penaltyMinutes")],
    /* S_PLAY_TEXT            */ &[Key("plays"), AnyIndex, Key("text")],
    /* S_PLAY_PERIOD          */ &[Key("plays"), AnyIndex, Key("period"), Key("number")],
    /* S_PLAY_CLOCK           */ &[Key("plays"), AnyIndex, Key("clock"), Key("displayValue")],
    /* S_PLAY_TEAM            */ &[Key("plays"), AnyIndex, Key("team"), Key("id")],
    /* S_PLAY_SCORING         */ &[Key("plays"), AnyIndex, Key("scoringPlay")],
    /* S_PARTICIPANT          */ &[Key("plays"), AnyIndex, Key("participants"), AnyIndex],
    /* S_PARTICIPANT_TYPE     */ &[Key("plays"), AnyIndex, Key("participants"), AnyIndex, Key("type")],
    /* S_PARTICIPANT_ID       */ &[Key("plays"), AnyIndex, Key("participants"), AnyIndex, Key("athlete"), Key("id")],
    /* S_PARTICIPANT_NAME     */ &[Key("plays"), AnyIndex, Key("participants"), AnyIndex, Key("athlete"), Key("shortName")],
    /* S_PLAY_X               */ &[Key("plays"), AnyIndex, Key("coordinate"), Key("x")],
    /* S_PLAY_Y               */ &[Key("plays"), AnyIndex, Key("coordinate"), Key("y")],
    /* S_PLAY_STRENGTH        */ &[Key("plays"), AnyIndex, Key("strength"), Key("id")],
    /* S_PLAY_SHOT_INFO       */ &[Key("plays"), AnyIndex, Key("shotInfo"), Key("id")],
    /* S_ON_ICE_TEAM          */ &[Key("onIce"), AnyIndex],
    /* S_ON_ICE_TEAM_ID       */ &[Key("onIce"), AnyIndex, Key("teamId")],
    /* S_ON_ICE_ENTRY         */ &[Key("onIce"), AnyIndex, Key("entries"), AnyIndex],
    /* S_ON_ICE_ATHLETE       */ &[Key("onIce"), AnyIndex, Key("entries"), AnyIndex, Key("athleteid")],
    /* S_PLAYERS              */ &[Key("boxscore"), Key("players"), AnyIndex],
    /* S_PLAYERS_TEAM_ID      */ &[Key("boxscore"), Key("players"), AnyIndex, Key("team"), Key("id")],
    /* S_PLAYERS_GROUP        */ &[Key("boxscore"), Key("players"), AnyIndex, Key("statistics"), AnyIndex],
    /* S_PLAYERS_GROUP_NAME   */ &[Key("boxscore"), Key("players"), AnyIndex, Key("statistics"), AnyIndex, Key("name")],
    /* S_PLAYERS_ATHLETE_ID   */ &[Key("boxscore"), Key("players"), AnyIndex, Key("statistics"), AnyIndex, Key("athletes"), AnyIndex, Key("athlete"), Key("id")],
    /* S_BOX_TEAM             */ &[Key("boxscore"), Key("teams"), AnyIndex],
    /* S_BOX_TEAM_ID          */ &[Key("boxscore"), Key("teams"), AnyIndex, Key("team"), Key("id")],
    /* S_BOX_STAT             */ &[Key("boxscore"), Key("teams"), AnyIndex, Key("statistics"), AnyIndex],
    /* S_BOX_STAT_NAME        */ &[Key("boxscore"), Key("teams"), AnyIndex, Key("statistics"), AnyIndex, Key("name")],
    /* S_BOX_STAT_VALUE       */ &[Key("boxscore"), Key("teams"), AnyIndex, Key("statistics"), AnyIndex, Key("displayValue")],
    /* S_HEADER_COMPETITOR    */ &[Key("header"), Key("competitions"), Index(0), Key("competitors"), AnyIndex],
    /* S_HEADER_TEAM_ID       */ &[Key("header"), Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("team"), Key("id")],
    /* S_HEADER_HOME_AWAY     */ &[Key("header"), Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("homeAway")],
];

// ------------------------------------------------------------ event scratch

#[derive(Debug, Clone, Default)]
struct CompetitorScratch {
    home_away: Option<HomeAway>,
    crest: Option<CrestPath>,
    score: Exact<SCORE_BYTES>,
    score_bad: bool,
    team_id: Exact<ID_BYTES>,
    abbreviation: WireText,
    color: Exact<COLOR_BYTES>,
    alternate: Exact<COLOR_BYTES>,
    /// The first `records[]` entry of type `total`/`ytd`: `None` = none yet,
    /// `Some(None)` = present but not W-L-OTL, `Some(Some(_))` = parsed.
    record: Option<Option<Record>>,
    line: heapless::Vec<u8, MAX_LINE_SCORE>,
    line_clipped: bool,
    /// A `linescores[]` entry without a numeric `value` (or a scalar entry):
    /// the periods after it would shift, so the event is dropped.
    line_bad: bool,
    /// Game `statistics[]` — season values before the game, `[]` at puck
    /// drop; the identity needs both.
    saves: Option<u32>,
    goals: Option<u32>,
    /// The probable starting goalie's `shortName` and `athlete.id`.
    goalie: WireText,
    goalie_id: Exact<ID_BYTES>,
}

/// Which `statistics[]` entry is in progress.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum StatName {
    #[default]
    Other,
    Saves,
    Goals,
}

/// The records / linescores / statistics / probables element in progress.
/// One shared instance: elements of different arrays never interleave.
#[derive(Debug, Clone, Default)]
struct EntryScratch {
    record_counts: bool,
    record_summary: WireText,
    line_value: Option<f64>,
    stat: StatName,
    stat_value: Option<u32>,
    probable_goalie: bool,
    probable_name: WireText,
    probable_id: Exact<ID_BYTES>,
}

/// `situation.lastPlay`, with the fields §3.6's filter can read there.
#[derive(Debug, Clone, Default)]
struct LastPlayScratch {
    present: bool,
    id: WireText,
    text: WireText,
    type_id: Option<u16>,
    penalty_type: bool,
    penalty_minutes: bool,
}

/// One of the three stars, as `featuredAthletes[]` named it.
#[derive(Debug, Clone, Default)]
struct StarScratch {
    name: WireText,
    team_id: Exact<ID_BYTES>,
}

/// The `featuredAthletes[]` element in progress; `role` is the star index
/// its `name` selects, `None` for the goalies and anything unknown.
#[derive(Debug, Clone, Default)]
struct FeaturedScratch {
    active: bool,
    role: Option<usize>,
    name: WireText,
    team_id: Exact<ID_BYTES>,
}

#[derive(Debug, Clone, Default)]
struct EventScratch {
    active: bool,
    is_target: bool,
    /// A required field had the wrong type, or a required container arrived
    /// as a scalar — dropped at finalize.
    invalid: bool,
    id: WireText,
    date: Exact<DATE_BYTES>,
    season_type: Option<u8>,
    competitions_present: bool,
    comp0_present: bool,
    state: Option<GameState>,
    status_name: Exact<STATUS_NAME_BYTES>,
    alt_detail: Exact<ALT_DETAIL_BYTES>,
    period: Option<u8>,
    period_bad: bool,
    clock: WireText,
    clock_bad: bool,
    venue_name: WireText,
    series: WireText,
    headline: WireText,
    last_play: LastPlayScratch,
    stars: [StarScratch; 3],
    featured: FeaturedScratch,
    competitor_count: u16,
    competitors: [CompetitorScratch; TEAMS],
    entry: EntryScratch,
}

/// The required-field rule from the module docs, applied at the event's
/// close. `false` drops the event and counts it failed.
fn du_ok(s: &EventScratch) -> bool {
    if s.invalid || !s.id.seen() || !s.date.seen() || !s.competitions_present {
        return false;
    }
    if !s.comp0_present {
        return true;
    }
    let Some(state) = s.state else {
        return false;
    };
    if s.competitor_count != 2 {
        return false;
    }
    for competitor in &s.competitors {
        if competitor.home_away.is_none()
            || !competitor.team_id.seen()
            || !competitor.abbreviation.seen()
            || !competitor.color.seen()
            || competitor.line_bad
        {
            return false;
        }
    }
    let scores_ok = s
        .competitors
        .iter()
        .all(|competitor| competitor.score.seen() && !competitor.score_bad);
    let period_ok = s.period.is_some() && !s.period_bad;
    match state {
        GameState::Pregame => true,
        GameState::Live => period_ok && s.clock.seen() && !s.clock_bad && scores_ok,
        GameState::Final => period_ok && scores_ok,
    }
}

// ---------------------------------------------------------------- transform

/// Transform-tier failure on the target event — the backend's 5xx class,
/// distinct from the required-field tier that silently drops an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransformError {
    /// `homeAway` markers were not exactly one home and one away.
    Sides,
    /// A team color failed the strip-`#`/exactly-6-hex-digits parse.
    Color,
    /// A competitor score failed the `u32` parse.
    Score,
    /// The pregame `event.date` failed the start-time parse.
    Date,
}

fn team_colors(competitor: &CompetitorScratch) -> Result<TeamColors, TransformError> {
    let primary = competitor
        .color
        .valid()
        .and_then(parse_hex_rgb)
        .ok_or(TransformError::Color)?;
    // Only a string is ever stored here; any other type read as absent.
    let alternate = if competitor.alternate.seen() {
        competitor
            .alternate
            .valid()
            .and_then(parse_hex_rgb)
            .ok_or(TransformError::Color)?
    } else {
        primary
    };
    Ok(TeamColors { primary, alternate })
}

fn team_score(competitor: &CompetitorScratch) -> Result<u32, TransformError> {
    competitor
        .score
        .valid()
        .and_then(|text| text.parse::<u32>().ok())
        .ok_or(TransformError::Score)
}

fn pregame_team<Q: Quirks>(
    competitor: &CompetitorScratch,
    quirks: &mut Q,
) -> Result<PregameTeam, TransformError> {
    let record = match competitor.record {
        None => None,
        Some(Some(record)) => Some(record),
        Some(None) => {
            quirks.quirk(Quirk::MalformedRecord);
            None
        }
    };
    Ok(PregameTeam {
        abbreviation: owned(competitor.abbreviation.as_str()),
        colors: team_colors(competitor)?,
        record,
        goalie: competitor.goalie.get().map(owned),
    })
}

fn live_team(competitor: &CompetitorScratch) -> Result<LiveTeam, TransformError> {
    Ok(LiveTeam {
        abbreviation: owned(competitor.abbreviation.as_str()),
        score: team_score(competitor)?,
        colors: team_colors(competitor)?,
    })
}

fn final_team(competitor: &CompetitorScratch) -> Result<FinalTeam, TransformError> {
    Ok(FinalTeam {
        abbreviation: owned(competitor.abbreviation.as_str()),
        score: team_score(competitor)?,
        colors: team_colors(competitor)?,
        line_score: competitor.line.clone(),
    })
}

/// §3.9's info line: the round and game, then the series, joined by " - ";
/// whichever half a playoff event has; nothing in the regular season.
fn info_secondary(playoffs: bool, headline: &WireText, series: &WireText) -> EText {
    let mut out = EText::new();
    if !playoffs {
        return out;
    }
    match (headline.get(), series.get()) {
        (Some(headline), Some(series)) => compose(&mut out, &[headline, " - ", series]),
        (Some(text), None) | (None, Some(text)) => compose(&mut out, &[text]),
        (None, None) => {}
    }
    out
}

/// §3.6 over the scoreboard's `situation.lastPlay`, as far as its fields
/// allow: a goal is type 505 (there is no `scoringPlay` here) outside the
/// shootout, which `status.period` and the season kind place; a penalty is
/// judged by the same `penaltyType`/`penaltyMinutes`/143 rule as the summary.
fn scoreboard_flash(play: &LastPlayScratch, in_shootout: bool) -> Option<LastPlay> {
    if !play.present {
        return None;
    }
    let goal = flash_kind(
        play.type_id,
        play.type_id == Some(TYPE_GOAL) && !in_shootout,
        play.penalty_type,
        play.penalty_minutes,
    )?;
    Some(LastPlay {
        id: owned(play.id.get()?),
        text: flash_text(goal, play.text.get()?),
    })
}

/// §3.10's stars: all three or none, each side from its own `team.id`.
fn stars(s: &EventScratch, home_id: Option<&str>, away_id: Option<&str>) -> Option<[Star; 3]> {
    if s.stars.iter().any(|star| !star.name.seen()) {
        return None;
    }
    let star = |index: usize| Star {
        name: owned(s.stars[index].name.as_str()),
        side: side_of(&s.stars[index].team_id, home_id, away_id),
    };
    Some([star(0), star(1), star(2)])
}

fn transform<Q: Quirks>(s: &EventScratch, quirks: &mut Q) -> Result<Extract, TransformError> {
    let marker_a = s.competitors[0].home_away.ok_or(TransformError::Sides)?;
    let marker_b = s.competitors[1].home_away.ok_or(TransformError::Sides)?;
    let (home_idx, away_idx) =
        order_home_away((marker_a, 0usize), (marker_b, 1usize)).ok_or(TransformError::Sides)?;
    let home = &s.competitors[home_idx];
    let away = &s.competitors[away_idx];
    let crests = Crests {
        away: away.crest.clone(),
        home: home.crest.clone(),
    };

    let playoffs = s.season_type == Some(SEASON_POSTSEASON);
    let period = s.period.unwrap_or(0); // required for live and final by du_ok
    let (state, phase) = resolve_state(
        s.state.unwrap_or(GameState::Pregame), // du_ok guaranteed Some
        &s.status_name,
        period,
        quirks,
    );

    let kind = match state {
        GameState::Pregame => {
            let start_time = s
                .date
                .valid()
                .and_then(parse_start_time)
                .ok_or(TransformError::Date)?;
            Kind::Pregame(Pregame {
                start_time,
                venue: owned(s.venue_name.as_str()),
                info_secondary: info_secondary(playoffs, &s.headline, &s.series),
                away: pregame_team(away, quirks)?,
                home: pregame_team(home, quirks)?,
            })
        }
        GameState::Live => Kind::Live(Live {
            period,
            phase,
            clock: owned(s.clock.as_str()),
            away: live_team(away)?,
            home: live_team(home)?,
            shots: identity_shots(away, home, false),
            last_play: scoreboard_flash(&s.last_play, !playoffs && period >= FIRST_SHOOTOUT_PERIOD),
        }),
        GameState::Final => {
            let away_team = final_team(away)?;
            let home_team = final_team(home)?;
            if away_team.score == home_team.score {
                quirks.quirk(Quirk::TiedFinal);
            }
            // The cross-check (§3.1): the label is derived from the period
            // and the season kind; `altDetail` only confirms it.
            let actual = if s.alt_detail.seen() {
                // An overlong altDetail can match nothing.
                s.alt_detail.valid().unwrap_or("?")
            } else {
                ""
            };
            if actual != expected_alt_detail(period, playoffs).as_str() {
                quirks.quirk(Quirk::FinalDetailMismatch);
            }
            let home_id = home.team_id.valid();
            let away_id = away.team_id.valid();
            Kind::Final(Final {
                periods_played: period,
                shots: identity_shots(away, home, true),
                stars: stars(s, home_id, away_id),
                // Playoffs only, like the pregame's: a regular-season event
                // can carry a season series nobody asked for.
                series: if playoffs {
                    s.series.get().map(owned)
                } else {
                    None
                },
                away: away_team,
                home: home_team,
            })
        }
    };
    Ok(Extract {
        game_id: owned(s.id.as_str()),
        crests,
        playoffs,
        probable_goalies: [athlete_id(&away.goalie_id), athlete_id(&home.goalie_id)],
        kind,
    })
}

/// A valid compare key as an owned id; an unseen or overlong one is none.
fn athlete_id(id: &Exact<ID_BYTES>) -> Option<AthleteId> {
    let mut out = AthleteId::new();
    // `valid` guarantees it fit the same capacity, so the push cannot fail.
    out.push_str(id.valid()?).ok()?;
    Some(out)
}

// --------------------------------------------------------------- the sink

#[derive(Debug)]
// Carries the whole bounded extract; boxing is unavailable (no_std, no
// alloc) and exactly one of these exists per stream.
#[allow(clippy::large_enum_variant)]
enum DetailResult {
    Game(Extract),
    /// Target found but its `competitions` array is empty — the backend
    /// serves `GameNotFound` (404), same as an absent id on a clean parse.
    NoCompetition,
    Failed(TransformError),
}

#[derive(Debug)]
// The list vec dwarfs the detail arm; one Mode exists per stream and no_std
// forbids boxing it away.
#[allow(clippy::large_enum_variant)]
enum Mode {
    List {
        games: heapless::Vec<ListEntry, LIST_MAX>,
        overflowed: bool,
    },
    Detail {
        /// Compare key, refuse-to-truncate (ruling 16).
        target: Exact<ID_BYTES>,
        done: bool,
        result: Option<DetailResult>,
    },
}

struct ScoreboardSink<Q: Quirks> {
    mode: Mode,
    scratch: EventScratch,
    quirks: Q,
    ok: u16,
    failed: u16,
    /// The scoreboard shell itself was malformed (`events` not an array) —
    /// a 502 before any event.
    body_bad: bool,
}

impl<Q: Quirks> ScoreboardSink<Q> {
    fn new(mode: Mode, quirks: Q) -> Self {
        Self {
            mode,
            scratch: EventScratch::default(),
            quirks,
            ok: 0,
            failed: 0,
            body_bad: false,
        }
    }

    /// Ruling 14: once the target is found, the document remainder is
    /// skipped and uncounted — the verdict is already `Found`.
    fn done(&self) -> bool {
        matches!(self.mode, Mode::Detail { done: true, .. })
    }

    /// Index 1 binds `competitors[*]`. A third competitor still counts
    /// toward the exactly-two rule but stores nothing.
    fn competitor(&mut self, indices: &[u16]) -> Option<&mut CompetitorScratch> {
        let index = usize::from(*indices.get(1)?);
        self.scratch.competitors.get_mut(index)
    }

    fn fold_record(&mut self, indices: &[u16]) {
        let counts = self.scratch.entry.record_counts;
        let record = self
            .scratch
            .entry
            .record_summary
            .get()
            .map(parse_record_otl);
        if let (true, Some(record), Some(competitor)) = (counts, record, self.competitor(indices)) {
            if competitor.record.is_none() {
                competitor.record = Some(record);
            }
        }
    }

    fn fold_linescore(&mut self, indices: &[u16]) {
        let value = self.scratch.entry.line_value;
        let mut clipped = false;
        if let Some(competitor) = self.competitor(indices) {
            match value {
                Some(value) => {
                    if competitor.line.push(linescore_byte(value)).is_err()
                        && !competitor.line_clipped
                    {
                        competitor.line_clipped = true;
                        clipped = true;
                    }
                }
                // NBA's rule: an entry that holds no number fails the event
                // rather than shifting the periods after it.
                None => competitor.line_bad = true,
            }
        }
        if clipped {
            self.quirks.quirk(Quirk::ClippedLineScore);
        }
    }

    fn fold_stat(&mut self, indices: &[u16]) {
        let (name, value) = (self.scratch.entry.stat, self.scratch.entry.stat_value);
        if let Some(competitor) = self.competitor(indices) {
            match name {
                StatName::Saves => competitor.saves = value,
                StatName::Goals => competitor.goals = value,
                StatName::Other => {}
            }
        }
    }

    /// The first `probableStartingGoalie` entry's name (the pregame line) and
    /// id (§3.4's known goalie), each kept from the first entry that has it.
    fn fold_probable(&mut self, indices: &[u16]) {
        if !self.scratch.entry.probable_goalie {
            return;
        }
        let entry = core::mem::take(&mut self.scratch.entry);
        let Some(competitor) = self.competitor(indices) else {
            return;
        };
        if let (Some(name), false) = (entry.probable_name.get(), competitor.goalie.seen()) {
            competitor.goalie.set(name);
        }
        if let (Some(id), false) = (entry.probable_id.valid(), competitor.goalie_id.seen()) {
            competitor.goalie_id.set(id);
        }
    }

    fn fold_featured(&mut self) {
        let featured = &mut self.scratch.featured;
        if !featured.active {
            return;
        }
        featured.active = false;
        if let Some(index) = featured.role {
            if let Some(name) = featured.name.get() {
                self.scratch.stars[index].name.set(name);
                self.scratch.stars[index].team_id = featured.team_id.clone();
            }
        }
    }

    /// Every event up to and including the target is fully validated and
    /// counted (ruling 14), so a missing target always comes with exact
    /// `ok`/`failed` counts. Finding the target answers `SkipElement` and
    /// flips `done` — nothing after it is matched or counted.
    fn finalize_event(&mut self) -> Directive {
        let scratch = &mut self.scratch;
        if !scratch.active {
            return Directive::Continue;
        }
        scratch.active = false;
        if !du_ok(scratch) {
            self.failed = self.failed.saturating_add(1);
            return Directive::Continue;
        }
        self.ok = self.ok.saturating_add(1);
        match &mut self.mode {
            Mode::List { games, overflowed } => {
                if let (true, Some(state)) = (scratch.comp0_present, scratch.state) {
                    // The list agrees with the detail: a DELAYED game at
                    // period 0 is listed as a pregame (§3.1).
                    let (state, _) = resolve_state(
                        state,
                        &scratch.status_name,
                        scratch.period.unwrap_or(0),
                        &mut self.quirks,
                    );
                    let entry = ListEntry {
                        state,
                        id: owned(scratch.id.as_str()),
                    };
                    if games.push(entry).is_err() {
                        *overflowed = true;
                        self.quirks.quirk(Quirk::BoundedOverflow);
                    }
                }
                Directive::Continue
            }
            Mode::Detail { done, result, .. } => {
                if scratch.is_target && !*done {
                    *done = true;
                    *result = Some(if scratch.comp0_present {
                        match transform(scratch, &mut self.quirks) {
                            Ok(game) => DetailResult::Game(game),
                            Err(error) => DetailResult::Failed(error),
                        }
                    } else {
                        DetailResult::NoCompetition
                    });
                    return Directive::SkipElement;
                }
                Directive::Continue
            }
        }
    }
}

impl<Q: Quirks> Sink for ScoreboardSink<Q> {
    fn value(&mut self, pattern: usize, indices: &[u16], value: Value<'_>) -> Directive {
        if self.done() {
            return Directive::SkipElement;
        }
        let s = &mut self.scratch;
        match pattern {
            // `events` present but not an array (null, scalar): the shell is
            // malformed and the whole request is an upstream error.
            P_EVENTS => self.body_bad = true,
            // A scalar events[] element parses as no event at all.
            P_EVENT => self.failed = self.failed.saturating_add(1),
            P_ID => match value {
                Value::Str(text) => {
                    s.id.set(text);
                    // Ruling 14: no skipping before the target is found. The
                    // raw token is compared against the refuse-to-truncate
                    // target (ruling 16).
                    if let Mode::Detail { target, done, .. } = &self.mode {
                        if !*done && target.valid() == Some(text) {
                            s.is_target = true;
                        }
                    }
                }
                _ => s.invalid = true,
            },
            P_DATE => match value {
                Value::Str(text) => s.date.set(text),
                _ => s.invalid = true,
            },
            P_SEASON_TYPE => {
                if let Value::Num(text) = value {
                    s.season_type = num_u8(text);
                }
            }
            // Required containers that arrived as scalars/null.
            P_COMPS | P_COMP0 => s.invalid = true,
            P_STATE => match value {
                Value::Str("pre") => s.state = Some(GameState::Pregame),
                Value::Str("in") => s.state = Some(GameState::Live),
                Value::Str("post") => s.state = Some(GameState::Final),
                _ => s.invalid = true,
            },
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
            P_PERIOD => match value {
                Value::Num(text) => match num_u8(text) {
                    Some(period) => s.period = Some(period),
                    None => s.period_bad = true,
                },
                Value::Null => {}
                _ => s.period_bad = true,
            },
            P_CLOCK => match value {
                Value::Str(text) => s.clock.set(text),
                Value::Null => {}
                _ => s.clock_bad = true,
            },
            P_VENUE_NAME => {
                if let Value::Str(text) = value {
                    s.venue_name.set(text);
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
            // `lastPlay: null` or a scalar: no play, nothing to flash.
            P_LAST_PLAY => {}
            P_LAST_PLAY_ID => {
                if let Value::Str(text) = value {
                    s.last_play.id.set(text);
                }
            }
            P_LAST_PLAY_TEXT => {
                if let Value::Str(text) = value {
                    s.last_play.text.set(text);
                }
            }
            P_LAST_PLAY_TYPE => s.last_play.type_id = code(value),
            P_LAST_PLAY_PENALTY_TYPE => {
                s.last_play.penalty_type = matches!(value, Value::Str(_));
            }
            P_LAST_PLAY_PENALTY_MINUTES => {
                s.last_play.penalty_minutes = matches!(value, Value::Str(text) if !text.is_empty());
            }
            P_FEATURED => {}
            P_FEATURED_NAME => {
                if let Value::Str(text) = value {
                    s.featured.role = match text {
                        "firstStar" => Some(0),
                        "secondStar" => Some(1),
                        "thirdStar" => Some(2),
                        _ => None,
                    };
                }
            }
            P_FEATURED_ATHLETE => {
                if let Value::Str(text) = value {
                    s.featured.name.set(text);
                }
            }
            P_FEATURED_TEAM => {
                if let Value::Str(text) = value {
                    s.featured.team_id.set(text);
                }
            }
            // A scalar competitor element: count it (the exactly-two rule)
            // and fail.
            P_COMPETITOR => {
                s.competitor_count = s.competitor_count.saturating_add(1);
                s.invalid = true;
            }
            P_HOME_AWAY | P_SCORE | P_TEAM_ID | P_TEAM_ABBR | P_TEAM_COLOR | P_TEAM_ALT
            | P_TEAM_LOGO => {
                let Some(competitor) = self.competitor(indices) else {
                    return Directive::Continue;
                };
                let mut invalid = false;
                match (pattern, value) {
                    (P_HOME_AWAY, Value::Str(text)) => match HomeAway::parse(text) {
                        Some(marker) => competitor.home_away = Some(marker),
                        None => invalid = true,
                    },
                    (P_HOME_AWAY, _) => invalid = true,
                    (P_SCORE, Value::Str(text)) => competitor.score.set(text),
                    (P_SCORE, _) => competitor.score_bad = true,
                    (P_TEAM_ID, Value::Str(text)) => competitor.team_id.set(text),
                    (P_TEAM_ABBR, Value::Str(text)) => competitor.abbreviation.set(text),
                    (P_TEAM_COLOR, Value::Str(text)) => competitor.color.set(text),
                    (P_TEAM_ID | P_TEAM_ABBR | P_TEAM_COLOR, _) => invalid = true,
                    (P_TEAM_ALT, Value::Str(text)) => competitor.alternate.set(text),
                    (P_TEAM_LOGO, Value::Str(href)) => competitor.crest = crest_path(href),
                    // An alternate that is not a string reads as absent; a
                    // malformed crest costs the event nothing, as in every
                    // lane.
                    _ => {}
                }
                if invalid {
                    self.scratch.invalid = true;
                }
            }
            P_RECORD_TYPE => {
                if let Value::Str(text) = value {
                    s.entry.record_counts = matches!(text, "total" | "ytd");
                }
            }
            P_RECORD_SUMMARY => {
                if let Value::Str(text) = value {
                    s.entry.record_summary.set(text);
                }
            }
            P_LINESCORE_VALUE => {
                if let Value::Num(text) = value {
                    s.entry.line_value = text.parse::<f64>().ok().filter(|v| v.is_finite());
                }
            }
            P_STAT_NAME => {
                if let Value::Str(text) = value {
                    s.entry.stat = match text {
                        "saves" => StatName::Saves,
                        "goals" => StatName::Goals,
                        _ => StatName::Other,
                    };
                }
            }
            P_STAT_VALUE => s.entry.stat_value = code(value).map(u32::from),
            P_PROBABLE_NAME => {
                if let Value::Str(text) = value {
                    s.entry.probable_goalie = text == "probableStartingGoalie";
                }
            }
            P_PROBABLE_ATHLETE => {
                if let Value::Str(text) = value {
                    s.entry.probable_name.set(text);
                }
            }
            P_PROBABLE_ATHLETE_ID => {
                if let Value::Str(text) = value {
                    s.entry.probable_id.set(text);
                }
            }
            // A scalar line-score entry holds no number: the event fails
            // (see `fold_linescore`).
            P_LINESCORE => {
                if let Some(competitor) = self.competitor(indices) {
                    competitor.line_bad = true;
                }
            }
            // Scalars where the other per-competitor arrays' elements belong:
            // an entry that holds nothing.
            P_RECORD | P_STAT | P_PROBABLE => {}
            _ => {}
        }
        Directive::Continue
    }

    fn enter(&mut self, pattern: usize, _indices: &[u16], kind: ContainerKind) -> Directive {
        if self.done() {
            return Directive::SkipElement;
        }
        // `{"events":{…}}` must 502-shape like a scalar shell; only an
        // array here is a scoreboard.
        if pattern == P_EVENTS {
            if kind == ContainerKind::Object {
                self.body_bad = true;
            }
            return Directive::Continue;
        }
        let s = &mut self.scratch;
        match pattern {
            P_EVENT => {
                *s = EventScratch::default();
                s.active = true;
            }
            P_COMPS => s.competitions_present = true,
            P_COMP0 => s.comp0_present = true,
            P_COMPETITOR => s.competitor_count = s.competitor_count.saturating_add(1),
            P_LAST_PLAY => s.last_play.present = true,
            P_FEATURED => {
                s.featured = FeaturedScratch {
                    active: true,
                    ..FeaturedScratch::default()
                };
            }
            P_RECORD | P_LINESCORE | P_STAT | P_PROBABLE => s.entry = EntryScratch::default(),
            _ => {}
        }
        Directive::Continue
    }

    fn leave(&mut self, pattern: usize, indices: &[u16]) -> Directive {
        if self.done() {
            return Directive::SkipElement;
        }
        match pattern {
            P_EVENT => return self.finalize_event(),
            P_RECORD => self.fold_record(indices),
            P_LINESCORE => self.fold_linescore(indices),
            P_STAT => self.fold_stat(indices),
            P_PROBABLE => self.fold_probable(indices),
            P_FEATURED => self.fold_featured(),
            _ => {}
        }
        Directive::Continue
    }
}

// ------------------------------------------------------------- public API

/// Extraction failure past the JSON stream itself.
#[derive(Debug, PartialEq)]
pub enum ExtractError {
    /// The tokenizer/engine rejected the input.
    Stream(Error),
    /// The scoreboard shell was malformed (`events` not an array) — a 502
    /// before any event.
    MalformedBody,
    /// The target event was found but failed the transform tier (502).
    Transform(TransformError),
}

impl From<Error> for ExtractError {
    fn from(error: Error) -> Self {
        ExtractError::Stream(error)
    }
}

/// Outcome of a detail extraction.
#[derive(Debug, PartialEq)]
// Found carries the whole bounded extract; no_std forbids boxing it away.
#[allow(clippy::large_enum_variant)]
pub enum GameOutcome {
    Found(Extract),
    /// The target event parsed clean but carried no competition — the
    /// backend serves `GameNotFound` (404) regardless of sibling failures.
    NoCompetition,
    /// The target id was not among the validated events. A missing target
    /// means nothing was ever skipped (ruling 14), so the report's counts
    /// are exact — the 404-vs-502 input (ruling 13): only `failed == 0` may
    /// map to "game ended".
    Absent,
}

/// Result of a detail extraction: the outcome, the per-event tallies, and
/// the quirk receiver handed back.
#[derive(Debug)]
pub struct DetailReport<Q> {
    pub outcome: GameOutcome,
    /// Events validated clean, up to the point the verdict was reached —
    /// post-found events are skipped and uncounted (ruling 14).
    pub ok: u16,
    /// Events the required-field rule dropped.
    pub failed: u16,
    pub quirks: Q,
}

/// Streams one league scoreboard body and extracts the target event.
/// Every event is fully validated and counted until the target is found
/// (ruling 14); only the post-found remainder is skipped, uncounted.
pub struct GameExtractor<'scratch, Q: Quirks> {
    matcher: StreamMatcher<'static, 'scratch, ScoreboardSink<Q>>,
}

impl<'scratch, Q: Quirks> GameExtractor<'scratch, Q> {
    /// `scratch` must hold the longest contiguous string token in the body
    /// (venue names, play texts, link URLs — kilobytes, not megabytes).
    pub fn new(target_id: &str, quirks: Q, scratch: &'scratch mut [u8]) -> Result<Self, Error> {
        let mut target = Exact::default();
        target.set(target_id);
        let sink = ScoreboardSink::new(
            Mode::Detail {
                target,
                done: false,
                result: None,
            },
            quirks,
        );
        Ok(Self {
            matcher: StreamMatcher::new(SCOREBOARD_TABLE, sink, scratch)?,
        })
    }

    pub fn write(&mut self, chunk: &[u8]) -> Result<(), Error> {
        self.matcher.write(chunk)
    }

    /// Finish the document. The report is only reachable on the Ok path —
    /// transform failures are terminal 5xx, not diagnostics.
    pub fn finish(self) -> Result<DetailReport<Q>, ExtractError> {
        let sink = self.matcher.finish()?;
        if sink.body_bad {
            return Err(ExtractError::MalformedBody);
        }
        let (ok, failed) = (sink.ok, sink.failed);
        let outcome = match sink.mode {
            Mode::Detail {
                result: Some(DetailResult::Game(game)),
                ..
            } => GameOutcome::Found(game),
            Mode::Detail {
                result: Some(DetailResult::Failed(error)),
                ..
            } => return Err(ExtractError::Transform(error)),
            Mode::Detail {
                result: Some(DetailResult::NoCompetition),
                ..
            } => GameOutcome::NoCompetition,
            _ => GameOutcome::Absent,
        };
        Ok(DetailReport {
            outcome,
            ok,
            failed,
            quirks: sink.quirks,
        })
    }
}

/// One games-list entry: the cross-sport `(state, id)` pair. The state is
/// the resolved one — a delayed start at period 0 lists as a pregame.
#[derive(Debug, Clone, PartialEq)]
pub struct ListEntry {
    pub state: GameState,
    pub id: EText,
}

/// The games list plus the counts `find_event` semantics need (ruling 13).
#[derive(Debug)]
pub struct GamesList {
    /// Clean events with a competition, in body order.
    pub games: heapless::Vec<ListEntry, LIST_MAX>,
    /// Events that parsed clean (including competition-less ones).
    pub ok: u16,
    /// Events the required-field rule dropped.
    pub failed: u16,
    /// More than [`LIST_MAX`] listable games arrived; the excess was dropped.
    pub overflowed: bool,
}

/// Streams one league scoreboard body into the games list — the same
/// per-event validation as [`GameExtractor`] (ruling 14), never terminating
/// early, so every event in the body is counted.
pub struct ListExtractor<'scratch> {
    matcher: StreamMatcher<'static, 'scratch, ScoreboardSink<IgnoreQuirks>>,
}

impl<'scratch> ListExtractor<'scratch> {
    pub fn new(scratch: &'scratch mut [u8]) -> Result<Self, Error> {
        let sink = ScoreboardSink::new(
            Mode::List {
                games: heapless::Vec::new(),
                overflowed: false,
            },
            IgnoreQuirks,
        );
        Ok(Self {
            matcher: StreamMatcher::new(SCOREBOARD_TABLE, sink, scratch)?,
        })
    }

    pub fn write(&mut self, chunk: &[u8]) -> Result<(), Error> {
        self.matcher.write(chunk)
    }

    pub fn finish(self) -> Result<GamesList, ExtractError> {
        let sink = self.matcher.finish()?;
        if sink.body_bad {
            return Err(ExtractError::MalformedBody);
        }
        let (ok, failed) = (sink.ok, sink.failed);
        match sink.mode {
            Mode::List { games, overflowed } => Ok(GamesList {
                games,
                ok,
                failed,
                overflowed,
            }),
            // Unreachable by construction (the constructor sets List mode).
            Mode::Detail { .. } => Ok(GamesList {
                games: heapless::Vec::new(),
                ok,
                failed,
                overflowed: false,
            }),
        }
    }
}

// ---------------------------------------------------------------- summary

/// The `participants[*]` element in progress.
#[derive(Debug, Clone, Default)]
struct ParticipantScratch {
    active: bool,
    is_scorer: bool,
    athlete_id: Exact<ID_BYTES>,
    name: WireText,
}

/// The `plays[*]` element in progress, folded at its close.
#[derive(Debug, Clone, Default)]
struct PlayScratch {
    active: bool,
    id: WireText,
    type_id: Option<u16>,
    penalty_type: bool,
    penalty_minutes: bool,
    text: WireText,
    period: Option<u8>,
    elapsed: Option<u16>,
    team_id: Exact<ID_BYTES>,
    scoring: bool,
    /// The `scorer` participant's short name, and the first participant's
    /// as the fallback (§3.5).
    scorer: WireText,
    first_name: WireText,
    participants: u16,
    /// Participant ids, kept for a team-less shootout attempt's goalie
    /// fallback (§3.8); the fourth and later are dropped.
    athletes: heapless::Vec<Exact<ID_BYTES>, ATTEMPT_ATHLETES>,
    x: Option<i8>,
    y: Option<i8>,
    strength: Option<u16>,
    shot_info: Option<u16>,
    participant: ParticipantScratch,
}

/// A goal awaiting its side (§3.5); the ring keeps the newest [`MAX_GOALS`].
#[derive(Debug, Clone)]
struct GoalRec {
    team_id: Exact<ID_BYTES>,
    period: u8,
    elapsed: u16,
    tag: GoalTag,
    coordinate: Option<(i8, i8)>,
    /// Kept per goal because the newest *kept* goal's scorer is the one the
    /// strip shows, and which goal that is waits for the team mapping.
    scorer: EText,
}

/// A shootout attempt awaiting its side (§3.8).
#[derive(Debug, Clone)]
struct AttemptRec {
    team_id: Exact<ID_BYTES>,
    athletes: heapless::Vec<Exact<ID_BYTES>, ATTEMPT_ATHLETES>,
    scored: bool,
}

/// One `onIce[*]` block: a team's unit on the ice, goalie included.
#[derive(Debug, Clone, Default)]
struct OnIceBlock {
    team_id: Exact<ID_BYTES>,
    /// Every entry counted, however many; `athletes` holds the first
    /// [`ON_ICE_IDS`] ids, which is all the goalie check can need.
    entries: u8,
    athletes: heapless::Vec<Exact<ID_BYTES>, ON_ICE_IDS>,
}

/// The `boxscore.players[*].statistics[*]` group in progress. Its `name` may
/// follow its `athletes`, so every group's first ids are collected and only
/// the goalies' are kept.
#[derive(Debug, Clone, Default)]
struct GroupScratch {
    active: bool,
    is_goalies: bool,
    ids: heapless::Vec<Exact<ID_BYTES>, GOALIES_PER_TEAM>,
    overflowed: bool,
}

/// One `boxscore.players[*]` block: a team, its goalie ids, and every id it
/// lists (§3.4 step 1's listed players).
#[derive(Debug, Clone, Default)]
struct TeamRoster {
    team_id: Exact<ID_BYTES>,
    goalies: heapless::Vec<Exact<ID_BYTES>, GOALIES_PER_TEAM>,
    players: heapless::Vec<Exact<ID_BYTES>, LISTED_PER_TEAM>,
    /// The listing is incomplete: "not listed" proves nothing any more.
    players_overflowed: bool,
    group: GroupScratch,
}

impl TeamRoster {
    fn lists(&self, id: &Exact<ID_BYTES>) -> bool {
        self.players.iter().any(|player| same_id(player, id))
    }

    fn has_goalie(&self, id: &Exact<ID_BYTES>) -> bool {
        self.goalies.iter().any(|goalie| same_id(goalie, id))
    }
}

/// The `boxscore.teams[*].statistics[*]` entry in progress.
#[derive(Debug, Clone, Default)]
struct StatScratch {
    active: bool,
    is_shots: bool,
    value: Option<u8>,
}

/// One `boxscore.teams[*]` block: a team and its `shotsTotal`.
#[derive(Debug, Clone, Default)]
struct BoxTeam {
    team_id: Exact<ID_BYTES>,
    shots: Option<u8>,
    stat: StatScratch,
}

/// One `header.competitions[0].competitors[*]`: the team-id → side mapping.
#[derive(Debug, Clone, Default)]
struct HeaderTeam {
    team_id: Exact<ID_BYTES>,
    home_away: Option<HomeAway>,
}

/// A penalty pending against the team that took it (§3.4 step 2b): from
/// its row until play resumes or the clock moves on. Keyed by team id — the
/// side waits for the header (ruling 4).
#[derive(Debug, Clone)]
struct PendingPenalty {
    team: Exact<ID_BYTES>,
    period: u8,
    elapsed: u16,
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
    context: SummaryContext,
    quirks: Q,
    play: PlayScratch,
    goals: heapless::Vec<GoalRec, MAX_GOALS>,
    flash: Option<LastPlay>,
    /// The latest team-bearing non-penalty play's team and strength code —
    /// §3.4's fallback.
    fallback: Option<(Exact<ID_BYTES>, Option<u16>)>,
    /// One slot per team: the newest penalty still pending against it.
    pending: [Option<PendingPenalty>; TEAMS],
    attempts: heapless::Vec<AttemptRec, ATTEMPT_MAX>,
    attempts_overflowed: bool,
    shootout_seen: bool,
    shootout_ended: bool,
    on_ice: Pool<OnIceBlock>,
    players: Pool<TeamRoster>,
    box_teams: Pool<BoxTeam>,
    header: Pool<HeaderTeam>,
}

impl<Q: Quirks> SummarySink<Q> {
    fn new(context: SummaryContext, quirks: Q) -> Self {
        Self {
            context,
            quirks,
            play: PlayScratch::default(),
            goals: heapless::Vec::new(),
            flash: None,
            fallback: None,
            pending: [None, None],
            attempts: heapless::Vec::new(),
            attempts_overflowed: false,
            shootout_seen: false,
            shootout_ended: false,
            on_ice: Pool::default(),
            players: Pool::default(),
            box_teams: Pool::default(),
            header: Pool::default(),
        }
    }

    fn fold_participant(&mut self) {
        let play = &mut self.play;
        let participant = &mut play.participant;
        if !participant.active {
            return;
        }
        participant.active = false;
        play.participants = play.participants.saturating_add(1);
        if let Some(name) = participant.name.get() {
            if play.participants == 1 {
                play.first_name.set(name);
            }
            if participant.is_scorer && !play.scorer.seen() {
                play.scorer.set(name);
            }
        }
        if participant.athlete_id.seen() {
            let _ = play.athletes.push(participant.athlete_id.clone());
        }
    }

    /// §3.4–3.8 over one play, in document order: the strength fallback,
    /// the goal ring, the shootout sequence, the flash candidate.
    fn fold_play(&mut self) {
        let play = &mut self.play;
        if !play.active {
            return;
        }
        play.active = false;
        // A play without a period cannot be placed in the game at all.
        let Some(period) = play.period else {
            return;
        };
        let in_shootout = !self.context.playoffs && period >= FIRST_SHOOTOUT_PERIOD;
        let penalty = is_penalty(play.type_id, play.penalty_type);

        // The latest play with a team that is not a penalty: a penalty
        // carries the strength from before it, and a team-less play's
        // strength names no side (§3.4 step 4).
        if play.team_id.seen() && !penalty {
            self.fallback = Some((play.team_id.clone(), play.strength));
        }

        // §3.4 step 2b: a penalty is pending from its row until play resumes
        // — a face-off, a period boundary, a goal — or any row at a later
        // clock; rows at its own clock are the same whistle, reordered.
        if matches!(
            play.type_id,
            Some(
                TYPE_FACE_OFF
                    | TYPE_PERIOD_START
                    | TYPE_PERIOD_END
                    | TYPE_GAME_END
                    | TYPE_PERIOD_BOUNDARY
                    | TYPE_GOAL
            )
        ) {
            self.pending = [None, None];
        } else if let Some(elapsed) = play.elapsed {
            for slot in &mut self.pending {
                if slot
                    .as_ref()
                    .is_some_and(|pending| (period, elapsed) > (pending.period, pending.elapsed))
                {
                    *slot = None;
                }
            }
            if penalty && play.team_id.seen() {
                let newest = PendingPenalty {
                    team: play.team_id.clone(),
                    period,
                    elapsed,
                };
                let same_team = |slot: &Option<PendingPenalty>| {
                    slot.as_ref()
                        .is_some_and(|pending| same_id(&pending.team, &newest.team))
                };
                let slot = self
                    .pending
                    .iter()
                    .position(same_team)
                    .or_else(|| self.pending.iter().position(Option::is_none));
                if let Some(slot) = slot {
                    self.pending[slot] = Some(newest);
                }
            }
        }

        // §3.5's goals exclude the shootout; §3.6's are the same goals.
        let goal = play.scoring && !in_shootout;
        if goal {
            if self.goals.is_full() {
                self.goals.remove(0);
            }
            let _ = self.goals.push(GoalRec {
                team_id: play.team_id.clone(),
                period,
                // A goal ESPN logged with an unreadable clock keeps its slot
                // at 0:00 rather than vanishing from the rink.
                elapsed: play.elapsed.unwrap_or(0),
                tag: goal_tag(play.strength, play.shot_info),
                coordinate: play.x.zip(play.y),
                scorer: owned(play.scorer.get().or(play.first_name.get()).unwrap_or("")),
            });
        }

        if in_shootout {
            self.shootout_seen = true;
            if matches!(
                play.type_id,
                Some(TYPE_GOAL | TYPE_SHOT | TYPE_MISSED | TYPE_FAILED_ATTEMPT)
            ) {
                let attempt = AttemptRec {
                    team_id: play.team_id.clone(),
                    athletes: play.athletes.clone(),
                    scored: play.type_id == Some(TYPE_GOAL),
                };
                if self.attempts.push(attempt).is_err() && !self.attempts_overflowed {
                    self.attempts_overflowed = true;
                    self.quirks.quirk(Quirk::BoundedOverflow);
                }
            }
            if play.type_id == Some(TYPE_SHOOTOUT_END) {
                self.shootout_ended = true;
            }
        }

        if let Some(goal) = flash_kind(play.type_id, goal, play.penalty_type, play.penalty_minutes)
        {
            if let (Some(id), Some(text)) = (play.id.get(), play.text.get()) {
                self.flash = Some(LastPlay {
                    id: owned(id),
                    text: flash_text(goal, text),
                });
            }
        }
    }

    fn fold_group(&mut self) {
        let Some(team) = self.players.current() else {
            return;
        };
        let group = &mut team.group;
        if !group.active {
            return;
        }
        group.active = false;
        if group.is_goalies {
            team.goalies = core::mem::take(&mut group.ids);
            if group.overflowed {
                self.quirks.quirk(Quirk::BoundedOverflow);
            }
        }
    }

    fn fold_box_stat(&mut self) {
        let Some(team) = self.box_teams.current() else {
            return;
        };
        if !team.stat.active {
            return;
        }
        team.stat.active = false;
        if team.stat.is_shots {
            team.shots = team.stat.value;
        }
    }

    /// Everything that waited for the team mapping (ruling 4), resolved.
    /// The sink is taken apart so the pools lend their ids while the quirk
    /// receiver is written to.
    fn resolve(self) -> SummaryReport<Q> {
        let Self {
            context,
            mut quirks,
            goals: goal_recs,
            flash,
            fallback,
            pending,
            attempts,
            shootout_seen,
            shootout_ended,
            on_ice,
            players,
            box_teams,
            header,
            ..
        } = self;
        let (home_id, away_id) = header_ids(&header.items);
        let home = home_id.as_ref().and_then(Exact::valid);
        let away = away_id.as_ref().and_then(Exact::valid);

        let mut shots = [None; TEAMS];
        for team in &box_teams.items {
            if let Some(side) = side_of(&team.team_id, home, away) {
                shots[slot(side)] = team.shots;
            }
        }
        let shots = match shots {
            [Some(away), Some(home)] => Some(Shots { away, home }),
            _ => None,
        };

        let mut rosters: [Option<&TeamRoster>; TEAMS] = [None, None];
        for team in &players.items {
            if let Some(side) = side_of(&team.team_id, home, away) {
                rosters[slot(side)] = Some(team);
            }
        }

        // The strip shows the scorer of the newest goal kept: a dropped
        // newest goal hands the line to the one before it, never leaves it
        // empty beside a goal that is still there.
        let mut goals = heapless::Vec::new();
        let mut last_scorer = EText::new();
        let mut unplaceable = false;
        for rec in &goal_recs {
            match side_of(&rec.team_id, home, away) {
                Some(side) => {
                    let _ = goals.push(Goal {
                        side,
                        tag: rec.tag,
                        period: rec.period,
                        elapsed: rec.elapsed,
                        coordinate: rec.coordinate.map(|(x, y)| normalize(side, x, y)),
                    });
                    last_scorer = rec.scorer.clone();
                }
                None => unplaceable = true,
            }
        }

        let mut pending_against = [false; TEAMS];
        for penalty in pending.iter().flatten() {
            if let Some(side) = side_of(&penalty.team, home, away) {
                pending_against[slot(side)] = true;
            }
        }
        let strength = if context.state == GameState::Live && !context.live_shootout {
            resolve_strength(
                &on_ice.items,
                fallback.as_ref(),
                home,
                away,
                &rosters,
                &context,
                pending_against,
                &mut quirks,
            )
        } else {
            None
        };
        let shootout = shootout_seen.then(|| {
            resolve_shootout(&attempts, shootout_ended, home, away, &rosters, &mut quirks)
        });
        if unplaceable {
            quirks.quirk(Quirk::UnplaceableGoal);
        }

        SummaryReport {
            summary: Summary {
                shots,
                strength,
                goals,
                last_scorer,
                last_play: flash,
                shootout,
            },
            quirks,
        }
    }
}

/// The team-id → side mapping, from the header competitors.
fn header_ids(header: &[HeaderTeam]) -> (Option<Exact<ID_BYTES>>, Option<Exact<ID_BYTES>>) {
    let mut home = None;
    let mut away = None;
    for team in header {
        match team.home_away {
            Some(HomeAway::Home) if home.is_none() => home = Some(team.team_id.clone()),
            Some(HomeAway::Away) if away.is_none() => away = Some(team.team_id.clone()),
            _ => {}
        }
    }
    (home, away)
}

/// §3.4 steps 1–5: `onIce` against the known goalies and the listed
/// players, else the latest play's strength. Quiet in the one expected case
/// — no `onIce` blocks, a period start — and a quirk for anything else
/// that fails. `pending_against[k]` is whether a penalty is pending against
/// side `k`'s team.
#[allow(clippy::too_many_arguments)] // the resolve pass's facts, each already resolved to sides
fn resolve_strength<Q: Quirks>(
    on_ice: &[OnIceBlock],
    fallback: Option<&(Exact<ID_BYTES>, Option<u16>)>,
    home: Option<&str>,
    away: Option<&str>,
    rosters: &[Option<&TeamRoster>; TEAMS],
    context: &SummaryContext,
    pending_against: [bool; TEAMS],
    quirks: &mut Q,
) -> Option<Strength> {
    if !on_ice.is_empty() {
        if let Some(strength) =
            on_ice_strength(on_ice, home, away, rosters, context, pending_against)
        {
            // Step 3: the intermission freezes the last unit, extra attacker
            // included, so a pulled net there is nothing to show; a power
            // play carrying over still is.
            if context.end_of_period && matches!(strength.kind, StrengthKind::EmptyNet(_)) {
                return None;
            }
            return Some(strength);
        }
        quirks.quirk(Quirk::OnIceInsane);
    }
    let (team, strength) = fallback?;
    let side = side_of(team, home, away)?;
    let kind = match *strength {
        Some(STRENGTH_POWER_PLAY) => StrengthKind::PowerPlay(side),
        Some(STRENGTH_SHORT_HANDED) => StrengthKind::PowerPlay(other(side)),
        _ => StrengthKind::Even,
    };
    Some(Strength {
        kind,
        away_skaters: 0,
        home_skaters: 0,
    })
}

/// §3.4 steps 1–2 and 4 over the two units; `None` is "not sane": a block
/// that maps to no side or to a side already seen, a side without a block,
/// a team with no known goalie, skaters outside 3–6, or two pulled nets at
/// once — which is not hockey, only a feed that lost both goalies.
fn on_ice_strength(
    on_ice: &[OnIceBlock],
    home: Option<&str>,
    away: Option<&str>,
    rosters: &[Option<&TeamRoster>; TEAMS],
    context: &SummaryContext,
    pending_against: [bool; TEAMS],
) -> Option<Strength> {
    let probables = &context.probable_goalies;
    // Who is on the ice, and whether the goalie is among them.
    let mut units: [Option<(u8, bool)>; TEAMS] = [None, None];
    for block in on_ice {
        let k = slot(side_of(&block.team_id, home, away)?);
        if units[k].is_some() {
            return None;
        }
        let (roster, probable) = (rosters[k], probables[k].as_deref());
        // A known goalie: the boxscore's group, or the scoreboard's probable
        // — the group lists only goalies who have played, so it is empty at
        // puck drop and behind every goalie change.
        let known = |id: &Exact<ID_BYTES>| {
            roster.is_some_and(|roster| roster.has_goalie(id)) || id.valid() == probable
        };
        // The relief goalie: on the ice before the boxscore lists him, so
        // not a listed player — provable only while the listing is complete.
        let unlisted = |id: &Exact<ID_BYTES>| {
            roster.is_none_or(|roster| !roster.players_overflowed && !roster.lists(id))
        };
        let present = block.athletes.iter().any(|id| known(id) || unlisted(id));
        units[k] = Some((block.entries, present));
    }
    let [Some((away_entries, away_goalie)), Some((home_entries, home_goalie))] = units else {
        return None;
    };
    let entries = [away_entries, home_entries];
    let goalie = [away_goalie, home_goalie];

    // A unit with its goalie skates entries − 1. A goalie-less unit is not
    // yet a pull: ESPN leaves goalies out of `onIce` and keeps a pulled unit
    // frozen after the goal that ended it (§3.4 step 2), so until the count
    // and the context both fit a pull the goalie is presumed omitted and the
    // unit capped at a full five. The overtime comparison reads the
    // opponent's own pre-pull skaters, so a goalie-less opponent counts as
    // omitted there — two pulls are insane either way.
    let omitted = |k: usize| {
        if goalie[k] {
            entries[k].saturating_sub(1)
        } else {
            entries[k].min(FULL_SKATERS)
        }
    };
    let mut skaters = [omitted(0), omitted(1)];
    let mut pulled = [false; TEAMS];
    for (k, &has_goalie) in goalie.iter().enumerate() {
        if has_goalie {
            continue;
        }
        let opponent = 1 - k;
        let trails = context.scores[k] < context.scores[opponent];
        // (a) The count: six attackers, five when trailing late, or — in
        // the regular season's 3-on-3 overtime — more than the opponent
        // has skaters.
        let count_fits = if !context.playoffs && context.period == FIRST_OVERTIME_PERIOD {
            entries[k] > omitted(opponent)
        } else {
            entries[k] >= PULLED_UNIT
                || (entries[k] == FULL_SKATERS
                    && trails
                    && context.period >= LAST_REGULATION_PERIOD)
        };
        // (b) The context: a team pulls its goalie when it trails or when a
        // delayed penalty against the other side lets it.
        if count_fits && (trails || pending_against[opponent]) {
            pulled[k] = true;
            skaters[k] = entries[k];
        }
    }
    let [away_skaters, home_skaters] = skaters;
    let [away_pulled, home_pulled] = pulled;

    let known_goalie = |k: usize| {
        rosters[k].is_some_and(|roster| !roster.goalies.is_empty()) || probables[k].is_some()
    };
    let in_range = |skaters: u8| (3..=6).contains(&skaters);
    if !known_goalie(0)
        || !known_goalie(1)
        || !in_range(away_skaters)
        || !in_range(home_skaters)
        || (away_pulled && home_pulled)
    {
        return None;
    }
    let kind = if away_pulled {
        StrengthKind::EmptyNet(Side::Away)
    } else if home_pulled {
        StrengthKind::EmptyNet(Side::Home)
    } else if away_skaters > home_skaters {
        StrengthKind::PowerPlay(Side::Away)
    } else if home_skaters > away_skaters {
        StrengthKind::PowerPlay(Side::Home)
    } else if away_skaters < 5 {
        StrengthKind::Matched
    } else {
        StrengthKind::Even
    };
    Some(Strength {
        kind,
        away_skaters,
        home_skaters,
    })
}

/// §3.8: sides by the play's team, then a participant in a goalie group
/// (that team defended), then alternation; the first 16 per side; the next
/// shooter opposite the last attempt, none after `Shootout End`.
fn resolve_shootout<Q: Quirks>(
    attempts: &[AttemptRec],
    ended: bool,
    home: Option<&str>,
    away: Option<&str>,
    rosters: &[Option<&TeamRoster>; TEAMS],
    quirks: &mut Q,
) -> Shootout {
    // What the play itself says: its team, else the team of a goalie among
    // its participants (the boxscore group's, as §3.8 reads), who defended.
    let anchors: heapless::Vec<Option<Side>, ATTEMPT_MAX> = attempts
        .iter()
        .map(|attempt| {
            side_of(&attempt.team_id, home, away).or_else(|| {
                attempt.athletes.iter().find_map(|athlete| {
                    [Side::Away, Side::Home].into_iter().find_map(|defended| {
                        rosters[slot(defended)]
                            .is_some_and(|roster| roster.has_goalie(athlete))
                            .then(|| other(defended))
                    })
                })
            })
        })
        .collect();
    // Shooters strictly alternate, so an attempt the play itself cannot
    // place takes its side from the nearest one that was, behind it or ahead
    // of it: the same side an even distance away, the other side an odd one.
    let placed = |i: usize| {
        anchors[i].or_else(|| {
            (1..anchors.len()).find_map(|distance| {
                let behind = i.checked_sub(distance).and_then(|j| anchors[j]);
                let ahead = anchors.get(i + distance).copied().flatten();
                behind
                    .or(ahead)
                    .map(|side| if distance % 2 == 0 { side } else { other(side) })
            })
        })
    };

    let mut taken = [0u8; TEAMS];
    let mut scored = [0u16; TEAMS];
    let mut last = None;
    let mut dropped = false;
    for (i, attempt) in attempts.iter().enumerate() {
        let Some(side) = placed(i) else {
            dropped = true;
            continue;
        };
        let k = slot(side);
        if usize::from(taken[k]) < MAX_SHOOTOUT_ATTEMPTS {
            if attempt.scored {
                scored[k] |= 1 << taken[k];
            }
            taken[k] += 1;
        }
        last = Some(side);
    }
    if dropped {
        quirks.quirk(Quirk::UnplaceableAttempt);
    }
    Shootout {
        away: ShootoutAttempts {
            taken: taken[0],
            scored: scored[0],
        },
        home: ShootoutAttempts {
            taken: taken[1],
            scored: scored[1],
        },
        next_shooter: if ended { None } else { last.map(other) },
    }
}

impl<Q: Quirks> Sink for SummarySink<Q> {
    fn value(&mut self, pattern: usize, _indices: &[u16], value: Value<'_>) -> Directive {
        let play = &mut self.play;
        match pattern {
            S_PLAY_ID => {
                if let Value::Str(text) = value {
                    play.id.set(text);
                }
            }
            S_PLAY_TYPE => play.type_id = code(value),
            S_PLAY_PENALTY_TYPE => play.penalty_type = matches!(value, Value::Str(_)),
            S_PLAY_PENALTY_MINUTES => {
                play.penalty_minutes = matches!(value, Value::Str(text) if !text.is_empty());
            }
            S_PLAY_TEXT => {
                if let Value::Str(text) = value {
                    play.text.set(text);
                }
            }
            S_PLAY_PERIOD => {
                if let Value::Num(text) = value {
                    play.period = num_u8(text);
                }
            }
            S_PLAY_CLOCK => {
                if let Value::Str(text) = value {
                    play.elapsed = elapsed_seconds(text);
                }
            }
            S_PLAY_TEAM => {
                if let Value::Str(text) = value {
                    play.team_id.set(text);
                }
            }
            S_PLAY_SCORING => {
                if let Value::Bool(flag) = value {
                    play.scoring = flag;
                }
            }
            S_PARTICIPANT_TYPE => play.participant.is_scorer = value == Value::Str("scorer"),
            S_PARTICIPANT_ID => {
                if let Value::Str(text) = value {
                    play.participant.athlete_id.set(text);
                }
            }
            S_PARTICIPANT_NAME => {
                if let Value::Str(text) = value {
                    play.participant.name.set(text);
                }
            }
            S_PLAY_X => play.x = coordinate_value(value, MAX_COORDINATE_X),
            S_PLAY_Y => play.y = coordinate_value(value, MAX_COORDINATE_Y),
            S_PLAY_STRENGTH => play.strength = code(value),
            S_PLAY_SHOT_INFO => play.shot_info = code(value),
            S_ON_ICE_TEAM_ID => {
                if let (Value::Str(text) | Value::Num(text), Some(block)) =
                    (value, self.on_ice.current())
                {
                    block.team_id.set(text);
                }
            }
            // A scalar entry still counts as someone on the ice.
            S_ON_ICE_ENTRY => {
                if let Some(block) = self.on_ice.current() {
                    block.entries = block.entries.saturating_add(1);
                }
            }
            S_ON_ICE_ATHLETE => {
                if let (Value::Str(text) | Value::Num(text), Some(block)) =
                    (value, self.on_ice.current())
                {
                    let mut id = Exact::default();
                    id.set(text);
                    let _ = block.athletes.push(id);
                }
            }
            S_PLAYERS_TEAM_ID => {
                if let (Value::Str(text), Some(team)) = (value, self.players.current()) {
                    team.team_id.set(text);
                }
            }
            S_PLAYERS_GROUP_NAME => {
                if let Some(team) = self.players.current() {
                    team.group.is_goalies = value == Value::Str("goalies");
                }
            }
            S_PLAYERS_ATHLETE_ID => {
                if let (Value::Str(text), Some(team)) = (value, self.players.current()) {
                    let mut id = Exact::default();
                    id.set(text);
                    // Every group lists the player; the goalies group's name
                    // may follow its athletes, so the group keeps its own
                    // copy until its close says which it was.
                    if !team.lists(&id) && team.players.push(id.clone()).is_err() {
                        team.players_overflowed = true;
                    }
                    if team.group.ids.push(id).is_err() {
                        team.group.overflowed = true;
                    }
                }
            }
            S_BOX_TEAM_ID => {
                if let (Value::Str(text), Some(team)) = (value, self.box_teams.current()) {
                    team.team_id.set(text);
                }
            }
            S_BOX_STAT_NAME => {
                if let Some(team) = self.box_teams.current() {
                    team.stat.is_shots = value == Value::Str("shotsTotal");
                }
            }
            S_BOX_STAT_VALUE => {
                if let Some(team) = self.box_teams.current() {
                    // Saturates at 255 (§3.3).
                    team.stat.value = code(value).map(|shots| shots.min(u16::from(u8::MAX)) as u8);
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
            _ => {}
        }
        Directive::Continue
    }

    fn enter(&mut self, pattern: usize, _indices: &[u16], _kind: ContainerKind) -> Directive {
        match pattern {
            S_PLAY => {
                self.play = PlayScratch {
                    active: true,
                    ..PlayScratch::default()
                };
            }
            S_PARTICIPANT => {
                self.play.participant = ParticipantScratch {
                    active: true,
                    ..ParticipantScratch::default()
                };
            }
            S_ON_ICE_TEAM => self.on_ice.open(),
            S_ON_ICE_ENTRY => {
                if let Some(block) = self.on_ice.current() {
                    block.entries = block.entries.saturating_add(1);
                }
            }
            S_PLAYERS => self.players.open(),
            S_PLAYERS_GROUP => {
                if let Some(team) = self.players.current() {
                    team.group = GroupScratch {
                        active: true,
                        ..GroupScratch::default()
                    };
                }
            }
            S_BOX_TEAM => self.box_teams.open(),
            S_BOX_STAT => {
                if let Some(team) = self.box_teams.current() {
                    team.stat = StatScratch {
                        active: true,
                        ..StatScratch::default()
                    };
                }
            }
            S_HEADER_COMPETITOR => self.header.open(),
            _ => {}
        }
        Directive::Continue
    }

    fn leave(&mut self, pattern: usize, _indices: &[u16]) -> Directive {
        let overflowed = match pattern {
            S_PLAY => {
                self.fold_play();
                false
            }
            S_PARTICIPANT => {
                self.fold_participant();
                false
            }
            S_ON_ICE_TEAM => self.on_ice.close(),
            S_PLAYERS_GROUP => {
                self.fold_group();
                false
            }
            S_PLAYERS => {
                let listing_overflowed = self
                    .players
                    .current()
                    .is_some_and(|team| team.players_overflowed);
                if listing_overflowed {
                    self.quirks.quirk(Quirk::BoundedOverflow);
                }
                self.players.close()
            }
            S_BOX_STAT => {
                self.fold_box_stat();
                false
            }
            S_BOX_TEAM => self.box_teams.close(),
            S_HEADER_COMPETITOR => self.header.close(),
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

/// Streams one per-event summary body (85 KB at puck drop, 630 KB at the
/// end of a long final) down to §3.4–3.8's facts, keeping only what those
/// rules need (the module docs list the bounds).
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
