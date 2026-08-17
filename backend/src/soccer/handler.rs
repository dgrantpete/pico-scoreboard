use axum::{http::HeaderMap, response::Response};

use super::adapter;
use super::types::SoccerGame;
use super::wire::encode_game;
use crate::AppState;
use crate::error::{AppError, ErrorResponse};
use crate::espn::league::{self, SoccerLeague};
use crate::shared::game::GameListEntry;
use crate::shared::handler;

use scoreboard_espn::soccer::{CommentaryExtract, SoccerExtract};

/// Latest commentary line for a live game, best-effort: commentary is polish,
/// so a summary fetch/parse failure degrades to None (with a warning) rather
/// than failing the live payload.
async fn fetch_commentary(
    state: &AppState,
    league: &SoccerLeague,
    event_id: &str,
) -> Option<CommentaryExtract> {
    let url = league::summary_url(&state.config.espn, league, event_id);
    match state.espn_client.fetch_bytes_cached(&url).await {
        Ok(bytes) => adapter::summary_commentary(&bytes, &url),
        Err(e) => {
            tracing::warn!(url = %url, error = ?e, "soccer summary fetch failed; serving live without commentary");
            None
        }
    }
}

/// GET /soccer/{league}/games — today's games for one league with their
/// current state (pregame/live/final), like the MLB list.
#[utoipa::path(
    get,
    path = "/soccer/{league}/games",
    params(("league" = String, Path, description = "ESPN soccer league slug (fifa.world, usa.1, eng.1, mex.1)")),
    responses(
        (status = 200, description = "Today's games with per-game state. Binary encoding available via `Accept: application/x-scoreboard-struct` (see crates/scoreboard-wire)", body = Vec<GameListEntry>),
        (status = 304, description = "Game set and states unchanged since client's If-None-Match"),
        (status = 404, description = "Unknown league", body = ErrorResponse),
        (status = 502, description = "ESPN upstream error", body = ErrorResponse),
    ),
    tag = "soccer"
)]
pub async fn list_games(
    state: &AppState,
    league: SoccerLeague,
    headers: &HeaderMap,
) -> Result<Response, AppError> {
    let url = league::scoreboard_url(&state.config.espn, &league);
    let bytes = state.espn_client.fetch_bytes_cached(&url).await?;
    let entries = adapter::list_entries(&bytes, &url)?;
    Ok(handler::list_response(entries, headers))
}

/// GET /soccer/{league}/games/{game_id} — state snapshot for one game.
#[utoipa::path(
    get,
    path = "/soccer/{league}/games/{game_id}",
    params(
        ("league" = String, Path, description = "ESPN soccer league slug (fifa.world, usa.1, eng.1, mex.1)"),
        ("game_id" = String, Path, description = "ESPN event ID"),
    ),
    responses(
        (status = 200, description = "Game state (pregame/live/final). Binary encoding available via `Accept: application/x-scoreboard-struct` (see crates/scoreboard-wire)", body = SoccerGame),
        (status = 404, description = "Unknown league, or game not on today's scoreboard", body = ErrorResponse),
        (status = 502, description = "ESPN upstream error", body = ErrorResponse),
    ),
    tag = "soccer"
)]
pub async fn get_game(
    state: &AppState,
    league: SoccerLeague,
    game_id: &str,
    headers: &HeaderMap,
) -> Result<Response, AppError> {
    let url = league::scoreboard_url(&state.config.espn, &league);
    let bytes = state.espn_client.fetch_bytes_cached(&url).await?;
    let extract = adapter::detail_extract(&bytes, game_id, &url)?;

    // Live games get the per-event summary's latest commentary line;
    // pregame/final have no commentary slot.
    let commentary = if matches!(extract, SoccerExtract::Live(_)) {
        fetch_commentary(state, &league, game_id).await
    } else {
        None
    };

    let game = adapter::game_from_extract(&extract, commentary);
    Ok(handler::game_response(headers, &game, encode_game))
}
