use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::espn::types::{CompetitionState, EspnTeam, HomeAway};
use crate::shared::team::TeamColors;

// ---------- ESPN inbound types ----------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnEvent {
    pub(crate) id: String,
    /// Scheduled start, ISO 8601.
    pub(crate) date: String,
    pub(crate) competitions: Vec<EspnCompetition>,
}

#[derive(Deserialize)]
#[serde(try_from = "EspnCompetitionDto")]
pub(crate) enum EspnCompetition {
    PreGame {
        competitors: [EspnCompetitor; 2],
    },
    Live {
        competitors: [EspnCompetitor; 2],
        /// Raw ESPN clock, already display-shaped (e.g. "45'+6'", "90'+3'").
        display_clock: String,
        /// Elapsed match seconds parsed from `display_clock` (see
        /// [`parse_display_clock`]) — the numeric `status.clock` caps at
        /// regulation during stoppage, so the string is the only source.
        clock_seconds: u16,
        period: u8,
        halftime: bool,
        details: Vec<EspnDetail>,
    },
    Final {
        competitors: [EspnCompetitor; 2],
        details: Vec<EspnDetail>,
    },
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
            CompetitionState::In => {
                let display_clock = dto
                    .status
                    .display_clock
                    .ok_or("live competition missing displayClock")?;
                let clock_seconds = parse_display_clock(&display_clock, dto.status.clock);
                Ok(Self::Live {
                    competitors: two_competitors(dto.competitors)?,
                    display_clock,
                    clock_seconds,
                    period: dto
                        .status
                        .period
                        .ok_or("live competition missing period")?,
                    halftime: is_halftime(dto.status.r#type.description.as_deref()),
                    details: dto.details,
                })
            }
            CompetitionState::Post => Ok(Self::Final {
                competitors: two_competitors(dto.competitors)?,
                details: dto.details,
            }),
        }
    }
}

/// Elapsed match seconds from ESPN's display-shaped clock.
///
/// ESPN's numeric `status.clock` caps at regulation (2700.0 while the display
/// reads "45'+6'"), so stoppage time only exists in the string. The display
/// uses floor minutes ("23'" = 23 full minutes elapsed; "45'+6'" = 45 + 6),
/// and the firmware renders the same convention, so encoding `minutes * 60`
/// reproduces ESPN's display exactly at poll time and extrapolates forward
/// from there. An unparseable string degrades to the numeric clock (capped,
/// but sane) with a warning.
pub(crate) fn parse_display_clock(display_clock: &str, numeric_fallback: Option<f64>) -> u16 {
    fn minutes(text: &str) -> Option<u32> {
        text.split('+')
            .map(|part| part.trim().trim_end_matches('\'').parse::<u32>().ok())
            .sum::<Option<u32>>()
    }
    match minutes(display_clock) {
        Some(m) => (m * 60).min(u16::MAX as u32) as u16,
        None => {
            tracing::warn!(
                display_clock,
                "unparseable soccer displayClock; falling back to numeric status.clock"
            );
            numeric_fallback.unwrap_or(0.0).clamp(0.0, u16::MAX as f64) as u16
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
    /// Numeric seconds — caps at regulation during stoppage; fallback only.
    pub(crate) clock: Option<f64>,
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

/// The summary endpoint (`/{sport}/{league}/summary?event=`), reduced to the
/// one array the scoreboard uses. Absent for games ESPN doesn't cover with
/// live commentary.
#[derive(Deserialize)]
pub(crate) struct RawSummary {
    #[serde(default)]
    pub(crate) commentary: Vec<EspnCommentaryItem>,
}

#[derive(Deserialize)]
pub(crate) struct EspnCommentaryItem {
    /// Monotonic per-match ordering — the change-detection id for the
    /// firmware's flash (analogous to MLB's play id).
    pub(crate) sequence: u32,
    pub(crate) text: String,
}

// ---------- Outbound domain model ----------

/// One soccer game, discriminated on the cross-sport `pre/in/post` state.
/// All three states are served (like MLB); the firmware renders pregame via
/// its shared pregame pipeline and final via the soccer full-time screen.
#[derive(Serialize, ToSchema)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum SoccerGame {
    Pregame {
        game_id: String,
        /// Scheduled start, ISO 8601 (ESPN `event.date`).
        date: String,
        /// Scheduled start, unix epoch seconds UTC (what the wire carries).
        start_time: u32,
        home: SoccerTeam,
        away: SoccerTeam,
    },
    Live {
        game_id: String,
        /// Raw ESPN clock, display-shaped (e.g. "45'+6'", "90'+3'").
        clock: String,
        /// Elapsed match seconds parsed from `clock` (floor minutes × 60);
        /// what the wire carries — the firmware extrapolates from it.
        clock_seconds: u16,
        /// Regulation halves are 1 and 2; extra-time periods pass through as-is.
        half: u8,
        /// True during the interval — the clock alone cannot distinguish
        /// halftime from first-half stoppage time.
        halftime: bool,
        home: SoccerTeamState,
        away: SoccerTeamState,
        last_event: Option<LastEvent>,
        /// Latest play-by-play commentary line (from the summary endpoint),
        /// e.g. "Goal! Argentina 3, Egypt 2. Lionel Messi converts...".
        /// Absent when the summary has no commentary or its fetch failed —
        /// commentary is best-effort and never blocks the live payload.
        commentary: Option<Commentary>,
    },
    Final {
        game_id: String,
        home: SoccerFinalTeam,
        away: SoccerFinalTeam,
    },
}

/// One commentary line; `id` is the ESPN sequence number as a string — the
/// firmware compares it to detect new lines (same contract as MLB's play id).
#[derive(Serialize, ToSchema)]
pub struct Commentary {
    pub id: String,
    pub text: String,
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

#[derive(Serialize, ToSchema)]
pub struct SoccerFinalTeam {
    pub abbreviation: String,
    pub score: u32,
    pub colors: TeamColors,
    /// Pre-formatted goal-scorer list ("M. Merino 90'+1', F. Torres 12'"),
    /// empty when the team didn't score. Built once here so the firmware
    /// never formats strings.
    pub scorers: String,
}

/// The most recent goal or red card.
#[derive(Serialize, ToSchema)]
pub struct LastEvent {
    /// e.g. "Goal - R. Lukaku" or "Red Card - J. Doe".
    pub text: String,
    /// Structured kind — what the wire carries (with `athlete`).
    pub kind: EventKind,
    /// Athlete short name ("R. Lukaku"), empty if ESPN lists no athlete.
    pub athlete: String,
    /// Match clock of the event, display-shaped (e.g. "90'+3'").
    pub clock: String,
    /// Which side the event belongs to; absent if ESPN omits the team.
    pub team: Option<Side>,
}

#[derive(Serialize, ToSchema, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Goal,
    RedCard,
}

#[derive(Serialize, ToSchema, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Home,
    Away,
}
