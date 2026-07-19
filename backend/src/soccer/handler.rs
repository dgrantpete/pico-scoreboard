use axum::{http::HeaderMap, response::Response};

use crate::AppState;
use crate::error::{AppError, ErrorResponse};
use crate::espn::league::{self, SoccerLeague};
use crate::shared::game::{GameListEntry, GameState};
use crate::shared::handler::{self, EventParts};
use crate::wire;

use super::transform::{
    final_competition_to_game, latest_commentary, live_competition_to_game,
    pregame_competition_to_game,
};
use super::types::{Commentary, EspnCompetition, RawSummary, SoccerGame};

/// Latest commentary line for a live game, best-effort: commentary is polish,
/// so a summary fetch/parse failure degrades to None (with a warning) rather
/// than failing the live payload.
async fn fetch_commentary(
    state: &AppState,
    league: &SoccerLeague,
    event_id: &str,
) -> Option<Commentary> {
    let url = league::summary_url(&state.config.espn, league, event_id);
    match state
        .espn_client
        .fetch_json_cached::<RawSummary>(&url)
        .await
    {
        Ok(summary) => latest_commentary(summary),
        Err(e) => {
            tracing::warn!(url = %url, error = ?e, "soccer summary fetch failed; serving live without commentary");
            None
        }
    }
}

fn list_state(competition: &EspnCompetition) -> Option<GameState> {
    Some(match competition {
        EspnCompetition::PreGame { .. } => GameState::Pregame,
        EspnCompetition::Live { .. } => GameState::Live,
        EspnCompetition::Final { .. } => GameState::Final,
    })
}

/// GET /soccer/{league}/games — today's games for one league with their
/// current state (pregame/live/final), like the MLB list.
#[utoipa::path(
    get,
    path = "/soccer/{league}/games",
    params(("league" = String, Path, description = "ESPN soccer league slug (fifa.world, usa.1, eng.1, mex.1)")),
    responses(
        (status = 200, description = "Today's games with per-game state. Binary encoding available via `Accept: application/x-scoreboard-struct` (see backend/src/wire.rs)", body = Vec<GameListEntry>),
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
    handler::list_games_response::<EspnCompetition>(
        state,
        &league::scoreboard_url(&state.config.espn, &league),
        headers,
        list_state,
    )
    .await
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
        (status = 200, description = "Game state (pregame/live/final). Binary encoding available via `Accept: application/x-scoreboard-struct` (see backend/src/wire.rs)", body = SoccerGame),
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
        } => SoccerGame::Pregame(
            pregame_competition_to_game(id, &date, venue_name, competitors)
                .map_err(|e| e.with_url(&url))?,
        ),
        EspnCompetition::Live {
            competitors,
            display_clock,
            clock_seconds,
            period,
            on_break,
            details,
        } => {
            let commentary = fetch_commentary(state, &league, &id).await;
            SoccerGame::Live(
                live_competition_to_game(
                    id,
                    competitors,
                    display_clock,
                    clock_seconds,
                    period,
                    on_break,
                    details,
                    commentary,
                )
                .map_err(|e| e.with_url(&url))?,
            )
        }
        EspnCompetition::Final {
            competitors,
            details,
            flavor,
        } => SoccerGame::Final(
            final_competition_to_game(id, competitors, details, flavor)
                .map_err(|e| e.with_url(&url))?,
        ),
    };

    Ok(handler::game_response(
        headers,
        &game,
        wire::encode_soccer_game,
    ))
}
