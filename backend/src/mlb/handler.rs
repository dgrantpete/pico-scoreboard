use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};

use crate::AppState;
use crate::auth::ApiKey;
use crate::error::{AppError, ErrorResponse};
use crate::espn::league::{self, Mlb};
use crate::espn::types::{RawScoreboard, find_event, parse_events};
use crate::shared::etag::compute_games_etag;
use crate::wire;

use super::transform::live_competition_to_game;
use super::types::{EspnCompetition, EspnEvent, LiveGame};

fn scoreboard_url(state: &AppState) -> String {
    league::scoreboard_url(&state.config.espn, &Mlb)
}

/// True when the client asked for the packed binary format (see `wire.rs`).
fn wants_struct(headers: &HeaderMap) -> bool {
    headers.get_all(header::ACCEPT).iter().any(|v| {
        v.to_str()
            .map(|s| s.contains(wire::STRUCT_CONTENT_TYPE))
            .unwrap_or(false)
    })
}

/// Build a 200-or-304 response for `GET /baseball/mlb/games` given the live game IDs,
/// the client's `If-None-Match` header, and the negotiated format.
///
/// The ETag is computed over the game IDs and shared by both representations
/// (with `Vary: Accept`). Strictly RFC-pedantic ETags would differ per
/// representation, but there are no shared caches in this deployment and a
/// given client always requests one format.
fn build_games_response(
    game_ids: Vec<String>,
    if_none_match: Option<&str>,
    use_struct: bool,
) -> Response {
    let etag = compute_games_etag(&game_ids);
    let quoted = format!("\"{}\"", etag);

    if if_none_match == Some(quoted.as_str()) {
        return (
            StatusCode::NOT_MODIFIED,
            [(header::ETAG, quoted.as_str()), (header::VARY, "Accept")],
        )
            .into_response();
    }

    if use_struct {
        (
            [
                (header::ETAG, quoted.as_str()),
                (header::VARY, "Accept"),
                (header::CONTENT_TYPE, wire::STRUCT_CONTENT_TYPE),
            ],
            wire::encode_game_ids(&game_ids),
        )
            .into_response()
    } else {
        (
            [(header::ETAG, quoted.as_str()), (header::VARY, "Accept")],
            Json(game_ids),
        )
            .into_response()
    }
}

/// GET /baseball/mlb/games — list ESPN event IDs whose first competition is currently live.
#[utoipa::path(
    get,
    path = "/baseball/mlb/games",
    responses(
        (status = 200, description = "IDs of currently live MLB games. Binary encoding available via `Accept: application/x-scoreboard-struct` (see backend/src/wire.rs)", body = Vec<String>),
        (status = 304, description = "Game set unchanged since client's If-None-Match"),
        (status = 401, description = "Missing or invalid API key", body = ErrorResponse),
        (status = 502, description = "ESPN upstream error", body = ErrorResponse),
    ),
    security(("api_key" = [])),
    tag = "mlb"
)]
pub async fn list_active_games(
    State(state): State<Arc<AppState>>,
    _auth: ApiKey,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let url = scoreboard_url(&state);
    let raw: RawScoreboard = state.espn_client.fetch_json_cached(&url).await?;
    // Serve whatever parsed: a transient ETag flap to a smaller set beats a 502.
    let (events, _failed) = parse_events::<EspnEvent>(raw, &url);

    let ids: Vec<String> = events
        .into_iter()
        .filter_map(|event| {
            let first = event.competitions.into_iter().next()?;
            matches!(first, EspnCompetition::Live { .. }).then_some(event.id)
        })
        .collect();

    let if_none_match = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok());

    Ok(build_games_response(ids, if_none_match, wants_struct(&headers)))
}

/// GET /baseball/mlb/games/{game_id} — live state snapshot for one MLB game.
#[utoipa::path(
    get,
    path = "/baseball/mlb/games/{game_id}",
    params(("game_id" = String, Path, description = "ESPN event ID")),
    responses(
        (status = 200, description = "Live game state. Binary encoding available via `Accept: application/x-scoreboard-struct` (see backend/src/wire.rs)", body = LiveGame),
        (status = 401, description = "Missing or invalid API key", body = ErrorResponse),
        (status = 404, description = "Game not found or not live", body = ErrorResponse),
        (status = 502, description = "ESPN upstream error", body = ErrorResponse),
    ),
    security(("api_key" = [])),
    tag = "mlb"
)]
pub async fn get_live_game(
    State(state): State<Arc<AppState>>,
    _auth: ApiKey,
    Path(game_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let url = scoreboard_url(&state);
    let raw: RawScoreboard = state.espn_client.fetch_json_cached(&url).await?;
    let (events, failed) = parse_events::<EspnEvent>(raw, &url);
    let event = find_event(events, failed, &game_id, &url, |e| &e.id)?;

    let first = event
        .competitions
        .into_iter()
        .next()
        .ok_or_else(|| AppError::GameNotFound(game_id.clone()))?;

    match first {
        EspnCompetition::Live {
            competitors,
            situation,
            period,
            short_detail,
        } => {
            let game =
                live_competition_to_game(event.id, competitors, situation, period, short_detail)
                    .map_err(|e| e.with_url(&url))?;
            if wants_struct(&headers) {
                Ok((
                    [
                        (header::CONTENT_TYPE, wire::STRUCT_CONTENT_TYPE),
                        (header::VARY, "Accept"),
                    ],
                    wire::encode_live_game(&game),
                )
                    .into_response())
            } else {
                Ok(([(header::VARY, "Accept")], Json(game)).into_response())
            }
        }
        // Live-only contract: pregame data is modeled (`PregameGame`) but not
        // served yet — 404 remains the "nothing to display" signal.
        EspnCompetition::PreGame { .. } | EspnCompetition::Final => {
            Err(AppError::GameNotFound(game_id))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    fn ids(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    fn etag_header(resp: &Response) -> &str {
        resp.headers()
            .get(header::ETAG)
            .expect("etag header present")
            .to_str()
            .unwrap()
    }

    #[tokio::test]
    async fn returns_200_with_etag_when_no_if_none_match() {
        let resp = build_games_response(ids(&["401570001", "401570002"]), None, false);
        assert_eq!(resp.status(), StatusCode::OK);
        let tag = etag_header(&resp).to_string();
        assert!(tag.starts_with('"') && tag.ends_with('"'));
        assert_eq!(tag.len(), 18);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        assert!(!body.is_empty());
    }

    #[tokio::test]
    async fn returns_304_without_body_when_if_none_match_matches() {
        let game_ids = ids(&["401570002", "401570001"]);
        let tag = format!("\"{}\"", compute_games_etag(&game_ids));
        let resp = build_games_response(game_ids, Some(tag.as_str()), false);
        assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(etag_header(&resp), tag);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn returns_200_with_new_etag_after_game_set_changes() {
        let initial = ids(&["401570001", "401570002"]);
        let initial_tag = format!("\"{}\"", compute_games_etag(&initial));

        let changed = ids(&["401570001", "401570003"]);
        let resp = build_games_response(changed, Some(initial_tag.as_str()), false);

        assert_eq!(resp.status(), StatusCode::OK);
        let new_tag = etag_header(&resp).to_string();
        assert_ne!(new_tag, initial_tag);
    }

    #[tokio::test]
    async fn struct_format_returns_binary_body_with_same_etag() {
        let game_ids = ids(&["401570729", "401570001"]);
        let json_tag = {
            let resp = build_games_response(game_ids.clone(), None, false);
            etag_header(&resp).to_string()
        };

        let resp = build_games_response(game_ids, None, true);
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(etag_header(&resp), json_tag);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            wire::STRUCT_CONTENT_TYPE
        );
        assert_eq!(resp.headers().get(header::VARY).unwrap(), "Accept");
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(body[0], wire::WIRE_VERSION);
        assert_eq!(body[1], 2); // count
    }
}
