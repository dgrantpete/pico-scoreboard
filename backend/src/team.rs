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
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::ApiKey;
use crate::error::{AppError, ErrorResponse};
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
        (status = 401, description = "Missing or invalid API key", body = ErrorResponse),
        (status = 404, description = "Unknown league, or team not on the current scoreboard", body = ErrorResponse),
        (status = 502, description = "Error fetching from ESPN", body = ErrorResponse),
    ),
    security(("api_key" = [])),
    tag = "team"
)]
pub async fn get_team_logo(
    State(state): State<Arc<AppState>>,
    _auth: ApiKey,
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

    let logo_bytes = state.espn_client.fetch_logo(&logo_url).await.map_err(|e| {
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
        assert_eq!(url, "https://a.espncdn.com/i/teamlogos/countries/500/por.png");
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
    fn missing_logo_field_resolves_to_none() {
        let evs = events(
            r#"[{"competitions":[{"competitors":[{"team":{"abbreviation":"BOS"}}]}]}]"#,
        );
        assert!(resolve_team_logo(evs, "BOS", CDN).is_none());
    }
}
