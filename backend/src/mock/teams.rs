use crate::game::types::Color;

/// NFL team data for mock generation
pub struct NflTeam {
    pub abbreviation: &'static str,
    pub color: Color,
}

/// All 32 NFL teams with their primary colors
pub const NFL_TEAMS: &[NflTeam] = &[
    // AFC East
    NflTeam { abbreviation: "BUF", color: Color { r: 0, g: 51, b: 141 } },
    NflTeam { abbreviation: "MIA", color: Color { r: 0, g: 142, b: 151 } },
    NflTeam { abbreviation: "NE", color: Color { r: 0, g: 34, b: 68 } },
    NflTeam { abbreviation: "NYJ", color: Color { r: 18, g: 87, b: 64 } },
    // AFC North
    NflTeam { abbreviation: "BAL", color: Color { r: 36, g: 23, b: 115 } },
    NflTeam { abbreviation: "CIN", color: Color { r: 251, g: 79, b: 20 } },
    NflTeam { abbreviation: "CLE", color: Color { r: 49, g: 29, b: 0 } },
    NflTeam { abbreviation: "PIT", color: Color { r: 255, g: 182, b: 18 } },
    // AFC South
    NflTeam { abbreviation: "HOU", color: Color { r: 3, g: 32, b: 47 } },
    NflTeam { abbreviation: "IND", color: Color { r: 0, g: 44, b: 95 } },
    NflTeam { abbreviation: "JAX", color: Color { r: 16, g: 24, b: 32 } },
    NflTeam { abbreviation: "TEN", color: Color { r: 12, g: 35, b: 64 } },
    // AFC West
    NflTeam { abbreviation: "DEN", color: Color { r: 251, g: 79, b: 20 } },
    NflTeam { abbreviation: "KC", color: Color { r: 227, g: 24, b: 55 } },
    NflTeam { abbreviation: "LV", color: Color { r: 0, g: 0, b: 0 } },
    NflTeam { abbreviation: "LAC", color: Color { r: 0, g: 128, b: 198 } },
    // NFC East
    NflTeam { abbreviation: "DAL", color: Color { r: 0, g: 53, b: 148 } },
    NflTeam { abbreviation: "NYG", color: Color { r: 1, g: 35, b: 82 } },
    NflTeam { abbreviation: "PHI", color: Color { r: 0, g: 76, b: 84 } },
    NflTeam { abbreviation: "WSH", color: Color { r: 90, g: 20, b: 20 } },
    // NFC North
    NflTeam { abbreviation: "CHI", color: Color { r: 11, g: 22, b: 42 } },
    NflTeam { abbreviation: "DET", color: Color { r: 0, g: 118, b: 182 } },
    NflTeam { abbreviation: "GB", color: Color { r: 24, g: 48, b: 40 } },
    NflTeam { abbreviation: "MIN", color: Color { r: 79, g: 38, b: 131 } },
    // NFC South
    NflTeam { abbreviation: "ATL", color: Color { r: 167, g: 25, b: 48 } },
    NflTeam { abbreviation: "CAR", color: Color { r: 0, g: 133, b: 202 } },
    NflTeam { abbreviation: "NO", color: Color { r: 211, g: 188, b: 141 } },
    NflTeam { abbreviation: "TB", color: Color { r: 213, g: 10, b: 10 } },
    // NFC West
    NflTeam { abbreviation: "ARI", color: Color { r: 151, g: 35, b: 63 } },
    NflTeam { abbreviation: "LAR", color: Color { r: 0, g: 53, b: 148 } },
    NflTeam { abbreviation: "SF", color: Color { r: 170, g: 0, b: 0 } },
    NflTeam { abbreviation: "SEA", color: Color { r: 0, g: 34, b: 68 } },
];

/// Get a random pair of different teams for a matchup
pub fn get_matchup(rng: &mut impl rand::Rng) -> (&'static NflTeam, &'static NflTeam) {
    use rand::seq::SliceRandom;

    let mut indices: Vec<usize> = (0..NFL_TEAMS.len()).collect();
    indices.shuffle(rng);

    (&NFL_TEAMS[indices[0]], &NFL_TEAMS[indices[1]])
}
