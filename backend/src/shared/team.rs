use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct TeamColors {
    /// RGB888 packed as 0x00RRGGBB for cheap parsing on the Pico.
    pub primary: u32,
    pub alternate: u32,
}

/// A live team's display state: identity, current score, and colors. Shared
/// verbatim by every sport's live payload.
#[derive(Serialize, ToSchema)]
pub struct TeamState {
    /// Team abbreviation, e.g. "BOS" — firmware uses this to fetch the logo.
    pub abbreviation: String,
    pub score: u32,
    pub colors: TeamColors,
}
