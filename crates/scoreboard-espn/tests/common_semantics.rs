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
fn integer_shape_parsers_match_serde_json() {
    use scoreboard_espn::common::{num_i16, num_u8};
    // The oracle: what serde_json accepts for integer fields. Scope: inputs
    // that are grammar-valid JSON numbers — the helpers only ever see text
    // picojson's tokenizer already validated, so grammar rejects (leading
    // zeros, bare signs, empty) are upstream's job, not theirs.
    for text in [
        "0", "7", "255", "256", "-1", "-32768", "-32769", "-0", "2.0", "1e2", "0.5",
    ] {
        assert_eq!(
            num_i16(text).is_some(),
            serde_json::from_str::<i16>(text).is_ok(),
            "num_i16 diverged from serde_json on {text:?}"
        );
        assert_eq!(
            num_u8(text).is_some(),
            serde_json::from_str::<u8>(text).is_ok(),
            "num_u8 diverged from serde_json on {text:?}"
        );
    }
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

// --------------------------------------------------------------- crest URLs

use scoreboard_espn::common::{CDN_ORIGIN, CREST_PATH_BYTES, crest_path, crest_url};

/// One real href per URL shape ESPN actually serves, with the league it came
/// from. These five are the whole reason the crest href is taken from the
/// payload rather than built from a template: the path family, the key
/// (abbreviation vs numeric team id) and the `scoreboard/` segment all vary,
/// and three of the eight leagues the firmware ships have no captured body to
/// check a sixth guess against.
const SHAPES: &[(&str, &str)] = &[
    // backend/testdata/mlb/pregame.json
    (
        "baseball/mlb",
        "https://a.espncdn.com/i/teamlogos/mlb/500/scoreboard/sf.png",
    ),
    // backend/testdata/nba/halftime.json — the longest path in the corpus.
    (
        "basketball/nba",
        "https://a.espncdn.com/i/teamlogos/nba/500/scoreboard/utah.png",
    ),
    // firmware-rs/bench/assets/body-cfb-live.json
    (
        "football/college-football",
        "https://a.espncdn.com/i/teamlogos/ncaa/500/2628.png",
    ),
    // firmware-rs/bench/assets/body-mls-max.json
    (
        "soccer/usa.1",
        "https://a.espncdn.com/i/teamlogos/soccer/500/17606.png",
    ),
    // backend/testdata/soccer/fifa.world/full_time.json
    (
        "soccer/fifa.world",
        "https://a.espncdn.com/i/teamlogos/countries/500/por.png",
    ),
];

/// The backend fetches the payload href verbatim (`backend/src/team.rs`), so
/// origin + path must put it back together byte for byte.
#[test]
fn every_corpus_shape_round_trips_to_the_href_the_backend_fetches() {
    for (league, href) in SHAPES {
        let path = crest_path(href).unwrap_or_else(|| panic!("{league}: {href} has no path"));
        let rebuilt = format!("{CDN_ORIGIN}{}", path.as_str());
        assert_eq!(&rebuilt, href, "{league} does not round trip");
        assert!(
            path.len() <= CREST_PATH_BYTES,
            "{league}: {} bytes exceeds the bound",
            path.len()
        );
    }
}

#[test]
fn only_the_espn_cdn_is_honored() {
    // The backend's rule: anything off the CDN is the logo being absent.
    assert!(crest_path("https://evil.example.com/i/teamlogos/mlb/500/scoreboard/sf.png").is_none());
    // A lookalike host must not pass a prefix check.
    assert!(crest_path("https://a.espncdn.com.evil.test/i/teamlogos/mlb/500/sf.png").is_none());
    // Scheme matters: the origin includes it.
    assert!(crest_path("http://a.espncdn.com/i/teamlogos/mlb/500/sf.png").is_none());
    // The origin alone names no image.
    assert!(crest_path(CDN_ORIGIN).is_none());
    assert!(crest_path("").is_none());
}

#[test]
fn an_over_long_path_is_no_crest_rather_than_a_truncated_one() {
    let long = format!("{CDN_ORIGIN}/i/teamlogos/{}.png", "x".repeat(CREST_PATH_BYTES));
    assert!(
        crest_path(&long).is_none(),
        "truncation would build a URL that 404s"
    );
}

/// The 100 px combiner variant S3 fetches: ~3–4 KB and ~8.3 ms to decode,
/// against 13–40 KB and 156–209 ms for the 500 px original the href names
/// (PARSE-PERF.md). Nothing in the payload links it — the corpus has no
/// team-level combiner URL at all — so the size is asked for here.
#[test]
fn the_combiner_url_wraps_the_payload_path_with_a_size() {
    let path = crest_path("https://a.espncdn.com/i/teamlogos/mlb/500/scoreboard/nyy.png")
        .expect("a corpus href");
    let url = crest_url(&path, 100).expect("fits");
    assert_eq!(
        url.as_str(),
        "https://a.espncdn.com/combiner/i?img=/i/teamlogos/mlb/500/scoreboard/nyy.png\
         &w=100&h=100&transparent=true"
    );
}

/// The bound has to hold for the longest path at `u16`'s widest size, or the
/// crest that overflows is the one that silently disappears.
#[test]
fn the_url_bound_covers_the_longest_path_at_any_size() {
    let path = "/".repeat(CREST_PATH_BYTES);
    for pixels in [1u16, 100, 500, u16::MAX] {
        assert!(
            crest_url(&path, pixels).is_some(),
            "{pixels} px overflowed the URL bound"
        );
    }
}
