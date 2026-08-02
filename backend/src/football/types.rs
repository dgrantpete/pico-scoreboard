use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::espn::types::{
    CompetitionState, EspnLastPlay, EspnLinescore, EspnRecord, EspnStatusType, EspnTeam, EspnVenue,
    HomeAway, parse_live_phase, two_competitors, venue_name,
};
use crate::shared::competitor::Competitor;
use crate::shared::game::{LastPlay, LivePhase, Record, Side};
use crate::shared::team::{TeamColors, TeamState};

// ---------- ESPN inbound types ----------

#[cfg(test)]
pub(crate) type EspnEvent = crate::espn::types::EspnEvent<EspnCompetition>;

#[derive(Deserialize)]
#[serde(try_from = "EspnCompetitionDto")]
pub(crate) enum EspnCompetition {
    PreGame {
        competitors: [EspnCompetitor; 2],
        venue_name: String,
    },
    Live {
        competitors: [EspnCompetitor; 2],
        /// Quarters 1–4, overtime 5+ (`status.period`).
        period: u8,
        /// Raw ESPN clock, display-shaped ("12:00", "0:37"); meaningless during
        /// breaks (see `phase`), never extrapolated (NBA convention).
        display_clock: String,
        phase: LivePhase,
        /// All-optional; every field defaults to a "not applicable" sentinel so
        /// the pre-snap empty `situation: {}` (and an absent `situation`) both
        /// parse. Validated into a `FootballSituation` in the transform.
        situation: EspnSituation,
    },
    Final {
        competitors: [EspnCompetitor; 2],
        /// Quarters played; 4 for regulation, 5+ with overtime.
        period: u8,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnCompetitionDto {
    competitors: Vec<EspnCompetitor>,
    status: EspnStatus,
    /// Absent (or all-sentinel `{}`) between plays and before the opening snap.
    #[serde(default)]
    situation: EspnSituation,
    /// Present in every sampled state, but only the pregame arm requires it.
    venue: Option<EspnVenue>,
}

impl TryFrom<EspnCompetitionDto> for EspnCompetition {
    type Error = String;

    fn try_from(dto: EspnCompetitionDto) -> Result<Self, Self::Error> {
        match dto.status.r#type.state {
            CompetitionState::Pre => Ok(Self::PreGame {
                competitors: two_competitors(dto.competitors)?,
                venue_name: venue_name(dto.venue)?,
            }),
            CompetitionState::In => Ok(Self::Live {
                competitors: two_competitors(dto.competitors)?,
                period: dto.status.period,
                display_clock: dto.status.display_clock,
                phase: parse_live_phase(dto.status.r#type.description.as_deref(), "football"),
                situation: dto.situation,
            }),
            CompetitionState::Post => Ok(Self::Final {
                competitors: two_competitors(dto.competitors)?,
                period: dto.status.period,
            }),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnStatus {
    pub(crate) r#type: EspnStatusType,
    pub(crate) period: u8,
    pub(crate) display_clock: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnCompetitor {
    pub(crate) home_away: HomeAway,
    pub(crate) score: String,
    pub(crate) team: EspnTeam,
    /// Season records; the `type=="total"` entry is the overall W-L. Consumed
    /// only pregame — defaulted so live/final events with odd records parse.
    #[serde(default)]
    pub(crate) records: Vec<EspnRecord>,
    /// Per-quarter points. Consumed only for final games.
    #[serde(default)]
    pub(crate) linescores: Vec<EspnLinescore>,
    /// AP/Coaches poll position; `current == 99` is ESPN's unranked sentinel.
    /// College only — the pros omit it — and defaulted so the pros still parse.
    #[serde(default)]
    pub(crate) curated_rank: Option<EspnCuratedRank>,
}

#[derive(Deserialize)]
pub(crate) struct EspnCuratedRank {
    pub(crate) current: u16,
}

impl Competitor for EspnCompetitor {
    fn home_away(&self) -> HomeAway {
        self.home_away
    }
    fn team(&self) -> &EspnTeam {
        &self.team
    }
    fn score(&self) -> &str {
        &self.score
    }
}

/// The live play situation, straight off the scoreboard. Every numeric field is
/// an `i16` because ESPN uses `-1` (and omission) as its "not applicable"
/// sentinel between plays; the transform validates them before they reach the
/// wire (see [`super::transform`]).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnSituation {
    #[serde(default = "minus_one")]
    pub(crate) down: i16,
    #[serde(default = "minus_one")]
    pub(crate) distance: i16,
    #[serde(default = "minus_one")]
    pub(crate) yard_line: i16,
    #[serde(default = "minus_one")]
    pub(crate) home_timeouts: i16,
    #[serde(default = "minus_one")]
    pub(crate) away_timeouts: i16,
    #[serde(default)]
    pub(crate) is_red_zone: bool,
    /// Team id of the side in possession; resolved to a [`Side`] in the transform.
    #[serde(default)]
    pub(crate) possession: Option<String>,
    #[serde(default)]
    pub(crate) last_play: Option<EspnLastPlay>,
}

fn minus_one() -> i16 {
    -1
}

impl Default for EspnSituation {
    fn default() -> Self {
        Self {
            down: -1,
            distance: -1,
            yard_line: -1,
            home_timeouts: -1,
            away_timeouts: -1,
            is_red_zone: false,
            possession: None,
            last_play: None,
        }
    }
}

/// Build the pregame rank line ("#3 OHIO STATE") for a competitor. `None` for
/// the pros (`is_college` false), for an unranked team (`curatedRank.current ==
/// 99`, ESPN's unranked sentinel), and when ESPN omits the short display name
/// there is nothing to uppercase into a line. The record still travels
/// separately as numeric wins/losses — this line only carries the poll rank.
pub(crate) fn rank_line(competitor: &EspnCompetitor, is_college: bool) -> Option<String> {
    if !is_college {
        return None;
    }
    let rank = competitor.curated_rank.as_ref()?.current;
    if rank == 99 {
        return None;
    }
    let name = competitor.team.short_display_name.as_deref()?;
    if name.is_empty() {
        return None;
    }
    Some(format!("#{} {}", rank, name.to_uppercase()))
}

// ---------- Outbound domain model ----------

/// One football game, discriminated on the cross-sport `pre/in/post` state. The
/// firmware parses the `state` byte (0/1/2) first, then the matching payload
/// (see `wire.rs`).
#[derive(Serialize, ToSchema)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum FootballGame {
    Pregame(FootballPregameGame),
    Live(FootballLiveGame),
    Final(FootballFinalGame),
}

/// Pre-game snapshot: matchup, scheduled start, venue, season records, and
/// (college only) the AP/Coaches rank line.
#[derive(Serialize, ToSchema)]
pub struct FootballPregameGame {
    pub game_id: String,
    /// Scheduled start as a unix epoch (seconds, UTC). The firmware applies the
    /// device's `utc_offset` for local display — it never parses dates.
    pub start_time: u32,
    pub venue: String,
    pub home: FootballPregameTeam,
    pub away: FootballPregameTeam,
}

/// Pre-game team: identity, colors, season record, and the college rank line.
#[derive(Serialize, ToSchema)]
pub struct FootballPregameTeam {
    /// Team abbreviation, e.g. "KC" — firmware uses this to fetch the logo.
    pub abbreviation: String,
    pub colors: TeamColors,
    /// Overall season record; absent when ESPN omits or malforms it.
    pub record: Option<Record>,
    /// Display-shaped poll line ("#3 OHIO STATE"); college only and only when
    /// ranked. Rides the wire's pitcher slot (the record travels numerically).
    pub rank_line: Option<String>,
}

/// Live state snapshot for one football game, tailored for the Pico firmware.
#[derive(Serialize, ToSchema)]
pub struct FootballLiveGame {
    pub game_id: String,
    /// Quarter 1–4; overtime periods pass through as 5+.
    pub period: u8,
    /// Raw ESPN clock, display-shaped ("12:00", "0:37"); meaningless during
    /// breaks (render by `phase`). Like NBA — and unlike soccer — football's
    /// clock stops with no running signal from ESPN, so the string is exact at
    /// poll time and must not be extrapolated.
    pub clock: String,
    pub phase: LivePhase,
    pub home: TeamState,
    pub away: TeamState,
    /// The current down/distance/ball spot; absent between plays (and whenever
    /// ESPN's situation fails validation — see [`super::transform`]).
    pub situation: Option<FootballSituation>,
    /// Remaining timeouts per side; absent when ESPN hasn't populated them.
    pub timeouts: Option<Timeouts>,
    /// Absent before the opening snap.
    pub last_play: Option<LastPlay>,
}

/// The current offensive situation. Present only for a well-formed snap; the
/// transform drops a half-formed one rather than misdraw the field markers.
#[derive(Serialize, ToSchema)]
pub struct FootballSituation {
    /// 1st–4th down (validated into 1..=4).
    pub down: u8,
    /// Yards to the first-down line.
    pub distance: u8,
    /// Ball spot as an absolute 0–100 yard line.
    pub yard_line: u8,
    pub possession: Side,
    pub red_zone: bool,
}

/// Remaining timeouts for both sides. All-or-nothing on the wire (one "timeouts
/// present" flag), because ESPN populates both counts together or neither.
#[derive(Serialize, ToSchema, Clone, Copy)]
pub struct Timeouts {
    pub away: u8,
    pub home: u8,
}

/// Final snapshot: score, per-quarter line score, and quarters played (4, or
/// more for overtime). Byte-identical to the NBA final on the wire.
#[derive(Serialize, ToSchema)]
pub struct FootballFinalGame {
    pub game_id: String,
    pub periods_played: u8,
    pub home: FootballFinalTeam,
    pub away: FootballFinalTeam,
}

#[derive(Serialize, ToSchema)]
pub struct FootballFinalTeam {
    pub abbreviation: String,
    pub score: u32,
    pub colors: TeamColors,
    /// Points per quarter, quarter 1 first; overtime periods extend past 4.
    pub line_score: Vec<u8>,
}
