use crate::error::AppError;

/// Trait for league types that can be used with the ESPN API.
///
/// Implemented by `FootballLeague` and `BasketballLeague` to provide
/// sport-specific ESPN URL segments while keeping the ESPN client generic.
pub trait EspnLeague {
    /// ESPN API sport path segment (e.g., "football", "basketball").
    fn espn_sport(&self) -> &'static str;

    /// ESPN API league path segment (e.g., "nfl", "mens-college-basketball").
    fn espn_league(&self) -> &'static str;

    /// ESPN CDN logo path segment (e.g., "nfl", "ncaa").
    fn espn_logo_path(&self) -> &'static str;

    /// Whether this is a college league (affects ranking display, period format, etc.).
    fn is_college(&self) -> bool;
}

/// Football league identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FootballLeague {
    Nfl,
    Ncaaf,
}

impl FootballLeague {
    pub fn from_league(s: &str) -> Result<Self, AppError> {
        match s {
            "nfl" => Ok(Self::Nfl),
            "ncaaf" => Ok(Self::Ncaaf),
            _ => Err(AppError::InvalidLeague {
                league: s.to_string(),
                valid: "nfl, ncaaf",
            }),
        }
    }
}

impl EspnLeague for FootballLeague {
    fn espn_sport(&self) -> &'static str {
        "football"
    }

    fn espn_league(&self) -> &'static str {
        match self {
            Self::Nfl => "nfl",
            Self::Ncaaf => "college-football",
        }
    }

    fn espn_logo_path(&self) -> &'static str {
        match self {
            Self::Nfl => "nfl",
            Self::Ncaaf => "ncaa",
        }
    }

    fn is_college(&self) -> bool {
        matches!(self, Self::Ncaaf)
    }
}

/// Basketball league identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BasketballLeague {
    Nba,
    Ncaab,
}

impl BasketballLeague {
    pub fn from_league(s: &str) -> Result<Self, AppError> {
        match s {
            "nba" => Ok(Self::Nba),
            "ncaab" => Ok(Self::Ncaab),
            _ => Err(AppError::InvalidLeague {
                league: s.to_string(),
                valid: "nba, ncaab",
            }),
        }
    }
}

impl EspnLeague for BasketballLeague {
    fn espn_sport(&self) -> &'static str {
        "basketball"
    }

    fn espn_league(&self) -> &'static str {
        match self {
            Self::Nba => "nba",
            Self::Ncaab => "mens-college-basketball",
        }
    }

    fn espn_logo_path(&self) -> &'static str {
        match self {
            Self::Nba => "nba",
            Self::Ncaab => "ncaa",
        }
    }

    fn is_college(&self) -> bool {
        matches!(self, Self::Ncaab)
    }
}
