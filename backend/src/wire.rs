//! Binary wire format for the Pico firmware — **this doc comment is the
//! normative spec** (the firmware parser in `firmware/src/scoreboard/mlb.py`
//! and the cross-check in `tools/wire_format_check.py` reference it).
//!
//! Content negotiation: the client sends `Accept: application/x-scoreboard-struct`
//! and the server responds with that content type; otherwise JSON is served.
//! Error responses (4xx/5xx) are always JSON. All integers are little-endian.
//! Strings are `u8 length` + UTF-8 bytes, truncated at a char boundary if they
//! exceed 255 bytes. No trailing bytes follow the last field.
//!
//! `WIRE_VERSION = 2`. State codes match [`crate::mlb::GameState::code`]:
//! `0 = pregame`, `1 = live`, `2 = final`.
//!
//! # Games list (`GET /baseball/mlb/games`)
//!
//! `u8 version = 2`, `u8 count`, then per game: `u8 state` + length-prefixed
//! `id`. The ETag / If-None-Match / 304 flow is format-independent.
//!
//! # Game detail (`GET /baseball/mlb/games/{game_id}`)
//!
//! Common 2-byte header: `u8 version = 2`, `u8 state`. The variant payload
//! follows at offset 2.
//!
//! ## Live (state = 1)
//!
//! Exactly the v1 LiveGame body minus its leading version byte. Fixed 27-byte
//! section at offset 2 (`struct.unpack_from('<BBBBBBBHHIIII', buf, 2)`):
//!
//! | off | type | field                                            |
//! |-----|------|--------------------------------------------------|
//! | 2   | u8   | flags (bit0 = at_bat present)                    |
//! | 3   | u8   | inning_number                                    |
//! | 4   | u8   | inning_half (0=top, 1=middle, 2=bottom, 3=end)   |
//! | 5   | u8   | balls                                            |
//! | 6   | u8   | strikes                                          |
//! | 7   | u8   | outs                                             |
//! | 8   | u8   | bases bitfield (bit0=first, bit1=second, bit2=third) |
//! | 9   | u16  | away_score (u32 saturated to u16)                |
//! | 11  | u16  | home_score                                       |
//! | 13  | u32  | away_colors.primary (0x00RRGGBB)                 |
//! | 17  | u32  | away_colors.alternate                            |
//! | 21  | u32  | home_colors.primary                              |
//! | 25  | u32  | home_colors.alternate                            |
//!
//! Strings from offset 29, in order: `game_id`, `away.abbreviation`,
//! `home.abbreviation`, then `at_bat.pitcher` and `at_bat.batter` **iff** flags
//! bit0, then `last_play.id`, `last_play.text`.
//!
//! ## Pregame (state = 0)
//!
//! Fixed 30-byte section at offset 2 (`<BBHHHHIIIII`):
//!
//! | off | type | field                                            |
//! |-----|------|--------------------------------------------------|
//! | 2   | u8   | flags (see below)                                |
//! | 3   | u8   | temperature °F, clamped 0..=255 (0 if no weather)|
//! | 4   | u16  | away_wins  (0 if no record)                      |
//! | 6   | u16  | away_losses                                      |
//! | 8   | u16  | home_wins                                        |
//! | 10  | u16  | home_losses                                      |
//! | 12  | u32  | start_time (unix epoch, seconds, UTC)            |
//! | 16  | u32  | away_colors.primary                              |
//! | 20  | u32  | away_colors.alternate                            |
//! | 24  | u32  | home_colors.primary                              |
//! | 28  | u32  | home_colors.alternate                            |
//!
//! Flags: bit0 = weather present, bit1 = away record, bit2 = home record,
//! bit3 = away probable, bit4 = home probable. Numeric fields whose flag is
//! unset are zero.
//!
//! Strings from offset 32, in order: `game_id`, `away.abbreviation`,
//! `home.abbreviation`, `venue`, then `weather.condition` **iff** bit0,
//! `away.probable_pitcher` **iff** bit3, `home.probable_pitcher` **iff** bit4.
//!
//! ## Final (state = 2)
//!
//! Fixed 23-byte section at offset 2 (`<BBBHHIIII`):
//!
//! | off | type | field                                            |
//! |-----|------|--------------------------------------------------|
//! | 2   | u8   | innings_played                                   |
//! | 3   | u8   | away_linescore_len (nA)                          |
//! | 4   | u8   | home_linescore_len (nH)                          |
//! | 5   | u16  | away_score (u32 saturated to u16)                |
//! | 7   | u16  | home_score                                       |
//! | 9   | u32  | away_colors.primary                              |
//! | 13  | u32  | away_colors.alternate                            |
//! | 17  | u32  | home_colors.primary                              |
//! | 21  | u32  | home_colors.alternate                            |
//!
//! Then `nA` bytes of away per-inning runs (u8, inning 1 first), `nH` bytes of
//! home runs, then strings: `game_id`, `away.abbreviation`,
//! `home.abbreviation`. Per-team lengths are independent (a walk-off leaves the
//! home line short; extras run past 9).

use crate::mlb::{FinalGame, GameListEntry, InningHalf, LiveGame, MlbGame, PregameGame};

pub const STRUCT_CONTENT_TYPE: &str = "application/x-scoreboard-struct";
pub const WIRE_VERSION: u8 = 2;

const FLAG_AT_BAT: u8 = 0x01;

const FLAG_WEATHER: u8 = 0x01;
const FLAG_AWAY_RECORD: u8 = 0x02;
const FLAG_HOME_RECORD: u8 = 0x04;
const FLAG_AWAY_PROBABLE: u8 = 0x08;
const FLAG_HOME_PROBABLE: u8 = 0x10;

const MAX_STRING_BYTES: usize = 255;
const MAX_LINESCORE_LEN: usize = 255;

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

fn score_u16(score: u32) -> [u8; 2] {
    (score.min(u16::MAX as u32) as u16).to_le_bytes()
}

/// Encode one game detail: the 2-byte header followed by the state's payload.
pub fn encode_game(game: &MlbGame) -> Vec<u8> {
    let mut out = Vec::with_capacity(256);
    out.push(WIRE_VERSION);
    match game {
        MlbGame::Pregame(g) => {
            out.push(0);
            write_pregame(&mut out, g);
        }
        MlbGame::Live(g) => {
            out.push(1);
            write_live(&mut out, g);
        }
        MlbGame::Final(g) => {
            out.push(2);
            write_final(&mut out, g);
        }
    }
    out
}

fn write_live(out: &mut Vec<u8>, game: &LiveGame) {
    out.push(if game.at_bat.is_some() { FLAG_AT_BAT } else { 0 });
    out.push(game.inning.number);
    out.push(half_code(&game.inning.half));
    out.push(game.count.balls);
    out.push(game.count.strikes);
    out.push(game.count.outs);
    out.push(
        (game.bases.first as u8)
            | ((game.bases.second as u8) << 1)
            | ((game.bases.third as u8) << 2),
    );
    out.extend_from_slice(&score_u16(game.away.score));
    out.extend_from_slice(&score_u16(game.home.score));
    out.extend_from_slice(&game.away.colors.primary.to_le_bytes());
    out.extend_from_slice(&game.away.colors.alternate.to_le_bytes());
    out.extend_from_slice(&game.home.colors.primary.to_le_bytes());
    out.extend_from_slice(&game.home.colors.alternate.to_le_bytes());

    push_str(out, &game.game_id);
    push_str(out, &game.away.abbreviation);
    push_str(out, &game.home.abbreviation);
    if let Some(at_bat) = &game.at_bat {
        push_str(out, &at_bat.pitcher);
        push_str(out, &at_bat.batter);
    }
    push_str(out, &game.last_play.id);
    push_str(out, &game.last_play.text);
}

fn write_pregame(out: &mut Vec<u8>, game: &PregameGame) {
    let mut flags = 0u8;
    let temperature = match &game.weather {
        Some(w) => {
            flags |= FLAG_WEATHER;
            w.temperature.clamp(0, u8::MAX as i16) as u8
        }
        None => 0,
    };
    let (away_wins, away_losses) = match &game.away.record {
        Some(r) => {
            flags |= FLAG_AWAY_RECORD;
            (r.wins, r.losses)
        }
        None => (0, 0),
    };
    let (home_wins, home_losses) = match &game.home.record {
        Some(r) => {
            flags |= FLAG_HOME_RECORD;
            (r.wins, r.losses)
        }
        None => (0, 0),
    };
    if game.away.probable_pitcher.is_some() {
        flags |= FLAG_AWAY_PROBABLE;
    }
    if game.home.probable_pitcher.is_some() {
        flags |= FLAG_HOME_PROBABLE;
    }

    out.push(flags);
    out.push(temperature);
    out.extend_from_slice(&away_wins.to_le_bytes());
    out.extend_from_slice(&away_losses.to_le_bytes());
    out.extend_from_slice(&home_wins.to_le_bytes());
    out.extend_from_slice(&home_losses.to_le_bytes());
    out.extend_from_slice(&game.start_time.to_le_bytes());
    out.extend_from_slice(&game.away.colors.primary.to_le_bytes());
    out.extend_from_slice(&game.away.colors.alternate.to_le_bytes());
    out.extend_from_slice(&game.home.colors.primary.to_le_bytes());
    out.extend_from_slice(&game.home.colors.alternate.to_le_bytes());

    push_str(out, &game.game_id);
    push_str(out, &game.away.abbreviation);
    push_str(out, &game.home.abbreviation);
    push_str(out, &game.venue);
    if let Some(w) = &game.weather {
        push_str(out, &w.condition);
    }
    if let Some(p) = &game.away.probable_pitcher {
        push_str(out, p);
    }
    if let Some(p) = &game.home.probable_pitcher {
        push_str(out, p);
    }
}

/// A per-team line score, capped at the u8 length prefix. The cap is a safety
/// net — a real game never reaches 255 innings.
fn line_score_bytes(line_score: &[u8]) -> &[u8] {
    if line_score.len() > MAX_LINESCORE_LEN {
        tracing::warn!(len = line_score.len(), "line score exceeds wire cap; truncating");
        &line_score[..MAX_LINESCORE_LEN]
    } else {
        line_score
    }
}

fn write_final(out: &mut Vec<u8>, game: &FinalGame) {
    let away_ls = line_score_bytes(&game.away.line_score);
    let home_ls = line_score_bytes(&game.home.line_score);

    out.push(game.innings_played);
    out.push(away_ls.len() as u8);
    out.push(home_ls.len() as u8);
    out.extend_from_slice(&score_u16(game.away.score));
    out.extend_from_slice(&score_u16(game.home.score));
    out.extend_from_slice(&game.away.colors.primary.to_le_bytes());
    out.extend_from_slice(&game.away.colors.alternate.to_le_bytes());
    out.extend_from_slice(&game.home.colors.primary.to_le_bytes());
    out.extend_from_slice(&game.home.colors.alternate.to_le_bytes());

    out.extend_from_slice(away_ls);
    out.extend_from_slice(home_ls);

    push_str(out, &game.game_id);
    push_str(out, &game.away.abbreviation);
    push_str(out, &game.home.abbreviation);
}

/// Encode the games list: version, count, then `u8 state` + id per entry.
pub fn encode_game_list(entries: &[GameListEntry]) -> Vec<u8> {
    if entries.len() > 255 {
        // u8 count caps the list; unreachable for MLB (~15 games/day) but
        // never truncate silently.
        tracing::warn!(count = entries.len(), "game list exceeds wire cap; truncating to 255");
    }
    let mut out = Vec::with_capacity(2 + entries.len() * 13);
    out.push(WIRE_VERSION);
    out.push(entries.len().min(255) as u8);
    for entry in entries.iter().take(255) {
        out.push(entry.state.code());
        push_str(&mut out, &entry.id);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mlb::{
        AtBat, Bases, Count, FinalTeam, GameState, Inning, LastPlay, PregameTeam, Record, TeamColors,
        TeamState, Weather,
    };

    fn team(abbrev: &str, score: u32, primary: u32, alternate: u32) -> TeamState {
        TeamState {
            abbreviation: abbrev.to_string(),
            score,
            colors: TeamColors { primary, alternate },
        }
    }

    // The golden fixtures below are shared verbatim with the firmware-parser
    // cross-check in `tools/wire_format_check.py`. If a spec change alters
    // them, update both copies (and bump WIRE_VERSION).

    // v1 LiveGame body (version byte + rest), retained to pin the "live = v2
    // header + v1 body minus version byte" invariant.
    const GOLDEN_FULL_V1: &str = "010107020302020503000500562c0c005c5c00003930bd0040230c00093430313537303732390353454103424f530b472e20576869746c6f636b0d4a2e20526f6472c3ad6775657a0d34303135373037323930303731294a756c696f20526f6472c3ad6775657a2073696e676c657320746f2063656e746572206669656c642e";

    fn full_live_fixture() -> LiveGame {
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
    fn golden_live_is_v2_header_plus_v1_body() {
        // The v2 live payload must equal [0x02, 0x01] ++ v1_body[1..] — i.e.
        // the old body with its version byte replaced by the 2-byte header.
        let expected = format!("0201{}", &GOLDEN_FULL_V1[2..]);
        let encoded = encode_game(&MlbGame::Live(full_live_fixture()));
        assert_eq!(hex::encode(&encoded), expected);
    }

    fn pregame_fixture() -> PregameGame {
        PregameGame {
            game_id: "401570001".to_string(),
            start_time: 1_783_647_600,
            venue: "Petco Park".to_string(),
            weather: Some(Weather {
                condition: "Mostly sunny".to_string(),
                temperature: 72,
            }),
            away: PregameTeam {
                abbreviation: "NYY".to_string(),
                colors: TeamColors {
                    primary: 0x003087,
                    alternate: 0xE4002C,
                },
                record: Some(Record { wins: 44, losses: 46 }),
                probable_pitcher: Some("G. Marquez".to_string()),
            },
            home: PregameTeam {
                abbreviation: "SD".to_string(),
                colors: TeamColors {
                    primary: 0x2F241D,
                    alternate: 0xFFC425,
                },
                record: Some(Record { wins: 50, losses: 40 }),
                probable_pitcher: Some("Y. Darvish".to_string()),
            },
        }
    }

    const GOLDEN_PRE: &str = "02001f482c002e0032002800704d506a873000002c00e4001d242f0025c4ff0009343031353730303031034e59590253440a506574636f205061726b0c4d6f73746c792073756e6e790a472e204d61727175657a0a592e2044617276697368";

    #[test]
    fn golden_pregame_all_flags() {
        assert_eq!(hex::encode(encode_game(&MlbGame::Pregame(pregame_fixture()))), GOLDEN_PRE);
    }

    const GOLDEN_PRE_NO_WEATHER: &str = "020000000000000000000000704d506a873000002c00e4001d242f0025c4ff0009343031353730303031034e59590253440a506574636f205061726b";

    #[test]
    fn golden_pregame_no_optional_data() {
        let mut game = pregame_fixture();
        game.weather = None;
        game.away.record = None;
        game.home.record = None;
        game.away.probable_pitcher = None;
        game.home.probable_pitcher = None;
        assert_eq!(
            hex::encode(encode_game(&MlbGame::Pregame(game))),
            GOLDEN_PRE_NO_WEATHER
        );
    }

    #[test]
    fn pregame_temperature_clamps_to_u8() {
        let mut game = pregame_fixture();
        game.weather = Some(Weather {
            condition: "Scorching".to_string(),
            temperature: 300,
        });
        let bytes = encode_game(&MlbGame::Pregame(game));
        // temperature sits at offset 3 (after version, state, flags).
        assert_eq!(bytes[3], 255);
    }

    #[test]
    fn pregame_negative_temperature_clamps_to_zero() {
        let mut game = pregame_fixture();
        game.weather = Some(Weather {
            condition: "Frigid".to_string(),
            temperature: -40,
        });
        let bytes = encode_game(&MlbGame::Pregame(game));
        assert_eq!(bytes[3], 0);
    }

    fn final_fixture() -> FinalGame {
        FinalGame {
            game_id: "401570729".to_string(),
            innings_played: 9,
            away: FinalTeam {
                abbreviation: "SEA".to_string(),
                score: 4,
                colors: TeamColors {
                    primary: 0x0C2C56,
                    alternate: 0x005C5C,
                },
                // 9 innings.
                line_score: vec![1, 0, 0, 2, 0, 0, 1, 0, 0],
            },
            home: FinalTeam {
                abbreviation: "BOS".to_string(),
                score: 5,
                colors: TeamColors {
                    primary: 0xBD3039,
                    alternate: 0x0C2340,
                },
                // Walk-off: home bats 8 innings (short line).
                line_score: vec![0, 1, 0, 0, 2, 0, 0, 2],
            },
        }
    }

    const GOLDEN_FINAL: &str = "020209090804000500562c0c005c5c00003930bd0040230c000100000200000100000001000002000002093430313537303732390353454103424f53";

    #[test]
    fn golden_final_uneven_line_scores() {
        assert_eq!(
            hex::encode(encode_game(&MlbGame::Final(final_fixture()))),
            GOLDEN_FINAL
        );
    }

    #[test]
    fn final_score_saturates_to_u16() {
        let mut game = final_fixture();
        game.home.score = 1_000_000;
        let bytes = encode_game(&MlbGame::Final(game));
        // home_score at offset 7..9 (version, state, innings, nA, nH, away u16).
        assert_eq!(u16::from_le_bytes([bytes[7], bytes[8]]), u16::MAX);
    }

    #[test]
    fn live_score_saturates_to_u16() {
        let mut game = full_live_fixture();
        game.home.score = 1_000_000;
        let bytes = encode_game(&MlbGame::Live(game));
        // home_score at offset 11..13 (see live spec table).
        assert_eq!(u16::from_le_bytes([bytes[11], bytes[12]]), u16::MAX);
    }

    fn entry(id: &str, state: GameState) -> GameListEntry {
        GameListEntry {
            id: id.to_string(),
            state,
        }
    }

    const GOLDEN_LIST: &str =
        "0203000934303135373037323901093430313537303030310209343031353730303032";

    #[test]
    fn golden_game_list_mixed_states() {
        let entries = vec![
            entry("401570729", GameState::Pregame),
            entry("401570001", GameState::Live),
            entry("401570002", GameState::Final),
        ];
        assert_eq!(hex::encode(encode_game_list(&entries)), GOLDEN_LIST);
    }

    #[test]
    fn push_str_truncates_at_char_boundary() {
        let mut out = Vec::new();
        push_str(&mut out, &format!("{}é", "x".repeat(254)));
        assert_eq!(out[0], 254);
        assert_eq!(out.len(), 255);
        assert!(std::str::from_utf8(&out[1..]).is_ok());
    }

    #[test]
    fn truncate_utf8_is_boundary_safe() {
        assert_eq!(truncate_utf8("héllo", 2), "h");
        assert_eq!(truncate_utf8("héllo", 3), "hé");
        assert_eq!(truncate_utf8("abc", 255), "abc");
    }
}
