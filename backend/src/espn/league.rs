//! Registry of ESPN (sport, league) API path pairs. Adding a sport means one
//! implementor here plus a sport module — nothing else changes.

use crate::config::EspnConfig;

pub trait EspnLeague {
    /// ESPN sport slug, e.g. "baseball".
    fn espn_sport(&self) -> &'static str;
    /// ESPN league slug, e.g. "mlb" or "fifa.world".
    fn espn_league(&self) -> &'static str;
}

pub struct Mlb;

impl EspnLeague for Mlb {
    fn espn_sport(&self) -> &'static str {
        "baseball"
    }
    fn espn_league(&self) -> &'static str {
        "mlb"
    }
}

/// Scoreboard URL for one league on ESPN's site API.
pub fn scoreboard_url(config: &EspnConfig, league: &impl EspnLeague) -> String {
    format!(
        "{}/{}/{}/scoreboard",
        config.base_url,
        league.espn_sport(),
        league.espn_league()
    )
}
