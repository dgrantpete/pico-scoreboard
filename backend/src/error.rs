use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use utoipa::ToSchema;

/// Application error types
#[derive(Debug)]
pub enum AppError {
    /// Error making request to ESPN API
    EspnRequest(reqwest::Error),
    /// Error fetching image from ESPN CDN
    ImageFetch(reqwest::Error),
    /// Error decoding or encoding image
    ImageDecode(String),
    /// Invalid hex color format
    InvalidColor(String),
    /// Team logo not found (ESPN returned 404)
    TeamNotFound(String),
    /// Game not found in scoreboard
    GameNotFound(String),
    /// Invalid event ID format
    InvalidEventId(String),
    /// Invalid mock scenario
    InvalidScenario(String),
    /// Mock game not found in repository
    MockGameNotFound(String),
    /// Missing API key header
    MissingApiKey,
    /// Invalid API key
    Unauthorized,
    /// ESPN API response deserialization failed
    EspnDeserialize { path: String, message: String },
}

/// Error response body
#[derive(Serialize, ToSchema)]
pub struct ErrorResponse {
    /// Error code (e.g., "game_not_found", "unauthorized")
    pub error: String,
    /// Human-readable error message
    pub message: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error, message) = match self {
            AppError::EspnRequest(e) => (
                StatusCode::BAD_GATEWAY,
                "espn_error".to_string(),
                format!("Failed to fetch data from ESPN: {}", e),
            ),
            AppError::ImageFetch(e) => (
                StatusCode::BAD_GATEWAY,
                "image_fetch_error".to_string(),
                format!("Failed to fetch logo from ESPN: {}", e),
            ),
            AppError::ImageDecode(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "image_decode_error".to_string(),
                format!("Failed to process image: {}", msg),
            ),
            AppError::InvalidColor(c) => (
                StatusCode::BAD_REQUEST,
                "invalid_color".to_string(),
                format!(
                    "Invalid hex color '{}'. Expected 6-digit RGB hex (e.g., 'FF0000')",
                    c
                ),
            ),
            AppError::TeamNotFound(team) => (
                StatusCode::NOT_FOUND,
                "team_not_found".to_string(),
                format!("Team '{}' not found", team),
            ),
            AppError::GameNotFound(id) => (
                StatusCode::NOT_FOUND,
                "game_not_found".to_string(),
                format!("Game with ID '{}' not found on current scoreboard", id),
            ),
            AppError::InvalidEventId(id) => (
                StatusCode::BAD_REQUEST,
                "invalid_event_id".to_string(),
                format!("Event ID '{}' is invalid. Must be numeric.", id),
            ),
            AppError::InvalidScenario(s) => (
                StatusCode::BAD_REQUEST,
                "invalid_scenario".to_string(),
                format!(
                    "Invalid scenario '{}'. Valid options: pregame, live, final, mixed, redzone, overtime",
                    s
                ),
            ),
            AppError::MockGameNotFound(id) => (
                StatusCode::NOT_FOUND,
                "mock_game_not_found".to_string(),
                format!("Mock game with ID '{}' not found", id),
            ),
            AppError::MissingApiKey => (
                StatusCode::UNAUTHORIZED,
                "missing_api_key".to_string(),
                "X-Api-Key header is required".to_string(),
            ),
            AppError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "unauthorized".to_string(),
                "Invalid API key".to_string(),
            ),
            AppError::EspnDeserialize { path, message } => (
                StatusCode::BAD_GATEWAY,
                "espn_deserialize_error".to_string(),
                format!("Failed to parse ESPN response at '{}': {}", path, message),
            ),
        };

        let body = ErrorResponse { error, message };

        (status, Json(body)).into_response()
    }
}
