use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::espn::types::{
    CompetitionState, EspnLastPlay, EspnLinescore, EspnRecord, EspnStatusType, EspnTeam, EspnVenue,
    HomeAway, parse_live_phase, two_competitors, venue_name,
};
use crate::shared::competitor::Competitor;
use crate::shared::game::{LastPlay, LivePhase, Record};
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
        period: u8,
        /// Raw ESPN clock, display-shaped: "10:08", "53.0" under a minute;
        /// "0.0" or a reset "12:00" during breaks (see `phase`).
        display_clock: String,
        phase: LivePhase,
        /// Carried whole (an empty `{}` before the opening tip parses to a
        /// last-play-less situation); the transform extracts `last_play`.
        situation: EspnSituation,
    },
    Final {
        competitors: [EspnCompetitor; 2],
        /// Quarters played (`status.period`); 4 for regulation, more with
        /// overtime (unobserved in the playoff corpus but tolerated).
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
        match dto.status.r#type.state {
            CompetitionState::Pre => Ok(Self::PreGame {
                competitors: two_competitors(dto.competitors)?,
                venue_name: venue_name(dto.venue)?,
            }),
            CompetitionState::In => {
                let situation = dto.situation.ok_or("live competition missing situation")?;
                Ok(Self::Live {
                    competitors: two_competitors(dto.competitors)?,
                    period: dto.status.period,
                    display_clock: dto.status.display_clock,
                    phase: parse_live_phase(dto.status.r#type.description.as_deref(), "nba"),
                    situation,
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

/// 100%-present in live payloads, but observed as an empty `{}` before the
/// opening tip — hence the defaulted optional last play.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnSituation {
    #[serde(default)]
    pub(crate) last_play: Option<EspnLastPlay>,
}

// ---------- Outbound domain model ----------

/// One NBA game, discriminated on the cross-sport `pre/in/post` state. The
/// firmware parses the `state` byte (0/1/2) first, then the matching payload
/// (see `wire.rs`).
#[derive(Serialize, ToSchema)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum NbaGame {
    Pregame(NbaPregameGame),
    Live(NbaLiveGame),
    Final(NbaFinalGame),
}

/// Pre-game snapshot: matchup, scheduled start, venue, and (when available)
/// season records.
#[derive(Serialize, ToSchema)]
pub struct NbaPregameGame {
    pub game_id: String,
    /// Scheduled start as a unix epoch (seconds, UTC). The firmware applies
    /// the device's `utc_offset` for local display — it never parses dates.
    pub start_time: u32,
    pub venue: String,
    pub home: NbaPregameTeam,
    pub away: NbaPregameTeam,
}

/// Pre-game team: identity and pre-game context, but no score (there is no
/// game yet — a fake 0 would invite the firmware to render one).
#[derive(Serialize, ToSchema)]
pub struct NbaPregameTeam {
    /// Team abbreviation, e.g. "LAL" — firmware uses this to fetch the logo.
    pub abbreviation: String,
    pub colors: TeamColors,
    /// Overall season record; absent when ESPN omits or malforms it.
    pub record: Option<Record>,
}

/// Live state snapshot for one NBA game, tailored for the Pico firmware.
#[derive(Serialize, ToSchema)]
pub struct NbaLiveGame {
    pub game_id: String,
    /// Quarter 1–4; overtime periods pass through as 5+.
    pub period: u8,
    /// Raw ESPN clock, display-shaped: "10:08", "53.0" under a minute; "0.0"
    /// or a reset "12:00" during breaks (see `phase`). NBA's clock stops
    /// unpredictably and ESPN sends no clock-running signal, so unlike
    /// soccer there is no `clock_seconds` — the string is exact at poll time
    /// and must not be extrapolated.
    pub clock: String,
    pub phase: LivePhase,
    pub home: TeamState,
    pub away: TeamState,
    /// Absent before the opening tip.
    pub last_play: Option<LastPlay>,
}

/// Final snapshot: score, per-quarter line score, and quarters played (4, or
/// more for overtime). No explicit winner — it is derivable from the scores,
/// and a second copy could disagree with them on a glitch.
#[derive(Serialize, ToSchema)]
pub struct NbaFinalGame {
    pub game_id: String,
    pub periods_played: u8,
    pub home: NbaFinalTeam,
    pub away: NbaFinalTeam,
}

#[derive(Serialize, ToSchema)]
pub struct NbaFinalTeam {
    pub abbreviation: String,
    pub score: u32,
    pub colors: TeamColors,
    /// Points per quarter, quarter 1 first; overtime periods extend past 4.
    pub line_score: Vec<u8>,
}
