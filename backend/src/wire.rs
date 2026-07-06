//! Binary wire format for the Pico firmware — **this doc comment is the
//! normative spec** (the firmware parser in `firmware/src/scoreboard/mlb.py`
//! references it).
//!
//! Content negotiation: the client sends `Accept: application/x-scoreboard-struct`
//! and the server responds with that content type; otherwise JSON is served as
//! before. Error responses (4xx/5xx) are always JSON. All integers are
//! little-endian. Every payload begins with a version byte (currently 1) —
//! bump it on any layout change.
//!
//! # LiveGame (`GET /mlb/games/{game_id}`)
//!
//! Fixed section, 28 bytes (MicroPython: one
//! `struct.unpack_from('<BBBBBBBBHHIIII', buf, 0)`):
//!
//! | off | type | field                                            |
//! |-----|------|--------------------------------------------------|
//! | 0   | u8   | version = 1                                      |
//! | 1   | u8   | flags (bit0 = at_bat present)                    |
//! | 2   | u8   | inning_number                                    |
//! | 3   | u8   | inning_half (0=top, 1=middle, 2=bottom, 3=end)   |
//! | 4   | u8   | balls                                            |
//! | 5   | u8   | strikes                                          |
//! | 6   | u8   | outs                                             |
//! | 7   | u8   | bases bitfield (bit0=first, bit1=second, bit2=third) |
//! | 8   | u16  | away_score (u32 saturated to u16)                |
//! | 10  | u16  | home_score                                       |
//! | 12  | u32  | away_colors.primary (0x00RRGGBB)                 |
//! | 16  | u32  | away_colors.alternate                            |
//! | 20  | u32  | home_colors.primary                              |
//! | 24  | u32  | home_colors.alternate                            |
//!
//! Strings section immediately follows: each string is `u8 length` + UTF-8
//! bytes (strings longer than 255 bytes are truncated at a char boundary),
//! in this fixed order:
//!
//! 1. `game_id`
//! 2. `away.abbreviation`
//! 3. `home.abbreviation`
//! 4. `at_bat.pitcher` — present **iff** flags bit0
//! 5. `at_bat.batter`  — present **iff** flags bit0
//! 6. `last_play.id`
//! 7. `last_play.text`
//!
//! No trailing bytes are permitted after the last string.
//!
//! # Game ID list (`GET /mlb/games`)
//!
//! `u8 version = 1`, `u8 count`, then per id: `u8 length` + UTF-8 bytes.
//! The ETag / If-None-Match / 304 flow is format-independent and unchanged.

use crate::mlb::{InningHalf, LiveGame};

pub const STRUCT_CONTENT_TYPE: &str = "application/x-scoreboard-struct";
pub const WIRE_VERSION: u8 = 1;

const FLAG_AT_BAT: u8 = 0x01;
const MAX_STRING_BYTES: usize = 255;

/// Truncate to at most `max` bytes without splitting a UTF-8 char.
/// (`str::floor_char_boundary` is nightly-only, hence the manual walk.)
fn truncate_utf8(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut n = max;
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    &s[..n]
}

/// Append one length-prefixed string (truncated to 255 bytes if needed).
fn push_str(out: &mut Vec<u8>, s: &str) {
    let bytes = truncate_utf8(s, MAX_STRING_BYTES).as_bytes();
    out.push(bytes.len() as u8);
    out.extend_from_slice(bytes);
}

fn half_code(half: &InningHalf) -> u8 {
    match half {
        InningHalf::Top => 0,
        InningHalf::Middle => 1,
        InningHalf::Bottom => 2,
        InningHalf::End => 3,
    }
}

pub fn encode_live_game(game: &LiveGame) -> Vec<u8> {
    // Fixed section (28) + typical string payload comfortably under 256.
    let mut out = Vec::with_capacity(256);

    out.push(WIRE_VERSION);
    out.push(if game.at_bat.is_some() { FLAG_AT_BAT } else { 0 });
    out.push(game.inning.number);
    out.push(half_code(&game.inning.half));
    out.push(game.count.balls);
    out.push(game.count.strikes);
    out.push(game.count.outs);
    out.push(
        (game.bases.first as u8) | ((game.bases.second as u8) << 1) | ((game.bases.third as u8) << 2),
    );
    out.extend_from_slice(&(game.away.score.min(u16::MAX as u32) as u16).to_le_bytes());
    out.extend_from_slice(&(game.home.score.min(u16::MAX as u32) as u16).to_le_bytes());
    out.extend_from_slice(&game.away.colors.primary.to_le_bytes());
    out.extend_from_slice(&game.away.colors.alternate.to_le_bytes());
    out.extend_from_slice(&game.home.colors.primary.to_le_bytes());
    out.extend_from_slice(&game.home.colors.alternate.to_le_bytes());

    push_str(&mut out, &game.game_id);
    push_str(&mut out, &game.away.abbreviation);
    push_str(&mut out, &game.home.abbreviation);
    if let Some(at_bat) = &game.at_bat {
        push_str(&mut out, &at_bat.pitcher);
        push_str(&mut out, &at_bat.batter);
    }
    push_str(&mut out, &game.last_play.id);
    push_str(&mut out, &game.last_play.text);

    out
}

pub fn encode_game_ids(ids: &[String]) -> Vec<u8> {
    if ids.len() > 255 {
        // u8 count caps the list; unreachable for MLB (~15 games/day) but
        // never truncate silently.
        tracing::warn!(count = ids.len(), "game id list exceeds wire cap; truncating to 255");
    }
    let mut out = Vec::with_capacity(2 + ids.len() * 12);
    out.push(WIRE_VERSION);
    out.push(ids.len().min(255) as u8);
    for id in ids.iter().take(255) {
        push_str(&mut out, id);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mlb::{AtBat, Bases, Count, Inning, LastPlay, TeamColors, TeamState};

    fn team(abbrev: &str, score: u32, primary: u32, alternate: u32) -> TeamState {
        TeamState {
            abbreviation: abbrev.to_string(),
            score,
            colors: TeamColors { primary, alternate },
        }
    }

    /// The golden fixtures below are shared verbatim with the firmware-parser
    /// cross-check in `tools/wire_format_check.py`. If a spec change alters
    /// them, update both copies (and bump WIRE_VERSION).
    const GOLDEN_FULL: &str = "010107020302020503000500562c0c005c5c00003930bd0040230c00093430313537303732390353454103424f530b472e20576869746c6f636b0d4a2e20526f6472c3ad6775657a0d34303135373037323930303731294a756c696f20526f6472c3ad6775657a2073696e676c657320746f2063656e746572206669656c642e";
    const GOLDEN_MINIMAL: &str = "010001000000000000000000332211006655440099887700ccbbaa0009343031353730303031034e595903544f5202703100";
    const GOLDEN_IDS: &str = "01020934303135373037323909343031353730303031";

    fn full_fixture() -> LiveGame {
        LiveGame {
            game_id: "401570729".to_string(),
            inning: Inning {
                number: 7,
                half: InningHalf::Bottom,
            },
            home: team("BOS", 5, 0xBD3039, 0x0C2340),
            away: team("SEA", 3, 0x0C2C56, 0x005C5C),
            count: Count {
                balls: 3,
                strikes: 2,
                outs: 2,
            },
            bases: Bases {
                first: true,
                second: false,
                third: true,
            },
            at_bat: Some(AtBat {
                pitcher: "G. Whitlock".to_string(),
                batter: "J. Rodríguez".to_string(),
            }),
            last_play: LastPlay {
                id: "4015707290071".to_string(),
                text: "Julio Rodríguez singles to center field.".to_string(),
            },
        }
    }

    #[test]
    fn golden_full_game() {
        assert_eq!(hex::encode(encode_live_game(&full_fixture())), GOLDEN_FULL);
    }

    #[test]
    fn golden_minimal_game_without_at_bat() {
        let game = LiveGame {
            game_id: "401570001".to_string(),
            inning: Inning {
                number: 1,
                half: InningHalf::Top,
            },
            home: team("TOR", 0, 0x778899, 0xAABBCC),
            away: team("NYY", 0, 0x112233, 0x445566),
            count: Count {
                balls: 0,
                strikes: 0,
                outs: 0,
            },
            bases: Bases {
                first: false,
                second: false,
                third: false,
            },
            at_bat: None,
            last_play: LastPlay {
                id: "p1".to_string(),
                text: String::new(),
            },
        };
        assert_eq!(hex::encode(encode_live_game(&game)), GOLDEN_MINIMAL);
    }

    #[test]
    fn golden_game_ids() {
        let ids = vec!["401570729".to_string(), "401570001".to_string()];
        assert_eq!(hex::encode(encode_game_ids(&ids)), GOLDEN_IDS);
    }

    #[test]
    fn score_saturates_to_u16() {
        let mut game = full_fixture();
        game.home.score = 1_000_000;
        let bytes = encode_live_game(&game);
        // home_score sits at offset 10..12 (see spec table).
        assert_eq!(u16::from_le_bytes([bytes[10], bytes[11]]), u16::MAX);
    }

    #[test]
    fn push_str_truncates_at_char_boundary() {
        // 254 ASCII bytes then 'é' (2 bytes, straddling the 255 limit): the
        // whole char must be dropped, leaving exactly 254 valid bytes.
        let mut out = Vec::new();
        push_str(&mut out, &format!("{}é", "x".repeat(254)));
        assert_eq!(out[0], 254);
        assert_eq!(out.len(), 255); // length prefix + 254 payload bytes
        assert!(std::str::from_utf8(&out[1..]).is_ok());
    }

    #[test]
    fn truncate_utf8_is_boundary_safe() {
        assert_eq!(truncate_utf8("héllo", 2), "h"); // 'é' straddles byte 2
        assert_eq!(truncate_utf8("héllo", 3), "hé");
        assert_eq!(truncate_utf8("abc", 255), "abc");
    }
}
