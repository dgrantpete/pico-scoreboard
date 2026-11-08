//! Golden payloads and their round trips.
//!
//! The hex constants are the byte-identity contract: they were produced by the
//! encoder the deployed MicroPython firmware decodes, and several were shared
//! verbatim with the Python cross-check this crate retired. Every
//! one is asserted in both directions — encode must produce it, decode must
//! reproduce the value — so the two halves cannot drift apart.

use std::string::String;
use std::{format, vec, vec::Vec};

use crate::error::{DecodeError, DecodeErrorKind};
use crate::{
    BufferFull, EXT_LEVEL_MAX, FinalTeam, GameState, LastPlay, LivePhase, Record, Side, SliceSink,
    TeamColors, TeamState, clamp_temperature, football, hockey, list, mlb, nba, saturate_score,
    soccer,
};

/// Every corpus payload fits comfortably; the largest is a soccer live game
/// with a full commentary line at ~200 bytes.
const SCRATCH: usize = 1024;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).expect("hex digit pair"))
        .collect()
}

/// Encode into a stack buffer (the firmware's no-alloc path) and hex it.
fn encoded(write: impl FnOnce(&mut SliceSink<'_>) -> Result<(), BufferFull>) -> String {
    let mut buf = [0u8; SCRATCH];
    let mut sink = SliceSink::new(&mut buf);
    write(&mut sink).expect("payload fits the scratch buffer");
    hex(sink.written())
}

fn team<'a>(abbreviation: &'a str, score: u16, primary: u32, alternate: u32) -> TeamState<'a> {
    TeamState {
        abbreviation,
        score,
        colors: TeamColors { primary, alternate },
    }
}

fn final_team<'a>(
    abbreviation: &'a str,
    score: u16,
    primary: u32,
    alternate: u32,
    line_score: &'a [u8],
) -> FinalTeam<'a> {
    FinalTeam {
        abbreviation,
        score,
        colors: TeamColors { primary, alternate },
        line_score,
    }
}

// --- MLB ---

// v1 LiveGame body (version byte + rest), retained to pin the "live = v2
// header + v1 body minus version byte" invariant.
const GOLDEN_MLB_LIVE_V1: &str = "010107020302020503000500562c0c005c5c00003930bd0040230c00093430313537303732390353454103424f530b472e20576869746c6f636b0d4a2e20526f6472c3ad6775657a0d34303135373037323930303731294a756c696f20526f6472c3ad6775657a2073696e676c657320746f2063656e746572206669656c642e";

fn mlb_live() -> mlb::Game<'static> {
    mlb::Game::Live(mlb::Live {
        game_id: "401570729",
        inning: mlb::Inning {
            number: 7,
            half: mlb::InningHalf::Bottom,
        },
        count: mlb::Count {
            balls: 3,
            strikes: 2,
            outs: 2,
        },
        bases: mlb::Bases {
            first: true,
            second: false,
            third: true,
        },
        away: team("SEA", 3, 0x0C2C56, 0x005C5C),
        home: team("BOS", 5, 0xBD3039, 0x0C2340),
        at_bat: Some(mlb::AtBat {
            pitcher: "G. Whitlock",
            batter: "J. Rodríguez",
        }),
        last_play: LastPlay {
            id: "4015707290071",
            text: "Julio Rodríguez singles to center field.",
        },
    })
}

#[test]
fn mlb_live_is_v2_header_plus_v1_body() {
    // The v2 live payload equals [0x02, 0x01] ++ v1_body[1..] — the old body
    // with its version byte replaced by the 2-byte header.
    let expected = format!("0201{}", &GOLDEN_MLB_LIVE_V1[2..]);
    assert_eq!(encoded(|out| mlb::encode(&mlb_live(), out)), expected);
    assert_eq!(mlb::decode(&unhex(&expected)).unwrap(), mlb_live());
}

fn mlb_pregame() -> mlb::Game<'static> {
    mlb::Game::Pregame(mlb::Pregame {
        game_id: "401570001",
        start_time: 1_783_647_600,
        venue: "Petco Park",
        weather: Some(mlb::Weather {
            condition: "Mostly sunny",
            temperature: 72,
        }),
        away: mlb::PregameTeam {
            abbreviation: "NYY",
            colors: TeamColors {
                primary: 0x003087,
                alternate: 0xE4002C,
            },
            record: Some(Record {
                wins: 44,
                losses: 46,
            }),
            probable_pitcher: Some("G. Marquez"),
        },
        home: mlb::PregameTeam {
            abbreviation: "SD",
            colors: TeamColors {
                primary: 0x2F241D,
                alternate: 0xFFC425,
            },
            record: Some(Record {
                wins: 50,
                losses: 40,
            }),
            probable_pitcher: Some("Y. Darvish"),
        },
    })
}

const GOLDEN_MLB_PRE: &str = "02001f482c002e0032002800704d506a873000002c00e4001d242f0025c4ff0009343031353730303031034e59590253440a506574636f205061726b0c4d6f73746c792073756e6e790a472e204d61727175657a0a592e2044617276697368";

#[test]
fn mlb_pregame_all_flags() {
    assert_eq!(
        encoded(|out| mlb::encode(&mlb_pregame(), out)),
        GOLDEN_MLB_PRE
    );
    assert_eq!(mlb::decode(&unhex(GOLDEN_MLB_PRE)).unwrap(), mlb_pregame());
}

const GOLDEN_MLB_PRE_BARE: &str = "020000000000000000000000704d506a873000002c00e4001d242f0025c4ff0009343031353730303031034e59590253440a506574636f205061726b";

#[test]
fn mlb_pregame_without_optional_data() {
    let mlb::Game::Pregame(mut pregame) = mlb_pregame() else {
        unreachable!()
    };
    pregame.weather = None;
    pregame.away.record = None;
    pregame.home.record = None;
    pregame.away.probable_pitcher = None;
    pregame.home.probable_pitcher = None;
    let game = mlb::Game::Pregame(pregame);
    assert_eq!(encoded(|out| mlb::encode(&game, out)), GOLDEN_MLB_PRE_BARE);
    assert_eq!(mlb::decode(&unhex(GOLDEN_MLB_PRE_BARE)).unwrap(), game);
}

fn mlb_final() -> mlb::Game<'static> {
    mlb::Game::Final(mlb::Final {
        game_id: "401570729",
        innings_played: 9,
        away: final_team("SEA", 4, 0x0C2C56, 0x005C5C, &[1, 0, 0, 2, 0, 0, 1, 0, 0]),
        // Walk-off: home bats 8 innings (short line).
        home: final_team("BOS", 5, 0xBD3039, 0x0C2340, &[0, 1, 0, 0, 2, 0, 0, 2]),
    })
}

const GOLDEN_MLB_FINAL: &str = "020209090804000500562c0c005c5c00003930bd0040230c000100000200000100000001000002000002093430313537303732390353454103424f53";

#[test]
fn mlb_final_uneven_line_scores() {
    assert_eq!(
        encoded(|out| mlb::encode(&mlb_final(), out)),
        GOLDEN_MLB_FINAL
    );
    assert_eq!(mlb::decode(&unhex(GOLDEN_MLB_FINAL)).unwrap(), mlb_final());
}

// --- Games list ---

const GOLDEN_LIST: &str = "0203000934303135373037323901093430313537303030310209343031353730303032";

fn list_entries() -> [list::Entry<'static>; 3] {
    [
        list::Entry {
            state: GameState::Pregame,
            id: "401570729",
        },
        list::Entry {
            state: GameState::Live,
            id: "401570001",
        },
        list::Entry {
            state: GameState::Final,
            id: "401570002",
        },
    ]
}

#[test]
fn game_list_mixed_states() {
    let entries = list_entries();
    assert_eq!(encoded(|out| list::encode(&entries, out)), GOLDEN_LIST);

    let bytes = unhex(GOLDEN_LIST);
    let iter = list::decode(&bytes).unwrap();
    assert_eq!(iter.remaining(), 3);
    let decoded: Vec<list::Entry<'_>> = iter.map(|entry| entry.unwrap()).collect();
    assert_eq!(decoded, entries.to_vec());
}

#[test]
fn empty_game_list_round_trips() {
    let encoded_empty = encoded(|out| list::encode(&[], out));
    assert_eq!(encoded_empty, "0200");
    let bytes = unhex(&encoded_empty);
    assert_eq!(list::decode(&bytes).unwrap().count(), 0);
}

// --- Soccer ---

fn soccer_live_base() -> soccer::Live<'static> {
    soccer::Live {
        game_id: "401800100",
        half: 1,
        clock_seconds: 51 * 60,
        on_break: false,
        away: team("BEL", 2, 0xE30613, 0xFDDA25),
        home: team("USA", 1, 0x002868, 0xBF0A30),
        last_event: Some(soccer::Event {
            kind: soccer::EventKind::Goal,
            side: Some(Side::Away),
            clock: "45'+1'",
            athlete: "R. Lukaku",
        }),
        commentary: None,
    }
}

const GOLDEN_SOCCER_LIVE: &str = "02010a01f40b020001001306e30025dafd0068280000300abf00093430313830303130300342454c03555341063435272b312709522e204c756b616b75";

#[test]
fn soccer_live_with_away_goal() {
    let game = soccer::Game::Live(soccer_live_base());
    assert_eq!(
        encoded(|out| soccer::encode(&game, out)),
        GOLDEN_SOCCER_LIVE
    );
    assert_eq!(soccer::decode(&unhex(GOLDEN_SOCCER_LIVE)).unwrap(), game);
}

const GOLDEN_SOCCER_LIVE_COMMENTARY: &str = "02012a01f40b020001001306e30025dafd0068280000300abf00093430313830303130300342454c03555341063435272b312709522e204c756b616b7502383753476f616c21202042656c6769756d20322c2055534120312e20526f6d656c75204c756b616b7520726967687420666f6f7465642073686f7420746f2074686520626f74746f6d206c65667420636f726e65722e";

#[test]
fn soccer_live_with_commentary() {
    let game = soccer::Game::Live(soccer::Live {
        commentary: Some(soccer::Commentary {
            id: "87",
            text: "Goal!  Belgium 2, USA 1. Romelu Lukaku right footed shot to the bottom left corner.",
        }),
        ..soccer_live_base()
    });
    assert_eq!(
        encoded(|out| soccer::encode(&game, out)),
        GOLDEN_SOCCER_LIVE_COMMENTARY
    );
    assert_eq!(
        soccer::decode(&unhex(GOLDEN_SOCCER_LIVE_COMMENTARY)).unwrap(),
        game
    );
}

const GOLDEN_SOCCER_HALFTIME: &str = "02011301f40b020002001306e30025dafd0068280000300abf00093430313830303130300342454c03555341063435272b31270a432e2050756c69736963";

#[test]
fn soccer_halftime_with_home_goal() {
    let game = soccer::Game::Live(soccer::Live {
        on_break: true,
        home: team("USA", 2, 0x002868, 0xBF0A30),
        last_event: Some(soccer::Event {
            kind: soccer::EventKind::Goal,
            side: Some(Side::Home),
            clock: "45'+1'",
            athlete: "C. Pulisic",
        }),
        ..soccer_live_base()
    });
    assert_eq!(
        encoded(|out| soccer::encode(&game, out)),
        GOLDEN_SOCCER_HALFTIME
    );
    assert_eq!(
        soccer::decode(&unhex(GOLDEN_SOCCER_HALFTIME)).unwrap(),
        game
    );
}

const GOLDEN_SOCCER_QUIET: &str =
    "020100026414000000001248000027e8ea0041975d00955500000934303138303031303103504f5203534541";

#[test]
fn soccer_live_without_an_event() {
    let game = soccer::Game::Live(soccer::Live {
        game_id: "401800101",
        half: 2,
        clock_seconds: 87 * 60,
        on_break: false,
        away: team("POR", 0, 0x004812, 0xEAE827),
        home: team("SEA", 0, 0x5D9741, 0x005595),
        last_event: None,
        commentary: None,
    });
    assert_eq!(
        encoded(|out| soccer::encode(&game, out)),
        GOLDEN_SOCCER_QUIET
    );
    assert_eq!(soccer::decode(&unhex(GOLDEN_SOCCER_QUIET)).unwrap(), game);
}

const GOLDEN_SOCCER_PRE: &str = "0200704d506a1248000027e8ea0041975d00955500000934303138303031303203504f52035345410b4c756d656e204669656c64";

#[test]
fn soccer_pregame() {
    let game = soccer::Game::Pregame(soccer::Pregame {
        game_id: "401800102",
        start_time: 1_783_647_600,
        venue: "Lumen Field",
        away: soccer::PregameTeam {
            abbreviation: "POR",
            colors: TeamColors {
                primary: 0x004812,
                alternate: 0xEAE827,
            },
        },
        home: soccer::PregameTeam {
            abbreviation: "SEA",
            colors: TeamColors {
                primary: 0x5D9741,
                alternate: 0x005595,
            },
        },
    });
    assert_eq!(encoded(|out| soccer::encode(&game, out)), GOLDEN_SOCCER_PRE);
    assert_eq!(soccer::decode(&unhex(GOLDEN_SOCCER_PRE)).unwrap(), game);
}

fn soccer_final(flavor: soccer::FinalFlavor) -> soccer::Game<'static> {
    soccer::Game::Final(soccer::Final {
        game_id: "401800103",
        flavor,
        away: soccer::FinalTeam {
            abbreviation: "ESP",
            score: 1,
            colors: TeamColors {
                primary: 0xFF0000,
                alternate: 0xFFC400,
            },
            scorers: "M. Merino 90'+1'",
        },
        home: soccer::FinalTeam {
            abbreviation: "POR",
            score: 0,
            colors: TeamColors {
                primary: 0x004812,
                alternate: 0xEAE827,
            },
            scorers: "",
        },
    })
}

const GOLDEN_SOCCER_FINAL: &str = "020200010000000000ff0000c4ff001248000027e8ea00093430313830303130330345535003504f52104d2e204d6572696e6f203930272b312700";

#[test]
fn soccer_final_with_scorers() {
    let game = soccer_final(soccer::FinalFlavor::FullTime);
    assert_eq!(
        encoded(|out| soccer::encode(&game, out)),
        GOLDEN_SOCCER_FINAL
    );
    assert_eq!(soccer::decode(&unhex(GOLDEN_SOCCER_FINAL)).unwrap(), game);
}

#[test]
fn soccer_final_flavor_byte_is_the_only_difference() {
    let full = unhex(&encoded(|out| {
        soccer::encode(&soccer_final(soccer::FinalFlavor::FullTime), out)
    }));
    for (flavor, code) in [
        (soccer::FinalFlavor::AfterExtraTime, 1),
        (soccer::FinalFlavor::AfterPenalties, 2),
    ] {
        let game = soccer_final(flavor);
        let bytes = unhex(&encoded(|out| soccer::encode(&game, out)));
        assert_eq!(bytes[2], code);
        assert_eq!(bytes[3..], full[3..]);
        assert_eq!(soccer::decode(&bytes).unwrap(), game);
    }
}

// --- NBA ---

const GOLDEN_NBA_LIVE: &str = "02010103004b004d00c17a0000243bef0040220e0024c5fe0009343031383131303337034f4b430344454e04343a33370c3430313831313033373431312a5a656b65204e6e616a69206f7574206f6620626f756e6473206261642070617373207475726e6f766572";

/// OKC @ DEN in the third with a last play: the frozen live golden's game.
/// Its `ext=2` groups are empty — what a v2 payload decodes to.
fn nba_live() -> nba::Game<'static> {
    nba::Game::Live(nba::Live {
        game_id: "401811037",
        period: 3,
        phase: LivePhase::InProgress,
        clock: "4:37",
        away: team("OKC", 75, 0x007AC1, 0xEF3B24),
        home: team("DEN", 77, 0x0E2240, 0xFEC524),
        last_play: Some(LastPlay {
            id: "401811037411",
            text: "Zeke Nnaji out of bounds bad pass turnover",
        }),
        ledger: None,
        run: None,
        newest_score: None,
        makes: nba::Makes::new(&[]),
    })
}

#[test]
fn nba_live_with_last_play() {
    assert_eq!(
        encoded(|out| nba::encode(&nba_live(), out)),
        GOLDEN_NBA_LIVE
    );
    assert_eq!(nba::decode(&unhex(GOLDEN_NBA_LIVE)).unwrap(), nba_live());
}

const GOLDEN_NBA_HALFTIME: &str = "020100020134004a00a9765d0012b1f5008e004e001ba0f90009343031383131303336034d454d045554414803302e30";

/// Break state: flags 0 (no last play), phase 1, clock reads "0.0".
fn nba_halftime() -> nba::Game<'static> {
    nba::Game::Live(nba::Live {
        game_id: "401811036",
        period: 2,
        phase: LivePhase::Halftime,
        clock: "0.0",
        away: team("MEM", 52, 0x5D76A9, 0xF5B112),
        home: team("UTAH", 74, 0x4E008E, 0xF9A01B),
        last_play: None,
        ledger: None,
        run: None,
        newest_score: None,
        makes: nba::Makes::new(&[]),
    })
}

#[test]
fn nba_halftime_without_last_play() {
    assert_eq!(
        encoded(|out| nba::encode(&nba_halftime(), out)),
        GOLDEN_NBA_HALFTIME
    );
    assert_eq!(
        nba::decode(&unhex(GOLDEN_NBA_HALFTIME)).unwrap(),
        nba_halftime()
    );
}

fn nba_pregame() -> nba::Game<'static> {
    nba::Game::Pregame(nba::Pregame {
        game_id: "401811040",
        start_time: 1_775_874_600,
        venue: "crypto.com Arena",
        info_secondary: "",
        away: nba::PregameTeam {
            abbreviation: "PHX",
            colors: TeamColors {
                primary: 0x29127A,
                alternate: 0xE56020,
            },
            record: Some(Record {
                wins: 40,
                losses: 42,
            }),
            line: None,
        },
        home: nba::PregameTeam {
            abbreviation: "LAL",
            colors: TeamColors {
                primary: 0x552583,
                alternate: 0xFDB927,
            },
            record: Some(Record {
                wins: 50,
                losses: 32,
            }),
            line: None,
        },
    })
}

const GOLDEN_NBA_PRE: &str = "02000328002a003200200028b2d9697a1229002060e5008325550027b9fd000934303138313130343003504858034c414c1063727970746f2e636f6d204172656e61";

#[test]
fn nba_pregame_all_flags() {
    assert_eq!(
        encoded(|out| nba::encode(&nba_pregame(), out)),
        GOLDEN_NBA_PRE
    );
    assert_eq!(nba::decode(&unhex(GOLDEN_NBA_PRE)).unwrap(), nba_pregame());
}

const GOLDEN_NBA_PRE_NO_RECORDS: &str = "020000000000000000000028b2d9697a1229002060e5008325550027b9fd000934303138313130343003504858034c414c1063727970746f2e636f6d204172656e61";

#[test]
fn nba_pregame_without_records() {
    let nba::Game::Pregame(mut pregame) = nba_pregame() else {
        unreachable!()
    };
    pregame.away.record = None;
    pregame.home.record = None;
    let game = nba::Game::Pregame(pregame);
    assert_eq!(
        encoded(|out| nba::encode(&game, out)),
        GOLDEN_NBA_PRE_NO_RECORDS
    );
    assert_eq!(
        nba::decode(&unhex(GOLDEN_NBA_PRE_NO_RECORDS)).unwrap(),
        game
    );
}

fn nba_pregame_with_lines() -> nba::Game<'static> {
    let nba::Game::Pregame(mut pregame) = nba_pregame() else {
        unreachable!()
    };
    pregame.away.line = Some("+1.5  -102");
    pregame.home.line = Some("-1.5  -118");
    nba::Game::Pregame(pregame)
}

/// The v2 pregame, then the `ext=1` block: flags 0x03 and both lines.
const GOLDEN_NBA_PRE_EXT: &str = "02000328002a003200200028b2d9697a1229002060e5008325550027b9fd000934303138313130343003504858034c414c1063727970746f2e636f6d204172656e61030a2b312e3520202d3130320a2d312e3520202d313138";

#[test]
fn nba_plain_encode_never_carries_the_team_lines() {
    // A device that did not ask for extensions decodes the frozen bytes.
    assert_eq!(
        encoded(|out| nba::encode(&nba_pregame_with_lines(), out)),
        GOLDEN_NBA_PRE
    );
}

#[test]
fn nba_ext_pregame_appends_the_team_lines() {
    assert_eq!(
        encoded(|out| nba::encode_ext(&nba_pregame_with_lines(), out)),
        GOLDEN_NBA_PRE_EXT
    );
    assert_eq!(
        nba::decode(&unhex(GOLDEN_NBA_PRE_EXT)).unwrap(),
        nba_pregame_with_lines()
    );
}

#[test]
fn nba_ext_pregame_without_lines_is_one_zero_flags_byte() {
    let golden = format!("{GOLDEN_NBA_PRE}00");
    assert_eq!(encoded(|out| nba::encode_ext(&nba_pregame(), out)), golden);
    assert_eq!(nba::decode(&unhex(&golden)).unwrap(), nba_pregame());
}

#[test]
fn nba_ext_pregame_carries_one_side_alone() {
    let nba::Game::Pregame(mut pregame) = nba_pregame() else {
        unreachable!()
    };
    pregame.away.line = Some("+1.5  -102");
    let game = nba::Game::Pregame(pregame);
    let golden = format!("{GOLDEN_NBA_PRE}010a2b312e3520202d313032");
    assert_eq!(encoded(|out| nba::encode_ext(&game, out)), golden);
    assert_eq!(nba::decode(&unhex(&golden)).unwrap(), game);
}

#[test]
fn nba_plain_v2_pregame_still_decodes_without_lines() {
    // What an `ext=1` device receives from a backend that predates extensions.
    assert_eq!(nba::decode(&unhex(GOLDEN_NBA_PRE)).unwrap(), nba_pregame());
}

#[test]
fn nba_ext_has_nothing_for_live_or_final() {
    let (live_bytes, final_bytes) = (unhex(GOLDEN_NBA_LIVE), unhex(GOLDEN_NBA_FINAL));
    let live = nba::decode(&live_bytes).unwrap();
    assert_eq!(encoded(|out| nba::encode_ext(&live, out)), GOLDEN_NBA_LIVE);
    let final_game = nba::decode(&final_bytes).unwrap();
    assert_eq!(
        encoded(|out| nba::encode_ext(&final_game, out)),
        GOLDEN_NBA_FINAL
    );
}

#[test]
fn nba_lines_block_rejects_unknown_flag_bits() {
    let unknown = unhex(&format!("{GOLDEN_NBA_PRE}04"));
    assert_eq!(
        nba::decode(&unknown).unwrap_err(),
        DecodeError {
            offset: unknown.len() - 1,
            kind: DecodeErrorKind::InvalidCode {
                field: "team lines flags",
                code: 4,
            },
        }
    );
    // A byte after the lines opens the `ext=2` info line, so a lone one is a
    // truncated string, not a trailing byte (the trailing check sits after
    // the last block the decoder knows: see the `ext=2` tests).
    let cut = unhex(&format!("{GOLDEN_NBA_PRE_EXT}01"));
    assert_eq!(
        nba::decode(&cut).unwrap_err().kind,
        DecodeErrorKind::TruncatedString {
            field: "info secondary",
            need: 1,
            have: 0,
        }
    );
}

const GOLDEN_NBA_FINAL: &str = "0202040404760064008a421d002e10c800a88c000060111d001e1c1e1e19191919093430313831313032360344455403434841";

/// DET 118, CHA 100 in regulation: the frozen final golden's game.
fn nba_final() -> nba::Game<'static> {
    nba::Game::Final(nba::Final {
        game_id: "401811026",
        periods_played: 4,
        away: final_team("DET", 118, 0x1D428A, 0xC8102E, &[30, 28, 30, 30]),
        home: final_team("CHA", 100, 0x008CA8, 0x1D1160, &[25, 25, 25, 25]),
        leaders: None,
        series: None,
        records: None,
    })
}

#[test]
fn nba_final_regulation() {
    assert_eq!(
        encoded(|out| nba::encode(&nba_final(), out)),
        GOLDEN_NBA_FINAL
    );
    assert_eq!(nba::decode(&unhex(GOLDEN_NBA_FINAL)).unwrap(), nba_final());
}

// --- NBA, `ext=2` ---
//
// No deployed encoder writes these blocks yet; the goldens were worked out by
// hand from the layout tables in `nba.rs` (NBA.md §4), byte by byte, before
// the encoder ran — which is what makes them a contract rather than a
// snapshot. Each is a frozen v2 golden above with its block appended; the
// derivation is spelled out beside each block constant.

static NBA_MAKES: [nba::Make; 2] = [
    nba::Make {
        side: Side::Away,
        three: false,
        x: -5,
        y: 12,
    },
    nba::Make {
        side: Side::Home,
        three: true,
        x: 22,
        y: 3,
    },
];

/// The live golden's game with a ledger (OKC in the bonus), DEN on an 8-0 run
/// and two makes on the court; no newest score.
fn nba_live_ext() -> nba::Game<'static> {
    let nba::Game::Live(live) = nba_live() else {
        unreachable!()
    };
    nba::Game::Live(nba::Live {
        ledger: Some(nba::Ledger {
            away: nba::TeamLedger {
                fouls: 3,
                in_bonus: true,
                timeouts: 4,
            },
            home: nba::TeamLedger {
                fouls: 5,
                in_bonus: false,
                timeouts: 2,
            },
        }),
        run: Some(nba::Run {
            side: Side::Home,
            points: 8,
        }),
        newest_score: None,
        makes: nba::Makes::new(&NBA_MAKES),
        ..live
    })
}

/// The `ext=2` live block of [`nba_live_ext`], by hand: `03` flags (ledger,
/// run) · `03 05` fouls · `01` bonus (away) · `04 02` timeouts · `02 08` run
/// (home, 8) · `00 00 00000000` newest-score placeholder · `02` N; then
/// `00 fb 0c` (away two at −5, 12) and `03 16 03` (home three at 22, 3). No
/// scorer: bit2 is clear.
const NBA_LIVE_EXT2_BLOCK: &str = "03030501040202080000000000000200fb0c031603";

#[test]
fn nba_live_ext2_carries_the_ledger_the_run_and_the_makes() {
    let golden = format!("{GOLDEN_NBA_LIVE}{NBA_LIVE_EXT2_BLOCK}");
    assert_eq!(
        encoded(|out| nba::encode_at(&nba_live_ext(), 2, out)),
        golden
    );
    let bytes = unhex(&golden);
    let decoded = nba::decode(&bytes).unwrap();
    assert_eq!(decoded, nba_live_ext());
    let nba::Game::Live(live) = decoded else {
        unreachable!()
    };
    assert_eq!(live.makes.len(), 2);
    assert_eq!(live.makes.get(0), Some(NBA_MAKES[0]));
    assert_eq!(live.makes.last(), Some(NBA_MAKES[1]));
    assert_eq!(live.makes.iter().next_back(), Some(NBA_MAKES[1]));
    assert_eq!(live.makes.get(2), None);
}

static NBA_NEWEST_MAKE: [nba::Make; 1] = [nba::Make {
    side: Side::Away,
    three: true,
    x: -24,
    y: 27,
}];

/// The live golden's game with only the newest score: a corner three that is
/// also the newest make (so it blinks), the scorer's 36th point, play 123456.
fn nba_live_newest_score() -> nba::Game<'static> {
    let nba::Game::Live(live) = nba_live() else {
        unreachable!()
    };
    nba::Game::Live(nba::Live {
        newest_score: Some(nba::NewestScore {
            side: Side::Away,
            scorer_points: 36,
            seq: 123_456,
            marked: true,
            scorer: "GILGEOUS-ALEXANDER",
        }),
        makes: nba::Makes::new(&NBA_NEWEST_MAKE),
        ..live
    })
}

/// By hand: `0c` flags (newest score, marked) · `00 00 00 00 00` ledger
/// placeholder · `00 00` run placeholder · `01` away · `24` 36 points ·
/// `40 e2 01 00` 123456 little-endian · `01` N; `02 e8 1b` (away three at
/// −24, 27); then `12` and the 18-byte scorer.
const NBA_LIVE_NEWEST_BLOCK: &str =
    "0c00000000000000012440e201000102e81b1247494c47454f55532d414c4558414e444552";

#[test]
fn nba_live_ext2_carries_the_newest_score_and_its_scorer() {
    let golden = format!("{GOLDEN_NBA_LIVE}{NBA_LIVE_NEWEST_BLOCK}");
    assert_eq!(
        encoded(|out| nba::encode_at(&nba_live_newest_score(), 2, out)),
        golden
    );
    assert_eq!(
        nba::decode(&unhex(&golden)).unwrap(),
        nba_live_newest_score()
    );
}

/// Fifteen zero bytes: every group absent and no makes — a game the summary
/// has nothing for yet, at level 2.
const NBA_LIVE_EMPTY_BLOCK: &str = "000000000000000000000000000000";

#[test]
fn nba_live_ext2_with_nothing_is_fifteen_zero_bytes() {
    let golden = format!("{GOLDEN_NBA_HALFTIME}{NBA_LIVE_EMPTY_BLOCK}");
    assert_eq!(
        encoded(|out| nba::encode_at(&nba_halftime(), 2, out)),
        golden
    );
    assert_eq!(nba::decode(&unhex(&golden)).unwrap(), nba_halftime());
}

/// The pregame golden with both lines and a Cup note.
fn nba_pregame_ext() -> nba::Game<'static> {
    let nba::Game::Pregame(mut pregame) = nba_pregame_with_lines() else {
        unreachable!()
    };
    pregame.info_secondary = "NBA CUP - GROUP PLAY";
    nba::Game::Pregame(pregame)
}

/// By hand: after the `ext=1` lines block (its golden above), one string —
/// `14` and the 20 bytes of "NBA CUP - GROUP PLAY".
const NBA_PRE_EXT2_BLOCK: &str = "144e424120435550202d2047524f555020504c4159";

#[test]
fn nba_pregame_ext2_carries_the_second_info_line() {
    let golden = format!("{GOLDEN_NBA_PRE_EXT}{NBA_PRE_EXT2_BLOCK}");
    assert_eq!(
        encoded(|out| nba::encode_at(&nba_pregame_ext(), 2, out)),
        golden
    );
    assert_eq!(nba::decode(&unhex(&golden)).unwrap(), nba_pregame_ext());
}

#[test]
fn nba_pregame_ext2_with_nothing_is_two_zero_bytes() {
    // `00` lines flags, `00` an empty info line.
    let golden = format!("{GOLDEN_NBA_PRE}0000");
    assert_eq!(
        encoded(|out| nba::encode_at(&nba_pregame(), 2, out)),
        golden
    );
    assert_eq!(nba::decode(&unhex(&golden)).unwrap(), nba_pregame());
}

/// The final golden's game as a playoff game: Cunningham led the points and
/// Ivey the assists for DET, Williams the rebounds for CHA; both records; the
/// series.
fn nba_final_ext() -> nba::Game<'static> {
    let nba::Game::Final(game) = nba_final() else {
        unreachable!()
    };
    nba::Game::Final(nba::Final {
        leaders: Some(nba::Leaders {
            points: nba::Leader {
                side: Side::Away,
                value: 34,
                name: "C. Cunningham",
            },
            rebounds: nba::Leader {
                side: Side::Home,
                value: 12,
                name: "M. Williams",
            },
            assists: nba::Leader {
                side: Side::Away,
                value: 9,
                name: "J. Ivey",
            },
        }),
        series: Some("DET leads series 2-1"),
        records: Some(nba::Records {
            away: Record {
                wins: 44,
                losses: 38,
            },
            home: Record {
                wins: 19,
                losses: 63,
            },
        }),
        ..game
    })
}

/// By hand: `07` flags (leaders, series, records) · `19` sides (points away
/// = 1, rebounds home = 2 << 2, assists away = 1 << 4) · `22 0c 09` values ·
/// `2c00 2600 1300 3f00` records (44-38, 19-63); then the three names (13,
/// 11 and 7 bytes, each behind its length) and the 20-byte series.
const NBA_FINAL_EXT2_BLOCK: &str = "0719220c092c00260013003f000d432e2043756e6e696e6768616d0b4d2e2057696c6c69616d73074a2e204976657914444554206c656164732073657269657320322d31";

#[test]
fn nba_final_ext2_carries_the_leaders_the_series_and_the_records() {
    let golden = format!("{GOLDEN_NBA_FINAL}{NBA_FINAL_EXT2_BLOCK}");
    assert_eq!(
        encoded(|out| nba::encode_at(&nba_final_ext(), 2, out)),
        golden
    );
    assert_eq!(nba::decode(&unhex(&golden)).unwrap(), nba_final_ext());
}

/// Thirteen zero bytes: no leaders, series or records.
const NBA_FINAL_EMPTY_BLOCK: &str = "00000000000000000000000000";

#[test]
fn nba_final_ext2_with_nothing_is_thirteen_zero_bytes() {
    let golden = format!("{GOLDEN_NBA_FINAL}{NBA_FINAL_EMPTY_BLOCK}");
    assert_eq!(encoded(|out| nba::encode_at(&nba_final(), 2, out)), golden);
    assert_eq!(nba::decode(&unhex(&golden)).unwrap(), nba_final());
}

// The level mechanism: what each level writes, and what a payload ending at
// each level decodes to.

#[test]
fn nba_levels_zero_and_one_are_encode_and_encode_ext() {
    // Byte for byte, on every fixture: the backend's two entry points can
    // move to the level without a byte moving. The `ext=2` fixtures drop
    // their groups below level 2.
    let pre_without_lines = format!("{GOLDEN_NBA_PRE}00");
    let fixtures = [
        (nba_live(), GOLDEN_NBA_LIVE, GOLDEN_NBA_LIVE),
        (nba_halftime(), GOLDEN_NBA_HALFTIME, GOLDEN_NBA_HALFTIME),
        (nba_pregame(), GOLDEN_NBA_PRE, pre_without_lines.as_str()),
        (nba_pregame_with_lines(), GOLDEN_NBA_PRE, GOLDEN_NBA_PRE_EXT),
        (nba_final(), GOLDEN_NBA_FINAL, GOLDEN_NBA_FINAL),
        (nba_live_ext(), GOLDEN_NBA_LIVE, GOLDEN_NBA_LIVE),
        (nba_live_newest_score(), GOLDEN_NBA_LIVE, GOLDEN_NBA_LIVE),
        (nba_pregame_ext(), GOLDEN_NBA_PRE, GOLDEN_NBA_PRE_EXT),
        (nba_final_ext(), GOLDEN_NBA_FINAL, GOLDEN_NBA_FINAL),
    ];
    for (game, v2, ext1) in fixtures {
        assert_eq!(encoded(|out| nba::encode_at(&game, 0, out)), v2);
        assert_eq!(encoded(|out| nba::encode(&game, out)), v2);
        assert_eq!(encoded(|out| nba::encode_at(&game, 1, out)), ext1);
        assert_eq!(encoded(|out| nba::encode_ext(&game, out)), ext1);
    }
}

#[test]
fn nba_levels_above_the_maximum_write_the_maximum() {
    for game in [nba_live_ext(), nba_pregame_ext(), nba_final_ext()] {
        let at_max = encoded(|out| nba::encode_at(&game, EXT_LEVEL_MAX, out));
        for level in [EXT_LEVEL_MAX + 1, u8::MAX] {
            assert_eq!(encoded(|out| nba::encode_at(&game, level, out)), at_max);
        }
    }
}

/// `game` as a payload written at `level` decodes: the groups the level does
/// not carry are gone, the rest intact.
fn nba_at_level(game: &nba::Game<'static>, level: u8) -> nba::Game<'static> {
    match *game {
        nba::Game::Live(live) if level < 2 => nba::Game::Live(nba::Live {
            ledger: None,
            run: None,
            newest_score: None,
            makes: nba::Makes::new(&[]),
            ..live
        }),
        nba::Game::Pregame(mut pregame) => {
            if level < 1 {
                pregame.away.line = None;
                pregame.home.line = None;
            }
            if level < 2 {
                pregame.info_secondary = "";
            }
            nba::Game::Pregame(pregame)
        }
        nba::Game::Final(game) if level < 2 => nba::Game::Final(nba::Final {
            leaders: None,
            series: None,
            records: None,
            ..game
        }),
        game => game,
    }
}

/// Every state with its `ext=2` groups present and absent.
fn nba_fixtures() -> [nba::Game<'static>; 8] {
    [
        nba_live_ext(),
        nba_live_newest_score(),
        nba_live(),
        nba_halftime(),
        nba_pregame_ext(),
        nba_pregame(),
        nba_final_ext(),
        nba_final(),
    ]
}

#[test]
fn nba_every_state_round_trips_at_every_level() {
    for game in nba_fixtures() {
        for level in 0..=EXT_LEVEL_MAX {
            let bytes = unhex(&encoded(|out| nba::encode_at(&game, level, out)));
            assert_eq!(
                nba::decode(&bytes).unwrap(),
                nba_at_level(&game, level),
                "level {level} of {game:?}"
            );
        }
    }
}

#[test]
fn nba_each_ext2_group_travels_alone() {
    let (nba::Game::Live(full), nba::Game::Live(newest), nba::Game::Live(bare)) =
        (nba_live_ext(), nba_live_newest_score(), nba_live())
    else {
        unreachable!()
    };
    let lives = [
        nba::Live {
            ledger: full.ledger,
            ..bare
        },
        nba::Live {
            run: full.run,
            ..bare
        },
        // An unmarked newest score with no scorer: a goaltending award.
        nba::Live {
            newest_score: newest.newest_score.map(|score| nba::NewestScore {
                marked: false,
                scorer: "",
                ..score
            }),
            ..bare
        },
        nba::Live {
            makes: full.makes,
            ..bare
        },
    ];
    for live in lives {
        let game = nba::Game::Live(live);
        let bytes = unhex(&encoded(|out| nba::encode_at(&game, 2, out)));
        assert_eq!(nba::decode(&bytes).unwrap(), game);
    }

    let (nba::Game::Final(full), nba::Game::Final(bare)) = (nba_final_ext(), nba_final()) else {
        unreachable!()
    };
    let finals = [
        nba::Final {
            leaders: full.leaders,
            ..bare
        },
        nba::Final {
            series: full.series,
            ..bare
        },
        nba::Final {
            records: full.records,
            ..bare
        },
    ];
    for game in finals {
        let game = nba::Game::Final(game);
        let bytes = unhex(&encoded(|out| nba::encode_at(&game, 2, out)));
        assert_eq!(nba::decode(&bytes).unwrap(), game);
    }
}

/// A fixture's level-2 payload and the lengths at which it is a complete
/// shorter payload: the end of the v2 bytes (level 0) and, for a pregame, the
/// end of the lines block (level 1). Live and final have no `ext=1` block, so
/// their one boundary is both levels.
struct Level2Payload {
    game: nba::Game<'static>,
    bytes: Vec<u8>,
    /// Each length with the level a prefix of that length decodes as.
    boundaries: Vec<(usize, u8)>,
}

fn nba_level2_payloads() -> Vec<Level2Payload> {
    nba_fixtures()
        .into_iter()
        .map(|game| {
            let at = |level| unhex(&encoded(|out| nba::encode_at(&game, level, out)));
            let (v2, ext1, ext2) = (at(0), at(1), at(2));
            let mut boundaries = vec![(v2.len(), 0)];
            if ext1.len() != v2.len() {
                boundaries.push((ext1.len(), 1));
            }
            Level2Payload {
                game,
                bytes: ext2,
                boundaries,
            }
        })
        .collect()
}

#[test]
fn nba_truncation_at_every_offset_is_a_clean_error_except_at_a_block_boundary() {
    for Level2Payload {
        game,
        bytes,
        boundaries,
    } in nba_level2_payloads()
    {
        for len in 0..bytes.len() {
            match boundaries.iter().find(|(at, _)| *at == len) {
                Some(&(_, level)) => assert_eq!(
                    nba::decode(&bytes[..len]).unwrap(),
                    nba_at_level(&game, level),
                    "prefix {len} of {game:?} is its level-{level} payload"
                ),
                None => {
                    let error = nba::decode(&bytes[..len])
                        .expect_err("a prefix inside a block is never a payload");
                    assert!(
                        matches!(
                            error.kind,
                            DecodeErrorKind::Empty
                                | DecodeErrorKind::Truncated(_)
                                | DecodeErrorKind::TruncatedFixed { .. }
                                | DecodeErrorKind::TruncatedLength(_)
                                | DecodeErrorKind::TruncatedString { .. }
                                | DecodeErrorKind::TruncatedLineScores { .. }
                                | DecodeErrorKind::TruncatedRecords { .. }
                        ),
                        "prefix {len} of {} reported {error}",
                        bytes.len()
                    );
                }
            }
        }
    }
}

#[test]
fn nba_a_payload_ending_after_any_block_decodes_as_that_level() {
    // The pregame has three ends: after `venue`, after the lines, after the
    // info line. Each shorter one is an older backend's answer.
    let bytes = unhex(&format!("{GOLDEN_NBA_PRE_EXT}{NBA_PRE_EXT2_BLOCK}"));
    let (v2, ext1) = (GOLDEN_NBA_PRE.len() / 2, GOLDEN_NBA_PRE_EXT.len() / 2);
    assert_eq!(nba::decode(&bytes[..v2]).unwrap(), nba_pregame());
    assert_eq!(
        nba::decode(&bytes[..ext1]).unwrap(),
        nba_pregame_with_lines()
    );
    assert_eq!(nba::decode(&bytes).unwrap(), nba_pregame_ext());

    // Live and final have two: after the v2 strings and after the block.
    let bytes = unhex(&format!("{GOLDEN_NBA_LIVE}{NBA_LIVE_EXT2_BLOCK}"));
    assert_eq!(
        nba::decode(&bytes[..GOLDEN_NBA_LIVE.len() / 2]).unwrap(),
        nba_live()
    );
    assert_eq!(nba::decode(&bytes).unwrap(), nba_live_ext());

    let bytes = unhex(&format!("{GOLDEN_NBA_FINAL}{NBA_FINAL_EXT2_BLOCK}"));
    assert_eq!(
        nba::decode(&bytes[..GOLDEN_NBA_FINAL.len() / 2]).unwrap(),
        nba_final()
    );
    assert_eq!(nba::decode(&bytes).unwrap(), nba_final_ext());
}

#[test]
fn nba_trailing_bytes_after_the_last_block_are_rejected() {
    for Level2Payload { mut bytes, .. } in nba_level2_payloads() {
        let end = bytes.len();
        bytes.push(0);
        assert_eq!(
            nba::decode(&bytes).unwrap_err(),
            DecodeError {
                offset: end,
                kind: DecodeErrorKind::Trailing(1),
            }
        );
    }
    // One byte past the v2 live or final bytes opens a block: a truncated
    // fixed section, not a trailing byte.
    for golden in [GOLDEN_NBA_LIVE, GOLDEN_NBA_FINAL] {
        let bytes = unhex(&format!("{golden}00"));
        assert!(matches!(
            nba::decode(&bytes).unwrap_err().kind,
            DecodeErrorKind::TruncatedFixed { .. }
        ));
    }
}

/// Where the live `ext=2` block starts in the live golden.
fn nba_live_block_at() -> usize {
    GOLDEN_NBA_LIVE.len() / 2
}

#[test]
fn nba_truncated_make_records_report_the_shortfall() {
    let bytes = unhex(&format!("{GOLDEN_NBA_LIVE}{NBA_LIVE_EXT2_BLOCK}"));
    let records_at = nba_live_block_at() + 15;
    assert_eq!(
        nba::decode(&bytes[..records_at + 3]).unwrap_err(),
        DecodeError {
            offset: records_at,
            kind: DecodeErrorKind::TruncatedRecords {
                field: "make records",
                need: 6,
                have: 3,
            },
        }
    );
}

fn nba_live_ext_err(block: &str, edit: impl FnOnce(&mut Vec<u8>)) -> DecodeError {
    let mut bytes = unhex(&format!("{GOLDEN_NBA_LIVE}{block}"));
    edit(&mut bytes);
    nba::decode(&bytes).expect_err("payload must be rejected")
}

fn nba_final_ext_err(edit: impl FnOnce(&mut Vec<u8>)) -> DecodeError {
    let mut bytes = unhex(&format!("{GOLDEN_NBA_FINAL}{NBA_FINAL_EXT2_BLOCK}"));
    edit(&mut bytes);
    nba::decode(&bytes).expect_err("payload must be rejected")
}

#[test]
fn nba_ext2_undefined_flag_bits_are_rejected() {
    let at = nba_live_block_at();
    for bit in [0x10, 0x20, 0x40, 0x80] {
        assert_eq!(
            nba_live_ext_err(NBA_LIVE_EXT2_BLOCK, |bytes| bytes[at] |= bit),
            invalid(at, "live extension flags", 0x03 | bit)
        );
    }
    // The mark without a newest score to mark.
    assert_eq!(
        nba_live_ext_err(NBA_LIVE_EXT2_BLOCK, |bytes| bytes[at] = 0x08),
        invalid(at, "live extension flags", 0x08)
    );

    let at = GOLDEN_NBA_FINAL.len() / 2;
    for bit in [0x08, 0x10, 0x20, 0x40, 0x80] {
        assert_eq!(
            nba_final_ext_err(|bytes| bytes[at] |= bit),
            invalid(at, "final extension flags", 0x07 | bit)
        );
    }
}

#[test]
fn nba_ext2_side_codes_outside_away_and_home_are_rejected() {
    let at = nba_live_block_at();
    for code in [0, 3, 0xFF] {
        assert_eq!(
            nba_live_ext_err(NBA_LIVE_EXT2_BLOCK, |bytes| bytes[at + 6] = code),
            invalid(at + 6, "run side", code)
        );
        assert_eq!(
            nba_live_ext_err(NBA_LIVE_NEWEST_BLOCK, |bytes| bytes[at + 8] = code),
            invalid(at + 8, "newest score side", code)
        );
    }

    // A zero for each leader in turn, a 3 for each, and the two spare bits.
    let at = GOLDEN_NBA_FINAL.len() / 2;
    for sides in [0x18, 0x11, 0x09, 0x1B, 0x2D, 0x39, 0x59, 0x99] {
        assert_eq!(
            nba_final_ext_err(|bytes| bytes[at + 1] = sides),
            invalid(at + 1, "leader sides", sides)
        );
    }
}

#[test]
fn nba_ext2_values_outside_their_ranges_are_rejected() {
    let at = nba_live_block_at();
    for points in [0u8, 5] {
        assert_eq!(
            nba_live_ext_err(NBA_LIVE_EXT2_BLOCK, |bytes| bytes[at + 7] = points),
            out_of_range(at + 7, "run points", points as i32)
        );
    }
    for bonus in [0x04, 0xFF] {
        assert_eq!(
            nba_live_ext_err(NBA_LIVE_EXT2_BLOCK, |bytes| bytes[at + 3] = bonus),
            invalid(at + 3, "bonus", bonus)
        );
    }
    assert_eq!(
        nba_live_ext_err(NBA_LIVE_EXT2_BLOCK, |bytes| bytes[at + 14] = 129),
        out_of_range(at + 14, "make count", 129)
    );
    // The first make record sits at +15 with a zero head byte.
    for bits in [0x04, 0x80] {
        assert_eq!(
            nba_live_ext_err(NBA_LIVE_EXT2_BLOCK, |bytes| bytes[at + 15] |= bits),
            invalid(at + 15, "make side/three", bits)
        );
    }
    for x in [26i8, -26] {
        assert_eq!(
            nba_live_ext_err(NBA_LIVE_EXT2_BLOCK, |bytes| bytes[at + 16] = x as u8),
            out_of_range(at + 16, "make x", x as i32)
        );
    }
    for y in [48i8, -9] {
        assert_eq!(
            nba_live_ext_err(NBA_LIVE_EXT2_BLOCK, |bytes| bytes[at + 17] = y as u8),
            out_of_range(at + 17, "make y", y as i32)
        );
    }
}

#[test]
fn nba_ext2_absent_groups_are_placeholders_decode_never_reads() {
    // Football's rule: clear every flag, then poison the thirteen group bytes
    // with values each check would reject if it ran.
    let at = nba_live_block_at();
    let mut bytes = unhex(&format!("{GOLDEN_NBA_LIVE}{NBA_LIVE_EXT2_BLOCK}"));
    bytes[at] = 0;
    bytes[at + 1..at + 14]
        .copy_from_slice(&[9, 9, 0xFF, 9, 9, 7, 1, 7, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
    let nba::Game::Live(live) = nba::decode(&bytes).unwrap() else {
        unreachable!()
    };
    assert_eq!(
        (live.ledger, live.run, live.newest_score),
        (None, None, None)
    );
    assert_eq!(live.makes.len(), 2);

    let at = GOLDEN_NBA_FINAL.len() / 2;
    let mut bytes = unhex(&format!("{GOLDEN_NBA_FINAL}{NBA_FINAL_EMPTY_BLOCK}"));
    bytes[at + 1..at + 13].fill(0xFF);
    let nba::Game::Final(game) = nba::decode(&bytes).unwrap() else {
        unreachable!()
    };
    assert_eq!(
        (game.leaders, game.series, game.records),
        (None, None, None)
    );
}

// Encode-side bounds: encode clamps rather than fail, and what it writes
// always decodes — to the clamped value.

#[test]
fn nba_makes_keep_the_newest_128() {
    let makes: Vec<nba::Make> = (0..130i32)
        .map(|n| nba::Make {
            side: if n % 2 == 0 { Side::Away } else { Side::Home },
            three: n % 3 == 0,
            x: (n % 51 - 25) as i8,
            y: (n % 56 - 8) as i8,
        })
        .collect();
    let view = nba::Makes::new(&makes);
    assert_eq!(view.len(), nba::MAX_MAKES);
    assert_eq!(view.get(0), Some(makes[2]));
    let nba::Game::Live(live) = nba_live() else {
        unreachable!()
    };
    let game = nba::Game::Live(nba::Live {
        makes: view,
        ..live
    });
    let bytes = unhex(&encoded(|out| nba::encode_at(&game, 2, out)));
    let nba::Game::Live(decoded) = nba::decode(&bytes).unwrap() else {
        unreachable!()
    };
    assert_eq!(decoded.makes.iter().collect::<Vec<_>>(), makes[2..]);
}

#[test]
fn nba_coordinates_clamp_to_the_court() {
    let wild = [
        nba::Make {
            side: Side::Away,
            three: true,
            x: 60,
            y: 90,
        },
        nba::Make {
            side: Side::Home,
            three: false,
            x: -128,
            y: -128,
        },
    ];
    let nba::Game::Live(live) = nba_live() else {
        unreachable!()
    };
    let game = nba::Game::Live(nba::Live {
        makes: nba::Makes::new(&wild),
        ..live
    });
    let bytes = unhex(&encoded(|out| nba::encode_at(&game, 2, out)));
    let nba::Game::Live(decoded) = nba::decode(&bytes).unwrap() else {
        unreachable!()
    };
    assert_eq!(
        decoded.makes.iter().collect::<Vec<_>>(),
        [
            nba::Make {
                side: Side::Away,
                three: true,
                x: 25,
                y: 47,
            },
            nba::Make {
                side: Side::Home,
                three: false,
                x: -25,
                y: -8,
            },
        ]
    );
    // The clamped game is its own round trip.
    assert_eq!(
        encoded(|out| nba::encode_at(&nba::Game::Live(decoded), 2, out)),
        hex(&bytes)
    );
}

#[test]
fn nba_a_run_under_six_is_not_a_run_on_the_wire() {
    let nba::Game::Live(live) = nba_live() else {
        unreachable!()
    };
    let game = nba::Game::Live(nba::Live {
        run: Some(nba::Run {
            side: Side::Away,
            points: 5,
        }),
        ..live
    });
    let golden = format!("{GOLDEN_NBA_LIVE}{NBA_LIVE_EMPTY_BLOCK}");
    assert_eq!(encoded(|out| nba::encode_at(&game, 2, out)), golden);
    assert_eq!(nba::decode(&unhex(&golden)).unwrap(), nba_live());
}

/// The receive buffer's detail half (`poll::DETAIL_BYTES` in
/// scoreboard-model), which the largest live payload must fit.
const DETAIL_BYTES: usize = 2048;

#[test]
fn nba_worst_case_live_payload_fits_the_detail_buffer() {
    // 128 makes, a 255-byte play text, a 40-byte scorer, every group present:
    // 2 header + 23 fixed + 10 game_id + 4 + 4 abbreviations + 6 clock +
    // 13 play id + 256 play text = 318 of v2, then 15 + 384 make records +
    // 41 scorer = 440 of `ext=2`: 758 bytes.
    let makes = vec![
        nba::Make {
            side: Side::Home,
            three: true,
            x: 25,
            y: 47,
        };
        nba::MAX_MAKES
    ];
    let text = "x".repeat(255);
    let scorer = "G".repeat(40);
    let nba::Game::Live(live) = nba_live_ext() else {
        unreachable!()
    };
    let game = nba::Game::Live(nba::Live {
        clock: "10:08",
        last_play: Some(LastPlay {
            id: "401811037411",
            text: &text,
        }),
        newest_score: Some(nba::NewestScore {
            side: Side::Home,
            scorer_points: 255,
            seq: u32::MAX,
            marked: true,
            scorer: &scorer,
        }),
        makes: nba::Makes::new(&makes),
        ..live
    });
    let mut buf = [0u8; DETAIL_BYTES];
    let mut sink = SliceSink::new(&mut buf);
    nba::encode_at(&game, 2, &mut sink).expect("fits the detail half");
    assert_eq!(sink.len(), 758);
    assert!(sink.len() < DETAIL_BYTES);
    assert_eq!(nba::decode(sink.written()).unwrap(), game);
}

// --- Football ---

fn football_live() -> football::Game<'static> {
    football::Game::Live(football::Live {
        game_id: "401772510",
        period: 3,
        phase: LivePhase::InProgress,
        clock: "8:24",
        away: team("BUF", 14, 0x00338D, 0xC60C30),
        home: team("KC", 17, 0xE31837, 0xFFB81C),
        situation: Some(football::Situation {
            down: 2,
            distance: 7,
            yard_line: 45,
            possession: Side::Home,
            red_zone: false,
        }),
        timeouts: Some(football::Timeouts { away: 2, home: 3 }),
        last_play: Some(LastPlay {
            id: "401772510105",
            text: "P. Mahomes pass complete to T. Kelce for 8 yards",
        }),
    })
}

const GOLDEN_FOOTBALL_LIVE: &str = "020117030002072d02030e0011008d330000300cc6003718e3001cb8ff000934303137373235313003425546024b4304383a32340c34303137373235313031303530502e204d61686f6d6573207061737320636f6d706c65746520746f20542e204b656c636520666f722038207961726473";

#[test]
fn football_live_with_situation() {
    assert_eq!(
        encoded(|out| football::encode(&football_live(), out)),
        GOLDEN_FOOTBALL_LIVE
    );
    assert_eq!(
        football::decode(&unhex(GOLDEN_FOOTBALL_LIVE)).unwrap(),
        football_live()
    );
}

const GOLDEN_FOOTBALL_LIVE_BREAK: &str = "020100020100000000000a000e008d330000300cc6003718e3001cb8ff000934303137373235313103425546024b4304303a3030";

#[test]
fn football_live_break_without_situation() {
    // Halftime: flags 0 (no last play, no situation, no timeouts), phase 1,
    // down/distance/yard_line/timeouts all zero.
    let game = football::Game::Live(football::Live {
        game_id: "401772511",
        period: 2,
        phase: LivePhase::Halftime,
        clock: "0:00",
        away: team("BUF", 10, 0x00338D, 0xC60C30),
        home: team("KC", 14, 0xE31837, 0xFFB81C),
        situation: None,
        timeouts: None,
        last_play: None,
    });
    assert_eq!(
        encoded(|out| football::encode(&game, out)),
        GOLDEN_FOOTBALL_LIVE_BREAK
    );
    assert_eq!(
        football::decode(&unhex(GOLDEN_FOOTBALL_LIVE_BREAK)).unwrap(),
        game
    );
}

#[test]
fn football_live_flags_encode_possession_and_red_zone() {
    // Away possession in the red zone with timeouts, no last play: bit1
    // (situation) + bit3 (red zone) + bit4 (timeouts) set, bit0 and bit2
    // (last play, possession-home) clear.
    let football::Game::Live(mut live) = football_live() else {
        unreachable!()
    };
    live.last_play = None;
    live.situation = Some(football::Situation {
        down: 1,
        distance: 8,
        yard_line: 92,
        possession: Side::Away,
        red_zone: true,
    });
    let game = football::Game::Live(live);
    let bytes = unhex(&encoded(|out| football::encode(&game, out)));
    assert_eq!(bytes[2], 0x02 | 0x08 | 0x10);
    assert_eq!(football::decode(&bytes).unwrap(), game);
}

fn football_pregame() -> football::Game<'static> {
    football::Game::Pregame(football::Pregame {
        game_id: "401772512",
        start_time: 1_783_647_600,
        venue: "Arrowhead Stadium",
        away: football::PregameTeam {
            abbreviation: "BUF",
            colors: TeamColors {
                primary: 0x00338D,
                alternate: 0xC60C30,
            },
            record: Some(Record {
                wins: 11,
                losses: 3,
            }),
            line: None,
        },
        home: football::PregameTeam {
            abbreviation: "KC",
            colors: TeamColors {
                primary: 0xE31837,
                alternate: 0xFFB81C,
            },
            record: Some(Record {
                wins: 13,
                losses: 1,
            }),
            line: None,
        },
    })
}

const GOLDEN_FOOTBALL_PRE: &str = "0200030b0003000d000100704d506a8d330000300cc6003718e3001cb8ff000934303137373235313203425546024b43114172726f7768656164205374616469756d";

#[test]
fn football_pregame_nfl_records_no_ranks() {
    assert_eq!(
        encoded(|out| football::encode(&football_pregame(), out)),
        GOLDEN_FOOTBALL_PRE
    );
    assert_eq!(
        football::decode(&unhex(GOLDEN_FOOTBALL_PRE)).unwrap(),
        football_pregame()
    );
}

const GOLDEN_FOOTBALL_PRE_RANKED: &str = "02000b0b0003000d000100704d506a8d330000300cc6003718e3001cb8ff0009343031373732353133044d494348034f53550c4f68696f205374616469756d0d2333204f48494f205354415445";

#[test]
fn football_pregame_ncaaf_home_ranked() {
    // College: home #3 carries a rank line (it rides the pitcher slot), away
    // unranked → absent. Records still travel numerically.
    let football::Game::Pregame(mut pregame) = football_pregame() else {
        unreachable!()
    };
    pregame.game_id = "401772513";
    pregame.away.abbreviation = "MICH";
    pregame.home.abbreviation = "OSU";
    pregame.venue = "Ohio Stadium";
    pregame.home.line = Some("#3 OHIO STATE");
    let game = football::Game::Pregame(pregame);
    assert_eq!(
        encoded(|out| football::encode(&game, out)),
        GOLDEN_FOOTBALL_PRE_RANKED
    );
    assert_eq!(
        football::decode(&unhex(GOLDEN_FOOTBALL_PRE_RANKED)).unwrap(),
        game
    );
}

const GOLDEN_FOOTBALL_PRE_LINES: &str = "02000f0b0003000d000100704d506a8d330000300cc6003718e3001cb8ff000934303137373235313203425546024b43114172726f7768656164205374616469756d082b3720202b323435082d3720202d333035";

#[test]
fn football_pregame_betting_lines_ride_the_rank_bits() {
    // An unranked game's betting lines use the slot and flag bits a rank
    // line uses, so every device in the field decodes them unchanged.
    let football::Game::Pregame(mut pregame) = football_pregame() else {
        unreachable!()
    };
    pregame.away.line = Some("+7  +245");
    pregame.home.line = Some("-7  -305");
    let game = football::Game::Pregame(pregame);
    assert_eq!(
        encoded(|out| football::encode(&game, out)),
        GOLDEN_FOOTBALL_PRE_LINES
    );
    assert_eq!(
        football::decode(&unhex(GOLDEN_FOOTBALL_PRE_LINES)).unwrap(),
        game
    );
}

const GOLDEN_FOOTBALL_FINAL_OT: &str = "020205050518001b008d330000300cc6003718e3001cb8ff0007030707000707000a030934303137373235313403425546024b43";

#[test]
fn football_final_overtime() {
    let game = football::Game::Final(football::Final {
        game_id: "401772514",
        periods_played: 5,
        away: final_team("BUF", 24, 0x00338D, 0xC60C30, &[7, 3, 7, 7, 0]),
        home: final_team("KC", 27, 0xE31837, 0xFFB81C, &[7, 7, 0, 10, 3]),
    });
    assert_eq!(
        encoded(|out| football::encode(&game, out)),
        GOLDEN_FOOTBALL_FINAL_OT
    );
    assert_eq!(
        football::decode(&unhex(GOLDEN_FOOTBALL_FINAL_OT)).unwrap(),
        game
    );
}

const GOLDEN_FOOTBALL_FINAL: &str = "020204040418001b003718e3001cb8ff008d330000300cc60007030707000a070a09343031353437343137024b4303425546";

#[test]
fn football_final_regulation() {
    let game = football::Game::Final(football::Final {
        game_id: "401547417",
        periods_played: 4,
        away: final_team("KC", 24, 0xE31837, 0xFFB81C, &[7, 3, 7, 7]),
        home: final_team("BUF", 27, 0x00338D, 0xC60C30, &[0, 10, 7, 10]),
    });
    assert_eq!(
        encoded(|out| football::encode(&game, out)),
        GOLDEN_FOOTBALL_FINAL
    );
    assert_eq!(
        football::decode(&unhex(GOLDEN_FOOTBALL_FINAL)).unwrap(),
        game
    );
}

// --- Hockey ---
//
// No deployed encoder predates these; the goldens were written by this
// encoder and checked field by field against the layout tables in
// `hockey.rs`, which is what makes them a contract rather than a snapshot.

const fn hockey_goal(
    side: Side,
    tag: hockey::GoalTag,
    period: u8,
    elapsed: u16,
    x: i8,
    y: i8,
) -> hockey::Goal {
    hockey::Goal {
        side,
        tag,
        period,
        elapsed,
        coordinate: Some(hockey::Coordinate { x, y }),
    }
}

/// SEA @ CGY early in the second: five goals, one on the power play.
static HOCKEY_GOALS: [hockey::Goal; 5] = [
    hockey_goal(Side::Away, hockey::GoalTag::Goal, 1, 441, 46, 7),
    hockey_goal(Side::Home, hockey::GoalTag::Goal, 1, 771, -66, -9),
    hockey_goal(Side::Home, hockey::GoalTag::PowerPlay, 1, 1078, -68, -2),
    hockey_goal(Side::Away, hockey::GoalTag::Goal, 1, 1124, 70, 14),
    hockey_goal(Side::Home, hockey::GoalTag::Goal, 2, 85, -75, -16),
];

/// The home side on the power play the last play (a tripping minor) gave it.
fn hockey_live() -> hockey::Live<'static> {
    hockey::Live {
        game_id: "401891781",
        period: 2,
        phase: LivePhase::InProgress,
        clock: "13:38",
        playoffs: false,
        away: team("SEA", 2, 0x001628, 0x99D9D9),
        home: team("CGY", 3, 0xC8102E, 0xF1BE48),
        shots: Some(hockey::Shots { away: 21, home: 18 }),
        strength: Some(hockey::Strength {
            kind: hockey::StrengthKind::PowerPlay(Side::Home),
            away_skaters: 4,
            home_skaters: 5,
        }),
        goals: hockey::Goals::new(&HOCKEY_GOALS),
        last_scorer: "N. Kadri",
        shootout: None,
        last_play: Some(LastPlay {
            id: "401891781000005110",
            text: "Jordan Eberle Tripping against Rasmus Andersson",
        }),
    }
}

const GOLDEN_HOCKEY_LIVE: &str = "02010b02000200030028160000d9d999002e10c80048bef100151202040505000000000000000801b9012e0709010303bef70b013604bcfe08016404460e09025500b5f00934303138393137383103534541034347590531333a3338084e2e204b61647269123430313839313738313030303030353131302f4a6f7264616e20456265726c65205472697070696e6720616761696e7374205261736d757320416e64657273736f6e";

#[test]
fn hockey_live_on_a_power_play() {
    let game = hockey::Game::Live(hockey_live());
    assert_eq!(
        encoded(|out| hockey::encode(&game, out)),
        GOLDEN_HOCKEY_LIVE
    );
    let bytes = unhex(GOLDEN_HOCKEY_LIVE);
    let decoded = hockey::decode(&bytes).unwrap();
    assert_eq!(decoded, game);
    let hockey::Game::Live(live) = decoded else {
        unreachable!()
    };
    assert_eq!(live.goals.len(), 5);
    assert_eq!(live.goals.last(), Some(HOCKEY_GOALS[4]));
    assert_eq!(live.goals.iter().next_back(), Some(HOCKEY_GOALS[4]));
    assert_eq!(live.goals.get(5), None);
}

const GOLDEN_HOCKEY_PUCK_DROP: &str = "0201000100000000001cb8ff00000000001646fa0000000000000000000000000000000000000934303138393138313603424f53035048490532303a3030";

#[test]
fn hockey_live_at_puck_drop_has_no_goals_and_no_scorer() {
    // Flags 0: no play, no strength, no shootout, no shots. N = 0, so the
    // scorer string is not on the wire either: the clock is the last field.
    let game = hockey::Game::Live(hockey::Live {
        game_id: "401891816",
        period: 1,
        phase: LivePhase::InProgress,
        clock: "20:00",
        playoffs: false,
        away: team("BOS", 0, 0xFFB81C, 0x000000),
        home: team("PHI", 0, 0xFA4616, 0x000000),
        shots: None,
        strength: None,
        goals: hockey::Goals::new(&[]),
        last_scorer: "",
        shootout: None,
        last_play: None,
    });
    assert_eq!(
        encoded(|out| hockey::encode(&game, out)),
        GOLDEN_HOCKEY_PUCK_DROP
    );
    assert_eq!(
        hockey::decode(&unhex(GOLDEN_HOCKEY_PUCK_DROP)).unwrap(),
        game
    );
}

#[test]
fn hockey_live_scorer_travels_only_with_a_goal() {
    // A goalless game has no "last goal": encode drops the scorer, decode
    // reports it empty.
    let game = hockey::Game::Live(hockey::Live {
        goals: hockey::Goals::new(&[]),
        last_scorer: "N. Kadri",
        ..hockey_live()
    });
    let bytes = unhex(&encoded(|out| hockey::encode(&game, out)));
    let hockey::Game::Live(decoded) = hockey::decode(&bytes).unwrap() else {
        unreachable!()
    };
    assert_eq!(decoded.last_scorer, "");
    assert!(decoded.goals.is_empty());
    assert_eq!(decoded.goals.last(), None);
}

fn sixteen_hockey_goals() -> Vec<hockey::Goal> {
    (0..16u16)
        .map(|n| hockey::Goal {
            side: if n % 2 == 0 { Side::Away } else { Side::Home },
            tag: hockey::GoalTag::from_code((n % 4) as u8).unwrap(),
            period: 1 + (n / 6) as u8,
            elapsed: n * 70,
            coordinate: (n != 7).then_some(hockey::Coordinate {
                x: if n % 2 == 0 { 100 } else { -100 },
                y: if n % 3 == 0 { -43 } else { 43 },
            }),
        })
        .collect()
}

#[test]
fn hockey_live_carries_sixteen_goals_at_the_coordinate_bounds() {
    let goals = sixteen_hockey_goals();
    let game = hockey::Game::Live(hockey::Live {
        goals: hockey::Goals::new(&goals),
        ..hockey_live()
    });
    let bytes = unhex(&encoded(|out| hockey::encode(&game, out)));
    assert_eq!(bytes[30], 16);
    let decoded = hockey::decode(&bytes).unwrap();
    assert_eq!(decoded, game);
    let hockey::Game::Live(live) = decoded else {
        unreachable!()
    };
    assert_eq!(live.goals.len(), hockey::MAX_GOALS);
    assert_eq!(live.goals.iter().collect::<Vec<_>>(), goals);
    // The one goal ESPN logged without a coordinate keeps its slot.
    assert_eq!(live.goals.get(7).unwrap().coordinate, None);
    assert_eq!(live.goals.get(7).unwrap().elapsed, 490);
}

#[test]
fn hockey_goal_tags_and_sides_each_round_trip() {
    for (code, tag) in [
        (0, hockey::GoalTag::Goal),
        (1, hockey::GoalTag::PowerPlay),
        (2, hockey::GoalTag::ShortHanded),
        (3, hockey::GoalTag::EmptyNet),
    ] {
        assert_eq!(tag.code(), code);
        assert_eq!(hockey::GoalTag::from_code(code), Some(tag));
        for (side, side_bit) in [(Side::Away, 0), (Side::Home, 1)] {
            let goal = [hockey_goal(side, tag, 3, 1199, -12, 30)];
            let game = hockey::Game::Live(hockey::Live {
                goals: hockey::Goals::new(&goal),
                ..hockey_live()
            });
            let bytes = unhex(&encoded(|out| hockey::encode(&game, out)));
            // bit0 side, bits1–2 tag, bit3 coordinate present.
            assert_eq!(bytes[38], side_bit | (code << 1) | 0x08);
            assert_eq!(hockey::decode(&bytes).unwrap(), game);
        }
    }
    assert_eq!(hockey::GoalTag::from_code(4), None);
}

#[test]
fn hockey_strength_codes_round_trip() {
    for (kind, code, away_skaters, home_skaters) in [
        (hockey::StrengthKind::Even, 0, 5, 5),
        (hockey::StrengthKind::PowerPlay(Side::Away), 1, 5, 4),
        (hockey::StrengthKind::PowerPlay(Side::Home), 2, 3, 5),
        (hockey::StrengthKind::Matched, 3, 4, 4),
        (hockey::StrengthKind::EmptyNet(Side::Away), 4, 6, 5),
        (hockey::StrengthKind::EmptyNet(Side::Home), 5, 5, 6),
        // The play-strength fallback: a power play, counts unknown.
        (hockey::StrengthKind::PowerPlay(Side::Away), 1, 0, 0),
    ] {
        assert_eq!(kind.code(), code);
        assert_eq!(hockey::StrengthKind::from_code(code), Some(kind));
        let game = hockey::Game::Live(hockey::Live {
            strength: Some(hockey::Strength {
                kind,
                away_skaters,
                home_skaters,
            }),
            ..hockey_live()
        });
        let bytes = unhex(&encoded(|out| hockey::encode(&game, out)));
        assert_eq!(bytes[27..30], [code, away_skaters, home_skaters]);
        assert_eq!(hockey::decode(&bytes).unwrap(), game);
    }
    assert_eq!(hockey::StrengthKind::from_code(6), None);
}

/// A regular-season shootout after three away and two home attempts: away
/// scored its first and third, home its first, home shoots next. The game
/// score stays the tied 3–3.
fn hockey_shootout() -> hockey::Live<'static> {
    hockey::Live {
        period: 5,
        clock: "0:00",
        away: team("SEA", 3, 0x001628, 0x99D9D9),
        strength: None,
        shootout: Some(hockey::Shootout {
            away: hockey::ShootoutAttempts {
                taken: 3,
                scored: 0b101,
            },
            home: hockey::ShootoutAttempts {
                taken: 2,
                scored: 0b01,
            },
            next_shooter: Some(Side::Home),
        }),
        last_play: None,
        ..hockey_live()
    }
}

const GOLDEN_HOCKEY_SHOOTOUT: &str = "02010c05000300030028160000d9d999002e10c80048bef100151200000005030205000100020801b9012e0709010303bef70b013604bcfe08016404460e09025500b5f009343031383931373831035345410343475904303a3030084e2e204b61647269";

#[test]
fn hockey_live_shootout_in_progress() {
    let game = hockey::Game::Live(hockey_shootout());
    assert_eq!(
        encoded(|out| hockey::encode(&game, out)),
        GOLDEN_HOCKEY_SHOOTOUT
    );
    assert_eq!(
        hockey::decode(&unhex(GOLDEN_HOCKEY_SHOOTOUT)).unwrap(),
        game
    );
}

#[test]
fn hockey_live_shootout_over_has_no_next_shooter() {
    // Away's third attempt decided it 2–1: the shootout block stays (the
    // screen keeps the tally) but nobody shoots next.
    let mut live = hockey_shootout();
    live.shootout = Some(hockey::Shootout {
        away: hockey::ShootoutAttempts {
            taken: 3,
            scored: 0b101,
        },
        home: hockey::ShootoutAttempts {
            taken: 3,
            scored: 0b001,
        },
        next_shooter: None,
    });
    let game = hockey::Game::Live(live);
    let bytes = unhex(&encoded(|out| hockey::encode(&game, out)));
    assert_eq!(bytes[2] & 0x04, 0x04);
    assert_eq!(bytes[37], 0);
    assert_eq!(hockey::decode(&bytes).unwrap(), game);

    // And away shooting next is code 1.
    live.shootout.as_mut().unwrap().next_shooter = Some(Side::Away);
    let game = hockey::Game::Live(live);
    let bytes = unhex(&encoded(|out| hockey::encode(&game, out)));
    assert_eq!(bytes[37], 1);
    assert_eq!(hockey::decode(&bytes).unwrap(), game);
}

#[test]
fn hockey_live_full_shootout_uses_the_whole_mask() {
    let mut live = hockey_shootout();
    live.shootout = Some(hockey::Shootout {
        away: hockey::ShootoutAttempts {
            taken: 16,
            scored: 0x8001,
        },
        home: hockey::ShootoutAttempts {
            taken: 16,
            scored: 0x0001,
        },
        next_shooter: None,
    });
    let game = hockey::Game::Live(live);
    let bytes = unhex(&encoded(|out| hockey::encode(&game, out)));
    assert_eq!(bytes[31..37], [16, 16, 0x01, 0x80, 0x01, 0x00]);
    assert_eq!(hockey::decode(&bytes).unwrap(), game);
}

#[test]
fn hockey_live_extends_the_nba_fixed_section() {
    // With only a last play flagged, hockey's first 23 fixed bytes are NBA's.
    let live = hockey::Live {
        strength: None,
        shots: None,
        ..hockey_live()
    };
    let nba_game = nba::Game::Live(nba::Live {
        game_id: live.game_id,
        period: live.period,
        phase: live.phase,
        clock: live.clock,
        away: live.away,
        home: live.home,
        last_play: live.last_play,
        ledger: None,
        run: None,
        newest_score: None,
        makes: nba::Makes::new(&[]),
    });
    let hockey_bytes = unhex(&encoded(|out| {
        hockey::encode(&hockey::Game::Live(live), out)
    }));
    let nba_bytes = unhex(&encoded(|out| nba::encode(&nba_game, out)));
    assert_eq!(hockey_bytes[..25], nba_bytes[..25]);
}

fn hockey_pregame() -> hockey::Pregame<'static> {
    hockey::Pregame {
        game_id: "401874176",
        start_time: 1_781_481_600,
        venue: "Lenovo Center",
        info_secondary: "Stanley Cup Final - Game 6 - CAR leads series 3-2",
        playoffs: true,
        away: hockey::PregameTeam {
            abbreviation: "VGK",
            colors: TeamColors {
                primary: 0x344555,
                alternate: 0xB4975A,
            },
            record: Some(hockey::Record {
                wins: 50,
                losses: 22,
                ot_losses: 10,
            }),
            goalie: Some("A. Hill"),
        },
        home: hockey::PregameTeam {
            abbreviation: "CAR",
            colors: TeamColors {
                primary: 0xCE1126,
                alternate: 0xA4A9AD,
            },
            record: Some(hockey::Record {
                wins: 53,
                losses: 22,
                ot_losses: 7,
            }),
            goalie: Some("F. Andersen"),
        },
    }
}

const GOLDEN_HOCKEY_PRE: &str = "02001f320016003500160080402f6a554534005a97b4002611ce00ada9a4000a000700093430313837343137360356474b034341520d4c656e6f766f2043656e746572315374616e6c6579204375702046696e616c202d2047616d652036202d20434152206c656164732073657269657320332d3207412e2048696c6c0b462e20416e64657273656e";

#[test]
fn hockey_pregame_playoffs_with_goalies() {
    let game = hockey::Game::Pregame(hockey_pregame());
    assert_eq!(encoded(|out| hockey::encode(&game, out)), GOLDEN_HOCKEY_PRE);
    assert_eq!(hockey::decode(&unhex(GOLDEN_HOCKEY_PRE)).unwrap(), game);
}

const GOLDEN_HOCKEY_PRE_BARE: &str = "020000000000000000000080402f6a554534005a97b4002611ce00ada9a40000000000093430313837343137360356474b034341520d4c656e6f766f2043656e74657200";

#[test]
fn hockey_pregame_without_optional_data() {
    // Regular season, no records, no probable goalies: flags 0, the record
    // fields zero, `info_secondary` empty but present.
    let mut pregame = hockey_pregame();
    pregame.info_secondary = "";
    pregame.playoffs = false;
    pregame.away.record = None;
    pregame.home.record = None;
    pregame.away.goalie = None;
    pregame.home.goalie = None;
    let game = hockey::Game::Pregame(pregame);
    assert_eq!(
        encoded(|out| hockey::encode(&game, out)),
        GOLDEN_HOCKEY_PRE_BARE
    );
    assert_eq!(
        hockey::decode(&unhex(GOLDEN_HOCKEY_PRE_BARE)).unwrap(),
        game
    );
}

#[test]
fn hockey_pregame_carries_one_goalie_alone() {
    let mut pregame = hockey_pregame();
    pregame.away.goalie = None;
    let game = hockey::Game::Pregame(pregame);
    let bytes = unhex(&encoded(|out| hockey::encode(&game, out)));
    assert_eq!(bytes[2], 0x01 | 0x02 | 0x08 | 0x10);
    assert_eq!(hockey::decode(&bytes).unwrap(), game);
}

#[test]
fn hockey_pregame_extends_the_nba_fixed_section() {
    let pregame = hockey::Pregame {
        playoffs: false,
        ..hockey_pregame()
    };
    let nba_team = |team: hockey::PregameTeam<'static>| nba::PregameTeam {
        abbreviation: team.abbreviation,
        colors: team.colors,
        record: team.record.map(|record| Record {
            wins: record.wins,
            losses: record.losses,
        }),
        line: None,
    };
    let nba_game = nba::Game::Pregame(nba::Pregame {
        game_id: pregame.game_id,
        start_time: pregame.start_time,
        venue: pregame.venue,
        info_secondary: "",
        away: nba_team(pregame.away),
        home: nba_team(pregame.home),
    });
    let mut hockey_bytes = unhex(&encoded(|out| {
        hockey::encode(&hockey::Game::Pregame(pregame), out)
    }));
    let nba_bytes = unhex(&encoded(|out| nba::encode(&nba_game, out)));
    // The goalie bits are hockey's; with them masked, the 29 bytes match.
    hockey_bytes[2] &= 0x03;
    assert_eq!(hockey_bytes[..31], nba_bytes[..31]);
}

/// Game 3 of the 2026 final: CAR wins 5–4 in the second overtime.
fn hockey_final() -> hockey::Final<'static> {
    hockey::Final {
        game_id: "401874173",
        periods_played: 5,
        playoffs: true,
        away: final_team("CAR", 5, 0xCE1126, 0xA4A9AD, &[0, 2, 2, 0, 1]),
        home: final_team("VGK", 4, 0x344555, 0xB4975A, &[0, 2, 2, 0, 0]),
        shots: Some(hockey::Shots { away: 44, home: 39 }),
        stars: Some([
            hockey::Star {
                name: "S. Aho",
                side: Some(Side::Away),
            },
            hockey::Star {
                name: "J. Eichel",
                side: Some(Side::Home),
            },
            hockey::Star {
                name: "F. Andersen",
                side: Some(Side::Away),
            },
        ]),
        series: Some("CAR leads series 2-1"),
    }
}

const GOLDEN_HOCKEY_FINAL: &str = "0202172c2719050505050004002611ce00ada9a400554534005a97b4000002020001000202000009343031383734313733034341520356474b06532e2041686f094a2e2045696368656c0b462e20416e64657273656e14434152206c656164732073657269657320322d31";

#[test]
fn hockey_final_playoff_double_overtime() {
    let game = hockey::Game::Final(hockey_final());
    assert_eq!(
        encoded(|out| hockey::encode(&game, out)),
        GOLDEN_HOCKEY_FINAL
    );
    assert_eq!(hockey::decode(&unhex(GOLDEN_HOCKEY_FINAL)).unwrap(), game);
}

const GOLDEN_HOCKEY_FINAL_SO: &str = "02020000000005050504000300476800008c8f8f002e10c800421e040000020100010002010000093430313830323533360344414c03464c41";

#[test]
fn hockey_final_shootout_without_stars_series_or_shots() {
    // DAL wins the shootout: the fifth "period" is the shootout's 1/0, and
    // the score includes it, as ESPN reports. Flags 0, prefix all zero.
    let game = hockey::Game::Final(hockey::Final {
        game_id: "401802536",
        periods_played: 5,
        playoffs: false,
        away: final_team("DAL", 4, 0x006847, 0x8F8F8C, &[0, 2, 1, 0, 1]),
        home: final_team("FLA", 3, 0xC8102E, 0x041E42, &[0, 2, 1, 0, 0]),
        shots: None,
        stars: None,
        series: None,
    });
    assert_eq!(
        encoded(|out| hockey::encode(&game, out)),
        GOLDEN_HOCKEY_FINAL_SO
    );
    assert_eq!(
        hockey::decode(&unhex(GOLDEN_HOCKEY_FINAL_SO)).unwrap(),
        game
    );
}

#[test]
fn hockey_final_star_sides_pack_two_bits_each() {
    // An overtime final whose second star ESPN gave no team: code 0.
    let mut game = hockey_final();
    game.periods_played = 4;
    game.playoffs = false;
    game.series = None;
    game.away.line_score = &[0, 2, 2, 1];
    game.home.line_score = &[0, 2, 2, 0];
    game.stars.as_mut().unwrap()[1].side = None;
    game.stars.as_mut().unwrap()[2].side = Some(Side::Home);
    let game = hockey::Game::Final(game);
    let bytes = unhex(&encoded(|out| hockey::encode(&game, out)));
    assert_eq!(bytes[2], 0x01 | 0x04);
    assert_eq!(bytes[5], 0b10_00_01);
    assert_eq!(hockey::decode(&bytes).unwrap(), game);
}

#[test]
fn hockey_final_embeds_the_shared_final_whole() {
    // Without stars or series, everything after the 4-byte prefix is the
    // shared final, byte for byte what NBA's encoder writes.
    let game = hockey::Final {
        stars: None,
        series: None,
        ..hockey_final()
    };
    let nba_game = nba::Game::Final(nba::Final {
        game_id: game.game_id,
        periods_played: game.periods_played,
        away: game.away,
        home: game.home,
        leaders: None,
        series: None,
        records: None,
    });
    let hockey_bytes = unhex(&encoded(|out| {
        hockey::encode(&hockey::Game::Final(game), out)
    }));
    let nba_bytes = unhex(&encoded(|out| nba::encode(&nba_game, out)));
    assert_eq!(hockey_bytes[6..], nba_bytes[2..]);
}

// Encode-side bounds: encode clamps rather than fail, and what it writes
// always decodes.

#[test]
fn hockey_goals_keep_the_newest_sixteen() {
    let goals: Vec<hockey::Goal> = (0..4)
        .map(|n| hockey_goal(Side::Away, hockey::GoalTag::Goal, 1, n, 1, 1))
        .chain(sixteen_hockey_goals())
        .collect();
    assert_eq!(goals.len(), 20);
    let view = hockey::Goals::new(&goals);
    assert_eq!(view.len(), hockey::MAX_GOALS);
    assert_eq!(view.get(0), Some(goals[4]));
    let game = hockey::Game::Live(hockey::Live {
        goals: view,
        ..hockey_live()
    });
    let bytes = unhex(&encoded(|out| hockey::encode(&game, out)));
    let hockey::Game::Live(decoded) = hockey::decode(&bytes).unwrap() else {
        unreachable!()
    };
    assert_eq!(decoded.goals.iter().collect::<Vec<_>>(), goals[4..]);
}

#[test]
fn hockey_shootout_attempts_cap_at_the_mask_width() {
    let mut live = hockey_shootout();
    live.shootout = Some(hockey::Shootout {
        away: hockey::ShootoutAttempts {
            taken: 20,
            scored: 0xFFFF,
        },
        // A goal bit past the attempt count is not an attempt.
        home: hockey::ShootoutAttempts {
            taken: 3,
            scored: 0xFF,
        },
        next_shooter: Some(Side::Home),
    });
    let bytes = unhex(&encoded(|out| {
        hockey::encode(&hockey::Game::Live(live), out)
    }));
    assert_eq!(bytes[31..37], [16, 3, 0xFF, 0xFF, 0x07, 0x00]);
    let hockey::Game::Live(decoded) = hockey::decode(&bytes).unwrap() else {
        unreachable!()
    };
    let shootout = decoded.shootout.unwrap();
    assert_eq!(shootout.away.taken as usize, hockey::MAX_SHOOTOUT_ATTEMPTS);
    assert_eq!(shootout.home.scored, 0b111);
}

#[test]
fn hockey_coordinates_clamp_to_the_boards() {
    let goal = [hockey_goal(
        Side::Home,
        hockey::GoalTag::EmptyNet,
        3,
        1150,
        i8::MIN,
        i8::MAX,
    )];
    let game = hockey::Game::Live(hockey::Live {
        goals: hockey::Goals::new(&goal),
        ..hockey_live()
    });
    let bytes = unhex(&encoded(|out| hockey::encode(&game, out)));
    assert_eq!(bytes[42..44], [(-100i8) as u8, 43]);
    let hockey::Game::Live(decoded) = hockey::decode(&bytes).unwrap() else {
        unreachable!()
    };
    assert_eq!(
        decoded.goals.last().unwrap().coordinate,
        Some(hockey::Coordinate {
            x: -hockey::MAX_COORDINATE_X,
            y: hockey::MAX_COORDINATE_Y,
        })
    );
}

// Decode-side: every prefix of a valid payload fails cleanly, and each
// rejection rule in the module docs has its case.

/// One golden per shape: each state with its optional parts present, and
/// again with them absent — where the payload ends on a different field.
fn hockey_payloads() -> [Vec<u8>; 7] {
    [
        unhex(GOLDEN_HOCKEY_LIVE),
        unhex(GOLDEN_HOCKEY_PUCK_DROP),
        unhex(GOLDEN_HOCKEY_SHOOTOUT),
        unhex(GOLDEN_HOCKEY_PRE),
        unhex(GOLDEN_HOCKEY_PRE_BARE),
        unhex(GOLDEN_HOCKEY_FINAL),
        unhex(GOLDEN_HOCKEY_FINAL_SO),
    ]
}

#[test]
fn hockey_truncation_at_every_offset_is_a_clean_error() {
    for bytes in hockey_payloads() {
        for len in 0..bytes.len() {
            let error = hockey::decode(&bytes[..len])
                .expect_err("a strict prefix of a payload is never a payload");
            assert!(
                matches!(
                    error.kind,
                    DecodeErrorKind::Empty
                        | DecodeErrorKind::Truncated(_)
                        | DecodeErrorKind::TruncatedFixed { .. }
                        | DecodeErrorKind::TruncatedLength(_)
                        | DecodeErrorKind::TruncatedString { .. }
                        | DecodeErrorKind::TruncatedLineScores { .. }
                        | DecodeErrorKind::TruncatedRecords { .. }
                ),
                "prefix {len} of {} reported {error}",
                bytes.len()
            );
        }
    }
}

#[test]
fn hockey_trailing_bytes_are_rejected_in_every_state() {
    for mut bytes in hockey_payloads() {
        let end = bytes.len();
        bytes.push(0);
        assert_eq!(
            hockey::decode(&bytes).unwrap_err(),
            DecodeError {
                offset: end,
                kind: DecodeErrorKind::Trailing(1),
            }
        );
    }
}

#[test]
fn hockey_truncated_goal_records_report_the_shortfall() {
    let bytes = unhex(GOLDEN_HOCKEY_LIVE);
    let error = hockey::decode(&bytes[..50]).unwrap_err();
    assert_eq!(
        error,
        DecodeError {
            offset: 38,
            kind: DecodeErrorKind::TruncatedRecords {
                field: "goal records",
                need: 30,
                have: 12,
            },
        }
    );
    assert_eq!(
        format!("{error}"),
        "@38: truncated goal records: need 30 bytes, have 12"
    );
}

fn hockey_live_err(edit: impl FnOnce(&mut Vec<u8>)) -> DecodeError {
    let mut bytes = unhex(GOLDEN_HOCKEY_LIVE);
    edit(&mut bytes);
    hockey::decode(&bytes).expect_err("payload must be rejected")
}

fn invalid(offset: usize, field: &'static str, code: u8) -> DecodeError {
    DecodeError {
        offset,
        kind: DecodeErrorKind::InvalidCode { field, code },
    }
}

fn out_of_range(offset: usize, field: &'static str, value: i32) -> DecodeError {
    DecodeError {
        offset,
        kind: DecodeErrorKind::OutOfRange { field, value },
    }
}

#[test]
fn hockey_undefined_flag_bits_are_rejected() {
    for bit in [0x20, 0x40, 0x80] {
        let error = hockey_live_err(|bytes| bytes[2] |= bit);
        assert_eq!(error, invalid(2, "live flags", 0x0B | bit));
    }

    for bit in [0x20, 0x40, 0x80] {
        let mut bytes = unhex(GOLDEN_HOCKEY_PRE);
        bytes[2] |= bit;
        assert_eq!(
            hockey::decode(&bytes).unwrap_err(),
            invalid(2, "pregame flags", 0x1F | bit)
        );
    }

    // Bit3 is the final's gap, kept so playoffs is bit4 in every state.
    for bit in [0x08, 0x20, 0x40, 0x80] {
        let mut bytes = unhex(GOLDEN_HOCKEY_FINAL);
        bytes[2] |= bit;
        assert_eq!(
            hockey::decode(&bytes).unwrap_err(),
            invalid(2, "final flags", 0x17 | bit)
        );
    }
}

#[test]
fn hockey_unknown_codes_are_rejected() {
    assert_eq!(
        hockey_live_err(|bytes| bytes[4] = 3),
        invalid(4, "live phase", 3)
    );
    assert_eq!(
        hockey_live_err(|bytes| bytes[27] = 6),
        invalid(27, "strength", 6)
    );

    let mut shooter = unhex(GOLDEN_HOCKEY_SHOOTOUT);
    shooter[37] = 3;
    assert_eq!(
        hockey::decode(&shooter).unwrap_err(),
        invalid(37, "next shooter", 3)
    );

    // A goal record's bits 4–7 are spare; the third record starts at 50.
    for spare in [0x10, 0x80] {
        let error = hockey_live_err(|bytes| bytes[50] |= spare);
        assert_eq!(error, invalid(50, "goal side/tag", 0x0B | spare));
    }

    // The third star's side 3, then a bit above the third star.
    for (sides, code) in [(0b11_10_01, 0b11_10_01), (0b0100_1001, 0b0100_1001)] {
        let mut bytes = unhex(GOLDEN_HOCKEY_FINAL);
        bytes[5] = sides;
        assert_eq!(
            hockey::decode(&bytes).unwrap_err(),
            invalid(5, "star sides", code)
        );
    }
}

#[test]
fn hockey_counts_past_sixteen_are_rejected() {
    assert_eq!(
        hockey_live_err(|bytes| bytes[30] = 17),
        out_of_range(30, "goal count", 17)
    );

    let mut away = unhex(GOLDEN_HOCKEY_SHOOTOUT);
    away[31] = 17;
    assert_eq!(
        hockey::decode(&away).unwrap_err(),
        out_of_range(31, "away shootout attempts", 17)
    );

    let mut home = unhex(GOLDEN_HOCKEY_SHOOTOUT);
    home[32] = 255;
    let error = hockey::decode(&home).unwrap_err();
    assert_eq!(error, out_of_range(32, "home shootout attempts", 255));
    assert_eq!(
        format!("{error}"),
        "@32: home shootout attempts out of range: 255"
    );
}

#[test]
fn hockey_shootout_goals_past_the_attempt_count_are_rejected() {
    // Away has taken 3: bit 3 is a fourth attempt that never happened.
    let mut away = unhex(GOLDEN_HOCKEY_SHOOTOUT);
    away[33] |= 0b1000;
    assert_eq!(
        hockey::decode(&away).unwrap_err(),
        out_of_range(33, "away shootout goals", 0b1101)
    );

    // Home has taken 2; bit 15 lives in the mask's high byte.
    let mut home = unhex(GOLDEN_HOCKEY_SHOOTOUT);
    home[36] = 0x80;
    assert_eq!(
        hockey::decode(&home).unwrap_err(),
        out_of_range(35, "home shootout goals", 0x8001)
    );

    // With no attempts, no bit may be set.
    let mut none = unhex(GOLDEN_HOCKEY_SHOOTOUT);
    none[32] = 0;
    assert_eq!(
        hockey::decode(&none).unwrap_err(),
        out_of_range(35, "home shootout goals", 1)
    );
}

#[test]
fn hockey_coordinates_past_the_boards_are_rejected() {
    // The first record's x at 42, y at 43.
    assert_eq!(
        hockey_live_err(|bytes| bytes[42] = 101),
        out_of_range(42, "goal x", 101)
    );
    assert_eq!(
        hockey_live_err(|bytes| bytes[42] = i8::MIN as u8),
        out_of_range(42, "goal x", -128)
    );
    assert_eq!(
        hockey_live_err(|bytes| bytes[43] = (-44i8) as u8),
        out_of_range(43, "goal y", -44)
    );
}

#[test]
fn hockey_absent_groups_are_placeholders_decode_never_reads() {
    // Football's rule: a group whose flag is unset decodes as absent whatever
    // its bytes hold. Clear strength and shots, then poison their bytes and
    // the (unflagged) shootout block with values that would be rejected if
    // read.
    let mut bytes = unhex(GOLDEN_HOCKEY_LIVE);
    bytes[2] &= !(0x02 | 0x08);
    bytes[25..38].copy_from_slice(&[
        9, 9, 0xEE, 9, 9, 5, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    ]);
    let hockey::Game::Live(live) = hockey::decode(&bytes).unwrap() else {
        unreachable!()
    };
    assert_eq!(
        (live.strength, live.shots, live.shootout),
        (None, None, None)
    );
    assert_eq!(live.goals.len(), 5);

    // A goal without a coordinate: its x/y bytes are not read either.
    let mut bytes = unhex(GOLDEN_HOCKEY_LIVE);
    bytes[38] &= !0x08;
    bytes[42..44].copy_from_slice(&[0x7F, 0x80]);
    let hockey::Game::Live(live) = hockey::decode(&bytes).unwrap() else {
        unreachable!()
    };
    assert_eq!(live.goals.get(0).unwrap().coordinate, None);

    // A final without stars ignores the star-sides byte, and without shots
    // ignores their bytes.
    let mut bytes = unhex(GOLDEN_HOCKEY_FINAL_SO);
    bytes[3..6].copy_from_slice(&[7, 7, 0xFF]);
    let hockey::Game::Final(game) = hockey::decode(&bytes).unwrap() else {
        unreachable!()
    };
    assert_eq!((game.stars, game.shots), (None, None));
}

// --- Encode-side rules ---

#[test]
fn scores_saturate_and_temperatures_clamp() {
    assert_eq!(saturate_score(1_000_000), u16::MAX);
    assert_eq!(saturate_score(7), 7);
    assert_eq!(clamp_temperature(300), 255);
    assert_eq!(clamp_temperature(-40), 0);
    assert_eq!(clamp_temperature(72), 72);
}

#[test]
fn long_strings_truncate_at_a_char_boundary() {
    let mut long = String::new();
    for _ in 0..254 {
        long.push('x');
    }
    long.push('é');
    let game = soccer::Game::Live(soccer::Live {
        commentary: Some(soccer::Commentary {
            id: "1",
            text: &long,
        }),
        ..soccer_live_base()
    });
    let bytes = unhex(&encoded(|out| soccer::encode(&game, out)));
    // The 2-byte 'é' can't fit under the 255-byte cap, so 254 bytes travel.
    let decoded = soccer::decode(&bytes).unwrap();
    let soccer::Game::Live(live) = decoded else {
        unreachable!()
    };
    let text = live.commentary.unwrap().text;
    assert_eq!(text.len(), 254);
    assert!(text.chars().all(|c| c == 'x'));
}

#[test]
fn line_scores_truncate_at_the_length_prefix_cap() {
    let long = [1u8; 300];
    let game = mlb::Game::Final(mlb::Final {
        game_id: "401570729",
        innings_played: 9,
        away: final_team("SEA", 4, 0, 0, &long),
        home: final_team("BOS", 5, 0, 0, &[]),
    });
    let bytes = unhex(&encoded(|out| mlb::encode(&game, out)));
    assert_eq!(bytes[3], 255);
    let mlb::Game::Final(decoded) = mlb::decode(&bytes).unwrap() else {
        unreachable!()
    };
    assert_eq!(decoded.away.line_score.len(), 255);
}

#[test]
fn a_full_buffer_is_an_error_not_a_panic() {
    let mut buf = [0u8; 8];
    let mut sink = SliceSink::new(&mut buf);
    assert_eq!(mlb::encode(&mlb_live(), &mut sink), Err(BufferFull));
}

// --- Decode-side failures ---

fn decode_err(bytes: &[u8]) -> DecodeError {
    mlb::decode(bytes).expect_err("payload must be rejected")
}

#[test]
fn empty_and_misversioned_payloads_are_rejected() {
    assert_eq!(decode_err(&[]).kind, DecodeErrorKind::Empty);
    // A JSON body served to a struct client starts with '{'.
    assert_eq!(
        decode_err(b"{\"state\":\"live\"}").kind,
        DecodeErrorKind::UnsupportedVersion(b'{')
    );
    assert_eq!(
        decode_err(&[1, 1]).kind,
        DecodeErrorKind::UnsupportedVersion(1)
    );
}

#[test]
fn header_and_state_failures_carry_their_offset() {
    assert_eq!(
        decode_err(&[2]),
        DecodeError {
            offset: 1,
            kind: DecodeErrorKind::Truncated("state byte"),
        }
    );
    assert_eq!(
        decode_err(&[2, 9]),
        DecodeError {
            offset: 1,
            kind: DecodeErrorKind::UnknownState(9),
        }
    );
}

#[test]
fn a_truncated_fixed_section_reports_what_it_needed() {
    let error = decode_err(&[2, 1, 0, 0]);
    assert_eq!(
        error,
        DecodeError {
            offset: 2,
            kind: DecodeErrorKind::TruncatedFixed { need: 29, have: 4 },
        }
    );
    assert_eq!(format!("{error}"), "@2: truncated fixed section: 4 < 29");
}

#[test]
fn a_truncated_string_reports_the_field_and_the_shortfall() {
    let mut bytes = unhex(&format!("0201{}", &GOLDEN_MLB_LIVE_V1[2..]));
    bytes.truncate(32);
    let error = decode_err(&bytes);
    assert_eq!(
        error,
        DecodeError {
            offset: 30,
            kind: DecodeErrorKind::TruncatedString {
                field: "game_id",
                need: 9,
                have: 2,
            },
        }
    );
    assert_eq!(
        format!("{error}"),
        "@30: truncated inside game_id: need 9 bytes, have 2"
    );
}

#[test]
fn a_missing_length_byte_reports_the_field() {
    let mut bytes = unhex(&format!("0201{}", &GOLDEN_MLB_LIVE_V1[2..]));
    bytes.truncate(29);
    assert_eq!(
        decode_err(&bytes),
        DecodeError {
            offset: 29,
            kind: DecodeErrorKind::TruncatedLength("game_id"),
        }
    );
}

#[test]
fn trailing_bytes_are_rejected() {
    let mut bytes = unhex(GOLDEN_MLB_FINAL);
    let end = bytes.len();
    bytes.extend_from_slice(&[0, 0, 0]);
    let error = decode_err(&bytes);
    assert_eq!(
        error,
        DecodeError {
            offset: end,
            kind: DecodeErrorKind::Trailing(3),
        }
    );
    assert_eq!(
        format!("{error}"),
        format!("@{end}: 3 unexpected trailing bytes")
    );
}

#[test]
fn invalid_utf8_in_a_string_is_an_error() {
    let mut bytes = unhex(GOLDEN_MLB_FINAL);
    // The tail is `09 <game_id> 03 <away> 03 <home>`; poison the id's first byte.
    let id_start = bytes.len() - 17;
    bytes[id_start] = 0xFF;
    assert_eq!(
        decode_err(&bytes).kind,
        DecodeErrorKind::InvalidUtf8("game_id")
    );
}

#[test]
fn truncated_line_scores_are_rejected() {
    let mut bytes = unhex(GOLDEN_MLB_FINAL);
    bytes.truncate(27);
    assert_eq!(
        decode_err(&bytes),
        DecodeError {
            offset: 25,
            kind: DecodeErrorKind::TruncatedLineScores { need: 17, have: 2 },
        }
    );
}

#[test]
fn out_of_range_enum_codes_are_rejected() {
    let mut mlb_bytes = unhex(&format!("0201{}", &GOLDEN_MLB_LIVE_V1[2..]));
    mlb_bytes[4] = 4;
    assert_eq!(
        decode_err(&mlb_bytes),
        DecodeError {
            offset: 4,
            kind: DecodeErrorKind::InvalidCode {
                field: "inning half",
                code: 4,
            },
        }
    );

    let mut nba_bytes = unhex(GOLDEN_NBA_LIVE);
    nba_bytes[4] = 3;
    assert_eq!(
        nba::decode(&nba_bytes).unwrap_err().kind,
        DecodeErrorKind::InvalidCode {
            field: "live phase",
            code: 3,
        }
    );

    let mut football_bytes = unhex(GOLDEN_FOOTBALL_LIVE);
    football_bytes[4] = 7;
    assert_eq!(
        football::decode(&football_bytes).unwrap_err().kind,
        DecodeErrorKind::InvalidCode {
            field: "live phase",
            code: 7,
        }
    );

    let mut soccer_bytes = unhex(GOLDEN_SOCCER_LIVE);
    soccer_bytes[3] = 6;
    assert_eq!(
        soccer::decode(&soccer_bytes).unwrap_err(),
        DecodeError {
            offset: 3,
            kind: DecodeErrorKind::InvalidCode {
                field: "soccer period",
                code: 6,
            },
        }
    );

    let mut flavor_bytes = unhex(GOLDEN_SOCCER_FINAL);
    flavor_bytes[2] = 3;
    assert_eq!(
        soccer::decode(&flavor_bytes).unwrap_err(),
        DecodeError {
            offset: 2,
            kind: DecodeErrorKind::InvalidCode {
                field: "full-time flavor",
                code: 3,
            },
        }
    );
}

#[test]
fn game_list_failures_stop_iteration() {
    // Count claims three entries; only one is present.
    let mut bytes = unhex(GOLDEN_LIST);
    bytes.truncate(13);
    let mut iter = list::decode(&bytes).unwrap();
    assert!(iter.next().unwrap().is_ok());
    assert!(iter.next().unwrap().is_err());
    assert!(iter.next().is_none());

    // A state byte outside 0..=2.
    let mut bad_state = unhex(GOLDEN_LIST);
    bad_state[2] = 5;
    let mut iter = list::decode(&bad_state).unwrap();
    assert_eq!(
        iter.next().unwrap().unwrap_err(),
        DecodeError {
            offset: 2,
            kind: DecodeErrorKind::InvalidCode {
                field: "game state",
                code: 5,
            },
        }
    );

    // Trailing bytes surface after the declared entries are read.
    let mut trailing = unhex(GOLDEN_LIST);
    let end = trailing.len();
    trailing.push(0);
    let iter = list::decode(&trailing).unwrap();
    let results: Vec<_> = iter.collect();
    assert_eq!(results.len(), 4);
    assert_eq!(
        results[3].unwrap_err(),
        DecodeError {
            offset: end,
            kind: DecodeErrorKind::Trailing(1),
        }
    );
}
