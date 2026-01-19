use axum::extract::{FromRef, FromRequestParts};
use axum::http::request::Parts;
use std::sync::Arc;

use crate::error::AppError;
use crate::AppState;

/// API key extractor that validates the X-Api-Key header
pub struct ApiKey;

impl<S> FromRequestParts<S> for ApiKey
where
    S: Send + Sync,
    Arc<AppState>: FromRef<S>,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state = Arc::<AppState>::from_ref(state);

        let provided_key = parts
            .headers
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
            .ok_or(AppError::MissingApiKey)?;

        if provided_key == app_state.config.api_key {
            Ok(ApiKey)
        } else {
            Err(AppError::Unauthorized)
        }
    }
}
