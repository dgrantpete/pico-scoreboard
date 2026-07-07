//! Registry of ESPN (sport, league) API path pairs. Adding a sport means one
//! implementor here plus a sport module — nothing else changes.

use crate::config::EspnConfig;
use crate::error::AppError;

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

/// Nba has no game endpoints yet; it exists so the generic logo route covers
/// basketball (resolution is payload-driven off the scoreboard, so any league
/// in this registry gets logos for free).
pub struct Nba;

impl EspnLeague for Nba {
    fn espn_sport(&self) -> &'static str {
        "basketball"
    }
    fn espn_league(&self) -> &'static str {
        "nba"
    }
}

const SOCCER_LEAGUES: &str = "fifa.world, usa.1, eng.1, mex.1";

#[derive(Clone, Copy)]
pub enum SoccerLeague {
    FifaWorld,
    Usa1,
    Eng1,
    Mex1,
}

impl SoccerLeague {
    pub fn from_path(league: &str) -> Result<Self, AppError> {
        match league {
            "fifa.world" => Ok(Self::FifaWorld),
            "usa.1" => Ok(Self::Usa1),
            "eng.1" => Ok(Self::Eng1),
            "mex.1" => Ok(Self::Mex1),
            _ => Err(AppError::InvalidLeague {
                league: league.to_string(),
                valid: SOCCER_LEAGUES,
            }),
        }
    }
}

impl EspnLeague for SoccerLeague {
    fn espn_sport(&self) -> &'static str {
        "soccer"
    }
    fn espn_league(&self) -> &'static str {
        match self {
            Self::FifaWorld => "fifa.world",
            Self::Usa1 => "usa.1",
            Self::Eng1 => "eng.1",
            Self::Mex1 => "mex.1",
        }
    }
}

/// Leagues addressable via `/{sport}/{league}/...` path segments.
const VALID_LEAGUES: &str = "baseball/mlb, basketball/nba, soccer/{fifa.world, usa.1, eng.1, mex.1}";

pub enum AnyLeague {
    Mlb,
    Nba,
    Soccer(SoccerLeague),
}

impl AnyLeague {
    pub fn from_path(sport: &str, league: &str) -> Result<Self, AppError> {
        match (sport, league) {
            ("baseball", "mlb") => Ok(Self::Mlb),
            ("basketball", "nba") => Ok(Self::Nba),
            ("soccer", lg) => Ok(Self::Soccer(SoccerLeague::from_path(lg).map_err(
                |_| AppError::InvalidLeague {
                    league: format!("{sport}/{league}"),
                    valid: VALID_LEAGUES,
                },
            )?)),
            _ => Err(AppError::InvalidLeague {
                league: format!("{sport}/{league}"),
                valid: VALID_LEAGUES,
            }),
        }
    }
}

impl EspnLeague for AnyLeague {
    fn espn_sport(&self) -> &'static str {
        match self {
            Self::Mlb => Mlb.espn_sport(),
            Self::Nba => Nba.espn_sport(),
            Self::Soccer(league) => league.espn_sport(),
        }
    }
    fn espn_league(&self) -> &'static str {
        match self {
            Self::Mlb => Mlb.espn_league(),
            Self::Nba => Nba.espn_league(),
            Self::Soccer(league) => league.espn_league(),
        }
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
