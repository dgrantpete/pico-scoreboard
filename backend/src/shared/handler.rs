//! Shared game-handler scaffolding: the fetch → lenient-parse → find /
//! negotiate spine that every sport's handlers share. The per-sport match block
//! (the sport's actual transform, and soccer's commentary fetch) stays in the
//! sport handler as straight-line code — these helpers are only the identical
//! parts around it, so there are no async closures or trait objects.

use axum::{
    Json,
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::AppState;
use crate::error::AppError;
use crate::espn::types::{EspnEvent, EspnWeather, RawScoreboard, find_event, parse_events};
use crate::shared::etag::{games_response, wants_struct};
use crate::shared::game::{GameListEntry, GameState};
use scoreboard_wire as wire;

/// The parts of one event a detail handler needs, with the first competition
/// already extracted (a no-competition event is `GameNotFound`, as today).
pub(crate) struct EventParts<C> {
    pub(crate) id: String,
    pub(crate) date: String,
    /// Event-level weather; MLB reads it, the other sports ignore it.
    pub(crate) weather: Option<EspnWeather>,
    pub(crate) competition: C,
}

/// Shared list body: fetch → lenient parse → per-competition `list_state` →
/// ETag/304/struct negotiation via `games_response`. `list_state` returns
/// `None` for an event with nothing displayable (MLB's rain-delay exclusion);
/// every other sport always returns `Some`.
pub(crate) async fn list_games_response<C: DeserializeOwned>(
    state: &AppState,
    url: &str,
    headers: &HeaderMap,
    list_state: impl Fn(&C) -> Option<GameState>,
) -> Result<Response, AppError> {
    let raw: RawScoreboard = state.espn_client.fetch_json_cached(url).await?;
    // Serve whatever parsed: a transient ETag flap to a smaller set beats a 502.
    let (events, _failed) = parse_events::<EspnEvent<C>>(raw, url);

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

    Ok(games_response(
        entries,
        if_none_match,
        wants_struct(headers),
    ))
}

/// Shared detail front half: fetch → lenient parse → find the event → split it
/// into [`EventParts`] (first competition extracted). `find_event`'s
/// glitched-scoreboard-502 vs 404 semantics flow through unchanged.
pub(crate) async fn fetch_game_parts<C: DeserializeOwned>(
    state: &AppState,
    url: &str,
    game_id: &str,
) -> Result<EventParts<C>, AppError> {
    let raw: RawScoreboard = state.espn_client.fetch_json_cached(url).await?;
    let (events, failed) = parse_events::<EspnEvent<C>>(raw, url);
    let event = find_event(events, failed, game_id, url, |e| &e.id)?;

    let EspnEvent {
        id,
        date,
        weather,
        competitions,
    } = event;
    let competition = competitions
        .into_iter()
        .next()
        .ok_or_else(|| AppError::GameNotFound(game_id.to_string()))?;

    Ok(EventParts {
        id,
        date,
        weather,
        competition,
    })
}

/// Shared detail back half: content-negotiate JSON vs the packed binary wire
/// format, always with `Vary: Accept`. `encode` is the sport's `wire::encode_game`.
pub(crate) fn game_response<G: Serialize>(
    headers: &HeaderMap,
    game: &G,
    encode: impl FnOnce(&G) -> Vec<u8>,
) -> Response {
    if wants_struct(headers) {
        (
            [
                (header::CONTENT_TYPE, wire::STRUCT_CONTENT_TYPE),
                (header::VARY, "Accept"),
            ],
            encode(game),
        )
            .into_response()
    } else {
        ([(header::VARY, "Accept")], Json(game)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[derive(Serialize)]
    struct Dummy {
        n: u8,
    }

    fn headers_with_accept(accept: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(a) = accept {
            headers.insert(header::ACCEPT, a.parse().unwrap());
        }
        headers
    }

    #[tokio::test]
    async fn struct_accept_gets_encoded_bytes_and_vary() {
        let headers = headers_with_accept(Some(wire::STRUCT_CONTENT_TYPE));
        let resp = game_response(&headers, &Dummy { n: 7 }, |d| vec![0xAB, d.n]);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            wire::STRUCT_CONTENT_TYPE
        );
        assert_eq!(resp.headers().get(header::VARY).unwrap(), "Accept");
        let body = to_bytes(resp.into_body(), 64).await.unwrap();
        assert_eq!(&body[..], &[0xAB, 7]);
    }

    #[tokio::test]
    async fn no_accept_gets_json_and_vary() {
        let headers = headers_with_accept(None);
        let resp = game_response(&headers, &Dummy { n: 7 }, |_| {
            unreachable!("JSON path must not invoke the encoder")
        });
        let content_type = resp.headers().get(header::CONTENT_TYPE).unwrap();
        assert!(
            content_type
                .to_str()
                .unwrap()
                .starts_with("application/json")
        );
        assert_eq!(resp.headers().get(header::VARY).unwrap(), "Accept");
        let body = to_bytes(resp.into_body(), 64).await.unwrap();
        assert_eq!(&body[..], br#"{"n":7}"#);
    }
}
