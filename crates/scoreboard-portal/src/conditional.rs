//! Does this `If-None-Match` mean "I already have it"?
//!
//! The SPA is a single 54 KB gzip served off a device with two TCP sockets, so
//! the conditional request is not a nicety — it is the difference between a
//! settings page that opens instantly and one that re-downloads the whole
//! bundle on every visit. `main.py:319-336` computed a build-time ETag and
//! compared it to the header with `==`; this is the same decision, made
//! properly, and tested.
//!
//! # Three deliberate differences from `main.py`
//!
//! 1. **The tag we send is quoted.** MicroPython set `ETag: 1a2b3c4d5e6f7788`
//!    — a bare hex string, which RFC 9110 §8.8.3 does not allow: an entity-tag
//!    is a quoted string, optionally weak-prefixed. Caches are entitled to
//!    ignore a malformed one, and the point of the header is to be obeyed. See
//!    [`quoted`].
//! 2. **An unquoted tag is still accepted on the way in.** A browser echoes
//!    back whatever it was given, so a client holding a tag from the
//!    MicroPython firmware sends it bare. Accepting both costs one branch.
//! 3. **Lists and `*` are handled.** `If-None-Match` is a comma-separated list
//!    and may be `*`; `==` matched neither. A client that had cached two
//!    versions sent two tags and got a 200 it did not need.
//!
//! # Weak comparison, on purpose
//!
//! RFC 9110 §13.1.2 specifies the *weak* comparison function for
//! `If-None-Match`, so `W/"abc"` and `"abc"` match. That is the correct
//! reading and also the useful one: nothing here ever emits a weak tag, but a
//! proxy between the phone and the device may have weakened one in transit.

/// The `ETag` header value for a tag: quoted, per RFC 9110 §8.8.3.
///
/// The caller owns the buffer, and the tag is 16 hex characters, so 18 bytes
/// is always enough — [`ETAG_HEADER_LEN`].
pub fn quoted<'a>(tag: &str, out: &'a mut [u8]) -> Option<&'a str> {
    let len = tag.len() + 2;
    let slot = out.get_mut(..len)?;
    slot[0] = b'"';
    slot[1..len - 1].copy_from_slice(tag.as_bytes());
    slot[len - 1] = b'"';
    // The input is hex from `build.rs` and the quotes are ASCII, so this is
    // UTF-8 by construction; the check is free and avoids an `unsafe`.
    core::str::from_utf8(slot).ok()
}

/// Bytes [`quoted`] needs for an 8-byte digest rendered as 16 hex characters.
pub const ETAG_HEADER_LEN: usize = 18;

/// Whether `If-None-Match` claims a cached copy matching `tag`.
///
/// `tag` is the bare form — the hex, without quotes. The header may be
/// quoted, unquoted, weak, a list, or `*`.
pub fn if_none_match(header: &str, tag: &str) -> bool {
    header.split(',').any(|candidate| {
        let candidate = candidate.trim();
        // `*` means "any current representation", which for a resource that
        // exists is always a match (RFC 9110 §13.1.2).
        candidate == "*" || normalize(candidate) == tag
    })
}

/// Strip a `W/` weak prefix and the surrounding quotes, leaving the opaque
/// tag itself.
fn normalize(candidate: &str) -> &str {
    let candidate = candidate.strip_prefix("W/").unwrap_or(candidate);
    candidate
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAG: &str = "1a2b3c4d5e6f7788";

    #[test]
    fn the_header_value_is_quoted() {
        let mut out = [0u8; ETAG_HEADER_LEN];
        assert_eq!(quoted(TAG, &mut out), Some("\"1a2b3c4d5e6f7788\""));
    }

    #[test]
    fn the_header_buffer_constant_is_exactly_big_enough() {
        let mut out = [0u8; ETAG_HEADER_LEN];
        assert!(quoted(TAG, &mut out).is_some());
        let mut small = [0u8; ETAG_HEADER_LEN - 1];
        assert_eq!(quoted(TAG, &mut small), None);
    }

    #[test]
    fn a_quoted_tag_matches() {
        assert!(if_none_match("\"1a2b3c4d5e6f7788\"", TAG));
    }

    #[test]
    fn an_unquoted_tag_matches_too() {
        // What a client cached under the MicroPython firmware sends back.
        assert!(if_none_match("1a2b3c4d5e6f7788", TAG));
    }

    #[test]
    fn a_weak_tag_matches_under_the_weak_comparison() {
        assert!(if_none_match("W/\"1a2b3c4d5e6f7788\"", TAG));
    }

    #[test]
    fn a_star_matches_anything() {
        assert!(if_none_match("*", TAG));
    }

    #[test]
    fn a_list_matches_if_any_member_does() {
        assert!(if_none_match("\"aaaa\", \"1a2b3c4d5e6f7788\"", TAG));
        assert!(if_none_match("\"1a2b3c4d5e6f7788\",\"bbbb\"", TAG));
        assert!(if_none_match("W/\"aaaa\", \"1a2b3c4d5e6f7788\"", TAG));
    }

    #[test]
    fn a_different_tag_does_not_match() {
        assert!(!if_none_match("\"0000000000000000\"", TAG));
        assert!(!if_none_match("\"aaaa\", \"bbbb\"", TAG));
        assert!(!if_none_match("", TAG));
    }

    #[test]
    fn a_prefix_is_not_a_match() {
        // The quotes are what make this unambiguous, and dropping them on the
        // way in must not lose it.
        assert!(!if_none_match("\"1a2b3c4d5e6f77\"", TAG));
        assert!(!if_none_match("1a2b3c4d5e6f778", TAG));
        assert!(!if_none_match("1a2b3c4d5e6f77880", TAG));
    }

    #[test]
    fn an_unbalanced_quote_is_not_stripped() {
        // `"abc` is a malformed tag, not the tag `abc` — stripping one side
        // would make a broken client's header match by accident.
        assert!(!if_none_match("\"1a2b3c4d5e6f7788", TAG));
        assert!(!if_none_match("1a2b3c4d5e6f7788\"", TAG));
    }
}
