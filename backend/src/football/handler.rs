use axum::{
    Json,
    extract::{Path, State},
};
use std::sync::Arc;

use crate::auth::ApiKey;
use crate::error::{AppError, ErrorResponse};
use crate::sport::FootballLeague;
use crate::AppState;

use super::transform;
use super::types::FootballGameResponse;

/// GET /api/{league}/games/{event_id}
/// Fetches game data from ESPN and returns a minimal payload for the Pi Pico
#[utoipa::path(
    get,
    path = "/api/football/{league}/games/{event_id}",
    operation_id = "get_football_game",
    params(
        ("league" = String, Path, description = "League identifier (nfl, ncaaf)"),
        ("event_id" = String, Path, description = "ESPN event ID (numeric)"),
    ),
    responses(
        (status = 200, description = "Game data retrieved successfully", body = FootballGameResponse),
        (status = 400, description = "Invalid league or event ID format", body = ErrorResponse),
        (status = 401, description = "Missing or invalid API key", body = ErrorResponse),
        (status = 404, description = "Game not found on current scoreboard", body = ErrorResponse),
        (status = 502, description = "Error fetching from ESPN API", body = ErrorResponse),
    ),
    security(
        ("api_key" = [])
    ),
    tag = "football"
)]
pub async fn get_game(
    _api_key: ApiKey,
    State(state): State<Arc<AppState>>,
    Path((league, event_id)): Path<(String, String)>,
) -> Result<Json<FootballGameResponse>, AppError> {
    let football_league = FootballLeague::from_league(&league)?;

    // Validate event_id is numeric only
    if !event_id.chars().all(|c| c.is_ascii_digit()) {
        return Err(AppError::InvalidEventId(event_id));
    }

    // Fetch game from ESPN
    let event = state.espn_client.fetch_game(football_league, &event_id).await?;

    // Transform to our response format
    let response = transform::transform(&event, football_league);

    Ok(Json(response))
}

/// GET /api/{league}/games
/// Fetches all games from ESPN and returns minimal payloads for the Pi Pico
#[utoipa::path(
    get,
    path = "/api/football/{league}/games",
    operation_id = "get_all_football_games",
    params(
        ("league" = String, Path, description = "League identifier (nfl, ncaaf)"),
    ),
    responses(
        (status = 200, description = "All games retrieved successfully", body = Vec<FootballGameResponse>),
        (status = 400, description = "Invalid league", body = ErrorResponse),
        (status = 401, description = "Missing or invalid API key", body = ErrorResponse),
        (status = 502, description = "Error fetching from ESPN API", body = ErrorResponse),
    ),
    security(
        ("api_key" = [])
    ),
    tag = "football"
)]
pub async fn get_all_games(
    _api_key: ApiKey,
    State(state): State<Arc<AppState>>,
    Path(league): Path<String>,
) -> Result<Json<Vec<FootballGameResponse>>, AppError> {
    let football_league = FootballLeague::from_league(&league)?;

    // Fetch all games from ESPN
    let events = state.espn_client.fetch_all_games(football_league).await?;

    // Transform each event to our response format
    let responses: Vec<FootballGameResponse> = events
        .iter()
        .map(|e| transform::transform(e, football_league))
        .collect();

    Ok(Json(responses))
}
