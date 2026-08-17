use axum::{http::HeaderMap, response::Response};

use super::adapter;
use super::types::FootballGame;
use super::wire::encode_game;
use crate::AppState;
use crate::error::{AppError, ErrorResponse};
use crate::espn::league::{self, FootballLeague};
use crate::shared::game::GameListEntry;
use crate::shared::handler;

/// GET /football/{league}/games — today's games for one league with their
/// current state (pregame/live/final), like the soccer list.
///
/// For `college-football` this is ESPN's **default (Top-25) scoreboard**, a
/// deliberate choice: the full FBS slate (`?groups=80`) is 60+ games on a
/// Saturday and would swamp the device's rotation, so the poller follows
/// whatever ESPN's ranked-team default returns.
#[utoipa::path(
    get,
    path = "/football/{league}/games",
    params(("league" = String, Path, description = "ESPN football league slug (nfl, college-football)")),
    responses(
        (status = 200, description = "Today's games with per-game state. Binary encoding available via `Accept: application/x-scoreboard-struct` (see crates/scoreboard-wire)", body = Vec<GameListEntry>),
        (status = 304, description = "Game set and states unchanged since client's If-None-Match"),
        (status = 404, description = "Unknown league", body = ErrorResponse),
        (status = 502, description = "ESPN upstream error", body = ErrorResponse),
    ),
    tag = "football"
)]
pub async fn list_games(
    state: &AppState,
    league: FootballLeague,
    headers: &HeaderMap,
) -> Result<Response, AppError> {
    let url = league::scoreboard_url(&state.config.espn, &league);
    let bytes = state.espn_client.fetch_bytes_cached(&url).await?;
    let entries = adapter::list_entries(&bytes, &url)?;
    Ok(handler::list_response(entries, headers))
}

/// GET /football/{league}/games/{game_id} — state snapshot for one game.
#[utoipa::path(
    get,
    path = "/football/{league}/games/{game_id}",
    params(
        ("league" = String, Path, description = "ESPN football league slug (nfl, college-football)"),
        ("game_id" = String, Path, description = "ESPN event ID"),
    ),
    responses(
        (status = 200, description = "Game state (pregame/live/final). Binary encoding available via `Accept: application/x-scoreboard-struct` (see crates/scoreboard-wire)", body = FootballGame),
        (status = 404, description = "Unknown league, or game not on today's scoreboard", body = ErrorResponse),
        (status = 502, description = "ESPN upstream error", body = ErrorResponse),
    ),
    tag = "football"
)]
pub async fn get_game(
    state: &AppState,
    league: FootballLeague,
    game_id: &str,
    headers: &HeaderMap,
) -> Result<Response, AppError> {
    let url = league::scoreboard_url(&state.config.espn, &league);
    let bytes = state.espn_client.fetch_bytes_cached(&url).await?;
    let game = adapter::detail_game(&bytes, game_id, league.is_college(), &url)?;
    Ok(handler::game_response(headers, &game, encode_game))
}
