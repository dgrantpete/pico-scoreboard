use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::auth::ApiKey;
use crate::error::{AppError, ErrorResponse};
use crate::game::types::GameResponse;
use crate::AppState;

use super::generator::{generate_game_by_id, generate_games, Scenario};

/// Query parameters for mock game generation
#[derive(Debug, Deserialize)]
pub struct MockGamesQuery {
    /// Test scenario: pregame, live, final, mixed (default), redzone, overtime
    #[serde(default)]
    pub scenario: Option<String>,
    /// Number of games to generate (default: 5)
    #[serde(default)]
    pub count: Option<usize>,
    /// Optional seed for deterministic generation
    #[serde(default)]
    pub seed: Option<u64>,
}

/// GET /api/mock/games
/// Returns procedurally generated mock games for testing
#[utoipa::path(
    get,
    path = "/api/mock/games",
    params(
        ("scenario" = Option<String>, Query, description = "Test scenario: pregame, live, final, mixed (default), redzone, overtime"),
        ("count" = Option<usize>, Query, description = "Number of games to generate (default: 5, max: 16)"),
        ("seed" = Option<u64>, Query, description = "Random seed for deterministic generation"),
    ),
    responses(
        (status = 200, description = "Mock games generated successfully", body = Vec<GameResponse>),
        (status = 400, description = "Invalid scenario parameter", body = ErrorResponse),
        (status = 401, description = "Missing or invalid API key", body = ErrorResponse),
    ),
    security(
        ("api_key" = [])
    ),
    tag = "mock"
)]
pub async fn get_mock_games(
    _api_key: ApiKey,
    _state: State<Arc<AppState>>,
    Query(params): Query<MockGamesQuery>,
) -> Result<Json<Vec<GameResponse>>, AppError> {
    // Parse scenario (default to Mixed)
    let scenario = match &params.scenario {
        Some(s) => Scenario::from_str(s).ok_or_else(|| AppError::InvalidScenario(s.clone()))?,
        None => Scenario::default(),
    };

    // Clamp count between 1 and 16 (max 16 games in NFL week)
    let count = params.count.unwrap_or(5).clamp(1, 16);

    let games = generate_games(scenario, count, params.seed);

    Ok(Json(games))
}

/// GET /api/mock/games/{event_id}
/// Returns a single mock game with deterministic generation based on event_id
#[utoipa::path(
    get,
    path = "/api/mock/games/{event_id}",
    params(
        ("event_id" = String, Path, description = "Mock event ID (used as seed for generation)"),
        ("scenario" = Option<String>, Query, description = "Test scenario: pregame, live, final, mixed (default), redzone, overtime"),
    ),
    responses(
        (status = 200, description = "Mock game generated successfully", body = GameResponse),
        (status = 400, description = "Invalid scenario parameter", body = ErrorResponse),
        (status = 401, description = "Missing or invalid API key", body = ErrorResponse),
    ),
    security(
        ("api_key" = [])
    ),
    tag = "mock"
)]
pub async fn get_mock_game(
    _api_key: ApiKey,
    _state: State<Arc<AppState>>,
    Path(event_id): Path<String>,
    Query(params): Query<MockGamesQuery>,
) -> Result<Json<GameResponse>, AppError> {
    // Parse scenario (default to Mixed)
    let scenario = match &params.scenario {
        Some(s) => Scenario::from_str(s).ok_or_else(|| AppError::InvalidScenario(s.clone()))?,
        None => Scenario::default(),
    };

    let game = generate_game_by_id(&event_id, scenario);

    Ok(Json(game))
}
