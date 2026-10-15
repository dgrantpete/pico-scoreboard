//! Golden payloads and their round trips.
//!
//! The hex constants are the byte-identity contract: they were produced by the
//! encoder the deployed MicroPython firmware decodes, and several were shared
//! verbatim with the Python cross-check this crate retired. Every
//! one is asserted in both directions — encode must produce it, decode must
//! reproduce the value — so the two halves cannot drift apart.

use std::string::String;
use std::{format, vec::Vec};

use crate::error::{DecodeError, DecodeErrorKind};
use crate::{
    BufferFull, FinalTeam, GameState, LastPlay, LivePhase, Record, Side, SliceSink, TeamColors,
    TeamState, clamp_temperature, football, hockey, list, mlb, nba, saturate_score, soccer,
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

#[test]
fn nba_live_with_last_play() {
    let game = nba::Game::Live(nba::Live {
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
    });
    assert_eq!(encoded(|out| nba::encode(&game, out)), GOLDEN_NBA_LIVE);
    assert_eq!(nba::decode(&unhex(GOLDEN_NBA_LIVE)).unwrap(), game);
}

const GOLDEN_NBA_HALFTIME: &str = "020100020134004a00a9765d0012b1f5008e004e001ba0f90009343031383131303336034d454d045554414803302e30";

#[test]
fn nba_halftime_without_last_play() {
    // Break state: flags 0 (no last play), phase 1, clock reads "0.0".
    let game = nba::Game::Live(nba::Live {
        game_id: "401811036",
        period: 2,
        phase: LivePhase::Halftime,
        clock: "0.0",
        away: team("MEM", 52, 0x5D76A9, 0xF5B112),
        home: team("UTAH", 74, 0x4E008E, 0xF9A01B),
        last_play: None,
    });
    assert_eq!(encoded(|out| nba::encode(&game, out)), GOLDEN_NBA_HALFTIME);
    assert_eq!(nba::decode(&unhex(GOLDEN_NBA_HALFTIME)).unwrap(), game);
}

fn nba_pregame() -> nba::Game<'static> {
    nba::Game::Pregame(nba::Pregame {
        game_id: "401811040",
        start_time: 1_775_874_600,
        venue: "crypto.com Arena",
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
fn nba_ext_rejects_unknown_flag_bits_and_bytes_past_the_lines() {
    let unknown = unhex(&format!("{GOLDEN_NBA_PRE}04"));
    assert_eq!(
        nba::decode(&unknown).unwrap_err(),
        DecodeError {
            offset: unknown.len() - 1,
            kind: DecodeErrorKind::InvalidCode {
                field: "pregame extension flags",
                code: 4,
            },
        }
    );
    let trailing = unhex(&format!("{GOLDEN_NBA_PRE_EXT}00"));
    assert_eq!(
        nba::decode(&trailing).unwrap_err().kind,
        DecodeErrorKind::Trailing(1)
    );
    // A live game defines no extension: anything after it is still trailing.
    let live = unhex(&format!("{GOLDEN_NBA_LIVE}00"));
    assert_eq!(
        nba::decode(&live).unwrap_err().kind,
        DecodeErrorKind::Trailing(1)
    );
}

const GOLDEN_NBA_FINAL: &str = "0202040404760064008a421d002e10c800a88c000060111d001e1c1e1e19191919093430313831313032360344455403434841";

#[test]
fn nba_final_regulation() {
    let game = nba::Game::Final(nba::Final {
        game_id: "401811026",
        periods_played: 4,
        away: final_team("DET", 118, 0x1D428A, 0xC8102E, &[30, 28, 30, 30]),
        home: final_team("CHA", 100, 0x008CA8, 0x1D1160, &[25, 25, 25, 25]),
    });
    assert_eq!(encoded(|out| nba::encode(&game, out)), GOLDEN_NBA_FINAL);
    assert_eq!(nba::decode(&unhex(GOLDEN_NBA_FINAL)).unwrap(), game);
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
