use crate::error::AppError;

/// Identifies a sport/league combination supported by the API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SportLeague {
    Nfl,
    Ncaaf,
    Nba,
    Ncaab,
}

impl SportLeague {
    /// Parse from a league path parameter (e.g., "nfl", "ncaab").
    pub fn from_league(s: &str) -> Result<Self, AppError> {
        match s {
            "nfl" => Ok(Self::Nfl),
            "ncaaf" => Ok(Self::Ncaaf),
            "nba" => Ok(Self::Nba),
            "ncaab" => Ok(Self::Ncaab),
            _ => Err(AppError::InvalidLeague(s.to_string())),
        }
    }

    /// Parse from a league path parameter, restricted to football leagues.
    pub fn football_from_league(s: &str) -> Result<Self, AppError> {
        match s {
            "nfl" => Ok(Self::Nfl),
            "ncaaf" => Ok(Self::Ncaaf),
            _ => Err(AppError::InvalidLeague(s.to_string())),
        }
    }

    /// Parse from a league path parameter, restricted to basketball leagues.
    pub fn basketball_from_league(s: &str) -> Result<Self, AppError> {
        match s {
            "nba" => Ok(Self::Nba),
            "ncaab" => Ok(Self::Ncaab),
            _ => Err(AppError::InvalidLeague(s.to_string())),
        }
    }

    /// ESPN API sport path segment.
    pub fn espn_sport(&self) -> &'static str {
        match self {
            Self::Nfl | Self::Ncaaf => "football",
            Self::Nba | Self::Ncaab => "basketball",
        }
    }

    /// ESPN API league path segment.
    pub fn espn_league(&self) -> &'static str {
        match self {
            Self::Nfl => "nfl",
            Self::Ncaaf => "college-football",
            Self::Nba => "nba",
            Self::Ncaab => "mens-college-basketball",
        }
    }

    /// ESPN CDN logo path segment for this league.
    pub fn espn_logo_path(&self) -> &'static str {
        match self {
            Self::Nfl => "nfl",
            Self::Ncaaf => "ncaa",
            Self::Nba => "nba",
            Self::Ncaab => "ncaa",
        }
    }

    pub fn is_football(&self) -> bool {
        matches!(self, Self::Nfl | Self::Ncaaf)
    }

    pub fn is_basketball(&self) -> bool {
        matches!(self, Self::Nba | Self::Ncaab)
    }

    pub fn is_college(&self) -> bool {
        matches!(self, Self::Ncaaf | Self::Ncaab)
    }
}
