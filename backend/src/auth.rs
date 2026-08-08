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

/// API key extractor for the Rust firmware's `/fw/*` endpoints.
///
/// # Why this is not [`ApiKey`]
///
/// The two fleets fetch their images over different transports, and only one of
/// them can keep a secret.
///
/// `firmware/src/ota.py` wraps its socket in TLS, so the key a MicroPython gift
/// unit sends to `/app/*` is confidential in transit. The Rust firmware has no
/// TLS at all — SPEC §8 removed it deliberately, because authenticity comes
/// from the ed25519 signature on the artifact rather than from the transport,
/// and the alternative was ~21 KB of standing mbedTLS record buffers on a
/// device that does not have them spare. Whatever key that firmware sends is
/// therefore **visible to anything on the local network**.
///
/// Sharing one key between them would mean the Rust fleet leaking the gift
/// units' credential onto every LAN it joins. Two keys keeps that exposure
/// where it belongs: `/fw/*` gates a download that is worthless without the
/// signing key, and `/app/*` is untouched.
///
/// Same "unconfigured means auth disabled" convention as [`ApiKey`], and the
/// same `X-Api-Key` header.
pub struct FwApiKey;

impl<S> FromRequestParts<S> for FwApiKey
where
    S: Send + Sync,
    Arc<AppState>: FromRef<S>,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state = Arc::<AppState>::from_ref(state);

        let expected_key = match &app_state.config.fw_api_key {
            Some(key) => key,
            None => return Ok(FwApiKey),
        };

        match parts.headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
            Some(provided_key) if provided_key == expected_key => Ok(FwApiKey),
            Some(_) => Err(AppError::Unauthorized),
            None => Err(AppError::MissingApiKey),
        }
    }
}
