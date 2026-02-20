use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use std::sync::Arc;

use crate::auth::ApiKey;
use crate::error::{AppError, ErrorResponse};
use crate::football::types::FootballGameResponse;
use crate::AppState;

use super::simulation::CreateGameRequest;

/// GET /api/mock/games
/// List all mock games in the repository
#[utoipa::path(
    get,
    path = "/api/mock/games",
    responses(
        (status = 200, description = "List of all mock games", body = Vec<FootballGameResponse>),
        (status = 401, description = "Missing or invalid API key", body = ErrorResponse),
    ),
    security(
        ("api_key" = [])
    ),
    tag = "mock"
)]
pub async fn list_mock_games(
    _api_key: ApiKey,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<FootballGameResponse>>, AppError> {
    let games = state.game_repository.list().await;
    let responses: Vec<FootballGameResponse> = games.iter().map(|g| g.to_game_response()).collect();
    Ok(Json(responses))
}

/// GET /api/mock/games/{id}
/// Get a single mock game by ID. Triggers state advancement for live games.
#[utoipa::path(
    get,
    path = "/api/mock/games/{id}",
    params(
        ("id" = String, Path, description = "Game ID (e.g., 'sim_1')"),
    ),
    responses(
        (status = 200, description = "Mock game state", body = FootballGameResponse),
        (status = 401, description = "Missing or invalid API key", body = ErrorResponse),
        (status = 404, description = "Game not found", body = ErrorResponse),
    ),
    security(
        ("api_key" = [])
    ),
    tag = "mock"
)]
pub async fn get_mock_game(
    _api_key: ApiKey,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<FootballGameResponse>, AppError> {
    let game = state
        .game_repository
        .get(&id)
        .await
        .ok_or_else(|| AppError::MockGameNotFound(id))?;

    Ok(Json(game.to_game_response()))
}

/// POST /api/mock/games
/// Create a new mock game
#[utoipa::path(
    post,
    path = "/api/mock/games",
    request_body = CreateGameRequest,
    responses(
        (status = 201, description = "Game created successfully", body = FootballGameResponse),
        (status = 400, description = "Invalid request body", body = ErrorResponse),
        (status = 401, description = "Missing or invalid API key", body = ErrorResponse),
    ),
    security(
        ("api_key" = [])
    ),
    tag = "mock"
)]
pub async fn create_mock_game(
    _api_key: ApiKey,
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateGameRequest>,
) -> Result<(StatusCode, Json<FootballGameResponse>), AppError> {
    let game = state.game_repository.create(request).await;
    Ok((StatusCode::CREATED, Json(game.to_game_response())))
}

/// DELETE /api/mock/games/{id}
/// Delete a mock game from the repository
#[utoipa::path(
    delete,
    path = "/api/mock/games/{id}",
    params(
        ("id" = String, Path, description = "Game ID to delete"),
    ),
    responses(
        (status = 204, description = "Game deleted successfully"),
        (status = 401, description = "Missing or invalid API key", body = ErrorResponse),
        (status = 404, description = "Game not found", body = ErrorResponse),
    ),
    security(
        ("api_key" = [])
    ),
    tag = "mock"
)]
pub async fn delete_mock_game(
    _api_key: ApiKey,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    if state.game_repository.delete(&id).await {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::MockGameNotFound(id))
    }
}
