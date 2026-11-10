//! What a basketball league plays by, as far as the device draws it
//! (NCAAB.md D2).
//!
//! A product of independent properties, not a sum of leagues: the periods and
//! the court vary separately in the real world (NCAA women's basketball plays
//! quarters on the NCAA court), and no code reads them together. The league is
//! matched once, here, at the edge; everything downstream reads properties.

use crate::feed::LeagueId;
use crate::periods::Periods;

/// One league's rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BasketballRules {
    pub periods: Periods,
    pub court: CourtLines,
    /// Whether the league's screen has team-foul and timeout tracks. ESPN
    /// publishes no college ledger (its fields read zero), so that band has
    /// no tracks and its scores centre on the crests (NCAAB.md D3) — decided
    /// by the league, never by what one poll carried.
    pub ledger: bool,
}

impl BasketballRules {
    pub const NBA: BasketballRules = BasketballRules {
        periods: Periods::QUARTERS,
        court: CourtLines::NBA,
        ledger: true,
    };

    pub const MENS_COLLEGE: BasketballRules = BasketballRules {
        periods: Periods::HALVES,
        court: CourtLines::NCAA,
        ledger: false,
    };

    /// The rules for a basketball league, by the slug its endpoint key ends
    /// in. A league registered nowhere — one the backend adds before the
    /// firmware knows it — plays by the NBA's.
    pub fn of(league: &LeagueId) -> BasketballRules {
        let slug = league.key.as_str().rsplit('/').next().unwrap_or("");
        LEAGUE_RULES
            .iter()
            .find(|(known, _)| *known == slug)
            .map_or(BasketballRules::NBA, |(_, rules)| *rules)
    }
}

/// Every league with rules of its own, by config slug.
const LEAGUE_RULES: &[(&str, BasketballRules)] = &[
    ("nba", BasketballRules::NBA),
    ("mens-college-basketball", BasketballRules::MENS_COLLEGE),
];

/// The rule-book lines that differ between courts. The floor's size, the rim,
/// the backboard and the free-throw line are the same everywhere and stay the
/// renderer's constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CourtLines {
    /// The three-point arc's radius about the rim.
    pub arc: Length,
    /// The corner three's distance from the centre line.
    pub corner: Length,
    /// Half the lane's width.
    pub lane_half: Length,
}

impl CourtLines {
    pub const NBA: CourtLines = CourtLines {
        arc: Length::ft(23, 9, 0),
        corner: Length::ft(22, 0, 0),
        lane_half: Length::ft(8, 0, 0),
    };

    /// NCAA basketball since 2019 (men) and 2021 (women): the international
    /// arc, 6.75 m, and a 12 ft lane.
    pub const NCAA: CourtLines = CourtLines {
        arc: Length::ft(22, 1, 6),
        corner: Length::ft(21, 7, 7),
        lane_half: Length::ft(6, 0, 0),
    };
}

/// A rule-book length in eighths of an inch — the unit the rule books' own
/// fractions resolve in ("22 ft 1¾ in"), so every line is exact and two courts
/// compare equal only when they are the same court.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Length(u16);

impl Length {
    /// `feet` ft, `inches` in and `eighths` of an inch.
    pub const fn ft(feet: u16, inches: u16, eighths: u16) -> Length {
        Length(feet * 96 + inches * 8 + eighths)
    }

    /// In feet, the unit the court's projection works in.
    pub fn feet(self) -> f32 {
        self.0 as f32 / 96.0
    }
}
