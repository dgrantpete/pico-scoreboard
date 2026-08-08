use axum::{http::HeaderMap, response::Response};

use super::wire::encode_game;
use crate::AppState;
use crate::error::{AppError, ErrorResponse};
use crate::espn::league::{self, Nba};
use crate::shared::game::{GameListEntry, GameState};
use crate::shared::handler::{self, EventParts};

use super::transform::{
    final_competition_to_game, live_competition_to_game, pregame_competition_to_game,
};
use super::types::{EspnCompetition, NbaGame};

fn scoreboard_url(state: &AppState) -> String {
    league::scoreboard_url(&state.config.espn, &Nba)
}

/// Every NBA state is displayable — breaks render via the live `phase` — so
/// the list state is a straight mapping, with no MLB-style delay exclusions.
fn list_state(competition: &EspnCompetition) -> Option<GameState> {
    Some(match competition {
        EspnCompetition::PreGame { .. } => GameState::Pregame,
        EspnCompetition::Live { .. } => GameState::Live,
        EspnCompetition::Final { .. } => GameState::Final,
    })
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
    handler::list_games_response::<EspnCompetition>(
        state,
        &scoreboard_url(state),
        headers,
        list_state,
    )
    .await
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
    let EventParts {
        id,
        date,
        competition,
        ..
    } = handler::fetch_game_parts::<EspnCompetition>(state, &url, game_id).await?;

    let game = match competition {
        EspnCompetition::PreGame {
            competitors,
            venue_name,
        } => NbaGame::Pregame(
            pregame_competition_to_game(id, &date, venue_name, competitors)
                .map_err(|e| e.with_url(&url))?,
        ),
        EspnCompetition::Live {
            competitors,
            period,
            display_clock,
            phase,
            situation,
        } => NbaGame::Live(
            live_competition_to_game(id, competitors, period, display_clock, phase, situation)
                .map_err(|e| e.with_url(&url))?,
        ),
        EspnCompetition::Final {
            competitors,
            period,
        } => NbaGame::Final(
            final_competition_to_game(id, competitors, period).map_err(|e| e.with_url(&url))?,
        ),
    };

    Ok(handler::game_response(headers, &game, encode_game))
}
