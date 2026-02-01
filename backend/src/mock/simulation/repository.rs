//! Thread-safe repository for storing game simulations.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Duration, Utc};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use tokio::sync::RwLock;

use super::options::{CreateFinalOptions, CreateGameRequest, CreateLiveOptions, CreatePregameOptions};
use super::state::{
    FinalState, GameState, LiveState, PregameState, SimulatedGame, TeamInfo, WeatherInfo,
};
use crate::game::types::{Down, Possession, Quarter};
use crate::mock::teams::{get_matchup, NflTeam, NFL_TEAMS};

/// Thread-safe repository for active game simulations.
#[derive(Clone)]
pub struct GameRepository {
    games: Arc<RwLock<HashMap<String, SimulatedGame>>>,
    next_id: Arc<AtomicU64>,
}

impl Default for GameRepository {
    fn default() -> Self {
        Self::new()
    }
}

impl GameRepository {
    pub fn new() -> Self {
        Self {
            games: Arc::new(RwLock::new(HashMap::new())),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Generate a unique game ID.
    fn generate_id(&self) -> String {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        format!("sim_{}", id)
    }

    /// Create a new game from the request options.
    pub async fn create(&self, request: CreateGameRequest) -> SimulatedGame {
        let id = self.generate_id();
        let now = Instant::now();

        let state = match request {
            CreateGameRequest::Pregame(opts) => GameState::Pregame(create_pregame_state(opts)),
            CreateGameRequest::Live(opts) => GameState::Live(create_live_state(opts)),
            CreateGameRequest::Final(opts) => GameState::Final(create_final_state(opts)),
        };

        let game = SimulatedGame {
            id: id.clone(),
            created_at: now,
            last_accessed: now,
            state,
        };

        // Store in repository
        {
            let mut games = self.games.write().await;
            games.insert(id.clone(), game);
        }

        // Re-fetch and return (this also advances state if needed)
        self.get(&id).await.expect("Game should exist after creation")
    }

    /// Get a game by ID, advancing its state if needed.
    pub async fn get(&self, id: &str) -> Option<SimulatedGame> {
        let mut games = self.games.write().await;

        if let Some(game) = games.get_mut(id) {
            game.touch();

            // Advance state if needed
            advance_game_state(&mut game.state);

            // Clone the game response data
            Some(SimulatedGame {
                id: game.id.clone(),
                created_at: game.created_at,
                last_accessed: game.last_accessed,
                state: clone_game_state(&game.state),
            })
        } else {
            None
        }
    }

    /// List all games (with state advancement).
    pub async fn list(&self) -> Vec<SimulatedGame> {
        let ids: Vec<String> = {
            let games = self.games.read().await;
            games.keys().cloned().collect()
        };

        let mut result = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(game) = self.get(&id).await {
                result.push(game);
            }
        }
        result
    }

    /// Delete a game by ID. Returns true if the game existed.
    pub async fn delete(&self, id: &str) -> bool {
        let mut games = self.games.write().await;
        games.remove(id).is_some()
    }
}

/// Clone a GameState (needed because we can't derive Clone due to StdRng)
fn clone_game_state(state: &GameState) -> GameState {
    match state {
        GameState::Pregame(p) => GameState::Pregame(PregameState {
            home_team: p.home_team.clone(),
            away_team: p.away_team.clone(),
            start_time: p.start_time,
            venue: p.venue.clone(),
            broadcast: p.broadcast.clone(),
            weather: p.weather.clone(),
            seed: p.seed,
            time_scale: p.time_scale,
        }),
        GameState::Live(l) => GameState::Live(LiveState {
            home_team: l.home_team.clone(),
            away_team: l.away_team.clone(),
            home_score: l.home_score,
            away_score: l.away_score,
            quarter: l.quarter,
            clock_seconds: l.clock_seconds,
            clock_running: l.clock_running,
            possession: l.possession,
            down: l.down,
            distance: l.distance,
            yard_line: l.yard_line,
            home_timeouts: l.home_timeouts,
            away_timeouts: l.away_timeouts,
            last_play: l.last_play.clone(),
            play_history: l.play_history.clone(),
            rng: StdRng::seed_from_u64(0), // Placeholder, won't be used for cloned state
            game_start_instant: l.game_start_instant,
            simulated_game_seconds: l.simulated_game_seconds,
            time_scale: l.time_scale,
            kickoff_pending: l.kickoff_pending,
            weather: l.weather.clone(),
        }),
        GameState::Final(f) => GameState::Final(FinalState {
            home_team: f.home_team.clone(),
            away_team: f.away_team.clone(),
            home_score: f.home_score,
            away_score: f.away_score,
            overtime: f.overtime,
        }),
    }
}

// === State creation helpers ===

fn create_pregame_state(opts: CreatePregameOptions) -> PregameState {
    let seed = opts.seed.unwrap_or_else(rand::random);
    let mut rng = StdRng::seed_from_u64(seed);

    let (home_team, away_team) = resolve_teams(opts.home_team, opts.away_team, &mut rng);

    let start_time = opts
        .start_time
        .and_then(|s| s.parse::<DateTime<Utc>>().ok())
        .unwrap_or_else(|| Utc::now() + Duration::seconds(30));

    let venue = opts.venue.unwrap_or_else(|| random_venue(&mut rng));
    let broadcast = opts.broadcast.unwrap_or_else(|| random_broadcast(&mut rng));
    let weather = opts
        .weather
        .map(|w| WeatherInfo {
            temp: w.temp.unwrap_or_else(|| rng.gen_range(20..=85)),
            description: w
                .description
                .unwrap_or_else(|| random_weather_description(&mut rng)),
        })
        .or_else(|| {
            // 80% chance of having weather (outdoor games)
            if rng.gen_bool(0.8) {
                Some(WeatherInfo {
                    temp: rng.gen_range(20..=85),
                    description: random_weather_description(&mut rng),
                })
            } else {
                None
            }
        });

    let time_scale = opts.time_scale.unwrap_or(60.0);

    PregameState {
        home_team,
        away_team,
        start_time,
        venue,
        broadcast,
        weather,
        seed,
        time_scale,
    }
}

fn create_live_state(opts: CreateLiveOptions) -> LiveState {
    let seed = opts.seed.unwrap_or_else(rand::random);
    let mut rng = StdRng::seed_from_u64(seed);

    let (home_team, away_team) = resolve_teams(opts.home_team, opts.away_team, &mut rng);

    let quarter = opts.quarter.unwrap_or(Quarter::First);
    let clock_seconds = opts
        .clock
        .and_then(|c| parse_clock(&c))
        .unwrap_or(900);

    let possession = opts.possession.unwrap_or_else(|| {
        if rng.gen_bool(0.5) {
            Possession::Home
        } else {
            Possession::Away
        }
    });

    let time_scale = opts.time_scale.unwrap_or(60.0);

    LiveState {
        home_team,
        away_team,
        home_score: opts.home_score.unwrap_or(0),
        away_score: opts.away_score.unwrap_or(0),
        quarter,
        clock_seconds,
        clock_running: false,
        possession,
        down: opts.down.unwrap_or(Down::First),
        distance: opts.distance.unwrap_or(10),
        yard_line: opts.yard_line.unwrap_or(25),
        home_timeouts: opts.home_timeouts.unwrap_or(3),
        away_timeouts: opts.away_timeouts.unwrap_or(3),
        last_play: None,
        play_history: Vec::new(),
        rng,
        game_start_instant: Instant::now(),
        simulated_game_seconds: 0,
        time_scale,
        kickoff_pending: opts.yard_line.is_none() && opts.possession.is_none(),
        weather: None, // Weather not supported for directly-created live games
    }
}

fn create_final_state(opts: CreateFinalOptions) -> FinalState {
    let mut rng = StdRng::from_entropy();

    let (home_team, away_team) = resolve_teams(opts.home_team, opts.away_team, &mut rng);

    // Generate realistic scores if not provided
    let (home_score, away_score) = match (opts.home_score, opts.away_score) {
        (Some(h), Some(a)) => (h, a),
        (Some(h), None) => (h, generate_realistic_score(&mut rng)),
        (None, Some(a)) => (generate_realistic_score(&mut rng), a),
        (None, None) => {
            let h = generate_realistic_score(&mut rng);
            let a = generate_realistic_score(&mut rng);
            (h, a)
        }
    };

    let overtime = opts.overtime.unwrap_or(false);

    FinalState {
        home_team,
        away_team,
        home_score,
        away_score,
        overtime,
    }
}

/// Resolve team options to TeamInfo, using random teams if not specified.
fn resolve_teams(
    home: Option<String>,
    away: Option<String>,
    rng: &mut StdRng,
) -> (TeamInfo, TeamInfo) {
    let home_team = home
        .and_then(|abbr| find_team(&abbr))
        .unwrap_or_else(|| {
            let (h, _) = get_matchup(rng);
            h
        });

    let away_team = away
        .and_then(|abbr| find_team(&abbr))
        .unwrap_or_else(|| {
            // Make sure we don't pick the same team
            loop {
                let (_, a) = get_matchup(rng);
                if a.abbreviation != home_team.abbreviation {
                    return a;
                }
            }
        });

    let home_record = Some(random_record(rng));
    let away_record = Some(random_record(rng));

    (
        TeamInfo::from_nfl_team(home_team, home_record),
        TeamInfo::from_nfl_team(away_team, away_record),
    )
}

/// Find a team by abbreviation (case-insensitive).
fn find_team(abbr: &str) -> Option<&'static NflTeam> {
    let abbr_upper = abbr.to_uppercase();
    NFL_TEAMS.iter().find(|t| t.abbreviation == abbr_upper)
}

/// Generate a random W-L record.
fn random_record(rng: &mut StdRng) -> String {
    let wins = rng.gen_range(0..=17);
    let losses = rng.gen_range(0..=(17 - wins));
    let ties = if rng.gen_bool(0.05) { 1 } else { 0 };

    if ties > 0 {
        format!("{}-{}-{}", wins, losses, ties)
    } else {
        format!("{}-{}", wins, losses)
    }
}

/// Generate a realistic NFL final score.
fn generate_realistic_score(rng: &mut StdRng) -> u8 {
    // NFL scores typically cluster around certain values
    // Common scores: 0, 3, 6, 7, 10, 13, 14, 17, 20, 21, 23, 24, 27, 28, 30, 31, 34, 35
    let weights = [
        (0, 5),
        (3, 15),
        (6, 10),
        (7, 15),
        (10, 20),
        (13, 15),
        (14, 15),
        (17, 25),
        (20, 20),
        (21, 15),
        (23, 10),
        (24, 20),
        (27, 15),
        (28, 10),
        (30, 8),
        (31, 8),
        (34, 5),
        (35, 5),
        (38, 3),
        (42, 2),
        (45, 1),
    ];

    let total_weight: u32 = weights.iter().map(|(_, w)| w).sum();
    let mut choice = rng.gen_range(0..total_weight);

    for (score, weight) in weights {
        if choice < weight {
            return score;
        }
        choice -= weight;
    }

    24 // Fallback
}

/// Parse "MM:SS" format to seconds.
fn parse_clock(clock: &str) -> Option<u16> {
    let parts: Vec<&str> = clock.split(':').collect();
    if parts.len() == 2 {
        let mins: u16 = parts[0].parse().ok()?;
        let secs: u16 = parts[1].parse().ok()?;
        Some(mins * 60 + secs)
    } else {
        None
    }
}

fn random_venue(rng: &mut StdRng) -> String {
    const VENUES: &[&str] = &[
        "Arrowhead Stadium",
        "AT&T Stadium",
        "Bank of America Stadium",
        "Caesars Superdome",
        "Empower Field at Mile High",
        "FedExField",
        "Gillette Stadium",
        "Hard Rock Stadium",
        "Highmark Stadium",
        "Levi's Stadium",
        "Lincoln Financial Field",
        "Lumen Field",
        "M&T Bank Stadium",
        "Mercedes-Benz Stadium",
        "MetLife Stadium",
        "Paycor Stadium",
        "Raymond James Stadium",
        "SoFi Stadium",
        "Soldier Field",
        "State Farm Stadium",
        "U.S. Bank Stadium",
    ];

    VENUES[rng.gen_range(0..VENUES.len())].to_string()
}

fn random_broadcast(rng: &mut StdRng) -> String {
    const BROADCASTS: &[&str] = &["CBS", "FOX", "NBC", "ESPN", "Amazon Prime", "NFL Network"];
    BROADCASTS[rng.gen_range(0..BROADCASTS.len())].to_string()
}

fn random_weather_description(rng: &mut StdRng) -> String {
    const DESCRIPTIONS: &[&str] = &[
        "Clear",
        "Partly Cloudy",
        "Cloudy",
        "Light Rain",
        "Rain",
        "Snow",
        "Windy",
        "Sunny",
    ];
    DESCRIPTIONS[rng.gen_range(0..DESCRIPTIONS.len())].to_string()
}

/// Advance game state (handle transitions and simulation)
fn advance_game_state(state: &mut GameState) {
    // Check for pregame -> live transition
    let should_transition_to_live = matches!(state, GameState::Pregame(p) if p.should_start());

    if should_transition_to_live {
        // Take ownership of the pregame state and convert to live
        let old_state = std::mem::replace(
            state,
            // Temporary placeholder - will be replaced immediately
            GameState::Final(FinalState {
                home_team: TeamInfo {
                    abbreviation: String::new(),
                    color: crate::game::types::Color { r: 0, g: 0, b: 0 },
                    record: None,
                },
                away_team: TeamInfo {
                    abbreviation: String::new(),
                    color: crate::game::types::Color { r: 0, g: 0, b: 0 },
                    record: None,
                },
                home_score: 0,
                away_score: 0,
                overtime: false,
            }),
        );

        if let GameState::Pregame(pregame) = old_state {
            *state = GameState::Live(pregame.to_live_state());
        }
    }

    // Advance live games
    let should_end_game = if let GameState::Live(live) = state {
        super::engine::advance_to_now(live);
        live.is_game_over()
    } else {
        false
    };

    // Transition live -> final if game over
    if should_end_game {
        if let GameState::Live(live) = state {
            let final_state = FinalState {
                home_team: live.home_team.clone(),
                away_team: live.away_team.clone(),
                home_score: live.home_score,
                away_score: live.away_score,
                overtime: matches!(live.quarter, Quarter::Overtime | Quarter::DoubleOvertime),
            };
            *state = GameState::Final(final_state);
        }
    }
}
