use axum::{http::HeaderMap, response::Response};

use super::adapter;
use super::types::NbaGame;
use super::wire::encode_game;
use crate::AppState;
use crate::error::{AppError, ErrorResponse};
use crate::espn::league::{self, Nba};
use crate::shared::game::GameListEntry;
use crate::shared::handler;

fn scoreboard_url(state: &AppState) -> String {
    league::scoreboard_url(&state.config.espn, &Nba)
}

/// GET /basketball/nba/games — list today's NBA games with their current state.
#[utoipa::path(
    get,
    path = "/basketball/nba/games",
    responses(
        (status = 200, description = "Today's NBA games with per-game state (pregame/live/final). Binary encoding available via `Accept: application/x-scoreboard-struct` (see crates/scoreboard-wire)", body = Vec<GameListEntry>),
        (status = 304, description = "Game set and states unchanged since client's If-None-Match"),
        (status = 502, description = "ESPN upstream error", body = ErrorResponse),
    ),
    tag = "nba"
)]
pub async fn list_games(state: &AppState, headers: &HeaderMap) -> Result<Response, AppError> {
    let url = scoreboard_url(state);
    let bytes = state.espn_client.fetch_bytes_cached(&url).await?;
    let entries = adapter::list_entries(&bytes, &url)?;
    Ok(handler::list_response(entries, headers))
}

/// GET /basketball/nba/games/{game_id} — state snapshot for one NBA game.
#[utoipa::path(
    get,
    path = "/basketball/nba/games/{game_id}",
    params(("game_id" = String, Path, description = "ESPN event ID")),
    responses(
        (status = 200, description = "Game state (pregame/live/final). Binary encoding available via `Accept: application/x-scoreboard-struct` (see crates/scoreboard-wire)", body = NbaGame),
        (status = 404, description = "Game not on today's scoreboard", body = ErrorResponse),
        (status = 502, description = "ESPN upstream error", body = ErrorResponse),
    ),
    tag = "nba"
)]
pub async fn get_game(
    state: &AppState,
    game_id: &str,
    headers: &HeaderMap,
) -> Result<Response, AppError> {
    let url = scoreboard_url(state);
    let bytes = state.espn_client.fetch_bytes_cached(&url).await?;
    let game = adapter::detail_game(&bytes, game_id, &url)?;
    Ok(handler::game_response(headers, &game, encode_game))
}
