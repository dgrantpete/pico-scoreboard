use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, header},
    response::Response,
};

use crate::AppState;
use crate::auth::ApiKey;
use crate::error::{AppError, ErrorResponse};
use crate::espn::league::{self, SoccerLeague};
use crate::espn::types::{RawScoreboard, find_event, parse_events};
use crate::shared::etag::json_games_response;

use super::transform::live_competition_to_game;
use super::types::{EspnCompetition, EspnEvent, SoccerGame};

/// GET /soccer/{league}/games — list ESPN event IDs currently in play
/// (halftime counts: the interval is displayable).
#[utoipa::path(
    get,
    path = "/soccer/{league}/games",
    params(("league" = String, Path, description = "ESPN soccer league slug (fifa.world, usa.1, eng.1, mex.1)")),
    responses(
        (status = 200, description = "IDs of currently live games", body = Vec<String>),
        (status = 304, description = "Game set unchanged since client's If-None-Match"),
        (status = 401, description = "Missing or invalid API key", body = ErrorResponse),
        (status = 404, description = "Unknown league", body = ErrorResponse),
        (status = 502, description = "ESPN upstream error", body = ErrorResponse),
    ),
    security(("api_key" = [])),
    tag = "soccer"
)]
pub async fn list_active_games(
    State(state): State<Arc<AppState>>,
    _auth: ApiKey,
    Path(league): Path<String>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let league = SoccerLeague::from_path(&league)?;
    let url = league::scoreboard_url(&state.config.espn, &league);
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

    Ok(json_games_response(ids, if_none_match))
}

/// GET /soccer/{league}/games/{game_id} — live state snapshot for one game.
#[utoipa::path(
    get,
    path = "/soccer/{league}/games/{game_id}",
    params(
        ("league" = String, Path, description = "ESPN soccer league slug (fifa.world, usa.1, eng.1, mex.1)"),
        ("game_id" = String, Path, description = "ESPN event ID"),
    ),
    responses(
        (status = 200, description = "Live game state", body = SoccerGame),
        (status = 401, description = "Missing or invalid API key", body = ErrorResponse),
        (status = 404, description = "Unknown league, or game not found / not live", body = ErrorResponse),
        (status = 502, description = "ESPN upstream error", body = ErrorResponse),
    ),
    security(("api_key" = [])),
    tag = "soccer"
)]
pub async fn get_live_game(
    State(state): State<Arc<AppState>>,
    _auth: ApiKey,
    Path((league, game_id)): Path<(String, String)>,
) -> Result<Json<SoccerGame>, AppError> {
    let league = SoccerLeague::from_path(&league)?;
    let url = league::scoreboard_url(&state.config.espn, &league);
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
            display_clock,
            period,
            halftime,
            details,
        } => {
            let game = live_competition_to_game(
                event.id,
                competitors,
                display_clock,
                period,
                halftime,
                details,
            )
            .map_err(|e| e.with_url(&url))?;
            Ok(Json(game))
        }
        // Live-only contract, matching MLB: pregame is modeled
        // (`SoccerGame::Pregame`) but not served yet.
        EspnCompetition::PreGame { .. } | EspnCompetition::Final => {
            Err(AppError::GameNotFound(game_id))
        }
    }
}
