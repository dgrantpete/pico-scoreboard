use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, Response, StatusCode, header},
};
use std::sync::Arc;

use crate::AppState;
use crate::auth::ApiKey;
use crate::error::{AppError, ErrorResponse};
use crate::sport::{BasketballLeague, EspnLeague, FootballLeague};

use super::image::{
    blend_with_background, decode_png, encode_png, encode_ppm_p6, encode_rgb565_raw,
    encode_rgb888_raw, parse_hex_color, resize_image,
};
use super::types::{LogoQuery, OutputFormat};

/// Determine output format from Accept header.
/// Uses get_all() to check all Accept header values, since browsers and API
/// clients may send multiple Accept headers (e.g., a default `*/*` plus a custom one).
fn parse_accept_header(headers: &HeaderMap) -> OutputFormat {
    for accept in headers.get_all(header::ACCEPT) {
        if let Ok(accept_str) = accept.to_str() {
            if accept_str.contains("image/x-rgb565") {
                return OutputFormat::Rgb565;
            }
            if accept_str.contains("image/x-rgb888") {
                return OutputFormat::Rgb888;
            }
            if accept_str.contains("image/x-portable-pixmap") {
                return OutputFormat::Ppm;
            }
        }
    }
    // Default to PNG for */*, image/png, or any other value
    OutputFormat::Png
}

/// Shared implementation for fetching team logos from ESPN CDN.
async fn get_team_logo_impl(
    _api_key: ApiKey,
    state: State<Arc<AppState>>,
    league: impl EspnLeague,
    team_id: String,
    params: LogoQuery,
    headers: HeaderMap,
) -> Result<Response<Body>, AppError> {
    let output_format = parse_accept_header(&headers);
    // Parse background color early to fail fast on invalid input
    let background = if let Some(ref hex) = params.background_color {
        Some(parse_hex_color(hex)?)
    } else {
        None
    };

    let supports_transparency = output_format == OutputFormat::Png;

    // Fetch native 500x500 logo from ESPN CDN
    let logo_bytes = state
        .espn_client
        .fetch_logo(league, &team_id)
        .await?;

    // Decode and resize using Lanczos3 for high-quality downscaling
    let img = decode_png(&logo_bytes)?;
    let resized = resize_image(&img, params.width, params.height);

    // Apply background blending
    // For formats without alpha (RGB565, RGB888, PPM), always blend against black
    // to prevent semi-transparent pixels from producing visible artifacts.
    let processed = if let Some(bg) = background {
        blend_with_background(&resized, bg)
    } else if !supports_transparency {
        blend_with_background(&resized, (0, 0, 0))
    } else {
        resized
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
        OutputFormat::Rgb888 => {
            let bytes = encode_rgb888_raw(&processed);
            (bytes, OutputFormat::Rgb888.content_type())
        }
        OutputFormat::Rgb565 => {
            let bytes = encode_rgb565_raw(&processed);
            (bytes, OutputFormat::Rgb565.content_type())
        }
    };

    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "public, max-age=86400")
        .header(header::VARY, "Accept");

    if matches!(output_format, OutputFormat::Png | OutputFormat::Ppm) {
        let ext = match output_format {
            OutputFormat::Png => "png",
            OutputFormat::Ppm => "ppm",
            _ => unreachable!(),
        };
        response = response.header(
            header::CONTENT_DISPOSITION,
            format!("inline; filename=\"logo.{ext}\""),
        );
    }

    Ok(response.body(Body::from(output_bytes)).unwrap())
}

/// GET /api/football/{league}/{team_id}/logo
///
/// Fetches a football team logo from ESPN CDN with optional processing.
#[utoipa::path(
    get,
    path = "/api/football/{league}/{team_id}/logo",
    params(
        ("league" = String, Path, description = "Football league: nfl or ncaaf"),
        ("team_id" = String, Path, description = "Team abbreviation (e.g., 'dal', 'nyg')"),
        LogoQuery
    ),
    responses(
        (status = 200, description = "Logo image", content(
            ("image/png"),
            ("image/x-portable-pixmap"),
            ("image/x-rgb888"),
            ("image/x-rgb565")
        )),
        (status = 400, description = "Invalid parameters", body = ErrorResponse),
        (status = 401, description = "Missing or invalid API key", body = ErrorResponse),
        (status = 404, description = "Team not found", body = ErrorResponse),
        (status = 502, description = "Error fetching from ESPN", body = ErrorResponse),
    ),
    security(("api_key" = [])),
    tag = "football"
)]
pub async fn get_football_team_logo(
    api_key: ApiKey,
    state: State<Arc<AppState>>,
    Path((league, team_id)): Path<(String, String)>,
    Query(params): Query<LogoQuery>,
    headers: HeaderMap,
) -> Result<Response<Body>, AppError> {
    let football_league = FootballLeague::from_league(&league)?;
    get_team_logo_impl(api_key, state, football_league, team_id, params, headers).await
}

/// GET /api/basketball/{league}/{team_id}/logo
///
/// Fetches a basketball team logo from ESPN CDN with optional processing.
#[utoipa::path(
    get,
    path = "/api/basketball/{league}/{team_id}/logo",
    params(
        ("league" = String, Path, description = "Basketball league: nba or ncaab"),
        ("team_id" = String, Path, description = "Team abbreviation (e.g., 'lal', 'bos')"),
        LogoQuery
    ),
    responses(
        (status = 200, description = "Logo image", content(
            ("image/png"),
            ("image/x-portable-pixmap"),
            ("image/x-rgb888"),
            ("image/x-rgb565")
        )),
        (status = 400, description = "Invalid parameters", body = ErrorResponse),
        (status = 401, description = "Missing or invalid API key", body = ErrorResponse),
        (status = 404, description = "Team not found", body = ErrorResponse),
        (status = 502, description = "Error fetching from ESPN", body = ErrorResponse),
    ),
    security(("api_key" = [])),
    tag = "basketball"
)]
pub async fn get_basketball_team_logo(
    api_key: ApiKey,
    state: State<Arc<AppState>>,
    Path((league, team_id)): Path<(String, String)>,
    Query(params): Query<LogoQuery>,
    headers: HeaderMap,
) -> Result<Response<Body>, AppError> {
    let basketball_league = BasketballLeague::from_league(&league)?;
    get_team_logo_impl(api_key, state, basketball_league, team_id, params, headers).await
}
