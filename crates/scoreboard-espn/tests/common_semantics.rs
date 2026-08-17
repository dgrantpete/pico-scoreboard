//! The shared-semantics helpers against their oracles. The load-bearing one
//! is `parse_start_time` vs chrono — the backend parses with chrono, and a
//! one-second drift is a 4-byte diff at pregame offset 11.

use chrono::NaiveDateTime;
use scoreboard_espn::common::{
    HomeAway, order_home_away, parse_hex_rgb, parse_record, parse_start_time, saturate_score,
    set_text,
};

/// The backend's exact expression (`espn/types.rs::parse_start_time`).
fn oracle(text: &str) -> Option<u32> {
    NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%MZ")
        .or_else(|_| NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%SZ"))
        .ok()
        .map(|dt| dt.and_utc().timestamp().max(0) as u32)
}

fn assert_matches_oracle(text: &str) {
    assert_eq!(
        parse_start_time(text),
        oracle(text),
        "diverged from chrono on {text:?}"
    );
}

#[test]
fn start_time_matches_chrono_over_generated_dates() {
    // Wide sweep: month/day/hour edges across padded and unpadded forms,
    // both formats, years spanning the negative-epoch clamp and the u32
    // truncation horizon.
    for year in [1969, 1970, 1999, 2000, 2024, 2026, 2038, 2100, 2106, 2107] {
        for month in 1..=12u32 {
            for day in [1, 2, 9, 10, 28, 29, 30, 31] {
                for (hour, minute, second) in [(0, 0, 0), (1, 40, 0), (23, 59, 59), (18, 15, 30)] {
                    let padded = format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}Z");
                    assert_matches_oracle(&padded);
                    let padded_s =
                        format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z");
                    assert_matches_oracle(&padded_s);
                    let unpadded = format!("{year}-{month}-{day}T{hour}:{minute}Z");
                    assert_matches_oracle(&unpadded);
                    let unpadded_s = format!("{year}-{month}-{day}T{hour}:{minute}:{second}Z");
                    assert_matches_oracle(&unpadded_s);
                }
            }
        }
    }
}

#[test]
fn start_time_matches_chrono_on_rejects() {
    for text in [
        "2026-02-30T18:15Z",  // no such day
        "2026-13-01T18:15Z",  // no such month
        "2026-00-10T18:15Z",  // zero month
        "2026-07-00T18:15Z",  // zero day
        "2026-07-08T24:00Z",  // hour out of range
        "2026-07-08T18:60Z",  // minute out of range
        "2026-07-08T18:15",   // missing Z
        "2026-07-08 18:15Z",  // missing T
        "2026-07-08T18Z",     // missing minutes
        "2026-07-08T18:15:30:00Z", // too many time parts
        "2026-07-08T183:15Z", // 3-digit hour: chrono %H reads 2 digits max
        "2026-07-08T18:157Z", // 3-digit minute
        "",
        "garbage",
        "2025-02-29T12:00Z",  // non-leap Feb 29
    ] {
        assert_matches_oracle(text);
    }
    // Leap years both ways around the century rule.
    assert_matches_oracle("2024-02-29T12:00Z");
    assert_matches_oracle("2000-02-29T12:00Z");
    assert_matches_oracle("1900-02-29T12:00Z");
}

#[test]
fn start_time_corpus_values() {
    // The two shapes the fixtures actually carry.
    assert_eq!(parse_start_time("2026-07-07T18:15Z"), oracle("2026-07-07T18:15Z"));
    assert!(parse_start_time("2026-07-07T18:15Z").is_some());
}

#[test]
fn hex_rgb_matches_backend_semantics() {
    assert_eq!(parse_hex_rgb("be0a14"), Some(0xbe0a14));
    assert_eq!(parse_hex_rgb("#0C2340"), Some(0x0C2340));
    assert_eq!(parse_hex_rgb("0C2340"), Some(0x0C2340));
    assert_eq!(parse_hex_rgb("#fff"), None); // 3-digit shorthand rejected
    assert_eq!(parse_hex_rgb("fffffff"), None); // 7 digits
    assert_eq!(parse_hex_rgb("ggg000"), None);
    // The documented from_str_radix quirk carries over by construction.
    assert_eq!(parse_hex_rgb("+12345"), Some(0x12345));
}

#[test]
fn record_split_and_the_tie_quirk() {
    assert_eq!(parse_record("51-29"), Some((51, 29)));
    assert_eq!(parse_record("0-0"), Some((0, 0)));
    assert_eq!(parse_record("TBD"), None);
    // split_once: a tie record drops entirely — bug-compatible (ruling 7).
    assert_eq!(parse_record("12-1-1"), None);
}

#[test]
fn score_saturates() {
    assert_eq!(saturate_score(103), 103);
    assert_eq!(saturate_score(70_000), u16::MAX);
}

#[test]
fn ordering_is_by_marker_never_index() {
    assert_eq!(
        order_home_away((HomeAway::Away, "KC"), (HomeAway::Home, "BUF")),
        Some(("BUF", "KC"))
    );
    assert_eq!(
        order_home_away((HomeAway::Home, "BUF"), (HomeAway::Away, "KC")),
        Some(("BUF", "KC"))
    );
    assert_eq!(
        order_home_away((HomeAway::Home, "A"), (HomeAway::Home, "B")),
        None
    );
}

#[test]
fn set_text_truncates_at_char_boundaries_like_the_wire() {
    let mut out = heapless::String::<8>::new();
    set_text(&mut out, "abcdefghij");
    assert_eq!(out.as_str(), "abcdefgh");
    // 'é' is two bytes; a cut inside it must back up.
    set_text(&mut out, "abcdefgé");
    assert_eq!(out.as_str(), "abcdefg");
}
