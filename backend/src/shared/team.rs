use serde::Serialize;
use utoipa::ToSchema;

use crate::error::AppError;

#[derive(Serialize, ToSchema)]
pub struct TeamColors {
    /// RGB888 packed as 0x00RRGGBB for cheap parsing on the Pico.
    pub primary: u32,
    pub alternate: u32,
}

/// Parse an ESPN team color hex string (with optional leading '#') into a
/// packed RGB888 `u32` (`0x00RRGGBB`). The team abbreviation is used purely
/// to give the returned error context for logs and the client response.
pub(crate) fn parse_hex_rgb(raw: &str, team: &str) -> Result<u32, AppError> {
    let hex = raw.strip_prefix('#').unwrap_or(raw);
    if hex.len() != 6 {
        return Err(AppError::InvalidTeamColor {
            team: team.to_string(),
            raw: raw.to_string(),
        });
    }
    u32::from_str_radix(hex, 16).map_err(|_| AppError::InvalidTeamColor {
        team: team.to_string(),
        raw: raw.to_string(),
    })
}
