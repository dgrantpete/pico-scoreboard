use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use sha1::{Digest, Sha1};

/// First 16 hex chars of SHA-1 over the sorted, comma-joined cache tokens.
/// Callers choose the token per game — bare ids (soccer) or `"{id}:{state}"`
/// (MLB, so a state flip busts a client's 304). Mirrors the firmware's
/// `_compute_index_etag` pattern so both sides agree on truncation length.
pub(crate) fn compute_games_etag(tokens: &[String]) -> String {
    let mut sorted: Vec<&str> = tokens.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    let joined = sorted.join(",");
    let digest = Sha1::digest(joined.as_bytes());
    hex::encode(&digest[..8])
}

/// 200-or-304 JSON response for a games-list endpoint: quoted strict-match
/// ETag over the ids, `304` with empty body on `If-None-Match` hit. No
/// `Vary` — used by endpoints with a single representation (MLB adds its own
/// binary negotiation on top and keeps its response builder locally).
pub(crate) fn json_games_response(game_ids: Vec<String>, if_none_match: Option<&str>) -> Response {
    let quoted = format!("\"{}\"", compute_games_etag(&game_ids));

    if if_none_match == Some(quoted.as_str()) {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, quoted.as_str())]).into_response();
    }

    ([(header::ETAG, quoted.as_str())], Json(game_ids)).into_response()
}
