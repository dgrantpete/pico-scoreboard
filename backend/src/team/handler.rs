use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, Response, StatusCode, header},
};
use std::sync::Arc;

use crate::AppState;
use crate::auth::ApiKey;
use crate::error::AppError;

use super::image::{blend_with_background, decode_png, encode_png, encode_ppm_p6, parse_hex_color};
use super::types::{LogoQuery, OutputFormat};

/// Determine output format from Accept header
fn parse_accept_header(headers: &HeaderMap) -> OutputFormat {
    if let Some(accept) = headers.get(header::ACCEPT) {
        if let Ok(accept_str) = accept.to_str() {
            if accept_str.contains("image/x-portable-pixmap") {
                return OutputFormat::Ppm;
            }
        }
    }
    // Default to PNG for */*, image/png, or any other value
    OutputFormat::Png
}

/// GET /api/teams/{team_id}/logo
///
/// Fetches team logo from ESPN CDN with optional processing.
///
/// Content negotiation via Accept header:
/// - `image/png` or `*/*` (default): Returns PNG
/// - `image/x-portable-pixmap`: Returns PPM P6 binary
#[utoipa::path(
    get,
    path = "/api/teams/{team_id}/logo",
    params(
        ("team_id" = String, Path, description = "Team abbreviation (e.g., 'dal', 'nyg')"),
        LogoQuery
    ),
    responses(
        (status = 200, description = "Logo image", content(
            ("image/png"),
            ("image/x-portable-pixmap")
        )),
        (status = 400, description = "Invalid parameters"),
        (status = 401, description = "Missing or invalid API key"),
        (status = 404, description = "Team not found"),
        (status = 502, description = "Error fetching from ESPN"),
    ),
    security(
        ("api_key" = [])
    ),
    tag = "teams"
)]
pub async fn get_team_logo(
    _api_key: ApiKey,
    State(state): State<Arc<AppState>>,
    Path(team_id): Path<String>,
    Query(params): Query<LogoQuery>,
    headers: HeaderMap,
) -> Result<Response<Body>, AppError> {
    let output_format = parse_accept_header(&headers);
    let has_background = params.background_color.is_some();

    // Parse background color early to fail fast on invalid input
    let background = if let Some(ref hex) = params.background_color {
        Some(parse_hex_color(hex)?)
    } else {
        None
    };

    // Determine whether to request transparent image from ESPN
    // - PNG without background: transparent=true, passthrough
    // - PNG with background: transparent=true, blend
    // - PPM without background: transparent=false, convert
    // - PPM with background: transparent=true, blend, convert
    let request_transparent = output_format == OutputFormat::Png || has_background;

    // Fetch logo from ESPN
    let logo_bytes = state
        .espn_client
        .fetch_logo(&team_id, params.width, params.height, request_transparent)
        .await?;

    // Optimization: PNG without background can be returned as-is
    if output_format == OutputFormat::Png && background.is_none() {
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, OutputFormat::Png.content_type())
            .header(header::CACHE_CONTROL, "public, max-age=86400")
            .body(Body::from(logo_bytes.to_vec()))
            .unwrap());
    }

    // Decode the image for processing
    let img = decode_png(&logo_bytes)?;

    // Apply background blending if requested
    let processed = if let Some(bg) = background {
        blend_with_background(&img, bg)
    } else {
        img.to_rgba8()
    };

    // Encode to output format
    let (output_bytes, content_type) = match output_format {
        OutputFormat::Png => {
            let bytes = encode_png(&processed)?;
            (bytes, OutputFormat::Png.content_type())
        }
        OutputFormat::Ppm => {
            let bytes = encode_ppm_p6(&processed);
            (bytes, OutputFormat::Ppm.content_type())
        }
    };

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "public, max-age=86400")
        .body(Body::from(output_bytes))
        .unwrap())
}
