use axum::{
    Json,
    extract::{Path, State},
};
use std::sync::Arc;

use crate::auth::ApiKey;
use crate::error::AppError;
use crate::AppState;

use super::transform;
use super::types::GameResponse;

/// GET /api/game/{event_id}
/// Fetches game data from ESPN and returns a minimal payload for the Pi Pico
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
