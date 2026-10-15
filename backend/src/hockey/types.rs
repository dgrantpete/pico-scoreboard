//! NHL outbound domain model — the JSON DTOs the handlers serialize and
//! `wire.rs` encodes. The inbound ESPN parse (scoreboard AND the per-event
//! summary) lives in `crates/scoreboard-espn::hockey`, which also combines the
//! two; `adapter.rs` copies its combined view here field for field.

use serde::Serialize;
use utoipa::ToSchema;

use crate::shared::game::{LastPlay, LivePhase, Side};
use crate::shared::team::{TeamColors, TeamState};

/// One NHL game, discriminated on the cross-sport `pre/in/post` state.
#[derive(Serialize, ToSchema)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum HockeyGame {
    Pregame(HockeyPregameGame),
    Live(HockeyLiveGame),
    Final(HockeyFinalGame),
}

/// Pre-game snapshot: matchup, scheduled start, venue, records and the
/// probable starting goalies.
#[derive(Serialize, ToSchema)]
pub struct HockeyPregameGame {
    pub game_id: String,
    /// Scheduled start, unix epoch seconds UTC.
    pub start_time: u32,
    pub venue: String,
    /// The playoff round and game, then the series ("Stanley Cup Final -
    /// Game 6 - CAR leads series 3-2"); empty in the regular season.
    pub info_secondary: String,
    pub playoffs: bool,
    pub home: HockeyPregameTeam,
    pub away: HockeyPregameTeam,
}

#[derive(Serialize, ToSchema)]
pub struct HockeyPregameTeam {
    /// Team abbreviation, e.g. "BOS" — key for `/{sport}/{league}/teams/{abbrev}/logo`.
    pub abbreviation: String,
    pub colors: TeamColors,
    /// Season record; absent when ESPN omits or malforms it.
    pub record: Option<HockeyRecord>,
    /// The probable starting goalie's short name ("C. DeSmith") — hockey's
    /// per-team line, where NBA and football show a betting line.
    pub goalie: Option<String>,
}

/// W-L-OTL: hockey's record has the third column the shared `Record` lacks.
#[derive(Serialize, ToSchema)]
pub struct HockeyRecord {
    pub wins: u16,
    pub losses: u16,
    pub ot_losses: u16,
}

/// Live state snapshot for one NHL game. Everything below `shots` comes from
/// the per-event summary; when its fetch failed the payload is the scoreboard
/// alone (no strength, no goals, no shootout tracker — NHL.md D3).
#[derive(Serialize, ToSchema)]
pub struct HockeyLiveGame {
    pub game_id: String,
    /// ESPN's raw period: 1–3 regulation, 4 the first overtime, 5 a
    /// regular-season shootout or a playoff second overtime (`playoffs` says
    /// which), and on through further playoff overtimes.
    pub period: u8,
    /// `end_of_period` is the intermission; hockey never sends `halftime`.
    pub phase: LivePhase,
    /// Time remaining, display-shaped ("13:38"). The feed only moves it when
    /// a play is logged, so it is exact at poll time and never extrapolated.
    pub clock: String,
    pub playoffs: bool,
    /// During a shootout both scores are the tied game score; the tally is
    /// `shootout`'s.
    pub home: TeamState,
    pub away: TeamState,
    /// Absent at puck drop, before ESPN has the stats it is derived from.
    pub shots: Option<HockeyShots>,
    /// Absent without a summary, during a shootout, or when nothing on the
    /// ice could be read.
    pub strength: Option<HockeyStrength>,
    /// The newest 16 goals outside the shootout, oldest first.
    pub goals: Vec<HockeyGoal>,
    /// The newest goal's scorer, ESPN's short name; empty with no goal.
    pub last_scorer: String,
    /// Present while the shootout runs.
    pub shootout: Option<HockeyShootout>,
    /// The newest goal or penalty; absent before either.
    pub last_play: Option<LastPlay>,
}

/// Final snapshot: score, raw per-period line score, shots, the three stars
/// and (playoffs) the series.
#[derive(Serialize, ToSchema)]
pub struct HockeyFinalGame {
    pub game_id: String,
    /// ESPN's raw period count: 3, 4 after overtime, 5 after a regular-season
    /// shootout or a second playoff overtime, and on.
    pub periods_played: u8,
    pub playoffs: bool,
    pub home: HockeyFinalTeam,
    pub away: HockeyFinalTeam,
    pub shots: Option<HockeyShots>,
    /// Absent until ESPN names all three — usually some minutes after the
    /// final horn.
    pub stars: Option<HockeyStars>,
    /// e.g. "CAR wins series 4-2"; playoffs only.
    pub series: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct HockeyFinalTeam {
    pub abbreviation: String,
    /// ESPN's score: a shootout winner's includes the deciding goal.
    pub score: u32,
    pub colors: TeamColors,
    /// Goals per period as ESPN lists them: each overtime its own entry, a
    /// shootout a fifth period of 1/0.
    pub line_score: Vec<u8>,
}

/// Shots on goal, both sides or neither.
#[derive(Serialize, ToSchema, Clone, Copy)]
pub struct HockeyShots {
    pub away: u8,
    pub home: u8,
}

/// Who is on the ice.
#[derive(Serialize, ToSchema, Clone, Copy)]
pub struct HockeyStrength {
    pub situation: StrengthSituation,
    /// Skaters, goalie excluded; 0 = unknown (the on-ice list was unusable and
    /// the situation came from the latest play's strength).
    pub away_skaters: u8,
    pub home_skaters: u8,
}

#[derive(Serialize, ToSchema, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StrengthSituation {
    /// Full strength — and a pulled goalie the feed cannot see.
    Even,
    /// `side` has more skaters.
    PowerPlay { side: Side },
    /// Equal and short-handed: 4-on-4, or 3-on-3 overtime.
    Matched,
    /// `side` has pulled its goalie. Wins over a power play.
    EmptyNet { side: Side },
}

#[derive(Serialize, ToSchema, Clone, Copy)]
pub struct HockeyGoal {
    pub side: Side,
    pub tag: GoalTag,
    /// ESPN's raw period, numbered as the live `period` is.
    pub period: u8,
    /// Seconds *elapsed* in the period — the scoring-summary convention, not
    /// the live clock's time remaining.
    pub elapsed: u16,
    /// Feet from center ice, away attacking +x; absent when ESPN logged the
    /// goal without one.
    pub coordinate: Option<HockeyCoordinate>,
}

#[derive(Serialize, ToSchema, Clone, Copy)]
pub struct HockeyCoordinate {
    pub x: i8,
    pub y: i8,
}

#[derive(Serialize, ToSchema, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum GoalTag {
    Goal,
    PowerPlay,
    ShortHanded,
    EmptyNet,
}

/// The shootout so far.
#[derive(Serialize, ToSchema)]
pub struct HockeyShootout {
    pub away: Vec<ShootoutAttempt>,
    pub home: Vec<ShootoutAttempt>,
    /// Absent once the shootout has ended.
    pub next_shooter: Option<Side>,
}

/// One attempt, in shooting order.
#[derive(Serialize, ToSchema, Clone, Copy)]
pub struct ShootoutAttempt {
    pub scored: bool,
}

#[derive(Serialize, ToSchema)]
pub struct HockeyStars {
    pub first: HockeyStar,
    pub second: HockeyStar,
    pub third: HockeyStar,
}

#[derive(Serialize, ToSchema)]
pub struct HockeyStar {
    /// ESPN's short name.
    pub name: String,
    /// Whose player; absent when ESPN did not say.
    pub side: Option<Side>,
}
