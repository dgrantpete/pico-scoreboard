use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, header};
use axum::response::IntoResponse;
use serde::Serialize;
use sha2::{Digest, Sha256};
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::ApiKey;
use crate::error::AppError;

/// Log the device-identity headers every ota.py request carries (see the
/// `X-Ota-Proto` contract in firmware/src/ota.py). Nothing routes on these
/// yet — they make `fly logs` a fleet dashboard today and give a future
/// backend the keys it needs (mpy ABI, firmware build, partition size) to
/// serve a mixed fleet without devices updating first.
fn log_device_meta(endpoint: &str, headers: &HeaderMap) {
    let get = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("-")
            .to_string()
    };
    tracing::info!(
        endpoint,
        device_id = get("x-device-id"),
        app_version = get("x-app-version"),
        context = get("x-ota-context"),
        ota_proto = get("x-ota-proto"),
        mpy = get("x-mpy"),
        firmware = get("x-firmware"),
        machine = get("x-machine"),
        romfs_bytes = get("x-romfs-bytes"),
        "ota device request"
    );
}

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
    headers: HeaderMap,
) -> Result<Json<AppManifest>, AppError> {
    log_device_meta("manifest", &headers);
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
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    log_device_meta("image", &headers);
    let image = state.app_image.as_ref().ok_or(AppError::AppImageUnavailable)?;
    Ok((
        [(header::CONTENT_TYPE, "application/octet-stream")],
        image.bytes.clone(),
    ))
}
