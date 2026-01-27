use rand::Rng;

use crate::game::types::{
    Color, Down, FinalGame, FinalStatus, GameResponse, LiveGame, Possession, PregameGame, Quarter,
    Situation, Team, TeamWithScore, Weather, Winner,
};

use super::teams::{get_matchup, NflTeam};

/// Available test scenarios
#[derive(Debug, Clone, Copy, Default)]
pub enum Scenario {
    /// All games in pregame state
    Pregame,
    /// All games currently live
    Live,
    /// All games completed
    Final,
    /// Realistic mix of pregame, live, and final
    #[default]
    Mixed,
    /// All live games in red zone
    RedZone,
    /// Games in overtime situations
    Overtime,
}

impl Scenario {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "pregame" => Some(Self::Pregame),
            "live" => Some(Self::Live),
            "final" => Some(Self::Final),
            "mixed" => Some(Self::Mixed),
            "redzone" | "red_zone" => Some(Self::RedZone),
            "overtime" | "ot" => Some(Self::Overtime),
            _ => None,
        }
    }
}

/// Generate mock games based on scenario
pub fn generate_games(scenario: Scenario, count: usize, seed: Option<u64>) -> Vec<GameResponse> {
    let mut rng = match seed {
        Some(s) => rand::rngs::StdRng::seed_from_u64(s),
        None => rand::rngs::StdRng::seed_from_u64(rand::random()),
    };

    (0..count)
        .map(|i| generate_game_for_scenario(scenario, i, &mut rng))
        .collect()
}

/// Generate a single mock game by event ID
pub fn generate_game_by_id(event_id: &str, scenario: Scenario) -> GameResponse {
    // Use event_id as seed for deterministic generation
    let seed: u64 = event_id
        .bytes()
        .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));

    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);

    // For mixed scenario, use the seed to determine state
    let actual_scenario = if matches!(scenario, Scenario::Mixed) {
        match seed % 3 {
            0 => Scenario::Pregame,
            1 => Scenario::Live,
            _ => Scenario::Final,
        }
    } else {
        scenario
    };

    generate_game_for_scenario(actual_scenario, 0, &mut rng)
}

fn generate_game_for_scenario(
    scenario: Scenario,
    index: usize,
    rng: &mut impl Rng,
) -> GameResponse {
    match scenario {
        Scenario::Pregame => generate_pregame(index, rng),
        Scenario::Live => generate_live(index, rng, false),
        Scenario::Final => generate_final(index, rng, false),
        Scenario::Mixed => {
            // Distribute: 30% pregame, 40% live, 30% final
            match index % 10 {
                0..=2 => generate_pregame(index, rng),
                3..=6 => generate_live(index, rng, false),
                _ => generate_final(index, rng, false),
            }
        }
        Scenario::RedZone => generate_live(index, rng, true),
        Scenario::Overtime => generate_overtime(index, rng),
    }
}

fn generate_pregame(index: usize, rng: &mut impl Rng) -> GameResponse {
    let (home_team, away_team) = get_matchup(rng);

    GameResponse::Pregame(PregameGame {
        event_id: format!("mock_{}", 1000 + index),
        home: team_from_nfl(home_team, rng),
        away: team_from_nfl(away_team, rng),
        start_time: generate_start_time(rng),
        venue: Some(generate_venue(rng)),
        broadcast: Some(generate_broadcast(rng)),
        weather: if rng.gen_bool(0.7) {
            Some(generate_weather(rng))
        } else {
            None
        },
    })
}

fn generate_live(index: usize, rng: &mut impl Rng, force_redzone: bool) -> GameResponse {
    let (home_team, away_team) = get_matchup(rng);

    GameResponse::Live(LiveGame {
        event_id: format!("mock_{}", 2000 + index),
        home: team_with_score_from_nfl(home_team, rng),
        away: team_with_score_from_nfl(away_team, rng),
        quarter: generate_quarter(rng),
        clock: generate_clock(rng),
        clock_running: rng.gen_bool(0.6), // 60% chance clock is running
        situation: Some(generate_situation(rng, force_redzone)),
        last_play: None, // Mock doesn't generate play-by-play
    })
}

fn generate_final(index: usize, rng: &mut impl Rng, overtime: bool) -> GameResponse {
    let (home_team, away_team) = get_matchup(rng);

    let home_score: u8 = rng.gen_range(0..=45);
    let away_score: u8 = rng.gen_range(0..=45);

    let winner = if home_score > away_score {
        Winner::Home
    } else if away_score > home_score {
        Winner::Away
    } else {
        Winner::Tie
    };

    let status = if overtime {
        FinalStatus::FinalOvertime
    } else {
        FinalStatus::Final
    };

    GameResponse::Final(FinalGame {
        event_id: format!("mock_{}", 3000 + index),
        home: TeamWithScore {
            abbreviation: home_team.abbreviation.to_string(),
            color: color_clone(&home_team.color),
            record: Some(generate_record(rng)),
            score: home_score,
            timeouts: 0,
        },
        away: TeamWithScore {
            abbreviation: away_team.abbreviation.to_string(),
            color: color_clone(&away_team.color),
            record: Some(generate_record(rng)),
            score: away_score,
            timeouts: 0,
        },
        status,
        winner,
    })
}

fn generate_overtime(index: usize, rng: &mut impl Rng) -> GameResponse {
    let (home_team, away_team) = get_matchup(rng);

    // 50% chance of live OT vs final/OT
    if rng.gen_bool(0.5) {
        // Live overtime
        let tied_score: u8 = rng.gen_range(14..=35);
        let home_ot_points: u8 = if rng.gen_bool(0.3) {
            rng.gen_range(0..=7)
        } else {
            0
        };
        let away_ot_points: u8 = if rng.gen_bool(0.3) {
            rng.gen_range(0..=7)
        } else {
            0
        };

        GameResponse::Live(LiveGame {
            event_id: format!("mock_{}", 4000 + index),
            home: TeamWithScore {
                abbreviation: home_team.abbreviation.to_string(),
                color: color_clone(&home_team.color),
                record: Some(generate_record(rng)),
                score: tied_score + home_ot_points,
                timeouts: rng.gen_range(0..=2),
            },
            away: TeamWithScore {
                abbreviation: away_team.abbreviation.to_string(),
                color: color_clone(&away_team.color),
                record: Some(generate_record(rng)),
                score: tied_score + away_ot_points,
                timeouts: rng.gen_range(0..=2),
            },
            quarter: if rng.gen_bool(0.8) {
                Quarter::Overtime
            } else {
                Quarter::DoubleOvertime
            },
            clock: generate_clock(rng),
            clock_running: rng.gen_bool(0.6),
            situation: Some(generate_situation(rng, false)),
            last_play: None,
        })
    } else {
        // Final with overtime
        generate_final(index, rng, true)
    }
}

// Helper functions

fn team_from_nfl(nfl_team: &NflTeam, rng: &mut impl Rng) -> Team {
    Team {
        abbreviation: nfl_team.abbreviation.to_string(),
        color: color_clone(&nfl_team.color),
        record: Some(generate_record(rng)),
    }
}

fn team_with_score_from_nfl(nfl_team: &NflTeam, rng: &mut impl Rng) -> TeamWithScore {
    TeamWithScore {
        abbreviation: nfl_team.abbreviation.to_string(),
        color: color_clone(&nfl_team.color),
        record: Some(generate_record(rng)),
        score: rng.gen_range(0..=42),
        timeouts: rng.gen_range(0..=3),
    }
}

fn color_clone(c: &Color) -> Color {
    Color {
        r: c.r,
        g: c.g,
        b: c.b,
    }
}

fn generate_record(rng: &mut impl Rng) -> String {
    let wins: u8 = rng.gen_range(0..=17);
    let losses: u8 = rng.gen_range(0..=(17 - wins));
    let ties: u8 = if rng.gen_bool(0.05) { 1 } else { 0 };

    if ties > 0 {
        format!("{}-{}-{}", wins, losses, ties)
    } else {
        format!("{}-{}", wins, losses)
    }
}

fn generate_start_time(rng: &mut impl Rng) -> String {
    // Generate ISO datetime format for firmware to parse
    let hours = [13, 16, 20]; // Common NFL start times in 24-hour (1 PM, 4 PM, 8 PM ET)
    let hour = hours[rng.gen_range(0..hours.len())];
    let minute = if rng.gen_bool(0.7) { 0 } else { 30 };
    // Use a sample date - in real usage, ESPN provides the actual date
    let day = rng.gen_range(1..=28);
    let month = rng.gen_range(9..=12); // NFL season months
    format!("2024-{:02}-{:02}T{:02}:{:02}:00Z", month, day, hour, minute)
}

fn generate_venue(rng: &mut impl Rng) -> String {
    let venues = [
        "Arrowhead Stadium",
        "AT&T Stadium",
        "Bank of America Stadium",
        "Caesars Superdome",
        "Empower Field at Mile High",
        "Gillette Stadium",
        "Hard Rock Stadium",
        "Highmark Stadium",
        "Levi's Stadium",
        "Lincoln Financial Field",
        "Lucas Oil Stadium",
        "Lumen Field",
        "M&T Bank Stadium",
        "MetLife Stadium",
        "NRG Stadium",
        "SoFi Stadium",
        "State Farm Stadium",
        "U.S. Bank Stadium",
    ];
    venues[rng.gen_range(0..venues.len())].to_string()
}

fn generate_broadcast(rng: &mut impl Rng) -> String {
    let broadcasters = ["CBS", "FOX", "NBC", "ESPN", "Amazon Prime", "NFL Network"];
    broadcasters[rng.gen_range(0..broadcasters.len())].to_string()
}

fn generate_weather(rng: &mut impl Rng) -> Weather {
    let descriptions = [
        "Clear",
        "Partly Cloudy",
        "Cloudy",
        "Light Rain",
        "Snow",
        "Windy",
    ];
    Weather {
        temp: rng.gen_range(20..=85),
        description: descriptions[rng.gen_range(0..descriptions.len())].to_string(),
    }
}

fn generate_quarter(rng: &mut impl Rng) -> Quarter {
    match rng.gen_range(0..4) {
        0 => Quarter::First,
        1 => Quarter::Second,
        2 => Quarter::Third,
        _ => Quarter::Fourth,
    }
}

fn generate_clock(rng: &mut impl Rng) -> String {
    let minutes: u8 = rng.gen_range(0..=15);
    let seconds: u8 = rng.gen_range(0..60);
    format!("{}:{:02}", minutes, seconds)
}

fn generate_situation(rng: &mut impl Rng, force_redzone: bool) -> Situation {
    let yard_line = if force_redzone {
        rng.gen_range(1..=20)
    } else {
        rng.gen_range(1..=99)
    };

    let red_zone = yard_line <= 20;

    Situation {
        down: match rng.gen_range(0..4) {
            0 => Down::First,
            1 => Down::Second,
            2 => Down::Third,
            _ => Down::Fourth,
        },
        distance: rng.gen_range(1..=15),
        yard_line,
        possession: if rng.gen_bool(0.5) {
            Possession::Home
        } else {
            Possession::Away
        },
        red_zone,
    }
}

use rand::SeedableRng;
