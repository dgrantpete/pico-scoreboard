use serde::Serialize;
use utoipa::ToSchema;

use crate::error::AppError;
use crate::espn::types::HomeAway;

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

/// Order two competitors into (home, away) by their `homeAway` markers,
/// erroring when ESPN doesn't supply exactly one of each. Sport modules map
/// the ordered pair into their own team types.
pub(crate) fn order_home_away<C>(
    event_id: &str,
    competitors: [C; 2],
    side: impl Fn(&C) -> HomeAway,
    describe: impl Fn(&C) -> &str,
) -> Result<(C, C), AppError> {
    let [a, b] = competitors;
    match (side(&a), side(&b)) {
        (HomeAway::Home, HomeAway::Away) => Ok((a, b)),
        (HomeAway::Away, HomeAway::Home) => Ok((b, a)),
        _ => {
            let json_path = format!(
                "events[?].competitions[0].competitors (event_id={})",
                event_id
            );
            tracing::error!(
                json_path = %json_path,
                event_id = %event_id,
                first_team = %describe(&a),
                second_team = %describe(&b),
                "ESPN competitors did not split into exactly one home and one away"
            );
            Err(AppError::EspnDeserialize {
                url: String::new(),
                json_path,
                message: format!(
                    "expected one home and one away competitor, got {}/{}",
                    describe(&a),
                    describe(&b)
                ),
            })
        }
    }
}
