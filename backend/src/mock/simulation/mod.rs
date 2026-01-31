//! Game simulation module for generating realistic, progressing NFL games.
//!
//! This module provides:
//! - `GameRepository`: Thread-safe storage for active game simulations
//! - `CreateGameRequest`: Discriminated union for creating games in different states
//! - `SimulatedGame`: Internal game state that converts to standard `GameResponse`
//! - Simulation engine for realistic play-by-play progression

mod drives;
mod engine;
mod options;
mod plays;
mod repository;
mod state;

pub use options::{CreateFinalOptions, CreateGameRequest, CreateLiveOptions, CreatePregameOptions};
pub use repository::GameRepository;
