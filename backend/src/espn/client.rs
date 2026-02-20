use bytes::Bytes;
use reqwest::Client;
use serde::de::DeserializeOwned;
use std::time::Duration;

use super::types::{EspnEvent, EspnScoreboard, EspnSummary};
use crate::config::EspnConfig;
use crate::error::AppError;
use crate::sport::SportLeague;

/// HTTP client for ESPN API requests
#[derive(Debug, Clone)]
pub struct EspnClient {
    client: Client,
    base_url: String,
    logo_url: String,
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
            base_url: config.base_url.clone(),
            logo_url: config.logo_url.clone(),
        }
    }

    /// Fetch the full scoreboard from ESPN for a given sport/league
    pub async fn fetch_scoreboard(
        &self,
        sport_league: SportLeague,
    ) -> Result<EspnScoreboard, AppError> {
        let url = format!(
            "{}/{}/{}/scoreboard",
            self.base_url,
            sport_league.espn_sport(),
            sport_league.espn_league()
        );

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(AppError::EspnRequest)?;

        // Get raw text first so we can log it on deserialization failure
        let body = response.text().await.map_err(AppError::EspnRequest)?;

        self.deserialize_with_logging::<EspnScoreboard>(&body, "scoreboard")
    }

    /// Fetch a game summary from ESPN (used for basketball single-game detail)
    pub async fn fetch_game_summary(
        &self,
        sport_league: SportLeague,
        event_id: &str,
    ) -> Result<EspnSummary, AppError> {
        let url = format!(
            "{}/{}/{}/summary?event={}",
            self.base_url,
            sport_league.espn_sport(),
            sport_league.espn_league(),
            event_id
        );

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(AppError::EspnRequest)?;

        let body = response.text().await.map_err(AppError::EspnRequest)?;

        self.deserialize_with_logging::<EspnSummary>(&body, "summary")
    }

    /// Deserialize JSON with detailed error logging using serde_path_to_error
    fn deserialize_with_logging<T: DeserializeOwned>(
        &self,
        body: &str,
        context: &str,
    ) -> Result<T, AppError> {
        let jd = &mut serde_json::Deserializer::from_str(body);

        serde_path_to_error::deserialize(jd).map_err(|err| {
            let path = err.path().to_string();
            let inner = err.inner().to_string();

            // Always log error path and message at ERROR level
            tracing::error!(
                target: "espn::deserialize",
                error_path = %path,
                error_message = %inner,
                context = %context,
                "ESPN API deserialization failed"
            );

            // Log raw JSON at DEBUG level (truncated to avoid log bloat)
            let truncated_body = if body.len() > 10_000 {
                format!(
                    "{}... [truncated, {} total bytes]",
                    &body[..10_000],
                    body.len()
                )
            } else {
                body.to_string()
            };

            tracing::debug!(
                target: "espn::deserialize",
                raw_json = %truncated_body,
                "Raw ESPN response that failed to deserialize"
            );

            AppError::EspnDeserialize {
                path,
                message: inner,
            }
        })
    }

    /// Fetch a single game by event ID from the scoreboard
    pub async fn fetch_game(
        &self,
        sport_league: SportLeague,
        event_id: &str,
    ) -> Result<EspnEvent, AppError> {
        let scoreboard = self.fetch_scoreboard(sport_league).await?;

        scoreboard
            .events
            .into_iter()
            .find(|event| event.id == event_id)
            .ok_or_else(|| AppError::GameNotFound(event_id.to_string()))
    }

    /// Fetch all games from the current scoreboard
    pub async fn fetch_all_games(
        &self,
        sport_league: SportLeague,
    ) -> Result<Vec<EspnEvent>, AppError> {
        let scoreboard = self.fetch_scoreboard(sport_league).await?;
        Ok(scoreboard.events)
    }

    /// Fetch team logo from ESPN CDN as raw PNG bytes
    ///
    /// # Arguments
    /// * `sport_league` - The sport/league for the logo path
    /// * `team_id` - Team abbreviation (e.g., "dal", "nyg")
    /// * `width` - Desired width in pixels
    /// * `height` - Desired height in pixels
    /// * `transparent` - Whether to request transparent background
    pub async fn fetch_logo(
        &self,
        sport_league: SportLeague,
        team_id: &str,
        width: u32,
        height: u32,
        transparent: bool,
    ) -> Result<Bytes, AppError> {
        // Build URL: {logo_url}?img=/i/teamlogos/{logo_path}/500/{team}.png&w={w}&h={h}&transparent={t}
        let url = format!(
            "{}?img=/i/teamlogos/{}/500/{}.png&w={}&h={}&transparent={}",
            self.logo_url,
            sport_league.espn_logo_path(),
            team_id.to_lowercase(),
            width,
            height,
            transparent
        );

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(AppError::ImageFetch)?;

        // Handle 404 from ESPN
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(AppError::TeamNotFound(team_id.to_string()));
        }

        // Check for other errors
        let response = response.error_for_status().map_err(AppError::ImageFetch)?;

        let bytes = response.bytes().await.map_err(AppError::ImageFetch)?;

        Ok(bytes)
    }
}

impl Default for EspnClient {
    fn default() -> Self {
        Self::new(&EspnConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_logo_url_format() {
        let client = EspnClient::default();
        // Just verify the URL is constructed correctly (don't actually fetch)
        let expected_base = "https://a.espncdn.com/combiner/i";
        assert!(client.logo_url.starts_with(expected_base));
    }

    #[test]
    fn test_base_url_default() {
        let client = EspnClient::default();
        assert_eq!(
            client.base_url,
            "https://site.api.espn.com/apis/site/v2/sports"
        );
    }
}
