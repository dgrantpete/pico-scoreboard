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
    /// Game not found in scoreboard
    GameNotFound(String),
    /// Invalid event ID format
    InvalidEventId(String),
    /// Missing API key header
    MissingApiKey,
    /// Invalid API key
    Unauthorized,
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
        };

        let body = ErrorResponse { error, message };

        (status, Json(body)).into_response()
    }
}
