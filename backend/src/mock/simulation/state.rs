//! Internal game state types for simulation.
//!
//! These types maintain more detailed state than the public `GameResponse`,
//! allowing for realistic game progression. Each state converts to the
//! corresponding `GameResponse` variant.

use std::time::Instant;

use chrono::{DateTime, Utc};
use rand::rngs::StdRng;

use crate::game::types::{
    Color, Down, FinalGame, FinalStatus, GameResponse, LastPlay, LiveGame, PlayType, Possession,
    PregameGame, Quarter, Situation, Team, TeamWithScore, Weather, Winner,
};
use crate::mock::teams::NflTeam;

/// A simulated play with its effects.
#[derive(Debug, Clone)]
pub struct SimulatedPlay {
    pub play_type: PlayType,
    pub yards_gained: i8,
    pub description: String,
    /// Seconds consumed by this play
    pub clock_elapsed: u16,
}

/// A game in the repository with all simulation state.
pub struct SimulatedGame {
    /// Unique identifier for this game
    pub id: String,
    /// When this game was created
    pub created_at: Instant,
    /// Last time this game was accessed (for potential cleanup)
    pub last_accessed: Instant,
    /// Current game state
    pub state: GameState,
}

impl SimulatedGame {
    /// Convert to the public `GameResponse` type.
    pub fn to_game_response(&self) -> GameResponse {
        match &self.state {
            GameState::Pregame(state) => GameResponse::Pregame(state.to_pregame_game(&self.id)),
            GameState::Live(state) => GameResponse::Live(state.to_live_game(&self.id)),
            GameState::Final(state) => GameResponse::Final(state.to_final_game(&self.id)),
        }
    }

    /// Update the last_accessed timestamp
    pub fn touch(&mut self) {
        self.last_accessed = Instant::now();
    }
}

/// Internal game state - more detailed than GameResponse.
pub enum GameState {
    Pregame(PregameState),
    Live(LiveState),
    Final(FinalState),
}

/// Internal state for a pregame.
pub struct PregameState {
    pub home_team: TeamInfo,
    pub away_team: TeamInfo,
    pub start_time: DateTime<Utc>,
    pub venue: String,
    pub broadcast: String,
    pub weather: Option<WeatherInfo>,
    /// Seed for RNG when game transitions to live
    pub seed: u64,
    /// Time scale for live simulation
    pub time_scale: f64,
}

impl PregameState {
    pub fn to_pregame_game(&self, event_id: &str) -> PregameGame {
        PregameGame {
            event_id: event_id.to_string(),
            home: self.home_team.to_team(),
            away: self.away_team.to_team(),
            start_time: self.start_time.to_rfc3339(),
            venue: Some(self.venue.clone()),
            broadcast: Some(self.broadcast.clone()),
            weather: self.weather.as_ref().map(|w| Weather {
                temp: w.temp,
                description: w.description.clone(),
            }),
        }
    }

    /// Check if it's time to transition to live state.
    pub fn should_start(&self) -> bool {
        Utc::now() >= self.start_time
    }

    /// Transition to live state.
    pub fn to_live_state(self) -> LiveState {
        LiveState::new(
            self.home_team,
            self.away_team,
            self.seed,
            self.time_scale,
            self.weather,
        )
    }
}

/// Internal state for a live game.
pub struct LiveState {
    pub home_team: TeamInfo,
    pub away_team: TeamInfo,
    pub home_score: u8,
    pub away_score: u8,
    pub quarter: Quarter,
    /// Seconds remaining in the quarter (900 = 15:00)
    pub clock_seconds: u16,
    pub clock_running: bool,
    pub possession: Possession,
    pub down: Down,
    /// Yards to go for first down
    pub distance: u8,
    /// Yard line from possessing team's perspective (0-100, 100 = opponent's end zone)
    pub yard_line: u8,
    pub home_timeouts: u8,
    pub away_timeouts: u8,
    pub last_play: Option<SimulatedPlay>,
    pub play_history: Vec<SimulatedPlay>,
    /// Random number generator for simulation
    pub rng: StdRng,
    /// When this game went live (wall-clock time)
    pub game_start_instant: Instant,
    /// Total game-seconds that have been simulated
    pub simulated_game_seconds: u64,
    /// Time acceleration factor
    pub time_scale: f64,
    /// Whether we're in a kickoff situation
    pub kickoff_pending: bool,
    /// Weather info (persists from pregame)
    pub weather: Option<WeatherInfo>,
}

impl LiveState {
    pub fn new(
        home_team: TeamInfo,
        away_team: TeamInfo,
        seed: u64,
        time_scale: f64,
        weather: Option<WeatherInfo>,
    ) -> Self {
        use rand::SeedableRng;

        let mut rng = StdRng::seed_from_u64(seed);

        // Coin toss - winner receives (random choice for simplicity)
        let possession = if rand::Rng::gen_bool(&mut rng, 0.5) {
            Possession::Home
        } else {
            Possession::Away
        };

        Self {
            home_team,
            away_team,
            home_score: 0,
            away_score: 0,
            quarter: Quarter::First,
            clock_seconds: 900, // 15:00
            clock_running: false,
            possession,
            down: Down::First,
            distance: 10,
            yard_line: 25, // After touchback
            home_timeouts: 3,
            away_timeouts: 3,
            last_play: None,
            play_history: Vec::new(),
            rng,
            game_start_instant: Instant::now(),
            simulated_game_seconds: 0,
            time_scale,
            kickoff_pending: true, // Start with opening kickoff
            weather,
        }
    }

    pub fn to_live_game(&self, event_id: &str) -> LiveGame {
        let situation = if self.kickoff_pending {
            None // No situation during kickoff
        } else {
            Some(Situation {
                down: self.down,
                distance: self.distance,
                yard_line: self.yard_line,
                possession: self.possession,
                red_zone: self.yard_line >= 80, // Within 20 yards of end zone
            })
        };

        LiveGame {
            event_id: event_id.to_string(),
            home: TeamWithScore {
                abbreviation: self.home_team.abbreviation.clone(),
                color: self.home_team.color,
                record: self.home_team.record.clone(),
                score: self.home_score,
                timeouts: self.home_timeouts,
            },
            away: TeamWithScore {
                abbreviation: self.away_team.abbreviation.clone(),
                color: self.away_team.color,
                record: self.away_team.record.clone(),
                score: self.away_score,
                timeouts: self.away_timeouts,
            },
            quarter: self.quarter,
            clock: format_clock(self.clock_seconds),
            clock_running: self.clock_running,
            situation,
            last_play: self.last_play.as_ref().map(|p| LastPlay {
                play_type: p.play_type,
                text: Some(p.description.clone()),
            }),
            weather: self.weather.as_ref().map(|w| Weather {
                temp: w.temp,
                description: w.description.clone(),
            }),
        }
    }

    /// Check if the game should end (transition to final).
    pub fn is_game_over(&self) -> bool {
        // Game ends when Q4 (or OT) clock hits 0 and one team is ahead
        if self.clock_seconds > 0 {
            return false;
        }

        match self.quarter {
            Quarter::Fourth => self.home_score != self.away_score,
            Quarter::Overtime | Quarter::DoubleOvertime => self.home_score != self.away_score,
            _ => false,
        }
    }

    /// Transition to final state.
    pub fn to_final_state(self) -> FinalState {
        let overtime = matches!(self.quarter, Quarter::Overtime | Quarter::DoubleOvertime);

        FinalState {
            home_team: self.home_team,
            away_team: self.away_team,
            home_score: self.home_score,
            away_score: self.away_score,
            overtime,
        }
    }
}

/// Internal state for a completed game.
pub struct FinalState {
    pub home_team: TeamInfo,
    pub away_team: TeamInfo,
    pub home_score: u8,
    pub away_score: u8,
    pub overtime: bool,
}

impl FinalState {
    pub fn to_final_game(&self, event_id: &str) -> FinalGame {
        let winner = if self.home_score > self.away_score {
            Winner::Home
        } else if self.away_score > self.home_score {
            Winner::Away
        } else {
            Winner::Tie
        };

        FinalGame {
            event_id: event_id.to_string(),
            home: TeamWithScore {
                abbreviation: self.home_team.abbreviation.clone(),
                color: self.home_team.color,
                record: self.home_team.record.clone(),
                score: self.home_score,
                timeouts: 0, // Timeouts don't matter for final
            },
            away: TeamWithScore {
                abbreviation: self.away_team.abbreviation.clone(),
                color: self.away_team.color,
                record: self.away_team.record.clone(),
                score: self.away_score,
                timeouts: 0,
            },
            status: if self.overtime {
                FinalStatus::FinalOvertime
            } else {
                FinalStatus::Final
            },
            winner,
        }
    }
}

/// Team information for internal state.
#[derive(Debug, Clone)]
pub struct TeamInfo {
    pub abbreviation: String,
    pub color: Color,
    pub record: Option<String>,
}

impl TeamInfo {
    pub fn from_nfl_team(team: &NflTeam, record: Option<String>) -> Self {
        Self {
            abbreviation: team.abbreviation.to_string(),
            color: team.color,
            record,
        }
    }

    pub fn to_team(&self) -> Team {
        Team {
            abbreviation: self.abbreviation.clone(),
            color: self.color,
            record: self.record.clone(),
        }
    }
}

/// Weather information for internal state.
#[derive(Debug, Clone)]
pub struct WeatherInfo {
    pub temp: i16,
    pub description: String,
}

/// Format clock seconds as "MM:SS".
fn format_clock(seconds: u16) -> String {
    let mins = seconds / 60;
    let secs = seconds % 60;
    format!("{}:{:02}", mins, secs)
}
