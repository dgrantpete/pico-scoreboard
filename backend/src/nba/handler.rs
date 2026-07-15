use axum::{
    Json,
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
};

use crate::AppState;
use crate::error::{AppError, ErrorResponse};
use crate::espn::league::{self, Nba};
use crate::espn::types::{RawScoreboard, find_event, parse_events};
use crate::shared::etag::{games_response, wants_struct};
use crate::shared::game::{GameListEntry, GameState};
use crate::wire;

use super::transform::{
    final_competition_to_game, live_competition_to_game, pregame_competition_to_game,
};
use super::types::{EspnCompetition, EspnEvent, NbaGame};

fn scoreboard_url(state: &AppState) -> String {
    league::scoreboard_url(&state.config.espn, &Nba)
}

/// Every NBA state is displayable — breaks render via the live `phase` — so
/// the list state is a straight mapping, with no MLB-style delay exclusions.
fn list_state(competition: &EspnCompetition) -> GameState {
    match competition {
        EspnCompetition::PreGame { .. } => GameState::Pregame,
        EspnCompetition::Live { .. } => GameState::Live,
        EspnCompetition::Final { .. } => GameState::Final,
    }
}

/// GET /basketball/nba/games — list today's NBA games with their current state.
#[utoipa::path(
    get,
    path = "/basketball/nba/games",
    responses(
        (status = 200, description = "Today's NBA games with per-game state (pregame/live/final). Binary encoding available via `Accept: application/x-scoreboard-struct` (see backend/src/wire.rs)", body = Vec<GameListEntry>),
        (status = 304, description = "Game set and states unchanged since client's If-None-Match"),
        (status = 401, description = "Missing or invalid API key", body = ErrorResponse),
        (status = 502, description = "ESPN upstream error", body = ErrorResponse),
    ),
    security(("api_key" = [])),
    tag = "nba"
)]
pub async fn list_games(state: &AppState, headers: &HeaderMap) -> Result<Response, AppError> {
    let url = scoreboard_url(state);
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

    Ok(games_response(entries, if_none_match, wants_struct(headers)))
}

/// GET /basketball/nba/games/{game_id} — state snapshot for one NBA game.
#[utoipa::path(
    get,
    path = "/basketball/nba/games/{game_id}",
    params(("game_id" = String, Path, description = "ESPN event ID")),
    responses(
        (status = 200, description = "Game state (pregame/live/final). Binary encoding available via `Accept: application/x-scoreboard-struct` (see backend/src/wire.rs)", body = NbaGame),
        (status = 401, description = "Missing or invalid API key", body = ErrorResponse),
        (status = 404, description = "Game not on today's scoreboard", body = ErrorResponse),
        (status = 502, description = "ESPN upstream error", body = ErrorResponse),
    ),
    security(("api_key" = [])),
    tag = "nba"
)]
pub async fn get_game(
    state: &AppState,
    game_id: &str,
    headers: &HeaderMap,
) -> Result<Response, AppError> {
    let url = scoreboard_url(state);
    let raw: RawScoreboard = state.espn_client.fetch_json_cached(&url).await?;
    let (events, failed) = parse_events::<EspnEvent>(raw, &url);
    let event = find_event(events, failed, game_id, &url, |e| &e.id)?;

    // Destructure the event before consuming its competition: the pregame
    // payload needs the event-level date.
    let EspnEvent {
        id,
        date,
        competitions,
    } = event;
    let first = competitions
        .into_iter()
        .next()
        .ok_or_else(|| AppError::GameNotFound(game_id.to_string()))?;

    let game = match first {
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
            last_play,
        } => NbaGame::Live(
            live_competition_to_game(id, competitors, period, display_clock, phase, last_play)
                .map_err(|e| e.with_url(&url))?,
        ),
        EspnCompetition::Final {
            competitors,
            period,
        } => NbaGame::Final(
            final_competition_to_game(id, competitors, period).map_err(|e| e.with_url(&url))?,
        ),
    };

    if wants_struct(headers) {
        Ok((
            [
                (header::CONTENT_TYPE, wire::STRUCT_CONTENT_TYPE),
                (header::VARY, "Accept"),
            ],
            wire::encode_nba_game(&game),
        )
            .into_response())
    } else {
        Ok(([(header::VARY, "Accept")], Json(game)).into_response())
    }
}
