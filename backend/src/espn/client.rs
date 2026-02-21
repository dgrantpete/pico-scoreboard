use bytes::Bytes;
use lru::LruCache;
use reqwest::Client;
use serde::de::DeserializeOwned;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::types::{EspnEvent, EspnScoreboard, EspnSummary};
use crate::config::EspnConfig;
use crate::error::AppError;
use crate::sport::EspnLeague;

/// Maximum number of 500x500 logos to cache in memory.
/// Covers all NFL (32) + NBA (30) teams with room for college logos.
const LOGO_CACHE_CAPACITY: usize = 64;

/// HTTP client for ESPN API requests
#[derive(Debug, Clone)]
pub struct EspnClient {
    client: Client,
    base_url: String,
    logo_url: String,
    logo_cache: Arc<Mutex<LruCache<String, Bytes>>>,
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
            logo_cache: Arc::new(Mutex::new(LruCache::new(
                NonZeroUsize::new(LOGO_CACHE_CAPACITY).unwrap(),
            ))),
        }
    }

    /// Fetch the full scoreboard from ESPN for a given sport/league
    pub async fn fetch_scoreboard(
        &self,
        league: impl EspnLeague,
    ) -> Result<EspnScoreboard, AppError> {
        let url = format!(
            "{}/{}/{}/scoreboard",
            self.base_url,
            league.espn_sport(),
            league.espn_league()
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
        league: impl EspnLeague,
        event_id: &str,
    ) -> Result<EspnSummary, AppError> {
        let url = format!(
            "{}/{}/{}/summary?event={}",
            self.base_url,
            league.espn_sport(),
            league.espn_league(),
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
        league: impl EspnLeague,
        event_id: &str,
    ) -> Result<EspnEvent, AppError> {
        let scoreboard = self.fetch_scoreboard(league).await?;

        scoreboard
            .events
            .into_iter()
            .find(|event| event.id == event_id)
            .ok_or_else(|| AppError::GameNotFound(event_id.to_string()))
    }

    /// Fetch all games from the current scoreboard
    pub async fn fetch_all_games(
        &self,
        league: impl EspnLeague,
    ) -> Result<Vec<EspnEvent>, AppError> {
        let scoreboard = self.fetch_scoreboard(league).await?;
        Ok(scoreboard.events)
    }

    /// Fetch native 500x500 team logo from ESPN CDN as raw PNG bytes.
    ///
    /// Results are cached in an LRU cache to avoid redundant ESPN CDN requests.
    /// For pro leagues (NFL, NBA), fetches directly from the CDN using the team abbreviation.
    /// For college leagues (NCAAF, NCAAB), first resolves the abbreviation to a logo URL
    /// via ESPN's teams API, since the CDN uses numeric team IDs for college.
    pub async fn fetch_logo(
        &self,
        league: impl EspnLeague,
        team_id: &str,
    ) -> Result<Bytes, AppError> {
        let cache_key = format!("{}/{}", league.espn_logo_path(), team_id.to_lowercase());

        // Check cache first
        if let Some(cached) = self.logo_cache.lock().unwrap().get(&cache_key) {
            return Ok(cached.clone());
        }

        let url = if league.is_college() {
            self.resolve_college_logo_url(&league, team_id).await?
        } else {
            format!(
                "{}/i/teamlogos/{}/500/{}.png",
                self.logo_url,
                league.espn_logo_path(),
                team_id.to_lowercase(),
            )
        };

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

        // Cache the result
        self.logo_cache
            .lock()
            .unwrap()
            .put(cache_key, bytes.clone());

        Ok(bytes)
    }

    /// Resolve a college team abbreviation to its ESPN logo URL via the teams API.
    ///
    /// ESPN's CDN uses numeric team IDs for college logos (e.g., ncaa/500/228.png),
    /// not abbreviations. This method looks up the team by abbreviation to get the
    /// correct logo URL.
    async fn resolve_college_logo_url(
        &self,
        league: &impl EspnLeague,
        team_id: &str,
    ) -> Result<String, AppError> {
        let url = format!(
            "{}/{}/{}/teams/{}",
            self.base_url,
            league.espn_sport(),
            league.espn_league(),
            team_id.to_lowercase()
        );

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(AppError::EspnRequest)?;

        if !response.status().is_success() {
            return Err(AppError::TeamNotFound(team_id.to_string()));
        }

        let body = response.text().await.map_err(AppError::EspnRequest)?;
        let team_response: super::types::EspnTeamLookup =
            self.deserialize_with_logging(&body, "team_lookup")?;

        team_response
            .team
            .logos
            .into_iter()
            .next()
            .map(|logo| logo.href)
            .ok_or_else(|| AppError::TeamNotFound(team_id.to_string()))
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
        let expected_base = "https://a.espncdn.com";
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
