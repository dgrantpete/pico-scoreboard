use bytes::Bytes;
use lru::LruCache;
use reqwest::Client;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config::EspnConfig;
use crate::error::AppError;

/// Maximum number of native-resolution logos to cache in memory.
const LOGO_CACHE_CAPACITY: usize = 64;

/// How long a cached JSON response stays fresh. ESPN's scoreboard feed is the
/// same URL for every game and every device, so a poll cycle across a fleet
/// of Picos collapses into (at most) one upstream fetch per TTL window.
const JSON_CACHE_TTL: Duration = Duration::from_secs(5);

/// HTTP client for ESPN with in-memory logo and JSON caches.
#[derive(Debug, Clone)]
pub struct EspnClient {
    client: Client,
    logo_cache: Arc<Mutex<LruCache<String, Bytes>>>,
    /// Single-slot TTL cache of the last JSON response: (fetched_at, url, body).
    json_cache: Arc<Mutex<Option<(Instant, String, Bytes)>>>,
}

impl EspnClient {
    /// Create a new ESPN client with configured timeout and user-agent.
    pub fn new(config: &EspnConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .user_agent(&config.user_agent)
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            logo_cache: Arc::new(Mutex::new(LruCache::new(
                NonZeroUsize::new(LOGO_CACHE_CAPACITY).unwrap(),
            ))),
            json_cache: Arc::new(Mutex::new(None)),
        }
    }

    /// Fetch a team logo from an ESPN CDN URL as raw PNG bytes.
    ///
    /// Results are cached in an LRU cache keyed by the URL, so repeated
    /// requests for the same logo avoid redundant ESPN CDN round-trips.
    /// The caller is responsible for constructing the full upstream URL.
    pub async fn fetch_logo(&self, upstream_url: &str) -> Result<Bytes, AppError> {
        if let Some(cached) = self.logo_cache.lock().unwrap().get(upstream_url) {
            return Ok(cached.clone());
        }

        let response = self
            .client
            .get(upstream_url)
            .send()
            .await
            .map_err(AppError::ImageFetch)?;

        let response = response.error_for_status().map_err(AppError::ImageFetch)?;
        let bytes = response.bytes().await.map_err(AppError::ImageFetch)?;

        self.logo_cache
            .lock()
            .unwrap()
            .put(upstream_url.to_string(), bytes.clone());

        Ok(bytes)
    }

    /// Fetch JSON from an ESPN endpoint, deserializing into `T`.
    ///
    /// Serves as the single choke-point for all ESPN JSON deserialization so
    /// structured error logging (including the exact JSON path of a failure
    /// and the full raw payload) is emitted uniformly across every league.
    pub async fn fetch_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
    ) -> Result<T, AppError> {
        let bytes = self.fetch_json_bytes(url).await?;
        Self::deserialize_logged(url, &bytes)
    }

    /// Like `fetch_json`, but serves from a short-TTL cache keyed by URL.
    ///
    /// Use for endpoints that many clients poll for identical data (the MLB
    /// scoreboard). Each cache hit still deserializes — cheap next to the
    /// upstream round-trip it saves.
    pub async fn fetch_json_cached<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
    ) -> Result<T, AppError> {
        {
            let cache = self.json_cache.lock().unwrap();
            if let Some((fetched_at, cached_url, bytes)) = cache.as_ref()
                && cached_url == url
                && fetched_at.elapsed() < JSON_CACHE_TTL
            {
                return Self::deserialize_logged(url, bytes);
            }
        }

        let bytes = self.fetch_json_bytes(url).await?;
        let value = Self::deserialize_logged(url, &bytes)?;
        *self.json_cache.lock().unwrap() = Some((Instant::now(), url.to_string(), bytes));
        Ok(value)
    }

    async fn fetch_json_bytes(&self, url: &str) -> Result<Bytes, AppError> {
        let resp = self.client.get(url).send().await.map_err(|e| {
            tracing::error!(url = %url, error = %e, "ESPN request failed");
            AppError::EspnRequest(e)
        })?;

        let http_status = resp.status();
        let resp = resp.error_for_status().map_err(|e| {
            tracing::error!(url = %url, http_status = %http_status, error = %e, "ESPN returned non-success status");
            AppError::EspnRequest(e)
        })?;

        resp.bytes().await.map_err(|e| {
            tracing::error!(url = %url, error = %e, "ESPN body read failed");
            AppError::EspnRequest(e)
        })
    }

    fn deserialize_logged<T: serde::de::DeserializeOwned>(
        url: &str,
        bytes: &Bytes,
    ) -> Result<T, AppError> {
        let de = &mut serde_json::Deserializer::from_slice(bytes);
        match serde_path_to_error::deserialize::<_, T>(de) {
            Ok(value) => Ok(value),
            Err(err) => {
                let json_path = err.path().to_string();
                let inner = err.into_inner();
                tracing::error!(
                    url = %url,
                    json_path = %json_path,
                    error = %inner,
                    payload_bytes = bytes.len(),
                    payload = %String::from_utf8_lossy(bytes),
                    "ESPN JSON deserialization failed"
                );
                Err(AppError::EspnDeserialize {
                    url: url.to_string(),
                    json_path,
                    message: inner.to_string(),
                })
            }
        }
    }
}

impl Default for EspnClient {
    fn default() -> Self {
        Self::new(&EspnConfig::default())
    }
}
