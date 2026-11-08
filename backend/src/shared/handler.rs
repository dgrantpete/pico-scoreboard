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

use crate::shared::etag::{ext_level, games_response, wants_struct};
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
/// A sport with no wire extension answers an `ext=N` request with plain v2.
pub(crate) fn game_response<G: Serialize>(
    headers: &HeaderMap,
    game: &G,
    encode: impl FnOnce(&G) -> Vec<u8>,
) -> Response {
    respond(headers, game, || encode(game))
}

/// [`game_response`] for a sport whose wire module defines extension levels:
/// `encode` is handed the level the client asked for ([`ext_level`] — 0 for
/// everyone who did not ask, capped at the highest the wire crate knows). The
/// content type is the plain struct type at every level (crate docs).
pub(crate) fn game_response_ext<G: Serialize>(
    headers: &HeaderMap,
    game: &G,
    encode: impl FnOnce(&G, u8) -> Vec<u8>,
) -> Response {
    let level = ext_level(headers);
    respond(headers, game, || encode(game, level))
}

fn respond<G: Serialize>(
    headers: &HeaderMap,
    game: &G,
    bytes: impl FnOnce() -> Vec<u8>,
) -> Response {
    if wants_struct(headers) {
        (
            [
                (header::CONTENT_TYPE, wire::STRUCT_CONTENT_TYPE),
                (header::VARY, "Accept"),
            ],
            bytes(),
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

    /// The encoder is told the level the client asked for, and the content
    /// type does not change with it.
    #[tokio::test]
    async fn ext_accept_hands_the_encoder_its_level() {
        for (accept, level) in [
            (wire::STRUCT_CONTENT_TYPE_EXT, 1u8),
            ("application/x-scoreboard-struct; ext=2", 2),
        ] {
            let headers = headers_with_accept(Some(accept));
            let resp = game_response_ext(&headers, &Dummy { n: 7 }, |d, level| vec![d.n, level]);
            assert_eq!(
                resp.headers().get(header::CONTENT_TYPE).unwrap(),
                wire::STRUCT_CONTENT_TYPE
            );
            let body = to_bytes(resp.into_body(), 64).await.unwrap();
            assert_eq!(&body[..], &[7, level], "{accept}");
        }
    }

    #[tokio::test]
    async fn plain_struct_accept_never_gets_the_extension() {
        // The gift fleet and pre-2026-10-12 firmware: exactly v2.
        let headers = headers_with_accept(Some(wire::STRUCT_CONTENT_TYPE));
        let resp = game_response_ext(&headers, &Dummy { n: 7 }, |d, level| vec![d.n, level]);
        let body = to_bytes(resp.into_body(), 64).await.unwrap();
        assert_eq!(&body[..], &[7, 0]);
    }

    #[tokio::test]
    async fn ext_accept_to_a_sport_without_extensions_gets_plain_v2() {
        let headers = headers_with_accept(Some(wire::STRUCT_CONTENT_TYPE_EXT));
        let resp = game_response(&headers, &Dummy { n: 7 }, |d| vec![0xAB, d.n]);
        let body = to_bytes(resp.into_body(), 64).await.unwrap();
        assert_eq!(&body[..], &[0xAB, 7]);
    }

    /// The level is read per media range, as a number, and capped at the
    /// highest the wire crate defines: a future `ext=3` device gets 2 from
    /// this backend.
    #[test]
    fn the_ext_level_is_read_per_media_range_and_capped() {
        let level = |accept: Option<&str>| ext_level(&headers_with_accept(accept));
        assert_eq!(level(Some("application/x-scoreboard-struct; ext=1")), 1);
        assert_eq!(level(Some("application/x-scoreboard-struct;ext=1")), 1);
        assert_eq!(level(Some("application/x-scoreboard-struct; ext=2")), 2);
        assert_eq!(level(Some("application/x-scoreboard-struct; ext=3")), 2);
        assert_eq!(level(Some("application/x-scoreboard-struct; ext=300")), 2);
        assert_eq!(
            level(Some(
                "application/json, application/x-scoreboard-struct; q=0.9; EXT=1"
            )),
            1
        );
        assert_eq!(level(Some("application/x-scoreboard-struct")), 0);
        assert_eq!(level(Some("application/x-scoreboard-struct; ext=x")), 0);
        assert_eq!(level(Some("application/x-scoreboard-struct; ext=")), 0);
        assert_eq!(
            level(Some(
                "application/json; ext=1, application/x-scoreboard-struct"
            )),
            0
        );
        assert_eq!(level(None), 0);
        assert_eq!(wire::EXT_LEVEL_MAX, 2, "the cap above follows the crate");
    }
}
