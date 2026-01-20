use reqwest::Client;
use std::time::Duration;

use super::types::{EspnEvent, EspnScoreboard};
use crate::config::EspnConfig;
use crate::error::AppError;

/// HTTP client for ESPN API requests
#[derive(Debug, Clone)]
pub struct EspnClient {
    client: Client,
    scoreboard_url: String,
}

impl EspnClient {
    /// Create a new ESPN client with configured timeout and user-agent
    pub fn new(config: &EspnConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .user_agent(&config.user_agent)
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            scoreboard_url: config.scoreboard_url.clone(),
        }
    }

    /// Fetch the full scoreboard from ESPN
    pub async fn fetch_scoreboard(&self) -> Result<EspnScoreboard, AppError> {
        let response = self
            .client
            .get(&self.scoreboard_url)
            .send()
            .await
            .map_err(AppError::EspnRequest)?;

        let scoreboard = response
            .json::<EspnScoreboard>()
            .await
            .map_err(AppError::EspnRequest)?;

        Ok(scoreboard)
    }

    /// Fetch a single game by event ID from the scoreboard
    pub async fn fetch_game(&self, event_id: &str) -> Result<EspnEvent, AppError> {
        let scoreboard = self.fetch_scoreboard().await?;

        scoreboard
            .events
            .into_iter()
            .find(|event| event.id == event_id)
            .ok_or_else(|| AppError::GameNotFound(event_id.to_string()))
    }

    /// Fetch all games from the current scoreboard
    pub async fn fetch_all_games(&self) -> Result<Vec<EspnEvent>, AppError> {
        let scoreboard = self.fetch_scoreboard().await?;
        Ok(scoreboard.events)
    }
}

impl Default for EspnClient {
    fn default() -> Self {
        Self::new(&EspnConfig::default())
    }
}
