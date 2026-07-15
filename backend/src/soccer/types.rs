use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::espn::types::{CompetitionState, EspnAthlete, EspnTeam, EspnVenue, HomeAway};
use crate::shared::competitor::Competitor;
use crate::shared::team::{TeamColors, TeamState};

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
        venue_name: String,
    },
    Live {
        competitors: [EspnCompetitor; 2],
        /// Raw ESPN clock, already display-shaped (e.g. "45'+6'", "90'+3'").
        display_clock: String,
        /// Elapsed match seconds parsed from `display_clock` (see
        /// [`parse_display_clock`]) — the numeric `status.clock` caps at
        /// regulation during stoppage, so the string is the only source.
        clock_seconds: u16,
        /// ESPN's raw competition period: regulation halves 1/2, extra-time
        /// halves 3/4, shootout 5 (see the range warn in the DU).
        period: u8,
        on_break: bool,
        details: Vec<EspnDetail>,
    },
    Final {
        competitors: [EspnCompetitor; 2],
        details: Vec<EspnDetail>,
        flavor: SoccerFinalFlavor,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnCompetitionDto {
    competitors: Vec<EspnCompetitor>,
    status: EspnStatus,
    /// Present in every sampled state, but only the pregame arm requires it.
    venue: Option<EspnVenue>,
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
                let display_clock = dto
                    .status
                    .display_clock
                    .ok_or("live competition missing displayClock")?;
                let clock_seconds = parse_display_clock(&display_clock, dto.status.clock);
                let period = dto
                    .status
                    .period
                    .ok_or("live competition missing period")?;
                if !(1..=5).contains(&period) {
                    tracing::warn!(
                        period,
                        "soccer live period outside the known 1..=5 set (regulation/extra-time/shootout) — passing through"
                    );
                }
                Ok(Self::Live {
                    competitors: two_competitors(dto.competitors)?,
                    display_clock,
                    clock_seconds,
                    period,
                    on_break: is_break(dto.status.r#type.description.as_deref()),
                    details: dto.details,
                })
            }
            CompetitionState::Post => Ok(Self::Final {
                competitors: two_competitors(dto.competitors)?,
                details: dto.details,
                flavor: final_flavor(dto.status.r#type.description.as_deref()),
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

/// Whether a live match is in a non-playing break — the clock is paused and
/// meaningless. Clock and period alone cannot distinguish a break from active
/// stoppage time (both read e.g. "45'+N'"), so the description is the only
/// upstream signal.
///
/// Break descriptions (the full observed set): Halftime, Extra Time Halftime,
/// End of Regulation (between second half and extra time), End of Extra Time
/// (before penalties). Active-play descriptions: First Half, Second Half, In
/// Progress, Overtime (extra time), Shootout. An unknown description degrades
/// to active play with a warning — the state is never guessed.
pub(crate) fn is_break(description: Option<&str>) -> bool {
    match description {
        Some("Halftime" | "Extra Time Halftime" | "End of Regulation" | "End of Extra Time") => {
            true
        }
        Some("First Half" | "Second Half" | "In Progress" | "Overtime" | "Shootout") | None => {
            false
        }
        Some(other) => {
            tracing::warn!(
                description = %other,
                "unknown live soccer status description — treating as active play"
            );
            false
        }
    }
}

/// Map a post-state `status.type.description` to how the match was decided, for
/// the wire `flavor` byte. Observed post descriptions: "Full Time", "Final
/// Score - After Extra Time", "Final Score - After Penalties". An unknown
/// description degrades to full time with a warning.
pub(crate) fn final_flavor(description: Option<&str>) -> SoccerFinalFlavor {
    match description {
        Some("Final Score - After Extra Time") => SoccerFinalFlavor::AfterExtraTime,
        Some("Final Score - After Penalties") => SoccerFinalFlavor::AfterPenalties,
        Some("Full Time") | None => SoccerFinalFlavor::FullTime,
        Some(other) => {
            tracing::warn!(
                description = %other,
                "unknown post-state soccer description — defaulting to full-time flavor"
            );
            SoccerFinalFlavor::FullTime
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
    pub(crate) athletes_involved: Vec<EspnAthlete>,
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
    Pregame(SoccerPregameGame),
    Live(SoccerLiveGame),
    Final(SoccerFinalGame),
}

/// Pre-game snapshot: matchup, scheduled start, and venue.
#[derive(Serialize, ToSchema)]
pub struct SoccerPregameGame {
    pub game_id: String,
    /// Scheduled start, unix epoch seconds UTC (what the wire carries).
    pub start_time: u32,
    /// Stadium name (ESPN `venue.fullName`); 100%-present in the corpus.
    pub venue: String,
    pub home: SoccerTeam,
    pub away: SoccerTeam,
}

/// Live state snapshot for one soccer game, tailored for the Pico firmware.
#[derive(Serialize, ToSchema)]
pub struct SoccerLiveGame {
    pub game_id: String,
    /// Raw ESPN clock, display-shaped (e.g. "45'+6'", "90'+3'").
    pub clock: String,
    /// Elapsed match seconds parsed from `clock` (floor minutes × 60);
    /// what the wire carries — the firmware extrapolates from it.
    pub clock_seconds: u16,
    /// ESPN's raw competition period: regulation halves 1/2, extra-time halves
    /// 3/4, shootout 5.
    pub half: u8,
    /// True during a non-playing break (halftime, extra-time halftime, end of
    /// regulation, end of extra time) — the clock alone cannot distinguish a
    /// break from active stoppage time.
    pub on_break: bool,
    pub home: TeamState,
    pub away: TeamState,
    pub last_event: Option<LastEvent>,
    /// Latest play-by-play commentary line (from the summary endpoint),
    /// e.g. "Goal! Argentina 3, Egypt 2. Lionel Messi converts...".
    /// Absent when the summary has no commentary or its fetch failed —
    /// commentary is best-effort and never blocks the live payload.
    pub commentary: Option<Commentary>,
}

/// Final snapshot: per-side scores, pre-formatted scorer lists, and how the
/// match was decided.
#[derive(Serialize, ToSchema)]
pub struct SoccerFinalGame {
    pub game_id: String,
    /// Full time, after extra time, or on penalties — the wire `flavor` byte.
    pub flavor: SoccerFinalFlavor,
    pub home: SoccerFinalTeam,
    pub away: SoccerFinalTeam,
}

/// How a finished match was decided. Carried as the final wire `flavor` byte
/// (0/1/2); the firmware renders the "AET"/"pens" annotation from it.
#[derive(Serialize, ToSchema, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum SoccerFinalFlavor {
    FullTime,
    AfterExtraTime,
    AfterPenalties,
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
