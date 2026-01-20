use axum::{
    Json,
    extract::{Path, State},
};
use std::sync::Arc;

use crate::auth::ApiKey;
use crate::error::{AppError, ErrorResponse};
use crate::AppState;

use super::transform;
use super::types::GameResponse;

/// GET /api/games/{event_id}
/// Fetches game data from ESPN and returns a minimal payload for the Pi Pico
#[utoipa::path(
    get,
    path = "/api/games/{event_id}",
    params(
        ("event_id" = String, Path, description = "ESPN event ID (numeric)")
    ),
    responses(
        (status = 200, description = "Game data retrieved successfully", body = GameResponse),
        (status = 400, description = "Invalid event ID format", body = ErrorResponse),
        (status = 401, description = "Missing or invalid API key", body = ErrorResponse),
        (status = 404, description = "Game not found on current scoreboard", body = ErrorResponse),
        (status = 502, description = "Error fetching from ESPN API", body = ErrorResponse),
    ),
    security(
        ("api_key" = [])
    ),
    tag = "games"
)]
pub async fn get_game(
    _api_key: ApiKey,
    State(state): State<Arc<AppState>>,
    Path(event_id): Path<String>,
) -> Result<Json<GameResponse>, AppError> {
    // Validate event_id is numeric only
    if !event_id.chars().all(|c| c.is_ascii_digit()) {
        return Err(AppError::InvalidEventId(event_id));
    }

    // Fetch game from ESPN
    let event = state.espn_client.fetch_game(&event_id).await?;

    // Transform to our response format
    let response = transform::transform(&event);

    Ok(Json(response))
}

/// GET /api/games
/// Fetches all games from ESPN and returns minimal payloads for the Pi Pico
#[utoipa::path(
    get,
    path = "/api/games",
    responses(
        (status = 200, description = "All games retrieved successfully", body = Vec<GameResponse>),
        (status = 401, description = "Missing or invalid API key", body = ErrorResponse),
        (status = 502, description = "Error fetching from ESPN API", body = ErrorResponse),
    ),
    security(
        ("api_key" = [])
    ),
    tag = "games"
)]
pub async fn get_all_games(
    _api_key: ApiKey,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<GameResponse>>, AppError> {
    // Fetch all games from ESPN
    let events = state.espn_client.fetch_all_games().await?;

    // Transform each event to our response format
    let responses: Vec<GameResponse> = events.iter().map(transform::transform).collect();

    Ok(Json(responses))
}
