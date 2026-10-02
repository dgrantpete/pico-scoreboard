//! Generic team-logo endpoint: `/{sport}/{league}/teams/{abbrev}/logo`.
//!
//! Logos are payload-resolved: the team's `logo` URL is taken from the
//! league's own scoreboard feed rather than constructed from CDN path
//! conventions — those conventions differ per sport (MLB uses
//! `mlb/500/scoreboard/…`, World Cup teams are country flags under
//! `countries/500/…`, clubs use numeric ids) and ESPN's payload is the
//! ground truth for all of them. The trade-off: a team appears here only
//! while it's on the current scoreboard, which is always true for the
//! firmware's use (it only asks about teams in games it is displaying).

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, Response, StatusCode},
};
use bytes::Bytes;
use scoreboard_espn::common::dark_crest_path;
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::error::{AppError, ErrorResponse};
use crate::espn::EspnClient;
use crate::espn::league::{self, AnyLeague};
use crate::espn::types::{RawScoreboard, parse_events};
use crate::logo::{LogoQuery, build_logo_response};

/// Minimal projection of a scoreboard event for logo resolution — valid for
/// every sport because it touches none of the sport-specific fields.
#[derive(Deserialize)]
struct LogoEvent {
    #[serde(default)]
    competitions: Vec<LogoCompetition>,
}

#[derive(Deserialize)]
struct LogoCompetition {
    #[serde(default)]
    competitors: Vec<LogoCompetitor>,
}

#[derive(Deserialize)]
struct LogoCompetitor {
    team: LogoTeam,
}

#[derive(Deserialize)]
struct LogoTeam {
    abbreviation: String,
    logo: Option<String>,
}

/// Find a team's payload-provided logo URL on the scoreboard.
///
/// Only URLs under the configured ESPN CDN host are honored — anything else
/// (never observed) is treated as the logo being absent, so this can never
/// proxy a foreign host.
fn resolve_team_logo(events: Vec<LogoEvent>, abbrev: &str, cdn_base: &str) -> Option<String> {
    let cdn_prefix = format!("{}/", cdn_base.trim_end_matches('/'));
    events
        .into_iter()
        .flat_map(|e| e.competitions)
        .flat_map(|c| c.competitors)
        .filter(|c| c.team.abbreviation.eq_ignore_ascii_case(abbrev))
        .filter_map(|c| c.team.logo)
        .find(|url| {
            let ok = url.starts_with(&cdn_prefix);
            if !ok {
                tracing::warn!(url = %url, "payload logo URL outside ESPN CDN — ignoring");
            }
            ok
        })
}

/// The artwork the panel shows: ESPN's dark-background variant of the
/// payload's logo, or the payload's own logo when no dark one exists.
///
/// The payload links artwork drawn for white pages, and on the black panel
/// navy and black marks vanish; `dark_crest_path` has the audit behind the
/// switch. Only a 404 on the dark file falls back — it is how the CDN says a
/// team has none (Coventry City, today). Any other failure is an outage, and
/// retrying the default would only hide it.
async fn fetch_crest(
    client: &EspnClient,
    logo_url: &str,
    cdn_base: &str,
) -> Result<Bytes, AppError> {
    if let Some(dark_url) = dark_logo_url(logo_url, cdn_base) {
        match client.fetch_logo(&dark_url).await {
            Err(AppError::ImageFetch(error)) if error.status() == Some(StatusCode::NOT_FOUND) => {
                tracing::debug!(url = %dark_url, "no dark crest variant; serving the default");
            }
            result => return result,
        }
    }
    client.fetch_logo(logo_url).await
}

/// The dark variant's URL, for a logo URL `resolve_team_logo` already
/// confirmed is on the CDN.
fn dark_logo_url(logo_url: &str, cdn_base: &str) -> Option<String> {
    let base = cdn_base.trim_end_matches('/');
    let dark = dark_crest_path(logo_url.strip_prefix(base)?)?;
    Some(format!("{base}{}", dark.as_str()))
}

/// GET /{sport}/{league}/teams/{abbrev}/logo
///
/// Resolves the team's logo from the league's scoreboard payload and returns
/// it in the format negotiated via the Accept header (PNG, PPM, raw RGB888,
/// or raw RGB565).
#[utoipa::path(
    get,
    path = "/{sport}/{league}/teams/{abbrev}/logo",
    params(
        ("sport" = String, Path, description = "ESPN sport slug (e.g. 'baseball')"),
        ("league" = String, Path, description = "ESPN league slug (e.g. 'mlb')"),
        ("abbrev" = String, Path, description = "Team abbreviation (e.g. 'BOS')"),
        LogoQuery
    ),
    responses(
        (status = 200, description = "Logo image", content(
            ("image/png"),
            ("image/x-portable-pixmap"),
            ("image/x-rgb888"),
            ("image/x-rgb565")
        )),
        (status = 400, description = "Invalid parameters", body = ErrorResponse),
        (status = 404, description = "Unknown league, or team not on the current scoreboard", body = ErrorResponse),
        (status = 502, description = "Error fetching from ESPN", body = ErrorResponse),
    ),
    tag = "team"
)]
pub async fn get_team_logo(
    State(state): State<Arc<AppState>>,
    Path((sport, league, abbrev)): Path<(String, String, String)>,
    Query(params): Query<LogoQuery>,
    headers: HeaderMap,
) -> Result<Response<Body>, AppError> {
    let league = AnyLeague::from_path(&sport, &league)?;

    let scoreboard_url = league::scoreboard_url(&state.config.espn, &league);
    let raw: RawScoreboard = state.espn_client.fetch_json_cached(&scoreboard_url).await?;
    let (events, _failed) = parse_events::<LogoEvent>(raw, &scoreboard_url);

    let logo_url = resolve_team_logo(events, &abbrev, &state.config.espn.logo_url)
        .ok_or_else(|| AppError::TeamNotFound(abbrev.clone()))?;

    let cdn_base = &state.config.espn.logo_url;
    let logo_bytes = fetch_crest(&state.espn_client, &logo_url, cdn_base)
        .await
        .map_err(|e| {
            if let AppError::ImageFetch(ref req_err) = e
                && req_err.status() == Some(StatusCode::NOT_FOUND)
            {
                return AppError::TeamNotFound(abbrev.clone());
            }
            e
        })?;

    build_logo_response(&logo_bytes, &params, &headers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(json: &str) -> Vec<LogoEvent> {
        serde_json::from_str(json).expect("test events json parses")
    }

    const CDN: &str = "https://a.espncdn.com";

    #[test]
    fn resolves_logo_case_insensitively() {
        let evs = events(
            r#"[{"competitions":[{"competitors":[
                {"team":{"abbreviation":"POR","logo":"https://a.espncdn.com/i/teamlogos/countries/500/por.png"}},
                {"team":{"abbreviation":"ESP","logo":"https://a.espncdn.com/i/teamlogos/countries/500/esp.png"}}
            ]}]}]"#,
        );
        let url = resolve_team_logo(evs, "por", CDN).unwrap();
        assert_eq!(
            url,
            "https://a.espncdn.com/i/teamlogos/countries/500/por.png"
        );
    }

    #[test]
    fn team_absent_from_scoreboard_resolves_to_none() {
        let evs = events(r#"[{"competitions":[{"competitors":[]}]}]"#);
        assert!(resolve_team_logo(evs, "BOS", CDN).is_none());
    }

    #[test]
    fn foreign_host_logo_is_ignored() {
        let evs = events(
            r#"[{"competitions":[{"competitors":[
                {"team":{"abbreviation":"BOS","logo":"https://evil.example.com/i/teamlogos/mlb/500/bos.png"}}
            ]}]}]"#,
        );
        assert!(resolve_team_logo(evs, "BOS", CDN).is_none());
    }

    #[test]
    fn the_dark_url_rewrites_one_segment_on_the_configured_cdn() {
        assert_eq!(
            dark_logo_url(
                "https://a.espncdn.com/i/teamlogos/mlb/500/scoreboard/nyy.png",
                CDN
            )
            .as_deref(),
            Some("https://a.espncdn.com/i/teamlogos/mlb/500-dark/scoreboard/nyy.png")
        );
        // A trailing slash on the configured base changes nothing.
        assert_eq!(
            dark_logo_url(
                "https://a.espncdn.com/i/teamlogos/countries/500/por.png",
                "https://a.espncdn.com/"
            )
            .as_deref(),
            Some("https://a.espncdn.com/i/teamlogos/countries/500-dark/por.png")
        );
        // No size segment: no variant, and the caller keeps the default.
        assert_eq!(
            dark_logo_url("https://a.espncdn.com/i/teamlogos/mlb/sf.png", CDN),
            None
        );
    }

    /// A local stand-in for the CDN: the Yankees have a dark crest, Coventry
    /// (the one team the audit found without one) 404s on it, and a third
    /// path fails the way an outage does.
    async fn fake_cdn() -> String {
        use axum::{Router, http::StatusCode, routing::get};
        let app = Router::new()
            .route(
                "/i/teamlogos/mlb/500/scoreboard/nyy.png",
                get(|| async { "nyy-default" }),
            )
            .route(
                "/i/teamlogos/mlb/500-dark/scoreboard/nyy.png",
                get(|| async { "nyy-dark" }),
            )
            .route(
                "/i/teamlogos/soccer/500/388.png",
                get(|| async { "cov-default" }),
            )
            .route(
                "/i/teamlogos/nba/500/scoreboard/bos.png",
                get(|| async { "bos-default" }),
            )
            .route(
                "/i/teamlogos/nba/500-dark/scoreboard/bos.png",
                get(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        base
    }

    #[tokio::test]
    async fn the_dark_crest_is_served_where_it_exists_and_the_default_where_it_404s() {
        let cdn = fake_cdn().await;
        let client = EspnClient::new(&crate::config::EspnConfig::default());

        let nyy = format!("{cdn}/i/teamlogos/mlb/500/scoreboard/nyy.png");
        assert_eq!(
            &fetch_crest(&client, &nyy, &cdn).await.unwrap()[..],
            b"nyy-dark"
        );

        let coventry = format!("{cdn}/i/teamlogos/soccer/500/388.png");
        assert_eq!(
            &fetch_crest(&client, &coventry, &cdn).await.unwrap()[..],
            b"cov-default"
        );
    }

    #[tokio::test]
    async fn an_outage_on_the_dark_crest_is_an_error_not_a_quiet_fallback() {
        let cdn = fake_cdn().await;
        let client = EspnClient::new(&crate::config::EspnConfig::default());
        let celtics = format!("{cdn}/i/teamlogos/nba/500/scoreboard/bos.png");
        assert!(matches!(
            fetch_crest(&client, &celtics, &cdn).await,
            Err(AppError::ImageFetch(_))
        ));
    }

    #[test]
    fn missing_logo_field_resolves_to_none() {
        let evs =
            events(r#"[{"competitions":[{"competitors":[{"team":{"abbreviation":"BOS"}}]}]}]"#);
        assert!(resolve_team_logo(evs, "BOS", CDN).is_none());
    }
}
