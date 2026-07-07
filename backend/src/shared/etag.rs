use sha1::{Digest, Sha1};

/// First 16 hex chars of SHA-1 over the sorted, comma-joined game IDs.
/// Mirrors the firmware's `_compute_index_etag` pattern so both sides agree
/// on truncation length.
pub(crate) fn compute_games_etag(game_ids: &[String]) -> String {
    let mut sorted: Vec<&str> = game_ids.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    let joined = sorted.join(",");
    let digest = Sha1::digest(joined.as_bytes());
    hex::encode(&digest[..8])
}
