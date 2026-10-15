use axum::{http::HeaderMap, response::Response};

use super::adapter;
use super::types::HockeyGame;
use super::wire::encode_game;
use crate::AppState;
use crate::error::{AppError, ErrorResponse};
use crate::espn::league::{self, Nhl};
use crate::shared::game::GameListEntry;
use crate::shared::handler;

use scoreboard_espn::hockey::{Extract, Summary};

fn scoreboard_url(state: &AppState) -> String {
    league::scoreboard_url(&state.config.espn, &Nhl)
}

/// The per-event summary's facts, best-effort: they are what the scoreboard
/// cannot say (strength, goals, the shootout), but the game is still worth
/// showing without them, so a fetch or parse failure degrades to the
/// scoreboard alone (with a warning) rather than failing the payload.
async fn fetch_summary(state: &AppState, extract: &Extract, game_id: &str) -> Option<Summary> {
    let url = league::summary_url(&state.config.espn, &Nhl, game_id);
    match state.espn_client.fetch_bytes_cached(&url).await {
        Ok(bytes) => adapter::summary(&bytes, extract, &url),
        Err(e) => {
            tracing::warn!(url = %url, error = ?e, "hockey summary fetch failed; serving the scoreboard alone");
            None
        }
    }
}

/// GET /hockey/nhl/games — list today's NHL games with their current state.
#[utoipa::path(
    get,
    path = "/hockey/nhl/games",
    responses(
        (status = 200, description = "Today's NHL games with per-game state (pregame/live/final). Binary encoding available via `Accept: application/x-scoreboard-struct` (see crates/scoreboard-wire)", body = Vec<GameListEntry>),
        (status = 304, description = "Game set and states unchanged since client's If-None-Match"),
        (status = 502, description = "ESPN upstream error", body = ErrorResponse),
    ),
    tag = "hockey"
)]
pub async fn list_games(state: &AppState, headers: &HeaderMap) -> Result<Response, AppError> {
    let url = scoreboard_url(state);
    let bytes = state.espn_client.fetch_bytes_cached(&url).await?;
    let entries = adapter::list_entries(&bytes, &url)?;
    Ok(handler::list_response(entries, headers))
}

/// GET /hockey/nhl/games/{game_id} — state snapshot for one NHL game.
#[utoipa::path(
    get,
    path = "/hockey/nhl/games/{game_id}",
    params(("game_id" = String, Path, description = "ESPN event ID")),
    responses(
        (status = 200, description = "Game state (pregame/live/final). Live and final games carry the per-event summary's facts when it could be fetched. Binary encoding available via `Accept: application/x-scoreboard-struct` (see crates/scoreboard-wire)", body = HockeyGame),
        (status = 404, description = "Game not on today's scoreboard", body = ErrorResponse),
        (status = 502, description = "ESPN upstream error", body = ErrorResponse),
    ),
    tag = "hockey"
)]
pub async fn get_game(
    state: &AppState,
    game_id: &str,
    headers: &HeaderMap,
) -> Result<Response, AppError> {
    let url = scoreboard_url(state);
    let bytes = state.espn_client.fetch_bytes_cached(&url).await?;
    let extract = adapter::detail_extract(&bytes, game_id, &url)?;

    let summary = if adapter::wants_summary(&extract) {
        fetch_summary(state, &extract, game_id).await
    } else {
        None
    };

    let game = adapter::game_from_extract(&extract, summary.as_ref());
    Ok(handler::game_response(headers, &game, encode_game))
}
