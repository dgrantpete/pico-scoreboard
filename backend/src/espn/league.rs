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

/// NBA league marker. Backs both the basketball games endpoints and the
/// generic logo route (logo resolution is payload-driven off the scoreboard,
/// so any league in this registry gets logos for free).
pub struct Nba;

impl EspnLeague for Nba {
    fn espn_sport(&self) -> &'static str {
        "basketball"
    }
    fn espn_league(&self) -> &'static str {
        "nba"
    }
}

/// Multi-league football sport: both leagues share the ESPN sport slug
/// "football" and one `football/` backend module.
#[derive(Clone, Copy)]
pub enum FootballLeague {
    Nfl,
    CollegeFootball,
}

impl FootballLeague {
    pub fn from_path(league: &str) -> Result<Self, AppError> {
        match league {
            "nfl" => Ok(Self::Nfl),
            "college-football" => Ok(Self::CollegeFootball),
            _ => Err(AppError::InvalidLeague {
                league: format!("football/{league}"),
                valid: VALID_LEAGUES,
            }),
        }
    }

    /// College football carries AP/Coaches poll ranks (the pregame rank line);
    /// the pros do not.
    pub fn is_college(self) -> bool {
        matches!(self, Self::CollegeFootball)
    }
}

impl EspnLeague for FootballLeague {
    fn espn_sport(&self) -> &'static str {
        "football"
    }
    fn espn_league(&self) -> &'static str {
        match self {
            Self::Nfl => "nfl",
            Self::CollegeFootball => "college-football",
        }
    }
}

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
                league: format!("soccer/{league}"),
                valid: VALID_LEAGUES,
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
const VALID_LEAGUES: &str = "baseball/mlb, basketball/nba, football/{nfl, college-football}, soccer/{fifa.world, usa.1, eng.1, mex.1}";

pub enum AnyLeague {
    Mlb,
    Nba,
    Football(FootballLeague),
    Soccer(SoccerLeague),
}

impl AnyLeague {
    pub fn from_path(sport: &str, league: &str) -> Result<Self, AppError> {
        match (sport, league) {
            ("baseball", "mlb") => Ok(Self::Mlb),
            ("basketball", "nba") => Ok(Self::Nba),
            // FootballLeague::from_path already reports its error against the
            // unified VALID_LEAGUES list, prefixed with the football segment.
            ("football", lg) => FootballLeague::from_path(lg).map(Self::Football),
            // SoccerLeague::from_path already reports its error against the
            // unified VALID_LEAGUES list, prefixed with the soccer segment.
            ("soccer", lg) => SoccerLeague::from_path(lg).map(Self::Soccer),
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
            Self::Football(league) => league.espn_sport(),
            Self::Soccer(league) => league.espn_sport(),
        }
    }
    fn espn_league(&self) -> &'static str {
        match self {
            Self::Mlb => Mlb.espn_league(),
            Self::Nba => Nba.espn_league(),
            Self::Football(league) => league.espn_league(),
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

/// Per-event summary URL (rich data: commentary, key events, boxscore).
pub fn summary_url(config: &EspnConfig, league: &impl EspnLeague, event_id: &str) -> String {
    format!(
        "{}/{}/{}/summary?event={}",
        config.base_url,
        league.espn_sport(),
        league.espn_league(),
        event_id
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_league_resolves_every_known_sport_and_league() {
        assert!(matches!(
            AnyLeague::from_path("baseball", "mlb"),
            Ok(AnyLeague::Mlb)
        ));
        assert!(matches!(
            AnyLeague::from_path("basketball", "nba"),
            Ok(AnyLeague::Nba)
        ));
        assert!(matches!(
            AnyLeague::from_path("football", "nfl"),
            Ok(AnyLeague::Football(FootballLeague::Nfl))
        ));
        assert!(matches!(
            AnyLeague::from_path("football", "college-football"),
            Ok(AnyLeague::Football(FootballLeague::CollegeFootball))
        ));
        assert!(matches!(
            AnyLeague::from_path("soccer", "usa.1"),
            Ok(AnyLeague::Soccer(SoccerLeague::Usa1))
        ));
    }

    #[test]
    fn football_leagues_map_to_the_shared_football_sport_slug() {
        assert_eq!(FootballLeague::Nfl.espn_sport(), "football");
        assert_eq!(FootballLeague::CollegeFootball.espn_sport(), "football");
        assert_eq!(FootballLeague::Nfl.espn_league(), "nfl");
        assert_eq!(
            FootballLeague::CollegeFootball.espn_league(),
            "college-football"
        );
        assert!(FootballLeague::CollegeFootball.is_college());
        assert!(!FootballLeague::Nfl.is_college());
    }

    /// Both an unknown sport and an unknown soccer league report the same
    /// unified `VALID_LEAGUES` list (the previously-divergent error strings).
    #[test]
    fn invalid_pairs_share_the_unified_valid_leagues_string() {
        let check = |sport: &str, league: &str, expected_label: &str| match AnyLeague::from_path(
            sport, league,
        ) {
            Err(AppError::InvalidLeague { league, valid }) => {
                assert_eq!(valid, VALID_LEAGUES);
                assert_eq!(league, expected_label);
            }
            _ => panic!("{sport}/{league} must be InvalidLeague"),
        };
        check("hockey", "nhl", "hockey/nhl");
        check("football", "xfl", "football/xfl");
        check("soccer", "eng.99", "soccer/eng.99");
    }
}
