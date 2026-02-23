use axum::extract::{FromRef, FromRequestParts};
use axum::http::request::Parts;
use chrono::Utc;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::sync::Arc;

use crate::error::AppError;
use crate::AppState;

type HmacSha256 = Hmac<Sha256>;

/// API key extractor that validates via X-Api-Key header or HMAC-signed URL.
///
/// Authentication methods (tried in order):
/// 1. `X-Api-Key` header — direct API key match
/// 2. `expires` + `sig` query params — HMAC-SHA256 signed URL
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

        // Method 1: X-Api-Key header
        if let Some(provided_key) = parts.headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
            return if provided_key == expected_key {
                Ok(ApiKey)
            } else {
                Err(AppError::Unauthorized)
            };
        }

        // Method 2: HMAC signature via query params (?expires=...&sig=...)
        if let Some(query) = parts.uri.query() {
            let mut expires_val = None;
            let mut sig_val = None;

            for pair in query.split('&') {
                if let Some(value) = pair.strip_prefix("expires=") {
                    expires_val = Some(value.to_string());
                } else if let Some(value) = pair.strip_prefix("sig=") {
                    sig_val = Some(value.to_string());
                }
            }

            if let (Some(expires_str), Some(sig)) = (expires_val, sig_val) {
                let expires: i64 = expires_str.parse().map_err(|_| AppError::InvalidSignature)?;

                // Check expiry
                if expires < Utc::now().timestamp() {
                    return Err(AppError::ExpiredSignature);
                }

                // Compute expected HMAC: sign("{path}|{expires}")
                let path = parts.uri.path();
                let message = format!("{}|{}", path, expires_str);

                let mut mac = HmacSha256::new_from_slice(expected_key.as_bytes())
                    .map_err(|_| AppError::InvalidSignature)?;
                mac.update(message.as_bytes());
                let expected_sig = hex::encode(mac.finalize().into_bytes());

                return if sig == expected_sig {
                    Ok(ApiKey)
                } else {
                    Err(AppError::InvalidSignature)
                };
            }
        }

        // No authentication provided
        Err(AppError::MissingApiKey)
    }
}
