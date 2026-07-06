use axum::extract::{FromRef, FromRequestParts};
use axum::http::request::Parts;
use std::sync::Arc;

use crate::AppState;
use crate::error::AppError;

/// API key extractor that validates the `X-Api-Key` header.
///
/// Add this as a handler parameter to require authentication on a route.
/// When no API key is configured on the server, authentication is disabled
/// and every request passes.
pub struct ApiKey;

impl<S> FromRequestParts<S> for ApiKey
where
    S: Send + Sync,
    Arc<AppState>: FromRef<S>,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state = Arc::<AppState>::from_ref(state);

        // If no API key is configured, skip authentication entirely
        let expected_key = match &app_state.config.api_key {
            Some(key) => key,
            None => return Ok(ApiKey),
        };

        match parts.headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
            Some(provided_key) if provided_key == expected_key => Ok(ApiKey),
            Some(_) => Err(AppError::Unauthorized),
            None => Err(AppError::MissingApiKey),
        }
    }
}
