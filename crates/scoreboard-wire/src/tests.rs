//! Golden payloads and their round trips.
//!
//! The hex constants are the byte-identity contract: they were produced by the
//! encoder the deployed MicroPython firmware decodes, and several were shared
//! verbatim with the retired `tools/wire_format_check.py` cross-check. Every
//! one is asserted in both directions — encode must produce it, decode must
//! reproduce the value — so the two halves cannot drift apart.

use std::string::String;
use std::{format, vec::Vec};

use crate::error::{DecodeError, DecodeErrorKind};
use crate::{
    BufferFull, FinalTeam, GameState, LastPlay, LivePhase, Record, Side, SliceSink, TeamColors,
    TeamState, clamp_temperature, football, list, mlb, nba, saturate_score, soccer,
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
    assert_eq!(encoded(|out| mlb::encode(&mlb_final(), out)), GOLDEN_MLB_FINAL);
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
    assert_eq!(soccer::decode(&unhex(GOLDEN_SOCCER_HALFTIME)).unwrap(), game);
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

const GOLDEN_NBA_HALFTIME: &str =
    "020100020134004a00a9765d0012b1f5008e004e001ba0f90009343031383131303336034d454d045554414803302e30";

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
    assert_eq!(nba::decode(&unhex(GOLDEN_NBA_PRE_NO_RECORDS)).unwrap(), game);
}

const GOLDEN_NBA_FINAL: &str =
    "0202040404760064008a421d002e10c800a88c000060111d001e1c1e1e19191919093430313831313032360344455403434841";

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

const GOLDEN_FOOTBALL_LIVE_BREAK: &str =
    "020100020100000000000a000e008d330000300cc6003718e3001cb8ff000934303137373235313103425546024b4304303a3030";

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
            rank_line: None,
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
            rank_line: None,
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
    pregame.home.rank_line = Some("#3 OHIO STATE");
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

const GOLDEN_FOOTBALL_FINAL_OT: &str =
    "020205050518001b008d330000300cc6003718e3001cb8ff0007030707000707000a030934303137373235313403425546024b43";

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

const GOLDEN_FOOTBALL_FINAL: &str =
    "020204040418001b003718e3001cb8ff008d330000300cc60007030707000a070a09343031353437343137024b4303425546";

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
    assert_eq!(decode_err(&[1, 1]).kind, DecodeErrorKind::UnsupportedVersion(1));
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
    assert_eq!(format!("{error}"), format!("@{end}: 3 unexpected trailing bytes"));
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
