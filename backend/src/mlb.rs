use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
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
        period: u8,
        short_detail: String,
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
struct EspnStatus {
    r#type: EspnStatusType,
    period: u8,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EspnStatusType {
    state: CompetitionState,
    short_detail: String,
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
    last_play: EspnLastPlay,
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

#[derive(Deserialize)]
struct EspnLastPlay {
    id: String,
    text: String,
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

fn parse_inning_half(short_detail: &str) -> Result<InningHalf, AppError> {
    match short_detail.split_whitespace().next().unwrap_or("") {
        "Top" => Ok(InningHalf::Top),
        "Mid" => Ok(InningHalf::Middle),
        "Bot" => Ok(InningHalf::Bottom),
        "End" => Ok(InningHalf::End),
        other => {
            tracing::error!(
                short_detail = %short_detail,
                prefix = %other,
                "ESPN shortDetail has unexpected inning-half prefix"
            );
            Err(AppError::EspnDeserialize {
                url: String::new(),
                json_path: "events[?].competitions[0].status.type.shortDetail".to_string(),
                message: format!("unexpected inning-half prefix '{}'", other),
            })
        }
    }
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
    period: u8,
    short_detail: String,
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
    let last_play = LastPlay {
        id: situation.last_play.id,
        text: situation.last_play.text,
    };

    let inning = Inning {
        number: period,
        half: parse_inning_half(&short_detail)?,
    };

    Ok(LiveGame {
        game_id: event_id,
        inning,
        home,
        away,
        count,
        bases,
        at_bat,
        last_play,
    })
}

// ---------- Handlers ----------

fn scoreboard_url(state: &AppState) -> String {
    format!("{}/baseball/mlb/scoreboard", state.config.espn.base_url)
}

/// First 16 hex chars of SHA-1 over the sorted, comma-joined game IDs.
/// Mirrors the firmware's `_compute_index_etag` pattern so both sides agree
/// on truncation length.
fn compute_games_etag(game_ids: &[String]) -> String {
    let mut sorted: Vec<&str> = game_ids.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    let joined = sorted.join(",");
    let digest = Sha1::digest(joined.as_bytes());
    hex::encode(&digest[..8])
}

/// Build a 200-or-304 response for `GET /mlb/games` given the live game IDs
/// and the client's `If-None-Match` header.
fn build_games_response(game_ids: Vec<String>, if_none_match: Option<&str>) -> Response {
    let etag = compute_games_etag(&game_ids);
    let quoted = format!("\"{}\"", etag);

    if if_none_match == Some(quoted.as_str()) {
        return (
            StatusCode::NOT_MODIFIED,
            [(header::ETAG, quoted.as_str())],
        )
            .into_response();
    }

    ([(header::ETAG, quoted.as_str())], Json(game_ids)).into_response()
}

/// GET /mlb/games — list ESPN event IDs whose first competition is currently live.
#[utoipa::path(
    get,
    path = "/mlb/games",
    responses(
        (status = 200, description = "IDs of currently live MLB games", body = Vec<String>),
        (status = 304, description = "Game set unchanged since client's If-None-Match"),
        (status = 502, description = "ESPN upstream error", body = ErrorResponse),
    ),
    tag = "mlb"
)]
pub async fn list_active_games(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let url = scoreboard_url(&state);
    let scoreboard: EspnScoreboard = state.espn_client.fetch_json(&url).await?;

    let ids: Vec<String> = scoreboard
        .events
        .into_iter()
        .filter_map(|event| {
            let first = event.competitions.into_iter().next()?;
            matches!(first, EspnCompetition::Live { .. }).then_some(event.id)
        })
        .collect();

    let if_none_match = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok());

    Ok(build_games_response(ids, if_none_match))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    fn ids(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    fn etag_header(resp: &Response) -> &str {
        resp.headers()
            .get(header::ETAG)
            .expect("etag header present")
            .to_str()
            .unwrap()
    }

    #[tokio::test]
    async fn returns_200_with_etag_when_no_if_none_match() {
        let resp = build_games_response(ids(&["401570001", "401570002"]), None);
        assert_eq!(resp.status(), StatusCode::OK);
        let tag = etag_header(&resp).to_string();
        assert!(tag.starts_with('"') && tag.ends_with('"'));
        assert_eq!(tag.len(), 18);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        assert!(!body.is_empty());
    }

    #[tokio::test]
    async fn returns_304_without_body_when_if_none_match_matches() {
        let game_ids = ids(&["401570002", "401570001"]);
        let tag = format!("\"{}\"", compute_games_etag(&game_ids));
        let resp = build_games_response(game_ids, Some(tag.as_str()));
        assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(etag_header(&resp), tag);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn returns_200_with_new_etag_after_game_set_changes() {
        let initial = ids(&["401570001", "401570002"]);
        let initial_tag = format!("\"{}\"", compute_games_etag(&initial));

        let changed = ids(&["401570001", "401570003"]);
        let resp = build_games_response(changed, Some(initial_tag.as_str()));

        assert_eq!(resp.status(), StatusCode::OK);
        let new_tag = etag_header(&resp).to_string();
        assert_ne!(new_tag, initial_tag);
    }
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
            period,
            short_detail,
        } => {
            let game = live_competition_to_game(event.id, competitors, situation, period, short_detail)?;
            Ok(Json(game))
        }
        EspnCompetition::PreGame | EspnCompetition::Final => {
            Err(AppError::GameNotFound(game_id))
        }
    }
}
