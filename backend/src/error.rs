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
    /// Requested image dimensions outside the allowed range
    InvalidDimensions { width: u32, height: u32 },
    /// Team abbreviation not found at ESPN CDN
    TeamNotFound(String),
    /// Missing API key header
    MissingApiKey,
    /// Invalid API key
    Unauthorized,
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
    /// Unknown league path segment for a sport's routes
    InvalidLeague {
        league: String,
        valid: &'static str,
    },
    /// Team color hex string could not be parsed
    InvalidTeamColor { team: String, raw: String },
}

impl AppError {
    /// Fill in the upstream URL on an `EspnDeserialize` error whose producer
    /// didn't have it in scope (the transformation helpers in `mlb.rs`).
    pub fn with_url(mut self, request_url: &str) -> Self {
        if let AppError::EspnDeserialize { ref mut url, .. } = self
            && url.is_empty()
        {
            *url = request_url.to_string();
        }
        self
    }
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
            AppError::InvalidDimensions { width, height } => (
                StatusCode::BAD_REQUEST,
                "invalid_dimensions".to_string(),
                format!(
                    "Requested dimensions {}x{} outside allowed range 1..=512",
                    width, height
                ),
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
            // 404, not 400: an unknown league is a path segment with no
            // resource behind it, same as an unknown team abbreviation.
            AppError::InvalidLeague { league, valid } => (
                StatusCode::NOT_FOUND,
                "invalid_league".to_string(),
                format!("Unknown league '{}'. Valid leagues: {}", league, valid),
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
