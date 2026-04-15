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
    /// Error fetching image from ESPN CDN
    ImageFetch(reqwest::Error),
    /// Error decoding or encoding image
    ImageDecode(String),
    /// Invalid hex color format
    InvalidColor(String),
    /// Team abbreviation not found at ESPN CDN
    TeamNotFound(String),
    /// Missing API key header
    MissingApiKey,
    /// Invalid API key
    Unauthorized,
    /// HMAC signature has expired
    ExpiredSignature,
    /// HMAC signature is invalid
    InvalidSignature,
    /// Network / HTTP status failure against ESPN
    EspnRequest(reqwest::Error),
    /// ESPN JSON response failed to deserialize
    EspnDeserialize {
        url: String,
        json_path: String,
        message: String,
    },
    /// Game ID not found or not currently live
    GameNotFound(String),
    /// Team color hex string could not be parsed
    InvalidTeamColor { team: String, raw: String },
}

/// Error response body
#[derive(Serialize, ToSchema)]
pub struct ErrorResponse {
    /// Error code (e.g., "unauthorized")
    pub error: String,
    /// Human-readable error message
    pub message: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error, message) = match self {
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
            AppError::TeamNotFound(abbrev) => (
                StatusCode::NOT_FOUND,
                "team_not_found".to_string(),
                format!("Team '{}' not found", abbrev),
            ),
            AppError::MissingApiKey => (
                StatusCode::UNAUTHORIZED,
                "missing_api_key".to_string(),
                "X-Api-Key header or valid signature is required".to_string(),
            ),
            AppError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "unauthorized".to_string(),
                "Invalid API key".to_string(),
            ),
            AppError::ExpiredSignature => (
                StatusCode::UNAUTHORIZED,
                "expired_signature".to_string(),
                "Signature has expired".to_string(),
            ),
            AppError::InvalidSignature => (
                StatusCode::UNAUTHORIZED,
                "invalid_signature".to_string(),
                "Invalid request signature".to_string(),
            ),
            AppError::EspnRequest(e) => (
                StatusCode::BAD_GATEWAY,
                "espn_request_error".to_string(),
                format!("ESPN upstream request failed: {}", e),
            ),
            AppError::EspnDeserialize {
                json_path, message, ..
            } => (
                StatusCode::BAD_GATEWAY,
                "espn_deserialize_error".to_string(),
                format!(
                    "Upstream response at {} failed to parse: {}",
                    json_path, message
                ),
            ),
            AppError::GameNotFound(id) => (
                StatusCode::NOT_FOUND,
                "game_not_found".to_string(),
                format!("Game '{}' not found or not live", id),
            ),
            AppError::InvalidTeamColor { team, raw } => (
                StatusCode::BAD_GATEWAY,
                "invalid_team_color".to_string(),
                format!(
                    "Upstream returned invalid team color for '{}': '{}'",
                    team, raw
                ),
            ),
        };

        let body = ErrorResponse { error, message };

        (status, Json(body)).into_response()
    }
}
