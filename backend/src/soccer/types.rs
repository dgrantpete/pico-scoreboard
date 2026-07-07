use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::espn::types::{CompetitionState, EspnTeam, HomeAway};
use crate::shared::team::TeamColors;

// ---------- ESPN inbound types ----------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnEvent {
    pub(crate) id: String,
    /// Scheduled start, ISO 8601. Read only by tests until pregame exposure
    /// lands (see `transform::pregame_competition_to_game`).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) date: String,
    pub(crate) competitions: Vec<EspnCompetition>,
}

#[derive(Deserialize)]
#[serde(try_from = "EspnCompetitionDto")]
pub(crate) enum EspnCompetition {
    PreGame {
        /// Read only by tests until pregame exposure lands (see
        /// `transform::pregame_competition_to_game`).
        #[cfg_attr(not(test), allow(dead_code))]
        competitors: [EspnCompetitor; 2],
    },
    Live {
        competitors: [EspnCompetitor; 2],
        /// Raw ESPN clock, already display-shaped (e.g. "45'+6'", "90'+3'").
        display_clock: String,
        period: u8,
        halftime: bool,
        details: Vec<EspnDetail>,
    },
    Final,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnCompetitionDto {
    competitors: Vec<EspnCompetitor>,
    status: EspnStatus,
    #[serde(default)]
    details: Vec<EspnDetail>,
}

impl TryFrom<EspnCompetitionDto> for EspnCompetition {
    type Error = String;

    fn try_from(dto: EspnCompetitionDto) -> Result<Self, Self::Error> {
        let two_competitors = |competitors: Vec<EspnCompetitor>| {
            <[EspnCompetitor; 2]>::try_from(competitors)
                .map_err(|v: Vec<_>| format!("expected 2 competitors, got {}", v.len()))
        };
        match dto.status.r#type.state {
            CompetitionState::Pre => Ok(Self::PreGame {
                competitors: two_competitors(dto.competitors)?,
            }),
            CompetitionState::In => Ok(Self::Live {
                competitors: two_competitors(dto.competitors)?,
                display_clock: dto
                    .status
                    .display_clock
                    .ok_or("live competition missing displayClock")?,
                period: dto
                    .status
                    .period
                    .ok_or("live competition missing period")?,
                halftime: is_halftime(dto.status.r#type.description.as_deref()),
                details: dto.details,
            }),
            CompetitionState::Post => Ok(Self::Final),
        }
    }
}

/// Halftime is indistinguishable from first-half stoppage time by clock and
/// period alone (both are period=1, clock "45'+N'"); the description is the
/// only upstream signal. Unknown live descriptions — extra time and shootout
/// phases have not been observed yet — degrade to in-play with a warning:
/// the state itself is never guessed.
fn is_halftime(description: Option<&str>) -> bool {
    match description {
        Some("Halftime") => true,
        Some("First Half" | "Second Half" | "In Progress") | None => false,
        Some(other) => {
            tracing::warn!(
                description = %other,
                "unknown live soccer status description (extra time / shootout?) — treating as in-play"
            );
            false
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnStatus {
    pub(crate) r#type: EspnStatusType,
    /// Absent pre-game.
    pub(crate) period: Option<u8>,
    pub(crate) display_clock: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnStatusType {
    pub(crate) state: CompetitionState,
    pub(crate) description: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnCompetitor {
    pub(crate) home_away: HomeAway,
    pub(crate) score: String,
    pub(crate) team: EspnTeam,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnDetail {
    pub(crate) r#type: EspnDetailType,
    pub(crate) clock: EspnDetailClock,
    pub(crate) team: Option<EspnDetailTeam>,
    #[serde(default)]
    pub(crate) scoring_play: bool,
    #[serde(default)]
    pub(crate) red_card: bool,
    #[serde(default)]
    pub(crate) athletes_involved: Vec<EspnDetailAthlete>,
}

#[derive(Deserialize)]
pub(crate) struct EspnDetailType {
    pub(crate) text: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnDetailClock {
    pub(crate) value: f64,
    pub(crate) display_value: String,
}

#[derive(Deserialize)]
pub(crate) struct EspnDetailTeam {
    pub(crate) id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnDetailAthlete {
    pub(crate) short_name: String,
}

// ---------- Outbound domain model ----------

/// One soccer game, discriminated on the cross-sport `pre/in/post` state.
///
/// The games endpoints currently serve only the `Live` variant (the same
/// live-only contract MLB has); `Pregame` is fully modeled and tested so
/// exposing it later is a handler-only change.
#[derive(Serialize, ToSchema)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum SoccerGame {
    Pregame {
        game_id: String,
        /// Scheduled start, ISO 8601 (ESPN `event.date`).
        date: String,
        home: SoccerTeam,
        away: SoccerTeam,
    },
    Live {
        game_id: String,
        /// Raw ESPN clock, display-shaped (e.g. "45'+6'", "90'+3'").
        clock: String,
        /// Regulation halves are 1 and 2; extra-time periods pass through as-is.
        half: u8,
        /// True during the interval — the clock alone cannot distinguish
        /// halftime from first-half stoppage time.
        halftime: bool,
        home: SoccerTeamState,
        away: SoccerTeamState,
        last_event: Option<LastEvent>,
    },
    Final {
        game_id: String,
    },
}

#[derive(Serialize, ToSchema)]
pub struct SoccerTeam {
    /// Team abbreviation, e.g. "POR" — key for `/{sport}/{league}/teams/{abbrev}/logo`.
    pub abbreviation: String,
    pub colors: TeamColors,
}

#[derive(Serialize, ToSchema)]
pub struct SoccerTeamState {
    pub abbreviation: String,
    pub score: u32,
    pub colors: TeamColors,
}

/// The most recent goal or red card.
#[derive(Serialize, ToSchema)]
pub struct LastEvent {
    /// e.g. "Goal - R. Lukaku" or "Red Card - J. Doe".
    pub text: String,
    /// Match clock of the event, display-shaped (e.g. "90'+3'").
    pub clock: String,
    /// Which side the event belongs to; absent if ESPN omits the team.
    pub team: Option<Side>,
}

#[derive(Serialize, ToSchema, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Home,
    Away,
}
