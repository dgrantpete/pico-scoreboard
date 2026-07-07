use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::shared::team::TeamColors;

// ---------- ESPN inbound types ----------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnScoreboard {
    pub(crate) events: Vec<EspnEvent>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnEvent {
    pub(crate) id: String,
    pub(crate) competitions: Vec<EspnCompetition>,
}

#[derive(Deserialize)]
#[serde(try_from = "EspnCompetitionDto")]
#[allow(clippy::large_enum_variant)] // Pre/Final are transient markers; boxing Live would cost more than it saves.
pub(crate) enum EspnCompetition {
    PreGame,
    Live {
        competitors: [EspnCompetitor; 2],
        situation: EspnSituation,
        period: u8,
        short_detail: String,
    },
    Final,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnCompetitionDto {
    competitors: Vec<EspnCompetitor>,
    status: EspnStatus,
    situation: Option<EspnSituation>,
}

impl TryFrom<EspnCompetitionDto> for EspnCompetition {
    type Error = String;

    fn try_from(dto: EspnCompetitionDto) -> Result<Self, Self::Error> {
        match dto.status.r#type.state {
            CompetitionState::Pre => Ok(Self::PreGame),
            CompetitionState::In => {
                let competitors: [EspnCompetitor; 2] = dto
                    .competitors
                    .try_into()
                    .map_err(|v: Vec<_>| format!("expected 2 competitors, got {}", v.len()))?;
                let situation = dto
                    .situation
                    .ok_or("live competition missing situation")?;
                Ok(Self::Live {
                    competitors,
                    situation,
                    period: dto.status.period,
                    short_detail: dto.status.r#type.short_detail,
                })
            }
            CompetitionState::Post => Ok(Self::Final),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnStatus {
    pub(crate) r#type: EspnStatusType,
    pub(crate) period: u8,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnStatusType {
    pub(crate) state: CompetitionState,
    pub(crate) short_detail: String,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CompetitionState {
    Pre,
    In,
    Post,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnCompetitor {
    pub(crate) home_away: HomeAway,
    pub(crate) score: String,
    pub(crate) team: EspnTeam,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum HomeAway {
    Home,
    Away,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnTeam {
    pub(crate) abbreviation: String,
    pub(crate) color: String,
    pub(crate) alternate_color: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnSituation {
    pub(crate) balls: u8,
    pub(crate) strikes: u8,
    pub(crate) outs: u8,
    pub(crate) on_first: bool,
    pub(crate) on_second: bool,
    pub(crate) on_third: bool,
    pub(crate) pitcher: Option<EspnPlayer>,
    pub(crate) batter: Option<EspnPlayer>,
    pub(crate) last_play: EspnLastPlay,
}

#[derive(Deserialize)]
pub(crate) struct EspnPlayer {
    pub(crate) athlete: EspnAthlete,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnAthlete {
    pub(crate) short_name: String,
}

#[derive(Deserialize)]
pub(crate) struct EspnLastPlay {
    pub(crate) id: String,
    pub(crate) text: String,
}

// ---------- Outbound domain model ----------

/// Live state snapshot for one MLB game, tailored for the Pico firmware.
#[derive(Serialize, ToSchema)]
pub struct LiveGame {
    pub game_id: String,
    pub inning: Inning,
    pub home: TeamState,
    pub away: TeamState,
    pub count: Count,
    pub bases: Bases,
    /// Absent between innings or before an at-bat starts.
    pub at_bat: Option<AtBat>,
    pub last_play: LastPlay,
}

#[derive(Serialize, ToSchema)]
pub struct TeamState {
    /// Team abbreviation, e.g. "BOS" — firmware uses this to fetch the logo.
    pub abbreviation: String,
    pub score: u32,
    pub colors: TeamColors,
}

#[derive(Serialize, ToSchema)]
pub struct Count {
    pub balls: u8,
    pub strikes: u8,
    pub outs: u8,
}

#[derive(Serialize, ToSchema)]
pub struct Bases {
    pub first: bool,
    pub second: bool,
    pub third: bool,
}

#[derive(Serialize, ToSchema)]
pub struct AtBat {
    pub pitcher: String,
    pub batter: String,
}

#[derive(Serialize, ToSchema)]
pub struct LastPlay {
    pub id: String,
    pub text: String,
}

#[derive(Serialize, ToSchema)]
pub struct Inning {
    pub number: u8,
    pub half: InningHalf,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum InningHalf {
    Top,
    Middle,
    Bottom,
    End,
}
