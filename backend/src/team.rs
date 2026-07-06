use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, Response, StatusCode, header},
};
use serde::Deserialize;
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

use crate::AppState;
use crate::auth::ApiKey;
use crate::error::{AppError, ErrorResponse};
use crate::logo::{
    blend_with_background, decode_png, encode_png, encode_ppm_p6, encode_rgb565_raw,
    encode_rgb888_raw, parse_hex_color, resize_image,
};

/// League identifier used as a path parameter.
#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum League {
    Nba,
    Mlb,
}

impl League {
    /// ESPN CDN logo path segment.
    fn logo_path(&self) -> &'static str {
        match self {
            League::Nba => "nba",
            League::Mlb => "mlb",
        }
    }
}

/// Query parameters for the logo endpoint.
#[derive(Debug, Deserialize, IntoParams, ToSchema)]
pub struct LogoQuery {
    /// Width in pixels (default: 128)
    #[serde(default = "default_size")]
    pub width: u32,

    /// Height in pixels (default: 128)
    #[serde(default = "default_size")]
    pub height: u32,

    /// Background color as hex RGB888 without # (e.g., "FFFFFF").
    /// If provided, transparent pixels are blended with this color.
    pub background_color: Option<String>,
}

fn default_size() -> u32 {
    128
}

/// Largest resize dimension the endpoint will perform. The panel is 128px
/// wide, so 512 leaves generous headroom for browser use while keeping an
/// unauthenticated-sized request from asking for a multi-hundred-MB resize.
const MAX_DIMENSION: u32 = 512;

/// Supported output formats, selected via the Accept header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ToSchema)]
pub enum OutputFormat {
    Png,
    Ppm,
    Rgb888,
    Rgb565,
}

impl OutputFormat {
    fn content_type(&self) -> &'static str {
        match self {
            OutputFormat::Png => "image/png",
            OutputFormat::Ppm => "image/x-portable-pixmap",
            OutputFormat::Rgb888 => "image/x-rgb888",
            OutputFormat::Rgb565 => "image/x-rgb565",
        }
    }
}

/// Determine output format from Accept header. Defaults to PNG.
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
    OutputFormat::Png
}

/// GET /{league}/{abbrev}/logo
///
/// Fetches a team logo from the ESPN CDN and returns it in the format
/// negotiated via the Accept header (PNG, PPM, raw RGB888, or raw RGB565).
#[utoipa::path(
    get,
    path = "/{league}/{abbrev}/logo",
    params(
        ("league" = League, Path, description = "League (nba or mlb)"),
        ("abbrev" = String, Path, description = "Team abbreviation (e.g., 'bos', 'lal')"),
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
    tag = "team"
)]
pub async fn get_team_logo(
    State(state): State<Arc<AppState>>,
    _auth: ApiKey,
    Path((league, abbrev)): Path<(League, String)>,
    Query(params): Query<LogoQuery>,
    headers: HeaderMap,
) -> Result<Response<Body>, AppError> {
    if !(1..=MAX_DIMENSION).contains(&params.width) || !(1..=MAX_DIMENSION).contains(&params.height)
    {
        return Err(AppError::InvalidDimensions {
            width: params.width,
            height: params.height,
        });
    }

    let output_format = parse_accept_header(&headers);

    let background = if let Some(ref hex) = params.background_color {
        Some(parse_hex_color(hex)?)
    } else {
        None
    };

    let supports_transparency = output_format == OutputFormat::Png;

    let url = format!(
        "{}/i/teamlogos/{}/500/{}.png",
        state.config.espn.logo_url,
        league.logo_path(),
        abbrev.to_lowercase(),
    );

    let logo_bytes = state.espn_client.fetch_logo(&url).await.map_err(|e| {
        if let AppError::ImageFetch(ref req_err) = e
            && req_err.status() == Some(StatusCode::NOT_FOUND)
        {
            return AppError::TeamNotFound(abbrev.clone());
        }
        e
    })?;

    let img = decode_png(&logo_bytes)?;
    let resized = resize_image(&img, params.width, params.height);

    let processed = if let Some(bg) = background {
        blend_with_background(&resized, bg)
    } else if !supports_transparency {
        blend_with_background(&resized, (0, 0, 0))
    } else {
        resized
    };

    let (output_bytes, content_type) = match output_format {
        OutputFormat::Png => (encode_png(&processed)?, OutputFormat::Png.content_type()),
        OutputFormat::Ppm => (encode_ppm_p6(&processed), OutputFormat::Ppm.content_type()),
        OutputFormat::Rgb888 => (
            encode_rgb888_raw(&processed),
            OutputFormat::Rgb888.content_type(),
        ),
        OutputFormat::Rgb565 => (
            encode_rgb565_raw(&processed),
            OutputFormat::Rgb565.content_type(),
        ),
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
