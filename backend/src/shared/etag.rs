use axum::{
    Json,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use sha1::{Digest, Sha1};

use crate::mlb::GameListEntry;
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
