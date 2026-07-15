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
use crate::espn::league::{self, Mlb};
use crate::espn::types::{RawScoreboard, find_event, parse_events};
use crate::shared::etag::{games_response, wants_struct};
use crate::shared::game::{GameListEntry, GameState};
use crate::wire;

use super::transform::{
    final_competition_to_game, live_competition_to_game, parse_inning_half,
    pregame_competition_to_game,
};
use super::types::{EspnCompetition, EspnEvent, MlbGame};

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
        (status = 200, description = "Today's MLB games with per-game state (pregame/live/final). Binary encoding available via `Accept: application/x-scoreboard-struct` (see backend/src/wire.rs)", body = Vec<GameListEntry>),
        (status = 304, description = "Game set and states unchanged since client's If-None-Match"),
        (status = 401, description = "Missing or invalid API key", body = ErrorResponse),
        (status = 502, description = "ESPN upstream error", body = ErrorResponse),
    ),
    security(("api_key" = [])),
    tag = "mlb"
)]
pub async fn list_games(
    State(state): State<Arc<AppState>>,
    _auth: ApiKey,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let url = scoreboard_url(&state);
    let raw: RawScoreboard = state.espn_client.fetch_json_cached(&url).await?;
    // Serve whatever parsed: a transient ETag flap to a smaller set beats a 502.
    let (events, _failed) = parse_events::<EspnEvent>(raw, &url);

    let entries: Vec<GameListEntry> = events
        .into_iter()
        .filter_map(|event| {
            let first = event.competitions.into_iter().next()?;
            list_state(&first).map(|state| GameListEntry {
                id: event.id,
                state,
            })
        })
        .collect();

    let if_none_match = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok());

    Ok(games_response(entries, if_none_match, wants_struct(&headers)))
}

/// GET /baseball/mlb/games/{game_id} — state snapshot for one MLB game.
#[utoipa::path(
    get,
    path = "/baseball/mlb/games/{game_id}",
    params(("game_id" = String, Path, description = "ESPN event ID")),
    responses(
        (status = 200, description = "Game state (pregame/live/final). Binary encoding available via `Accept: application/x-scoreboard-struct` (see backend/src/wire.rs)", body = MlbGame),
        (status = 401, description = "Missing or invalid API key", body = ErrorResponse),
        (status = 404, description = "Not on today's scoreboard, or a non-displayable delay", body = ErrorResponse),
        (status = 502, description = "ESPN upstream error", body = ErrorResponse),
    ),
    security(("api_key" = [])),
    tag = "mlb"
)]
pub async fn get_game(
    State(state): State<Arc<AppState>>,
    _auth: ApiKey,
    Path(game_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let url = scoreboard_url(&state);
    let raw: RawScoreboard = state.espn_client.fetch_json_cached(&url).await?;
    let (events, failed) = parse_events::<EspnEvent>(raw, &url);
    let event = find_event(events, failed, &game_id, &url, |e| &e.id)?;

    // Destructure the event before consuming its competition: the pregame
    // payload needs the event-level date and weather.
    let EspnEvent {
        id,
        date,
        weather,
        competitions,
    } = event;
    let first = competitions
        .into_iter()
        .next()
        .ok_or_else(|| AppError::GameNotFound(game_id.clone()))?;

    let game = match first {
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
        EspnCompetition::Final { competitors, period } => MlbGame::Final(
            final_competition_to_game(id, competitors, period).map_err(|e| e.with_url(&url))?,
        ),
    };

    if wants_struct(&headers) {
        Ok((
            [
                (header::CONTENT_TYPE, wire::STRUCT_CONTENT_TYPE),
                (header::VARY, "Accept"),
            ],
            wire::encode_game(&game),
        )
            .into_response())
    } else {
        Ok(([(header::VARY, "Accept")], Json(game)).into_response())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::etag::compute_games_etag;
    use axum::body::to_bytes;
    use axum::http::StatusCode;

    fn entries(xs: &[(&str, GameState)]) -> Vec<GameListEntry> {
        xs.iter()
            .map(|(id, state)| GameListEntry {
                id: id.to_string(),
                state: *state,
            })
            .collect()
    }

    fn etag_header(resp: &Response) -> &str {
        resp.headers()
            .get(header::ETAG)
            .expect("etag header present")
            .to_str()
            .unwrap()
    }

    fn tokens(entries: &[GameListEntry]) -> Vec<String> {
        entries
            .iter()
            .map(|e| format!("{}:{}", e.id, e.state.code()))
            .collect()
    }

    #[tokio::test]
    async fn returns_200_with_etag_when_no_if_none_match() {
        let resp = games_response(
            entries(&[("401570001", GameState::Live), ("401570002", GameState::Final)]),
            None,
            false,
        );
        assert_eq!(resp.status(), StatusCode::OK);
        let tag = etag_header(&resp).to_string();
        assert!(tag.starts_with('"') && tag.ends_with('"'));
        assert_eq!(tag.len(), 18);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        assert!(!body.is_empty());
    }

    #[tokio::test]
    async fn returns_304_without_body_when_if_none_match_matches() {
        let list = entries(&[("401570002", GameState::Live), ("401570001", GameState::Pregame)]);
        let tag = format!("\"{}\"", compute_games_etag(&tokens(&list)));
        let resp = games_response(list, Some(tag.as_str()), false);
        assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(etag_header(&resp), tag);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn etag_busts_when_a_game_flips_state_with_identical_ids() {
        // Same id set, one game pregame → live: the 304 must break.
        let before = entries(&[("401570001", GameState::Pregame), ("401570002", GameState::Live)]);
        let before_tag = format!("\"{}\"", compute_games_etag(&tokens(&before)));

        let after = entries(&[("401570001", GameState::Live), ("401570002", GameState::Live)]);
        let resp = games_response(after, Some(before_tag.as_str()), false);

        assert_eq!(resp.status(), StatusCode::OK);
        assert_ne!(etag_header(&resp), before_tag);
    }

    #[tokio::test]
    async fn struct_list_encodes_version_2_and_per_entry_state() {
        let list = entries(&[
            ("401570729", GameState::Pregame),
            ("401570001", GameState::Live),
        ]);
        let json_tag = {
            let resp = games_response(list.clone(), None, false);
            etag_header(&resp).to_string()
        };

        let resp = games_response(list, None, true);
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(etag_header(&resp), json_tag);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            wire::STRUCT_CONTENT_TYPE
        );
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        assert_eq!(body[0], wire::WIRE_VERSION);
        assert_eq!(body[1], 2); // count
        assert_eq!(body[2], GameState::Pregame.code()); // first entry's state byte
    }

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
