use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::header;
use axum::response::IntoResponse;
use serde::Serialize;
use sha2::{Digest, Sha256};
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::ApiKey;
use crate::error::AppError;

/// The device app image, loaded once at startup.
///
/// A ROMFS image is ~240 KB, so holding it in memory is cheaper and simpler
/// than re-reading and re-hashing per request. It only changes with a
/// redeploy, which restarts the process anyway.
pub struct AppImage {
    pub bytes: Vec<u8>,
    pub sha256: String,
}

impl AppImage {
    /// Load the image from disk. Returns None (with a log line) when the
    /// file is absent — a dev-friendly state where the OTA endpoints 404.
    pub fn load(path: &str) -> Option<Self> {
        match std::fs::read(path) {
            Ok(bytes) => {
                let sha256 = hex::encode(Sha256::digest(&bytes));
                tracing::info!(path, size = bytes.len(), sha256, "app image loaded");
                Some(Self { bytes, sha256 })
            }
            Err(e) => {
                tracing::warn!(
                    path,
                    error = %e,
                    "app image not available — OTA endpoints will return 404"
                );
                None
            }
        }
    }
}

/// Manifest the firmware polls to decide whether to update.
#[derive(Serialize, ToSchema)]
pub struct AppManifest {
    /// Hex sha256 of the ROMFS image — the app's identity
    pub sha256: String,
    /// Image size in bytes
    pub size: usize,
}

/// Get the current app manifest
#[utoipa::path(
    get,
    path = "/app/manifest",
    tag = "app",
    responses(
        (status = 200, description = "Current app image identity", body = AppManifest),
        (status = 404, description = "No app image published", body = crate::error::ErrorResponse),
        (status = 401, description = "Missing or invalid API key", body = crate::error::ErrorResponse),
    ),
    security(("api_key" = []))
)]
pub async fn get_app_manifest(
    _auth: ApiKey,
    State(state): State<Arc<AppState>>,
) -> Result<Json<AppManifest>, AppError> {
    let image = state.app_image.as_ref().ok_or(AppError::AppImageUnavailable)?;
    Ok(Json(AppManifest {
        sha256: image.sha256.clone(),
        size: image.bytes.len(),
    }))
}

/// Download the current app image (ROMFS)
#[utoipa::path(
    get,
    path = "/app/image",
    tag = "app",
    responses(
        (status = 200, description = "ROMFS image bytes", content_type = "application/octet-stream"),
        (status = 404, description = "No app image published", body = crate::error::ErrorResponse),
        (status = 401, description = "Missing or invalid API key", body = crate::error::ErrorResponse),
    ),
    security(("api_key" = []))
)]
pub async fn get_app_image(
    _auth: ApiKey,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {
    let image = state.app_image.as_ref().ok_or(AppError::AppImageUnavailable)?;
    Ok((
        [(header::CONTENT_TYPE, "application/octet-stream")],
        image.bytes.clone(),
    ))
}
