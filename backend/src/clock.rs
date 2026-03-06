use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use chrono::{Offset, Utc};
use chrono_tz::Tz;
use serde::Serialize;
use std::net::IpAddr;
use std::sync::Arc;
use utoipa::ToSchema;

use crate::AppState;

/// Response from the /time endpoint
#[derive(Serialize, ToSchema)]
pub struct TimeResponse {
    /// Current Unix timestamp in seconds (UTC)
    pub timestamp: i64,
    /// UTC offset in seconds for the client's inferred timezone.
    /// Add this value to `timestamp` to get approximate local time.
    /// Null when the client's timezone cannot be determined.
    pub utc_offset: Option<i32>,
}

/// Extract the client's IP address from reverse-proxy headers.
///
/// Checks Fly-Client-IP first (set by Fly.io's proxy), then falls back
/// to the first address in X-Forwarded-For.
fn client_ip(headers: &HeaderMap) -> Option<IpAddr> {
    if let Some(ip) = headers
        .get("fly-client-ip")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<IpAddr>().ok())
    {
        return Some(ip);
    }

    if let Some(ip) = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .and_then(|s| s.trim().parse::<IpAddr>().ok())
    {
        return Some(ip);
    }

    None
}

/// GET /time — current timestamp and client's UTC offset via GeoIP
#[utoipa::path(
    get,
    path = "/time",
    operation_id = "get_time",
    responses(
        (status = 200, description = "Current time and timezone offset", body = TimeResponse),
    ),
    tag = "clock"
)]
pub async fn time(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Json<TimeResponse> {
    let now = Utc::now();
    let timestamp = now.timestamp();
    let utc_offset = resolve_utc_offset(&state, &headers, &now);

    Json(TimeResponse {
        timestamp,
        utc_offset,
    })
}

/// Attempt to resolve the UTC offset for the client.
/// Returns None on any failure (missing IP, DB miss, bad timezone).
fn resolve_utc_offset(
    state: &AppState,
    headers: &HeaderMap,
    now: &chrono::DateTime<Utc>,
) -> Option<i32> {
    let reader = state.geoip_reader.as_ref()?;

    let ip = client_ip(headers).or_else(|| {
        tracing::debug!("No client IP found in headers for timezone resolution");
        None
    })?;

    let city: maxminddb::geoip2::City = reader.lookup(ip).map_err(|e| {
        tracing::warn!(ip = %ip, error = %e, "GeoIP lookup failed");
        e
    }).ok()?;

    let tz_name = city.location?.time_zone?;

    let tz: Tz = tz_name.parse().inspect_err(|&e| {
        tracing::warn!(timezone = tz_name, error = ?e, "Failed to parse IANA timezone");
    }).ok()?;

    let offset_seconds = now
        .with_timezone(&tz)
        .offset()
        .fix()
        .local_minus_utc();

    Some(offset_seconds)
}
