use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
};

use crate::AppState;
use crate::auth::ApiKey;
use crate::error::{AppError, ErrorResponse};
use crate::espn::league::{self, SoccerLeague};
use crate::espn::types::{RawScoreboard, find_event, parse_events};
use crate::shared::etag::{games_response, wants_struct};
use crate::shared::game::{GameListEntry, GameState};
use crate::wire;

use super::transform::{
    final_competition_to_game, latest_commentary, live_competition_to_game,
    pregame_competition_to_game,
};
use super::types::{Commentary, EspnCompetition, EspnEvent, RawSummary, SoccerGame};

/// Latest commentary line for a live game, best-effort: commentary is polish,
/// so a summary fetch/parse failure degrades to None (with a warning) rather
/// than failing the live payload.
async fn fetch_commentary(
    state: &AppState,
    league: &SoccerLeague,
    event_id: &str,
) -> Option<Commentary> {
    let url = league::summary_url(&state.config.espn, league, event_id);
    match state.espn_client.fetch_json_cached::<RawSummary>(&url).await {
        Ok(summary) => latest_commentary(summary),
        Err(e) => {
            tracing::warn!(url = %url, error = ?e, "soccer summary fetch failed; serving live without commentary");
            None
        }
    }
}

fn list_state(competition: &EspnCompetition) -> GameState {
    match competition {
        EspnCompetition::PreGame { .. } => GameState::Pregame,
        EspnCompetition::Live { .. } => GameState::Live,
        EspnCompetition::Final { .. } => GameState::Final,
    }
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
        (status = 401, description = "Missing or invalid API key", body = ErrorResponse),
        (status = 404, description = "Unknown league", body = ErrorResponse),
        (status = 502, description = "ESPN upstream error", body = ErrorResponse),
    ),
    security(("api_key" = [])),
    tag = "soccer"
)]
pub async fn list_games(
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

    let entries: Vec<GameListEntry> = events
        .into_iter()
        .filter_map(|event| {
            let first = event.competitions.into_iter().next()?;
            Some(GameListEntry {
                id: event.id,
                state: list_state(&first),
            })
        })
        .collect();

    let if_none_match = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok());

    Ok(games_response(entries, if_none_match, wants_struct(&headers)))
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
        (status = 401, description = "Missing or invalid API key", body = ErrorResponse),
        (status = 404, description = "Unknown league, or game not on today's scoreboard", body = ErrorResponse),
        (status = 502, description = "ESPN upstream error", body = ErrorResponse),
    ),
    security(("api_key" = [])),
    tag = "soccer"
)]
pub async fn get_game(
    State(state): State<Arc<AppState>>,
    _auth: ApiKey,
    Path((league, game_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let league = SoccerLeague::from_path(&league)?;
    let url = league::scoreboard_url(&state.config.espn, &league);
    let raw: RawScoreboard = state.espn_client.fetch_json_cached(&url).await?;
    let (events, failed) = parse_events::<EspnEvent>(raw, &url);
    let event = find_event(events, failed, &game_id, &url, |e| &e.id)?;

    let EspnEvent {
        id,
        date,
        competitions,
    } = event;
    let first = competitions
        .into_iter()
        .next()
        .ok_or_else(|| AppError::GameNotFound(game_id.clone()))?;

    let game = match first {
        EspnCompetition::PreGame { competitors } => SoccerGame::Pregame(
            pregame_competition_to_game(id, &date, competitors).map_err(|e| e.with_url(&url))?,
        ),
        EspnCompetition::Live {
            competitors,
            display_clock,
            clock_seconds,
            period,
            halftime,
            details,
        } => {
            let commentary = fetch_commentary(&state, &league, &id).await;
            SoccerGame::Live(
                live_competition_to_game(
                    id,
                    competitors,
                    display_clock,
                    clock_seconds,
                    period,
                    halftime,
                    details,
                    commentary,
                )
                .map_err(|e| e.with_url(&url))?,
            )
        }
        EspnCompetition::Final {
            competitors,
            details,
        } => SoccerGame::Final(
            final_competition_to_game(id, competitors, details).map_err(|e| e.with_url(&url))?,
        ),
    };

    if wants_struct(&headers) {
        Ok((
            [
                (header::CONTENT_TYPE, wire::STRUCT_CONTENT_TYPE),
                (header::VARY, "Accept"),
            ],
            wire::encode_soccer_game(&game),
        )
            .into_response())
    } else {
        Ok(([(header::VARY, "Accept")], Json(game)).into_response())
    }
}
