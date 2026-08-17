use axum::{http::HeaderMap, response::Response};

use super::adapter;
use super::types::MlbGame;
use super::wire::encode_game;
use crate::AppState;
use crate::error::{AppError, ErrorResponse};
use crate::espn::league::{self, Mlb};
use crate::shared::game::GameListEntry;
use crate::shared::handler;

fn scoreboard_url(state: &AppState) -> String {
    league::scoreboard_url(&state.config.espn, &Mlb)
}

/// GET /baseball/mlb/games — list today's MLB games with their current state.
///
/// A rain-delayed live game (non-inning `shortDetail`) is excluded so the
/// firmware never advertises a live game it can't then fetch (the veto lives
/// in `scoreboard-espn::mlb`).
#[utoipa::path(
    get,
    path = "/baseball/mlb/games",
    responses(
        (status = 200, description = "Today's MLB games with per-game state (pregame/live/final). Binary encoding available via `Accept: application/x-scoreboard-struct` (see crates/scoreboard-wire)", body = Vec<GameListEntry>),
        (status = 304, description = "Game set and states unchanged since client's If-None-Match"),
        (status = 502, description = "ESPN upstream error", body = ErrorResponse),
    ),
    tag = "mlb"
)]
pub async fn list_games(state: &AppState, headers: &HeaderMap) -> Result<Response, AppError> {
    let url = scoreboard_url(state);
    let bytes = state.espn_client.fetch_bytes_cached(&url).await?;
    let entries = adapter::list_entries(&bytes, &url)?;
    Ok(handler::list_response(entries, headers))
}

/// GET /baseball/mlb/games/{game_id} — state snapshot for one MLB game.
#[utoipa::path(
    get,
    path = "/baseball/mlb/games/{game_id}",
    params(("game_id" = String, Path, description = "ESPN event ID")),
    responses(
        (status = 200, description = "Game state (pregame/live/final). Binary encoding available via `Accept: application/x-scoreboard-struct` (see crates/scoreboard-wire)", body = MlbGame),
        (status = 404, description = "Not on today's scoreboard, or a non-displayable delay", body = ErrorResponse),
        (status = 502, description = "ESPN upstream error", body = ErrorResponse),
    ),
    tag = "mlb"
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
