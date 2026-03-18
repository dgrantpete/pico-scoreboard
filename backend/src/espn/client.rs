use bytes::Bytes;
use lru::LruCache;
use reqwest::Client;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::OnceCell;

use super::types::{EspnEvent, EspnScoreboard, EspnSummary, EspnTeamsListResponse};
use crate::config::EspnConfig;
use crate::error::AppError;
use crate::sport::EspnLeague;

/// Maximum number of 500x500 logos to cache in memory.
/// Covers all NFL (32) + NBA (30) teams with room for college logos.
const LOGO_CACHE_CAPACITY: usize = 64;

/// Maximum number of pages to fetch when building the college team abbreviation map.
const MAX_TEAM_LIST_PAGES: usize = 20;

/// HTTP client for ESPN API requests
#[derive(Debug, Clone)]
pub struct EspnClient {
    client: Client,
    base_url: String,
    logo_url: String,
    logo_cache: Arc<Mutex<LruCache<String, Bytes>>>,
    /// Cached abbreviation/ID → logo URL map for NCAAF teams
    ncaaf_abbrev_map: Arc<OnceCell<HashMap<String, String>>>,
    /// Cached abbreviation/ID → logo URL map for NCAAB teams
    ncaab_abbrev_map: Arc<OnceCell<HashMap<String, String>>>,
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
            ncaaf_abbrev_map: Arc::new(OnceCell::new()),
            ncaab_abbrev_map: Arc::new(OnceCell::new()),
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

    /// Resolve a college team identifier (abbreviation or numeric ID) to its ESPN logo URL.
    ///
    /// Uses a cached map built from ESPN's teams list endpoint. The map is populated
    /// once per league on first request and reused for all subsequent lookups.
    async fn resolve_college_logo_url(
        &self,
        league: &impl EspnLeague,
        team_id: &str,
    ) -> Result<String, AppError> {
        let map = self.get_college_team_map(league).await?;
        let key = team_id.to_lowercase();

        map.get(&key)
            .cloned()
            .ok_or_else(|| AppError::TeamNotFound(team_id.to_string()))
    }

    /// Get or initialize the college team abbreviation→logo URL map for a league.
    async fn get_college_team_map(
        &self,
        league: &impl EspnLeague,
    ) -> Result<&HashMap<String, String>, AppError> {
        let cell = if league.espn_league() == "college-football" {
            &self.ncaaf_abbrev_map
        } else {
            &self.ncaab_abbrev_map
        };

        cell.get_or_try_init(|| self.fetch_college_team_map(league))
            .await
    }

    /// Fetch all college teams for a league by paginating through ESPN's teams list API.
    ///
    /// Builds a HashMap mapping both lowercase abbreviation and numeric ID to the
    /// team's first logo URL. Paginates with `limit=500&page=N` until an empty page
    /// is returned (capped at `MAX_TEAM_LIST_PAGES` pages).
    async fn fetch_college_team_map(
        &self,
        league: &impl EspnLeague,
    ) -> Result<HashMap<String, String>, AppError> {
        let mut map = HashMap::new();

        for page in 1..=MAX_TEAM_LIST_PAGES {
            let url = format!(
                "{}/{}/{}/teams?limit=500&page={}",
                self.base_url,
                league.espn_sport(),
                league.espn_league(),
                page
            );

            let response = self
                .client
                .get(&url)
                .send()
                .await
                .map_err(AppError::EspnRequest)?;

            let body = response.text().await.map_err(AppError::EspnRequest)?;
            let teams_response: EspnTeamsListResponse =
                self.deserialize_with_logging(&body, "teams_list")?;

            let mut page_had_teams = false;

            for sport in &teams_response.sports {
                for league_entry in &sport.leagues {
                    for entry in &league_entry.teams {
                        page_had_teams = true;
                        if let Some(logo) = entry.team.logos.first() {
                            let logo_url = logo.href.clone();
                            // Map by lowercase abbreviation (skip teams without one)
                            if let Some(ref abbrev) = entry.team.abbreviation {
                                map.insert(abbrev.to_lowercase(), logo_url.clone());
                            }
                            // Also map by numeric ID
                            map.insert(entry.team.id.clone(), logo_url);
                        }
                    }
                }
            }

            if !page_had_teams {
                break;
            }
        }

        tracing::info!(
            sport = league.espn_sport(),
            league = league.espn_league(),
            team_count = map.len(),
            "Built college team abbreviation map"
        );

        Ok(map)
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

    use crate::sport::{BasketballLeague, FootballLeague};

    /// PNG files start with these 4 magic bytes.
    const PNG_MAGIC: [u8; 4] = [0x89, 0x50, 0x4E, 0x47];

    fn assert_png(bytes: &[u8], label: &str) {
        assert!(
            bytes.len() > 4,
            "{label}: expected non-empty PNG but got {} bytes",
            bytes.len()
        );
        assert_eq!(
            &bytes[..4],
            &PNG_MAGIC,
            "{label}: first 4 bytes are not PNG magic"
        );
    }

    // ── NFL (pro) ──

    #[tokio::test]
    #[ignore]
    async fn nfl_logo_dal() {
        let client = EspnClient::default();
        let bytes = client
            .fetch_logo(FootballLeague::Nfl, "dal")
            .await
            .expect("failed to fetch DAL logo");
        assert_png(&bytes, "NFL DAL");
    }

    #[tokio::test]
    #[ignore]
    async fn nfl_logo_kc() {
        let client = EspnClient::default();
        let bytes = client
            .fetch_logo(FootballLeague::Nfl, "kc")
            .await
            .expect("failed to fetch KC logo");
        assert_png(&bytes, "NFL KC");
    }

    // ── NBA (pro) ──

    #[tokio::test]
    #[ignore]
    async fn nba_logo_lal() {
        let client = EspnClient::default();
        let bytes = client
            .fetch_logo(BasketballLeague::Nba, "lal")
            .await
            .expect("failed to fetch LAL logo");
        assert_png(&bytes, "NBA LAL");
    }

    #[tokio::test]
    #[ignore]
    async fn nba_logo_bos() {
        let client = EspnClient::default();
        let bytes = client
            .fetch_logo(BasketballLeague::Nba, "bos")
            .await
            .expect("failed to fetch BOS logo");
        assert_png(&bytes, "NBA BOS");
    }

    // ── NCAAF (college football) ──

    #[tokio::test]
    #[ignore]
    async fn ncaaf_logo_by_abbreviation() {
        let client = EspnClient::default();
        let bytes = client
            .fetch_logo(FootballLeague::Ncaaf, "usu")
            .await
            .expect("failed to fetch NCAAF USU logo");
        assert_png(&bytes, "NCAAF USU");
    }

    #[tokio::test]
    #[ignore]
    async fn ncaaf_logo_by_abbreviation_clemson() {
        let client = EspnClient::default();
        let bytes = client
            .fetch_logo(FootballLeague::Ncaaf, "clem")
            .await
            .expect("failed to fetch NCAAF CLEM logo");
        assert_png(&bytes, "NCAAF CLEM");
    }

    #[tokio::test]
    #[ignore]
    async fn ncaaf_logo_by_numeric_id() {
        let client = EspnClient::default();
        let bytes = client
            .fetch_logo(FootballLeague::Ncaaf, "328")
            .await
            .expect("failed to fetch NCAAF team 328 (Utah State) logo");
        assert_png(&bytes, "NCAAF 328");
    }

    #[tokio::test]
    #[ignore]
    async fn ncaaf_logo_case_insensitive() {
        let client = EspnClient::default();
        for variant in ["USU", "usu", "Usu"] {
            let bytes = client
                .fetch_logo(FootballLeague::Ncaaf, variant)
                .await
                .unwrap_or_else(|e| panic!("failed to fetch NCAAF '{variant}': {e:?}"));
            assert_png(&bytes, &format!("NCAAF {variant}"));
        }
    }

    // ── NCAAB (college basketball) ──

    #[tokio::test]
    #[ignore]
    async fn ncaab_logo_by_abbreviation() {
        let client = EspnClient::default();
        let bytes = client
            .fetch_logo(BasketballLeague::Ncaab, "duke")
            .await
            .expect("failed to fetch NCAAB DUKE logo");
        assert_png(&bytes, "NCAAB DUKE");
    }

    #[tokio::test]
    #[ignore]
    async fn ncaab_logo_by_abbreviation_usu() {
        let client = EspnClient::default();
        let bytes = client
            .fetch_logo(BasketballLeague::Ncaab, "usu")
            .await
            .expect("failed to fetch NCAAB USU logo");
        assert_png(&bytes, "NCAAB USU");
    }

    #[tokio::test]
    #[ignore]
    async fn ncaab_logo_by_numeric_id() {
        let client = EspnClient::default();
        let bytes = client
            .fetch_logo(BasketballLeague::Ncaab, "150")
            .await
            .expect("failed to fetch NCAAB team 150 (Duke) logo");
        assert_png(&bytes, "NCAAB 150");
    }

    #[tokio::test]
    #[ignore]
    async fn ncaab_logo_case_insensitive() {
        let client = EspnClient::default();
        for variant in ["DUKE", "duke", "Duke"] {
            let bytes = client
                .fetch_logo(BasketballLeague::Ncaab, variant)
                .await
                .unwrap_or_else(|e| panic!("failed to fetch NCAAB '{variant}': {e:?}"));
            assert_png(&bytes, &format!("NCAAB {variant}"));
        }
    }

    // ── Error cases ──

    #[tokio::test]
    #[ignore]
    async fn college_logo_invalid_team() {
        let client = EspnClient::default();
        let result = client
            .fetch_logo(FootballLeague::Ncaaf, "zzzzzzz")
            .await;
        assert!(
            result.is_err(),
            "expected error for invalid NCAAF team 'zzzzzzz'"
        );
    }

    #[tokio::test]
    #[ignore]
    async fn college_logo_invalid_team_ncaab() {
        let client = EspnClient::default();
        let result = client
            .fetch_logo(BasketballLeague::Ncaab, "zzzzzzz")
            .await;
        assert!(
            result.is_err(),
            "expected error for invalid NCAAB team 'zzzzzzz'"
        );
    }
}
