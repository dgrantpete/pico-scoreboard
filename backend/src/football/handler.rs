use axum::{http::HeaderMap, response::Response};

use crate::AppState;
use crate::error::{AppError, ErrorResponse};
use crate::espn::league::{self, FootballLeague};
use crate::shared::game::{GameListEntry, GameState};
use crate::shared::handler::{self, EventParts};
use crate::wire;

use super::transform::{
    final_competition_to_game, live_competition_to_game, pregame_competition_to_game,
};
use super::types::{EspnCompetition, FootballGame};

fn list_state(competition: &EspnCompetition) -> Option<GameState> {
    Some(match competition {
        EspnCompetition::PreGame { .. } => GameState::Pregame,
        EspnCompetition::Live { .. } => GameState::Live,
        EspnCompetition::Final { .. } => GameState::Final,
    })
}

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
        (status = 200, description = "Today's games with per-game state. Binary encoding available via `Accept: application/x-scoreboard-struct` (see backend/src/wire.rs)", body = Vec<GameListEntry>),
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
    handler::list_games_response::<EspnCompetition>(
        state,
        &league::scoreboard_url(&state.config.espn, &league),
        headers,
        list_state,
    )
    .await
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
        (status = 200, description = "Game state (pregame/live/final). Binary encoding available via `Accept: application/x-scoreboard-struct` (see backend/src/wire.rs)", body = FootballGame),
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
        } => FootballGame::Pregame(
            pregame_competition_to_game(id, &date, venue_name, competitors, league.is_college())
                .map_err(|e| e.with_url(&url))?,
        ),
        EspnCompetition::Live {
            competitors,
            period,
            display_clock,
            phase,
            situation,
        } => FootballGame::Live(
            live_competition_to_game(id, competitors, period, display_clock, phase, situation)
                .map_err(|e| e.with_url(&url))?,
        ),
        EspnCompetition::Final {
            competitors,
            period,
        } => FootballGame::Final(
            final_competition_to_game(id, competitors, period).map_err(|e| e.with_url(&url))?,
        ),
    };

    Ok(handler::game_response(
        headers,
        &game,
        wire::encode_football_game,
    ))
}
