//! Shared game-handler scaffolding: the response-negotiation halves every
//! sport's handlers share. The fetch → extract spine now lives in each
//! sport's adapter (the `scoreboard-espn` extractor APIs are per-sport);
//! these helpers are only the identical parts around it.

use axum::{
    Json,
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::shared::etag::{games_response, wants_struct};
use crate::shared::game::GameListEntry;
use scoreboard_wire as wire;

/// Shared list back half: ETag/304/struct negotiation via `games_response`.
pub(crate) fn list_response(entries: Vec<GameListEntry>, headers: &HeaderMap) -> Response {
    let if_none_match = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok());
    games_response(entries, if_none_match, wants_struct(headers))
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
