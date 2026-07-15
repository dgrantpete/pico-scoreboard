use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::espn::types::{
    CompetitionState, EspnAthlete, EspnLastPlay, EspnLinescore, EspnRecord, EspnTeam, EspnVenue,
    HomeAway,
};
use crate::shared::competitor::Competitor;
use crate::shared::game::Record;
use crate::shared::team::{TeamColors, TeamState};

// (shared inbound leaves re-used here: CompetitionState, EspnTeam, HomeAway,
// EspnVenue, EspnRecord, EspnLinescore, EspnLastPlay, EspnAthlete)

// ---------- ESPN inbound types ----------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnEvent {
    pub(crate) id: String,
    /// Scheduled start, ISO 8601 (`%Y-%m-%dT%H:%MZ`); 100%-present in the
    /// corpus. Parsed to a unix epoch for the pregame wire payload.
    pub(crate) date: String,
    /// Event-level weather (present pre/live, absent post). All-`Option` so a
    /// malformed block degrades to "no weather" instead of dropping the event.
    #[serde(default)]
    pub(crate) weather: Option<EspnWeather>,
    pub(crate) competitions: Vec<EspnCompetition>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnWeather {
    /// ESPN randomly swaps `displayValue`/`conditionId` between polls; the
    /// non-numeric member is the human condition text (see
    /// `transform::normalize_weather`).
    #[serde(default)]
    pub(crate) display_value: Option<String>,
    #[serde(default)]
    pub(crate) condition_id: Option<String>,
    #[serde(default)]
    pub(crate) temperature: Option<i16>,
}

#[derive(Deserialize)]
#[serde(try_from = "EspnCompetitionDto")]
pub(crate) enum EspnCompetition {
    PreGame {
        competitors: [EspnCompetitor; 2],
        venue_name: String,
    },
    Live {
        competitors: [EspnCompetitor; 2],
        situation: EspnSituation,
        period: u8,
        short_detail: String,
    },
    Final {
        competitors: [EspnCompetitor; 2],
        /// Innings played (`status.period`); 9 for a standard game, more for
        /// extras.
        period: u8,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnCompetitionDto {
    competitors: Vec<EspnCompetitor>,
    status: EspnStatus,
    situation: Option<EspnSituation>,
    /// Present in every sampled state, but only the pregame arm requires it.
    venue: Option<EspnVenue>,
}

impl TryFrom<EspnCompetitionDto> for EspnCompetition {
    type Error = String;

    fn try_from(dto: EspnCompetitionDto) -> Result<Self, Self::Error> {
        let two_competitors = |competitors: Vec<EspnCompetitor>| {
            <[EspnCompetitor; 2]>::try_from(competitors)
                .map_err(|v: Vec<_>| format!("expected 2 competitors, got {}", v.len()))
        };
        match dto.status.r#type.state {
            CompetitionState::Pre => {
                let venue_name = dto
                    .venue
                    .ok_or("pregame competition missing venue")?
                    .full_name;
                Ok(Self::PreGame {
                    competitors: two_competitors(dto.competitors)?,
                    venue_name,
                })
            }
            CompetitionState::In => {
                let competitors = two_competitors(dto.competitors)?;
                let situation = dto.situation.ok_or("live competition missing situation")?;
                Ok(Self::Live {
                    competitors,
                    situation,
                    period: dto.status.period,
                    short_detail: dto.status.r#type.short_detail,
                })
            }
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
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnStatusType {
    pub(crate) state: CompetitionState,
    pub(crate) short_detail: String,
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
    /// Probable starting pitcher(s). Consumed only pregame.
    #[serde(default)]
    pub(crate) probables: Vec<EspnProbable>,
    /// Per-inning runs. Consumed only for final games.
    #[serde(default)]
    pub(crate) linescores: Vec<EspnLinescore>,
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

#[derive(Deserialize)]
pub(crate) struct EspnProbable {
    pub(crate) athlete: EspnAthlete,
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

// ---------- Outbound domain model ----------

/// One MLB game, discriminated on the cross-sport `pre/in/post` state. The
/// firmware parses the `state` byte (0/1/2) first, then the matching payload
/// (see `wire.rs`).
#[derive(Serialize, ToSchema)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum MlbGame {
    Pregame(MlbPregameGame),
    Live(MlbLiveGame),
    Final(MlbFinalGame),
}

/// Pre-game snapshot: matchup, scheduled start, venue, and (when available)
/// weather, records, and probable pitchers.
#[derive(Serialize, ToSchema)]
pub struct MlbPregameGame {
    pub game_id: String,
    /// Scheduled start as a unix epoch (seconds, UTC). The firmware applies
    /// the device's `utc_offset` for local display — it never parses dates.
    pub start_time: u32,
    pub venue: String,
    /// Absent when ESPN's weather block is missing or unusable.
    pub weather: Option<MlbWeather>,
    pub home: MlbPregameTeam,
    pub away: MlbPregameTeam,
}

#[derive(Serialize, ToSchema)]
pub struct MlbWeather {
    pub condition: String,
    pub temperature: i16,
}

/// Pre-game team: identity and pre-game context, but no score (there is no
/// game yet — a fake 0 would invite the firmware to render one).
#[derive(Serialize, ToSchema)]
pub struct MlbPregameTeam {
    /// Team abbreviation, e.g. "BOS" — firmware uses this to fetch the logo.
    pub abbreviation: String,
    pub colors: TeamColors,
    /// Overall season record; absent when ESPN omits or malforms it.
    pub record: Option<Record>,
    /// Probable starting pitcher's short name, e.g. "G. Marquez".
    pub probable_pitcher: Option<String>,
}

/// Final snapshot: score, per-inning line score, and innings played (9, or
/// more for extras). No explicit winner — it is derivable from the scores, and
/// a second copy could disagree with them on a glitch.
#[derive(Serialize, ToSchema)]
pub struct MlbFinalGame {
    pub game_id: String,
    pub innings_played: u8,
    pub home: MlbFinalTeam,
    pub away: MlbFinalTeam,
}

#[derive(Serialize, ToSchema)]
pub struct MlbFinalTeam {
    pub abbreviation: String,
    pub score: u32,
    pub colors: TeamColors,
    /// Runs per inning, inning 1 first. Per-team lengths are independent: a
    /// walk-off leaves the home line short, extras run past 9.
    pub line_score: Vec<u8>,
}

/// Live state snapshot for one MLB game, tailored for the Pico firmware.
#[derive(Serialize, ToSchema)]
pub struct MlbLiveGame {
    pub game_id: String,
    pub inning: MlbInning,
    pub home: TeamState,
    pub away: TeamState,
    pub count: MlbCount,
    pub bases: MlbBases,
    /// Absent between innings or before an at-bat starts.
    pub at_bat: Option<MlbAtBat>,
    pub last_play: MlbLastPlay,
}

#[derive(Serialize, ToSchema)]
pub struct MlbCount {
    pub balls: u8,
    pub strikes: u8,
    pub outs: u8,
}

#[derive(Serialize, ToSchema)]
pub struct MlbBases {
    pub first: bool,
    pub second: bool,
    pub third: bool,
}

#[derive(Serialize, ToSchema)]
pub struct MlbAtBat {
    pub pitcher: String,
    pub batter: String,
}

#[derive(Serialize, ToSchema)]
pub struct MlbLastPlay {
    pub id: String,
    pub text: String,
}

#[derive(Serialize, ToSchema)]
pub struct MlbInning {
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
