use axum::{http::HeaderMap, response::Response};

use super::wire::encode_game;
use crate::AppState;
use crate::error::{AppError, ErrorResponse};
use crate::espn::league::{self, Mlb};
use crate::shared::game::{GameListEntry, GameState};
use crate::shared::handler::{self, EventParts};

use super::transform::{
    final_competition_to_game, live_competition_to_game, parse_inning_half,
    pregame_competition_to_game,
};
use super::types::{EspnCompetition, MlbGame};

fn scoreboard_url(state: &AppState) -> String {
    league::scoreboard_url(&state.config.espn, &Mlb)
}

/// The list state of an event's first competition, or `None` for events with
/// nothing displayable — a rain-delayed live game (non-inning `shortDetail`) is
/// excluded so the firmware never advertises a live game it can't then fetch.
fn list_state(competition: &EspnCompetition) -> Option<GameState> {
    match competition {
        EspnCompetition::PreGame { .. } => Some(GameState::Pregame),
        EspnCompetition::Final { .. } => Some(GameState::Final),
        EspnCompetition::Live { short_detail, .. } => {
            parse_inning_half(short_detail).map(|_| GameState::Live)
        }
    }
}

/// GET /baseball/mlb/games — list today's MLB games with their current state.
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
    handler::list_games_response::<EspnCompetition>(
        state,
        &scoreboard_url(state),
        headers,
        list_state,
    )
    .await
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
    let EventParts {
        id,
        date,
        weather,
        competition,
    } = handler::fetch_game_parts::<EspnCompetition>(state, &url, game_id).await?;

    let game = match competition {
        EspnCompetition::PreGame {
            competitors,
            venue_name,
        } => MlbGame::Pregame(
            pregame_competition_to_game(id, &date, weather.as_ref(), venue_name, competitors)
                .map_err(|e| e.with_url(&url))?,
        ),
        EspnCompetition::Live {
            competitors,
            situation,
            period,
            short_detail,
        } => MlbGame::Live(
            live_competition_to_game(id, competitors, situation, period, short_detail)
                .map_err(|e| e.with_url(&url))?,
        ),
        EspnCompetition::Final {
            competitors,
            period,
        } => MlbGame::Final(
            final_competition_to_game(id, competitors, period).map_err(|e| e.with_url(&url))?,
        ),
    };

    Ok(handler::game_response(headers, &game, encode_game))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_state_excludes_rain_delayed_live_game() {
        let competitor = |abbrev: &str, home_away: &str| {
            serde_json::from_value::<super::super::types::EspnCompetitor>(serde_json::json!({
                "homeAway": home_away,
                "score": "0",
                "team": {"id": "1", "abbreviation": abbrev, "color": "0C2340", "alternateColor": "BD3039"},
            }))
            .unwrap()
        };
        let situation = serde_json::from_value(serde_json::json!({
            "balls": 0, "strikes": 0, "outs": 0,
            "onFirst": false, "onSecond": false, "onThird": false,
            "lastPlay": {"id": "p1", "text": ""},
        }))
        .unwrap();
        let delayed = EspnCompetition::Live {
            competitors: [competitor("NYY", "away"), competitor("BOS", "home")],
            situation,
            period: 1,
            short_detail: "Rain Delay, Top 1st".to_string(),
        };
        assert!(list_state(&delayed).is_none());
    }
}
