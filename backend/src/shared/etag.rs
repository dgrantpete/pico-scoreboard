use axum::{
    Json,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use sha1::{Digest, Sha1};

use crate::shared::game::GameListEntry;
use crate::wire;

/// First 16 hex chars of SHA-1 over the sorted, comma-joined cache tokens.
/// Tokens are `"{id}:{state_code}"` per game, so a state flip (pregame →
/// live) with the same id set busts a client's 304. Mirrors the firmware's
/// `_compute_index_etag` pattern so both sides agree on truncation length.
pub(crate) fn compute_games_etag(tokens: &[String]) -> String {
    let mut sorted: Vec<&str> = tokens.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    let joined = sorted.join(",");
    let digest = Sha1::digest(joined.as_bytes());
    hex::encode(&digest[..8])
}

/// True when the client asked for the packed binary format (see `wire.rs`).
pub(crate) fn wants_struct(headers: &HeaderMap) -> bool {
    headers.get_all(header::ACCEPT).iter().any(|v| {
        v.to_str()
            .map(|s| s.contains(wire::STRUCT_CONTENT_TYPE))
            .unwrap_or(false)
    })
}

/// Build a 200-or-304 response for a games-list endpoint from the entries,
/// the client's `If-None-Match` header, and the negotiated format. Shared by
/// every sport — the list wire encoding is sport-agnostic (`state` + id).
///
/// Both representations share the ETag (with `Vary: Accept`); there are no
/// shared caches in this deployment and a given client always requests one
/// format.
pub(crate) fn games_response(
    entries: Vec<GameListEntry>,
    if_none_match: Option<&str>,
    use_struct: bool,
) -> Response {
    let tokens: Vec<String> = entries
        .iter()
        .map(|e| format!("{}:{}", e.id, e.state.code()))
        .collect();
    let etag = compute_games_etag(&tokens);
    let quoted = format!("\"{}\"", etag);

    if if_none_match == Some(quoted.as_str()) {
        return (
            StatusCode::NOT_MODIFIED,
            [(header::ETAG, quoted.as_str()), (header::VARY, "Accept")],
        )
            .into_response();
    }

    if use_struct {
        (
            [
                (header::ETAG, quoted.as_str()),
                (header::VARY, "Accept"),
                (header::CONTENT_TYPE, wire::STRUCT_CONTENT_TYPE),
            ],
            wire::encode_game_list(&entries),
        )
            .into_response()
    } else {
        (
            [(header::ETAG, quoted.as_str()), (header::VARY, "Accept")],
            Json(entries),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::game::GameState;
    use axum::body::to_bytes;

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
            entries(&[
                ("401570001", GameState::Live),
                ("401570002", GameState::Final),
            ]),
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
        let list = entries(&[
            ("401570002", GameState::Live),
            ("401570001", GameState::Pregame),
        ]);
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
        let before = entries(&[
            ("401570001", GameState::Pregame),
            ("401570002", GameState::Live),
        ]);
        let before_tag = format!("\"{}\"", compute_games_etag(&tokens(&before)));

        let after = entries(&[
            ("401570001", GameState::Live),
            ("401570002", GameState::Live),
        ]);
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
}
