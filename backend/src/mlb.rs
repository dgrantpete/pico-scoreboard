use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AppState;
use crate::error::{AppError, ErrorResponse};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EspnScoreboard {
    events: Vec<EspnEvent>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EspnEvent {
    id: String,
    competitions: Vec<EspnCompetition>,
}

#[derive(Deserialize)]
#[serde(try_from = "EspnCompetitionDto")]
#[allow(clippy::large_enum_variant)] // Pre/Final are transient markers; boxing Live would cost more than it saves.
enum EspnCompetition {
    PreGame,
    Live {
        competitors: [EspnCompetitor; 2],
        situation: EspnSituation,
    },
    Final,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EspnCompetitionDto {
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
                })
            }
            CompetitionState::Post => Ok(Self::Final),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EspnStatus {
    r#type: EspnStatusType,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EspnStatusType {
    state: CompetitionState,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum CompetitionState {
    Pre,
    In,
    Post,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EspnCompetitor {
    home_away: HomeAway,
    score: String,
    team: EspnTeam,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum HomeAway {
    Home,
    Away,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EspnTeam {
    abbreviation: String,
    color: String,
    alternate_color: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EspnSituation {
    balls: u8,
    strikes: u8,
    outs: u8,
    on_first: bool,
    on_second: bool,
    on_third: bool,
    pitcher: Option<EspnPlayer>,
    batter: Option<EspnPlayer>,
}

#[derive(Deserialize)]
struct EspnPlayer {
    athlete: EspnAthlete,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EspnAthlete {
    short_name: String,
}

// ---------- Outbound domain model ----------

/// Live state snapshot for one MLB game, tailored for the Pico firmware.
#[derive(Serialize, ToSchema)]
pub struct LiveGame {
    pub game_id: String,
    pub home: TeamState,
    pub away: TeamState,
    pub count: Count,
    pub bases: Bases,
    /// Absent between innings or before an at-bat starts.
    pub at_bat: Option<AtBat>,
}

#[derive(Serialize, ToSchema)]
pub struct TeamState {
    /// Team abbreviation, e.g. "BOS" — firmware uses this to fetch the logo.
    pub abbreviation: String,
    pub score: u32,
    pub colors: TeamColors,
}

#[derive(Serialize, ToSchema)]
pub struct TeamColors {
    /// RGB888 packed as 0x00RRGGBB for cheap parsing on the Pico.
    pub primary: u32,
    pub alternate: u32,
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

// ---------- Helpers ----------

/// Parse an ESPN team color hex string (with optional leading '#') into a
/// packed RGB888 `u32` (`0x00RRGGBB`). The team abbreviation is used purely
/// to give the returned error context for logs and the client response.
fn parse_hex_rgb(raw: &str, team: &str) -> Result<u32, AppError> {
    let hex = raw.strip_prefix('#').unwrap_or(raw);
    if hex.len() != 6 {
        return Err(AppError::InvalidTeamColor {
            team: team.to_string(),
            raw: raw.to_string(),
        });
    }
    u32::from_str_radix(hex, 16).map_err(|_| AppError::InvalidTeamColor {
        team: team.to_string(),
        raw: raw.to_string(),
    })
}

/// Build a `TeamState` from an ESPN competitor, parsing the score and colors.
fn competitor_to_team_state(c: &EspnCompetitor) -> Result<TeamState, AppError> {
    let score = c.score.parse::<u32>().map_err(|e| {
        let json_path = format!(
            "events[?].competitions[0].competitors[{}].score",
            c.team.abbreviation
        );
        tracing::error!(
            json_path = %json_path,
            team = %c.team.abbreviation,
            raw_score = %c.score,
            error = %e,
            "ESPN competitor score failed to parse as u32"
        );
        AppError::EspnDeserialize {
            url: String::new(),
            json_path,
            message: format!("invalid score '{}': {}", c.score, e),
        }
    })?;

    let primary = parse_hex_rgb(&c.team.color, &c.team.abbreviation).inspect_err(|e| {
        tracing::error!(
            team = %c.team.abbreviation,
            raw_color = %c.team.color,
            error = ?e,
            "ESPN primary team color failed to parse"
        );
    })?;
    let alternate = parse_hex_rgb(&c.team.alternate_color, &c.team.abbreviation).inspect_err(
        |e| {
            tracing::error!(
                team = %c.team.abbreviation,
                raw_color = %c.team.alternate_color,
                error = ?e,
                "ESPN alternate team color failed to parse"
            );
        },
    )?;

    Ok(TeamState {
        abbreviation: c.team.abbreviation.clone(),
        score,
        colors: TeamColors { primary, alternate },
    })
}

/// Transform a live competition into a `LiveGame`. Callers must pattern-match
/// `EspnCompetition::Live` at the call site, so no runtime state check lives
/// inside this function.
fn live_competition_to_game(
    event_id: String,
    competitors: [EspnCompetitor; 2],
    situation: EspnSituation,
) -> Result<LiveGame, AppError> {
    let [a, b] = competitors;
    let (home, away) = match (a.home_away, b.home_away) {
        (HomeAway::Home, HomeAway::Away) => (competitor_to_team_state(&a)?, competitor_to_team_state(&b)?),
        (HomeAway::Away, HomeAway::Home) => (competitor_to_team_state(&b)?, competitor_to_team_state(&a)?),
        _ => {
            let json_path = format!(
                "events[?].competitions[0].competitors (event_id={})",
                event_id
            );
            tracing::error!(
                json_path = %json_path,
                event_id = %event_id,
                first_team = %a.team.abbreviation,
                second_team = %b.team.abbreviation,
                "ESPN competitors did not split into exactly one home and one away"
            );
            return Err(AppError::EspnDeserialize {
                url: String::new(),
                json_path,
                message: format!(
                    "expected one home and one away competitor, got {}/{}",
                    a.team.abbreviation, b.team.abbreviation
                ),
            });
        }
    };

    let count = Count {
        balls: situation.balls,
        strikes: situation.strikes,
        outs: situation.outs,
    };
    let bases = Bases {
        first: situation.on_first,
        second: situation.on_second,
        third: situation.on_third,
    };
    let at_bat = match (situation.pitcher, situation.batter) {
        (Some(pitcher), Some(batter)) => Some(AtBat {
            pitcher: pitcher.athlete.short_name,
            batter: batter.athlete.short_name,
        }),
        _ => None,
    };

    Ok(LiveGame {
        game_id: event_id,
        home,
        away,
        count,
        bases,
        at_bat,
    })
}

// ---------- Handlers ----------

fn scoreboard_url(state: &AppState) -> String {
    format!("{}/baseball/mlb/scoreboard", state.config.espn.base_url)
}

/// GET /mlb/games — list ESPN event IDs whose first competition is currently live.
#[utoipa::path(
    get,
    path = "/mlb/games",
    responses(
        (status = 200, description = "IDs of currently live MLB games", body = Vec<String>),
        (status = 502, description = "ESPN upstream error", body = ErrorResponse),
    ),
    tag = "mlb"
)]
pub async fn list_active_games(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<String>>, AppError> {
    let url = scoreboard_url(&state);
    let scoreboard: EspnScoreboard = state.espn_client.fetch_json(&url).await?;

    let ids = scoreboard
        .events
        .into_iter()
        .filter_map(|event| {
            let first = event.competitions.into_iter().next()?;
            matches!(first, EspnCompetition::Live { .. }).then_some(event.id)
        })
        .collect();

    Ok(Json(ids))
}

/// GET /mlb/games/{game_id} — live state snapshot for one MLB game.
#[utoipa::path(
    get,
    path = "/mlb/games/{game_id}",
    params(("game_id" = String, Path, description = "ESPN event ID")),
    responses(
        (status = 200, description = "Live game state", body = LiveGame),
        (status = 404, description = "Game not found or not live", body = ErrorResponse),
        (status = 502, description = "ESPN upstream error", body = ErrorResponse),
    ),
    tag = "mlb"
)]
pub async fn get_live_game(
    State(state): State<Arc<AppState>>,
    Path(game_id): Path<String>,
) -> Result<Json<LiveGame>, AppError> {
    let url = scoreboard_url(&state);
    let scoreboard: EspnScoreboard = state.espn_client.fetch_json(&url).await?;

    let event = scoreboard
        .events
        .into_iter()
        .find(|e| e.id == game_id)
        .ok_or_else(|| AppError::GameNotFound(game_id.clone()))?;

    let first = event
        .competitions
        .into_iter()
        .next()
        .ok_or_else(|| AppError::GameNotFound(game_id.clone()))?;

    match first {
        EspnCompetition::Live {
            competitors,
            situation,
        } => {
            let game = live_competition_to_game(event.id, competitors, situation)?;
            Ok(Json(game))
        }
        EspnCompetition::PreGame | EspnCompetition::Final => {
            Err(AppError::GameNotFound(game_id))
        }
    }
}
