use axum::{
    Json,
    extract::{Path, State},
};
use std::sync::Arc;

use crate::auth::ApiKey;
use crate::error::{AppError, ErrorResponse};
use crate::sport::BasketballLeague;
use crate::AppState;

use super::transform;
use super::types::{BasketballGameDetail, BasketballGameResponse};

/// GET /api/{league}/games
/// Fetches all basketball games from ESPN scoreboard
#[utoipa::path(
    get,
    path = "/api/basketball/{league}/games",
    operation_id = "get_all_basketball_games",
    params(
        ("league" = String, Path, description = "Basketball league: nba or ncaab")
    ),
    responses(
        (status = 200, description = "Basketball games retrieved successfully", body = Vec<BasketballGameResponse>),
        (status = 400, description = "Invalid league", body = ErrorResponse),
        (status = 401, description = "Missing or invalid API key", body = ErrorResponse),
        (status = 502, description = "Error fetching from ESPN API", body = ErrorResponse),
    ),
    security(
        ("api_key" = [])
    ),
    tag = "basketball"
)]
pub async fn get_all_games(
    _api_key: ApiKey,
    State(state): State<Arc<AppState>>,
    Path(league): Path<String>,
) -> Result<Json<Vec<BasketballGameResponse>>, AppError> {
    let basketball_league = BasketballLeague::from_league(&league)?;
    let events = state.espn_client.fetch_all_games(basketball_league).await?;

    let responses: Vec<BasketballGameResponse> = events
        .iter()
        .map(|e| transform::transform_from_scoreboard(e, basketball_league))
        .collect();

    Ok(Json(responses))
}

/// GET /api/{league}/games/{event_id}
/// Fetches a single basketball game detail from ESPN summary endpoint
#[utoipa::path(
    get,
    path = "/api/basketball/{league}/games/{event_id}",
    operation_id = "get_basketball_game",
    params(
        ("league" = String, Path, description = "Basketball league: nba or ncaab"),
        ("event_id" = String, Path, description = "ESPN event ID (numeric)")
    ),
    responses(
        (status = 200, description = "Basketball game detail retrieved successfully", body = BasketballGameDetail),
        (status = 400, description = "Invalid league or event ID", body = ErrorResponse),
        (status = 401, description = "Missing or invalid API key", body = ErrorResponse),
        (status = 502, description = "Error fetching from ESPN API", body = ErrorResponse),
    ),
    security(
        ("api_key" = [])
    ),
    tag = "basketball"
)]
pub async fn get_game(
    _api_key: ApiKey,
    State(state): State<Arc<AppState>>,
    Path((league, event_id)): Path<(String, String)>,
) -> Result<Json<BasketballGameDetail>, AppError> {
    let basketball_league = BasketballLeague::from_league(&league)?;

    // Validate event_id is numeric only
    if !event_id.chars().all(|c| c.is_ascii_digit()) {
        return Err(AppError::InvalidEventId(event_id));
    }

    let summary = state
        .espn_client
        .fetch_game_summary(basketball_league, &event_id)
        .await?;

    let response = transform::transform_from_summary(&summary, basketball_league);

    Ok(Json(response))
}
