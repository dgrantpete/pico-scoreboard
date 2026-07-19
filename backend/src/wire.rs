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
//! `WIRE_VERSION = 2`. State codes match [`crate::shared::game::GameState::code`]:
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
//!
//! # Soccer game detail (`GET /soccer/{league}/games/{game_id}`)
//!
//! Same 2-byte header (`u8 version = 2`, `u8 state`); the payload layout is
//! sport-specific — the firmware picks the parser by which endpoint it
//! polled, not by sniffing bytes. The games list reuses the shared list
//! encoding above. Parsed by `firmware/src/scoreboard/soccer.py`.
//!
//! ## Soccer live (state = 1)
//!
//! Fixed 24-byte section at offset 2 (`struct.unpack_from('<BBHHHIIII', buf, 2)`):
//!
//! | off | type | field                                            |
//! |-----|------|--------------------------------------------------|
//! | 2   | u8   | flags (see below)                                |
//! | 3   | u8   | half — ESPN's raw period: regulation halves 1/2, extra-time halves 3/4, shootout 5 |
//! | 4   | u16  | clock_seconds — elapsed match seconds, floor-minute convention (parsed from ESPN's displayClock; the firmware extrapolates forward from this anchor) |
//! | 6   | u16  | away_score (u32 saturated to u16)                |
//! | 8   | u16  | home_score                                       |
//! | 10  | u32  | away_colors.primary (0x00RRGGBB)                 |
//! | 14  | u32  | away_colors.alternate                            |
//! | 18  | u32  | home_colors.primary                              |
//! | 22  | u32  | home_colors.alternate                            |
//!
//! Flags: bit0 = break (a non-playing interval — halftime, extra-time
//! halftime, end of regulation, end of extra time; the clock is paused), bit1
//! = last event present, bit2 = event is a red card (else a goal), bit3 =
//! event is the away side's, bit4 = home side's (neither set = unattributed),
//! bit5 = commentary present. Bits 2-4 are meaningless unless bit1.
//!
//! Strings from offset 26: `game_id`, `away.abbreviation`,
//! `home.abbreviation`, then **iff** bit1: `event.clock` (display-shaped,
//! e.g. "90'+3'") and `event.athlete` (short name, may be empty), then
//! **iff** bit5: `commentary.id` (ESPN sequence, change-detection key) and
//! `commentary.text` (latest play-by-play line — the firmware flashes it
//! like MLB's play text).
//!
//! ## Soccer pregame (state = 0)
//!
//! Fixed 20-byte section at offset 2 (`<IIIII`): `start_time` (unix epoch
//! seconds UTC), then away primary/alternate + home primary/alternate colors.
//! Strings from offset 22: `game_id`, `away.abbreviation`,
//! `home.abbreviation`, `venue` (stadium `fullName`). (The league's display
//! name is firmware-side config — the device knows which league it polled.)
//!
//! ## Soccer final (state = 2)
//!
//! Fixed 21-byte section at offset 2 (`<BHHIIII`): `flavor` u8 (0 = full time,
//! 1 = after extra time, 2 = after penalties — how the match was decided),
//! `away_score` u16, `home_score` u16, then the four colors. Strings from
//! offset 23: `game_id`, `away.abbreviation`, `home.abbreviation`,
//! `away.scorers`, `home.scorers` (pre-formatted "M. Merino 90'+1', ..." lists,
//! always present, empty when scoreless).
//!
//! # NBA game detail (`GET /basketball/nba/games/{game_id}`)
//!
//! Same 2-byte header (`u8 version = 2`, `u8 state`); the payload layout is
//! sport-specific, like soccer's. Parsed by `firmware/src/scoreboard/nba.py`.
//!
//! ## NBA live (state = 1)
//!
//! Fixed 23-byte section at offset 2 (`struct.unpack_from('<BBBHHIIII', buf, 2)`):
//!
//! | off | type | field                                            |
//! |-----|------|--------------------------------------------------|
//! | 2   | u8   | flags (bit0 = last play present)                 |
//! | 3   | u8   | period (quarter 1–4; overtime = 5+)              |
//! | 4   | u8   | phase (0=in progress, 1=halftime, 2=end of period) |
//! | 5   | u16  | away_score (u32 saturated to u16)                |
//! | 7   | u16  | home_score                                       |
//! | 9   | u32  | away_colors.primary (0x00RRGGBB)                 |
//! | 13  | u32  | away_colors.alternate                            |
//! | 17  | u32  | home_colors.primary                              |
//! | 21  | u32  | home_colors.alternate                            |
//!
//! Strings from offset 25: `game_id`, `away.abbreviation`,
//! `home.abbreviation`, `clock` (display-shaped: "10:08"; "53.0" under a
//! minute; meaningless during breaks — render by `phase`; never
//! extrapolated, the firmware re-renders it each poll), then **iff** bit0:
//! `last_play.id` (change-detection key for the flash) and `last_play.text`.
//!
//! ## NBA pregame (state = 0)
//!
//! Fixed 29-byte section at offset 2 (`<BHHHHIIIII`):
//!
//! | off | type | field                                            |
//! |-----|------|--------------------------------------------------|
//! | 2   | u8   | flags (bit0 = away record, bit1 = home record)   |
//! | 3   | u16  | away_wins  (0 if no record)                      |
//! | 5   | u16  | away_losses                                      |
//! | 7   | u16  | home_wins                                        |
//! | 9   | u16  | home_losses                                      |
//! | 11  | u32  | start_time (unix epoch, seconds, UTC)            |
//! | 15  | u32  | away_colors.primary                              |
//! | 19  | u32  | away_colors.alternate                            |
//! | 23  | u32  | home_colors.primary                              |
//! | 27  | u32  | home_colors.alternate                            |
//!
//! Strings from offset 31, in order: `game_id`, `away.abbreviation`,
//! `home.abbreviation`, `venue`.
//!
//! ## NBA final (state = 2)
//!
//! Identical layout to the MLB final (`<BBBHHIIII`, 23 bytes at offset 2)
//! with `periods_played` in place of `innings_played` and per-quarter points
//! in the line scores (overtime extends both past 4). Then the away and home
//! line-score bytes and strings: `game_id`, `away.abbreviation`,
//! `home.abbreviation`.
//!
//! # Football game detail (`GET /football/{league}/games/{game_id}`)
//!
//! Same 2-byte header (`u8 version = 2`, `u8 state`); the payload layout is
//! sport-specific, like the others (the firmware picks the parser by which
//! endpoint it polled). The games list reuses the shared list encoding above.
//! Parsed by `firmware/src/scoreboard/football.py`.
//!
//! ## Football live (state = 1)
//!
//! Fixed 28-byte section at offset 2 (`struct.unpack_from('<BBBBBBBBHHIIII', buf, 2)`):
//!
//! | off | type | field                                            |
//! |-----|------|--------------------------------------------------|
//! | 2   | u8   | flags (see below)                                |
//! | 3   | u8   | period (quarter 1–4; overtime = 5+)              |
//! | 4   | u8   | phase (0=in progress, 1=halftime, 2=end of period) — NBA `LivePhase` twin |
//! | 5   | u8   | down (1–4; 0 when no situation)                  |
//! | 6   | u8   | distance — yards to the first-down line (0 when no situation) |
//! | 7   | u8   | yard_line — absolute ball spot 0–100 (0 when no situation) |
//! | 8   | u8   | away_timeouts (0 when timeouts absent)           |
//! | 9   | u8   | home_timeouts (0 when timeouts absent)           |
//! | 10  | u16  | away_score (u32 saturated to u16)                |
//! | 12  | u16  | home_score                                       |
//! | 14  | u32  | away_colors.primary (0x00RRGGBB)                 |
//! | 18  | u32  | away_colors.alternate                            |
//! | 22  | u32  | home_colors.primary                              |
//! | 26  | u32  | home_colors.alternate                            |
//!
//! Flags: bit0 = last play present, bit1 = situation present, bit2 = possession
//! is home (meaningless unless bit1; unset = away), bit3 = red zone (meaningless
//! unless bit1), bit4 = timeouts present. When bit1 is unset, down/distance/
//! yard_line are 0; when bit4 is unset, both timeout counts are 0.
//!
//! Strings from offset 30: `game_id`, `away.abbreviation`,
//! `home.abbreviation`, `clock` (display-shaped: "12:00", "0:37"; meaningless
//! during breaks — render by `phase`; never extrapolated, the firmware
//! re-renders it each poll), then **iff** bit0: `last_play.id`
//! (change-detection key for the flash) and `last_play.text`.
//!
//! ## Football pregame (state = 0)
//!
//! Byte-identical to the NBA pregame (`<BHHHHIIIII`, 29 bytes at offset 2:
//! flags, away wins/losses, home wins/losses, start_time, then the four
//! colors) plus two rank flag bits. Flags: bit0 = away record, bit1 = home
//! record, bit2 = away rank line present, bit3 = home rank line present.
//! Numeric record fields whose flag is unset are zero.
//!
//! Strings from offset 31, in order: `game_id`, `away.abbreviation`,
//! `home.abbreviation`, `venue`, then `away.rank_line` **iff** bit2 and
//! `home.rank_line` **iff** bit3 (display-shaped "#3 OHIO STATE" — college
//! only, present only when ranked). Mirrors MLB's probable-pitcher flag/string
//! pattern (the rank line rides the pitcher slot).
//!
//! ## Football final (state = 2)
//!
//! Byte-identical to the NBA final (`<BBBHHIIII`, 23 bytes at offset 2):
//! `periods_played`, away/home line-score lengths, scores, colors, then the
//! away and home per-quarter points bytes (overtime extends past 4) and
//! strings `game_id`, `away.abbreviation`, `home.abbreviation`.

use crate::football::{FootballFinalGame, FootballGame, FootballLiveGame, FootballPregameGame};
use crate::mlb::{InningHalf, MlbFinalGame, MlbGame, MlbLiveGame, MlbPregameGame};
use crate::nba::{NbaFinalGame, NbaGame, NbaLiveGame, NbaPregameGame};
use crate::shared::game::{GameListEntry, Side};
use crate::soccer::{
    LastEvent, SoccerFinalFlavor, SoccerFinalGame, SoccerGame, SoccerLiveGame, SoccerPregameGame,
};

pub const STRUCT_CONTENT_TYPE: &str = "application/x-scoreboard-struct";
pub const WIRE_VERSION: u8 = 2;

const MLB_FLAG_AT_BAT: u8 = 0x01;

const MLB_FLAG_WEATHER: u8 = 0x01;
const MLB_FLAG_AWAY_RECORD: u8 = 0x02;
const MLB_FLAG_HOME_RECORD: u8 = 0x04;
const MLB_FLAG_AWAY_PROBABLE: u8 = 0x08;
const MLB_FLAG_HOME_PROBABLE: u8 = 0x10;

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
pub fn encode_mlb_game(game: &MlbGame) -> Vec<u8> {
    let mut out = Vec::with_capacity(256);
    out.push(WIRE_VERSION);
    match game {
        MlbGame::Pregame(g) => {
            out.push(0);
            write_mlb_pregame(&mut out, g);
        }
        MlbGame::Live(g) => {
            out.push(1);
            write_mlb_live(&mut out, g);
        }
        MlbGame::Final(g) => {
            out.push(2);
            write_mlb_final(&mut out, g);
        }
    }
    out
}

fn write_mlb_live(out: &mut Vec<u8>, game: &MlbLiveGame) {
    out.push(if game.at_bat.is_some() {
        MLB_FLAG_AT_BAT
    } else {
        0
    });
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

fn write_mlb_pregame(out: &mut Vec<u8>, game: &MlbPregameGame) {
    let mut flags = 0u8;
    let temperature = match &game.weather {
        Some(w) => {
            flags |= MLB_FLAG_WEATHER;
            w.temperature.clamp(0, u8::MAX as i16) as u8
        }
        None => 0,
    };
    let (away_wins, away_losses) = match &game.away.record {
        Some(r) => {
            flags |= MLB_FLAG_AWAY_RECORD;
            (r.wins, r.losses)
        }
        None => (0, 0),
    };
    let (home_wins, home_losses) = match &game.home.record {
        Some(r) => {
            flags |= MLB_FLAG_HOME_RECORD;
            (r.wins, r.losses)
        }
        None => (0, 0),
    };
    if game.away.probable_pitcher.is_some() {
        flags |= MLB_FLAG_AWAY_PROBABLE;
    }
    if game.home.probable_pitcher.is_some() {
        flags |= MLB_FLAG_HOME_PROBABLE;
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
        tracing::warn!(
            len = line_score.len(),
            "line score exceeds wire cap; truncating"
        );
        &line_score[..MAX_LINESCORE_LEN]
    } else {
        line_score
    }
}

fn write_mlb_final(out: &mut Vec<u8>, game: &MlbFinalGame) {
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

const SOCCER_FLAG_BREAK: u8 = 0x01;
const SOCCER_FLAG_EVENT: u8 = 0x02;
const SOCCER_FLAG_EVENT_RED: u8 = 0x04;
const SOCCER_FLAG_EVENT_AWAY: u8 = 0x08;
const SOCCER_FLAG_EVENT_HOME: u8 = 0x10;
const SOCCER_FLAG_COMMENTARY: u8 = 0x20;

fn soccer_event_flags(event: &Option<LastEvent>) -> u8 {
    match event {
        None => 0,
        Some(ev) => {
            let mut flags = SOCCER_FLAG_EVENT;
            if ev.kind == crate::soccer::EventKind::RedCard {
                flags |= SOCCER_FLAG_EVENT_RED;
            }
            match ev.team {
                Some(Side::Away) => flags |= SOCCER_FLAG_EVENT_AWAY,
                Some(Side::Home) => flags |= SOCCER_FLAG_EVENT_HOME,
                None => {}
            }
            flags
        }
    }
}

/// Encode one soccer game detail (see the "Soccer game detail" spec above).
pub fn encode_soccer_game(game: &SoccerGame) -> Vec<u8> {
    let mut out = Vec::with_capacity(128);
    out.push(WIRE_VERSION);
    match game {
        SoccerGame::Pregame(g) => {
            out.push(0);
            write_soccer_pregame(&mut out, g);
        }
        SoccerGame::Live(g) => {
            out.push(1);
            write_soccer_live(&mut out, g);
        }
        SoccerGame::Final(g) => {
            out.push(2);
            write_soccer_final(&mut out, g);
        }
    }
    out
}

fn write_soccer_pregame(out: &mut Vec<u8>, game: &SoccerPregameGame) {
    out.extend_from_slice(&game.start_time.to_le_bytes());
    out.extend_from_slice(&game.away.colors.primary.to_le_bytes());
    out.extend_from_slice(&game.away.colors.alternate.to_le_bytes());
    out.extend_from_slice(&game.home.colors.primary.to_le_bytes());
    out.extend_from_slice(&game.home.colors.alternate.to_le_bytes());
    push_str(out, &game.game_id);
    push_str(out, &game.away.abbreviation);
    push_str(out, &game.home.abbreviation);
    push_str(out, &game.venue);
}

fn soccer_flavor_code(flavor: SoccerFinalFlavor) -> u8 {
    match flavor {
        SoccerFinalFlavor::FullTime => 0,
        SoccerFinalFlavor::AfterExtraTime => 1,
        SoccerFinalFlavor::AfterPenalties => 2,
    }
}

fn write_soccer_live(out: &mut Vec<u8>, game: &SoccerLiveGame) {
    let mut flags = soccer_event_flags(&game.last_event);
    if game.on_break {
        flags |= SOCCER_FLAG_BREAK;
    }
    if game.commentary.is_some() {
        flags |= SOCCER_FLAG_COMMENTARY;
    }
    out.push(flags);
    out.push(game.half);
    out.extend_from_slice(&game.clock_seconds.to_le_bytes());
    out.extend_from_slice(&score_u16(game.away.score));
    out.extend_from_slice(&score_u16(game.home.score));
    out.extend_from_slice(&game.away.colors.primary.to_le_bytes());
    out.extend_from_slice(&game.away.colors.alternate.to_le_bytes());
    out.extend_from_slice(&game.home.colors.primary.to_le_bytes());
    out.extend_from_slice(&game.home.colors.alternate.to_le_bytes());
    push_str(out, &game.game_id);
    push_str(out, &game.away.abbreviation);
    push_str(out, &game.home.abbreviation);
    if let Some(ev) = &game.last_event {
        push_str(out, &ev.clock);
        push_str(out, &ev.athlete);
    }
    if let Some(c) = &game.commentary {
        push_str(out, &c.id);
        push_str(out, &c.text);
    }
}

fn write_soccer_final(out: &mut Vec<u8>, game: &SoccerFinalGame) {
    out.push(soccer_flavor_code(game.flavor));
    out.extend_from_slice(&score_u16(game.away.score));
    out.extend_from_slice(&score_u16(game.home.score));
    out.extend_from_slice(&game.away.colors.primary.to_le_bytes());
    out.extend_from_slice(&game.away.colors.alternate.to_le_bytes());
    out.extend_from_slice(&game.home.colors.primary.to_le_bytes());
    out.extend_from_slice(&game.home.colors.alternate.to_le_bytes());
    push_str(out, &game.game_id);
    push_str(out, &game.away.abbreviation);
    push_str(out, &game.home.abbreviation);
    push_str(out, &game.away.scorers);
    push_str(out, &game.home.scorers);
}

const NBA_FLAG_LAST_PLAY: u8 = 0x01;

const NBA_FLAG_AWAY_RECORD: u8 = 0x01;
const NBA_FLAG_HOME_RECORD: u8 = 0x02;

/// Encode one NBA game detail (see the "NBA game detail" spec above).
pub fn encode_nba_game(game: &NbaGame) -> Vec<u8> {
    let mut out = Vec::with_capacity(128);
    out.push(WIRE_VERSION);
    match game {
        NbaGame::Pregame(g) => {
            out.push(0);
            write_nba_pregame(&mut out, g);
        }
        NbaGame::Live(g) => {
            out.push(1);
            write_nba_live(&mut out, g);
        }
        NbaGame::Final(g) => {
            out.push(2);
            write_nba_final(&mut out, g);
        }
    }
    out
}

fn write_nba_pregame(out: &mut Vec<u8>, game: &NbaPregameGame) {
    let mut flags = 0u8;
    let (away_wins, away_losses) = match &game.away.record {
        Some(r) => {
            flags |= NBA_FLAG_AWAY_RECORD;
            (r.wins, r.losses)
        }
        None => (0, 0),
    };
    let (home_wins, home_losses) = match &game.home.record {
        Some(r) => {
            flags |= NBA_FLAG_HOME_RECORD;
            (r.wins, r.losses)
        }
        None => (0, 0),
    };

    out.push(flags);
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
}

fn write_nba_live(out: &mut Vec<u8>, game: &NbaLiveGame) {
    out.push(if game.last_play.is_some() {
        NBA_FLAG_LAST_PLAY
    } else {
        0
    });
    out.push(game.period);
    out.push(game.phase.code());
    out.extend_from_slice(&score_u16(game.away.score));
    out.extend_from_slice(&score_u16(game.home.score));
    out.extend_from_slice(&game.away.colors.primary.to_le_bytes());
    out.extend_from_slice(&game.away.colors.alternate.to_le_bytes());
    out.extend_from_slice(&game.home.colors.primary.to_le_bytes());
    out.extend_from_slice(&game.home.colors.alternate.to_le_bytes());

    push_str(out, &game.game_id);
    push_str(out, &game.away.abbreviation);
    push_str(out, &game.home.abbreviation);
    push_str(out, &game.clock);
    if let Some(play) = &game.last_play {
        push_str(out, &play.id);
        push_str(out, &play.text);
    }
}

fn write_nba_final(out: &mut Vec<u8>, game: &NbaFinalGame) {
    let away_ls = line_score_bytes(&game.away.line_score);
    let home_ls = line_score_bytes(&game.home.line_score);

    out.push(game.periods_played);
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

const FOOTBALL_FLAG_LAST_PLAY: u8 = 0x01;
const FOOTBALL_FLAG_SITUATION: u8 = 0x02;
const FOOTBALL_FLAG_POSSESSION_HOME: u8 = 0x04;
const FOOTBALL_FLAG_RED_ZONE: u8 = 0x08;
const FOOTBALL_FLAG_TIMEOUTS: u8 = 0x10;

const FOOTBALL_FLAG_AWAY_RECORD: u8 = 0x01;
const FOOTBALL_FLAG_HOME_RECORD: u8 = 0x02;
const FOOTBALL_FLAG_AWAY_RANK: u8 = 0x04;
const FOOTBALL_FLAG_HOME_RANK: u8 = 0x08;

/// Encode one football game detail (see the "Football game detail" spec above).
pub fn encode_football_game(game: &FootballGame) -> Vec<u8> {
    let mut out = Vec::with_capacity(128);
    out.push(WIRE_VERSION);
    match game {
        FootballGame::Pregame(g) => {
            out.push(0);
            write_football_pregame(&mut out, g);
        }
        FootballGame::Live(g) => {
            out.push(1);
            write_football_live(&mut out, g);
        }
        FootballGame::Final(g) => {
            out.push(2);
            write_football_final(&mut out, g);
        }
    }
    out
}

fn write_football_pregame(out: &mut Vec<u8>, game: &FootballPregameGame) {
    let mut flags = 0u8;
    let (away_wins, away_losses) = match &game.away.record {
        Some(r) => {
            flags |= FOOTBALL_FLAG_AWAY_RECORD;
            (r.wins, r.losses)
        }
        None => (0, 0),
    };
    let (home_wins, home_losses) = match &game.home.record {
        Some(r) => {
            flags |= FOOTBALL_FLAG_HOME_RECORD;
            (r.wins, r.losses)
        }
        None => (0, 0),
    };
    if game.away.rank_line.is_some() {
        flags |= FOOTBALL_FLAG_AWAY_RANK;
    }
    if game.home.rank_line.is_some() {
        flags |= FOOTBALL_FLAG_HOME_RANK;
    }

    out.push(flags);
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
    if let Some(rank) = &game.away.rank_line {
        push_str(out, rank);
    }
    if let Some(rank) = &game.home.rank_line {
        push_str(out, rank);
    }
}

fn write_football_live(out: &mut Vec<u8>, game: &FootballLiveGame) {
    let mut flags = 0u8;
    if game.last_play.is_some() {
        flags |= FOOTBALL_FLAG_LAST_PLAY;
    }
    let (down, distance, yard_line) = match &game.situation {
        Some(s) => {
            flags |= FOOTBALL_FLAG_SITUATION;
            if s.possession == Side::Home {
                flags |= FOOTBALL_FLAG_POSSESSION_HOME;
            }
            if s.red_zone {
                flags |= FOOTBALL_FLAG_RED_ZONE;
            }
            (s.down, s.distance, s.yard_line)
        }
        None => (0, 0, 0),
    };
    let (away_timeouts, home_timeouts) = match &game.timeouts {
        Some(t) => {
            flags |= FOOTBALL_FLAG_TIMEOUTS;
            (t.away, t.home)
        }
        None => (0, 0),
    };

    out.push(flags);
    out.push(game.period);
    out.push(game.phase.code());
    out.push(down);
    out.push(distance);
    out.push(yard_line);
    out.push(away_timeouts);
    out.push(home_timeouts);
    out.extend_from_slice(&score_u16(game.away.score));
    out.extend_from_slice(&score_u16(game.home.score));
    out.extend_from_slice(&game.away.colors.primary.to_le_bytes());
    out.extend_from_slice(&game.away.colors.alternate.to_le_bytes());
    out.extend_from_slice(&game.home.colors.primary.to_le_bytes());
    out.extend_from_slice(&game.home.colors.alternate.to_le_bytes());

    push_str(out, &game.game_id);
    push_str(out, &game.away.abbreviation);
    push_str(out, &game.home.abbreviation);
    push_str(out, &game.clock);
    if let Some(play) = &game.last_play {
        push_str(out, &play.id);
        push_str(out, &play.text);
    }
}

fn write_football_final(out: &mut Vec<u8>, game: &FootballFinalGame) {
    let away_ls = line_score_bytes(&game.away.line_score);
    let home_ls = line_score_bytes(&game.home.line_score);

    out.push(game.periods_played);
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
        tracing::warn!(
            count = entries.len(),
            "game list exceeds wire cap; truncating to 255"
        );
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
        MlbAtBat, MlbBases, MlbCount, MlbFinalTeam, MlbInning, MlbPregameTeam, MlbWeather,
    };
    use crate::shared::game::{GameState, LastPlay, LivePhase, Record};
    use crate::shared::team::{TeamColors, TeamState};

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

    fn full_live_fixture() -> MlbLiveGame {
        MlbLiveGame {
            game_id: "401570729".to_string(),
            inning: MlbInning {
                number: 7,
                half: InningHalf::Bottom,
            },
            home: team("BOS", 5, 0xBD3039, 0x0C2340),
            away: team("SEA", 3, 0x0C2C56, 0x005C5C),
            count: MlbCount {
                balls: 3,
                strikes: 2,
                outs: 2,
            },
            bases: MlbBases {
                first: true,
                second: false,
                third: true,
            },
            at_bat: Some(MlbAtBat {
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
        let encoded = encode_mlb_game(&MlbGame::Live(full_live_fixture()));
        assert_eq!(hex::encode(&encoded), expected);
    }

    fn pregame_fixture() -> MlbPregameGame {
        MlbPregameGame {
            game_id: "401570001".to_string(),
            start_time: 1_783_647_600,
            venue: "Petco Park".to_string(),
            weather: Some(MlbWeather {
                condition: "Mostly sunny".to_string(),
                temperature: 72,
            }),
            away: MlbPregameTeam {
                abbreviation: "NYY".to_string(),
                colors: TeamColors {
                    primary: 0x003087,
                    alternate: 0xE4002C,
                },
                record: Some(Record {
                    wins: 44,
                    losses: 46,
                }),
                probable_pitcher: Some("G. Marquez".to_string()),
            },
            home: MlbPregameTeam {
                abbreviation: "SD".to_string(),
                colors: TeamColors {
                    primary: 0x2F241D,
                    alternate: 0xFFC425,
                },
                record: Some(Record {
                    wins: 50,
                    losses: 40,
                }),
                probable_pitcher: Some("Y. Darvish".to_string()),
            },
        }
    }

    const GOLDEN_PRE: &str = "02001f482c002e0032002800704d506a873000002c00e4001d242f0025c4ff0009343031353730303031034e59590253440a506574636f205061726b0c4d6f73746c792073756e6e790a472e204d61727175657a0a592e2044617276697368";

    #[test]
    fn golden_pregame_all_flags() {
        assert_eq!(
            hex::encode(encode_mlb_game(&MlbGame::Pregame(pregame_fixture()))),
            GOLDEN_PRE
        );
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
            hex::encode(encode_mlb_game(&MlbGame::Pregame(game))),
            GOLDEN_PRE_NO_WEATHER
        );
    }

    #[test]
    fn pregame_temperature_clamps_to_u8() {
        let mut game = pregame_fixture();
        game.weather = Some(MlbWeather {
            condition: "Scorching".to_string(),
            temperature: 300,
        });
        let bytes = encode_mlb_game(&MlbGame::Pregame(game));
        // temperature sits at offset 3 (after version, state, flags).
        assert_eq!(bytes[3], 255);
    }

    #[test]
    fn pregame_negative_temperature_clamps_to_zero() {
        let mut game = pregame_fixture();
        game.weather = Some(MlbWeather {
            condition: "Frigid".to_string(),
            temperature: -40,
        });
        let bytes = encode_mlb_game(&MlbGame::Pregame(game));
        assert_eq!(bytes[3], 0);
    }

    fn final_fixture() -> MlbFinalGame {
        MlbFinalGame {
            game_id: "401570729".to_string(),
            innings_played: 9,
            away: MlbFinalTeam {
                abbreviation: "SEA".to_string(),
                score: 4,
                colors: TeamColors {
                    primary: 0x0C2C56,
                    alternate: 0x005C5C,
                },
                // 9 innings.
                line_score: vec![1, 0, 0, 2, 0, 0, 1, 0, 0],
            },
            home: MlbFinalTeam {
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
            hex::encode(encode_mlb_game(&MlbGame::Final(final_fixture()))),
            GOLDEN_FINAL
        );
    }

    #[test]
    fn final_score_saturates_to_u16() {
        let mut game = final_fixture();
        game.home.score = 1_000_000;
        let bytes = encode_mlb_game(&MlbGame::Final(game));
        // home_score at offset 7..9 (version, state, innings, nA, nH, away u16).
        assert_eq!(u16::from_le_bytes([bytes[7], bytes[8]]), u16::MAX);
    }

    #[test]
    fn live_score_saturates_to_u16() {
        let mut game = full_live_fixture();
        game.home.score = 1_000_000;
        let bytes = encode_mlb_game(&MlbGame::Live(game));
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

    // --- Soccer goldens (shared verbatim with tools/wire_format_check.py) ---

    use crate::soccer::{EventKind, LastEvent, SoccerFinalTeam, SoccerGame, SoccerPregameTeam};

    fn soccer_side(abbrev: &str, score: u32, primary: u32, alternate: u32) -> TeamState {
        TeamState {
            abbreviation: abbrev.to_string(),
            score,
            colors: TeamColors { primary, alternate },
        }
    }

    const GOLDEN_SOCCER_LIVE: &str = "02010a01f40b020001001306e30025dafd0068280000300abf00093430313830303130300342454c03555341063435272b312709522e204c756b616b75";

    #[test]
    fn golden_soccer_live_with_away_goal() {
        let game = SoccerGame::Live(SoccerLiveGame {
            game_id: "401800100".to_string(),
            clock: "45'+1'".to_string(),
            clock_seconds: 51 * 60,
            half: 1,
            on_break: false,
            away: soccer_side("BEL", 2, 0xE30613, 0xFDDA25),
            home: soccer_side("USA", 1, 0x002868, 0xBF0A30),
            last_event: Some(LastEvent {
                text: "Goal - R. Lukaku".to_string(),
                kind: EventKind::Goal,
                athlete: "R. Lukaku".to_string(),
                clock: "45'+1'".to_string(),
                team: Some(Side::Away),
            }),
            commentary: None,
        });
        assert_eq!(hex::encode(encode_soccer_game(&game)), GOLDEN_SOCCER_LIVE);
    }

    const GOLDEN_SOCCER_LIVE_COMMENTARY: &str = "02012a01f40b020001001306e30025dafd0068280000300abf00093430313830303130300342454c03555341063435272b312709522e204c756b616b7502383753476f616c21202042656c6769756d20322c2055534120312e20526f6d656c75204c756b616b7520726967687420666f6f7465642073686f7420746f2074686520626f74746f6d206c65667420636f726e65722e";

    #[test]
    fn golden_soccer_live_with_commentary() {
        let game = SoccerGame::Live(SoccerLiveGame {
            game_id: "401800100".to_string(),
            clock: "45'+1'".to_string(),
            clock_seconds: 51 * 60,
            half: 1,
            on_break: false,
            away: soccer_side("BEL", 2, 0xE30613, 0xFDDA25),
            home: soccer_side("USA", 1, 0x002868, 0xBF0A30),
            last_event: Some(LastEvent {
                text: "Goal - R. Lukaku".to_string(),
                kind: EventKind::Goal,
                athlete: "R. Lukaku".to_string(),
                clock: "45'+1'".to_string(),
                team: Some(Side::Away),
            }),
            commentary: Some(crate::soccer::Commentary {
                id: "87".to_string(),
                text: "Goal!  Belgium 2, USA 1. Romelu Lukaku right footed shot to the bottom left corner.".to_string(),
            }),
        });
        assert_eq!(
            hex::encode(encode_soccer_game(&game)),
            GOLDEN_SOCCER_LIVE_COMMENTARY
        );
    }

    const GOLDEN_SOCCER_HALFTIME: &str = "02011301f40b020002001306e30025dafd0068280000300abf00093430313830303130300342454c03555341063435272b31270a432e2050756c69736963";

    #[test]
    fn golden_soccer_halftime_with_home_goal() {
        let game = SoccerGame::Live(SoccerLiveGame {
            game_id: "401800100".to_string(),
            clock: "45'+6'".to_string(),
            clock_seconds: 51 * 60,
            half: 1,
            on_break: true,
            away: soccer_side("BEL", 2, 0xE30613, 0xFDDA25),
            home: soccer_side("USA", 2, 0x002868, 0xBF0A30),
            last_event: Some(LastEvent {
                text: "Goal - C. Pulisic".to_string(),
                kind: EventKind::Goal,
                athlete: "C. Pulisic".to_string(),
                clock: "45'+1'".to_string(),
                team: Some(Side::Home),
            }),
            commentary: None,
        });
        assert_eq!(
            hex::encode(encode_soccer_game(&game)),
            GOLDEN_SOCCER_HALFTIME
        );
    }

    const GOLDEN_SOCCER_QUIET: &str =
        "020100026414000000001248000027e8ea0041975d00955500000934303138303031303103504f5203534541";

    #[test]
    fn golden_soccer_live_no_event() {
        let game = SoccerGame::Live(SoccerLiveGame {
            game_id: "401800101".to_string(),
            clock: "87'".to_string(),
            clock_seconds: 87 * 60,
            half: 2,
            on_break: false,
            away: soccer_side("POR", 0, 0x004812, 0xEAE827),
            home: soccer_side("SEA", 0, 0x5D9741, 0x005595),
            last_event: None,
            commentary: None,
        });
        assert_eq!(hex::encode(encode_soccer_game(&game)), GOLDEN_SOCCER_QUIET);
    }

    const GOLDEN_SOCCER_PRE: &str = "0200704d506a1248000027e8ea0041975d00955500000934303138303031303203504f52035345410b4c756d656e204669656c64";

    #[test]
    fn golden_soccer_pregame() {
        let game = SoccerGame::Pregame(SoccerPregameGame {
            game_id: "401800102".to_string(),
            start_time: 1_783_647_600,
            venue: "Lumen Field".to_string(),
            away: SoccerPregameTeam {
                abbreviation: "POR".to_string(),
                colors: TeamColors {
                    primary: 0x004812,
                    alternate: 0xEAE827,
                },
            },
            home: SoccerPregameTeam {
                abbreviation: "SEA".to_string(),
                colors: TeamColors {
                    primary: 0x5D9741,
                    alternate: 0x005595,
                },
            },
        });
        assert_eq!(hex::encode(encode_soccer_game(&game)), GOLDEN_SOCCER_PRE);
    }

    const GOLDEN_SOCCER_FINAL: &str = "020200010000000000ff0000c4ff001248000027e8ea00093430313830303130330345535003504f52104d2e204d6572696e6f203930272b312700";

    fn soccer_final_fixture(flavor: SoccerFinalFlavor) -> SoccerGame {
        SoccerGame::Final(SoccerFinalGame {
            game_id: "401800103".to_string(),
            flavor,
            away: SoccerFinalTeam {
                abbreviation: "ESP".to_string(),
                score: 1,
                colors: TeamColors {
                    primary: 0xFF0000,
                    alternate: 0xFFC400,
                },
                scorers: "M. Merino 90'+1'".to_string(),
            },
            home: SoccerFinalTeam {
                abbreviation: "POR".to_string(),
                score: 0,
                colors: TeamColors {
                    primary: 0x004812,
                    alternate: 0xEAE827,
                },
                scorers: String::new(),
            },
        })
    }

    #[test]
    fn golden_soccer_final_with_scorers() {
        // Full-time flavor → leading flavor byte 0.
        let game = soccer_final_fixture(SoccerFinalFlavor::FullTime);
        assert_eq!(hex::encode(encode_soccer_game(&game)), GOLDEN_SOCCER_FINAL);
    }

    #[test]
    fn soccer_final_flavor_byte_encodes_variant() {
        // The flavor byte sits at offset 2 (after version, state); everything
        // after it is identical to the full-time golden.
        let aet = encode_soccer_game(&soccer_final_fixture(SoccerFinalFlavor::AfterExtraTime));
        let pens = encode_soccer_game(&soccer_final_fixture(SoccerFinalFlavor::AfterPenalties));
        assert_eq!(aet[2], 1);
        assert_eq!(pens[2], 2);
        // Only byte 2 differs from the full-time encoding.
        let full = encode_soccer_game(&soccer_final_fixture(SoccerFinalFlavor::FullTime));
        assert_eq!(&aet[3..], &full[3..]);
        assert_eq!(&pens[3..], &full[3..]);
    }

    // --- NBA goldens (shared verbatim with tools/wire_format_check.py) ---

    use crate::nba::{NbaFinalTeam, NbaPregameTeam};
    use crate::shared::game::Record as NbaRecord;

    fn nba_side(abbrev: &str, score: u32, primary: u32, alternate: u32) -> TeamState {
        TeamState {
            abbreviation: abbrev.to_string(),
            score,
            colors: TeamColors { primary, alternate },
        }
    }

    const GOLDEN_NBA_LIVE: &str = "02010103004b004d00c17a0000243bef0040220e0024c5fe0009343031383131303337034f4b430344454e04343a33370c3430313831313033373431312a5a656b65204e6e616a69206f7574206f6620626f756e6473206261642070617373207475726e6f766572";

    #[test]
    fn golden_nba_live_with_last_play() {
        let game = NbaGame::Live(NbaLiveGame {
            game_id: "401811037".to_string(),
            period: 3,
            clock: "4:37".to_string(),
            phase: LivePhase::InProgress,
            away: nba_side("OKC", 75, 0x007AC1, 0xEF3B24),
            home: nba_side("DEN", 77, 0x0E2240, 0xFEC524),
            last_play: Some(LastPlay {
                id: "401811037411".to_string(),
                text: "Zeke Nnaji out of bounds bad pass turnover".to_string(),
            }),
        });
        assert_eq!(hex::encode(encode_nba_game(&game)), GOLDEN_NBA_LIVE);
    }

    const GOLDEN_NBA_HALFTIME: &str = "020100020134004a00a9765d0012b1f5008e004e001ba0f90009343031383131303336034d454d045554414803302e30";

    #[test]
    fn golden_nba_halftime_no_last_play() {
        // Break state: flags 0 (no last play), phase 1, clock reads "0.0".
        let game = NbaGame::Live(NbaLiveGame {
            game_id: "401811036".to_string(),
            period: 2,
            clock: "0.0".to_string(),
            phase: LivePhase::Halftime,
            away: nba_side("MEM", 52, 0x5D76A9, 0xF5B112),
            home: nba_side("UTAH", 74, 0x4E008E, 0xF9A01B),
            last_play: None,
        });
        assert_eq!(hex::encode(encode_nba_game(&game)), GOLDEN_NBA_HALFTIME);
    }

    fn nba_pregame_fixture() -> NbaPregameGame {
        NbaPregameGame {
            game_id: "401811040".to_string(),
            start_time: 1_775_874_600,
            venue: "crypto.com Arena".to_string(),
            away: NbaPregameTeam {
                abbreviation: "PHX".to_string(),
                colors: TeamColors {
                    primary: 0x29127A,
                    alternate: 0xE56020,
                },
                record: Some(NbaRecord {
                    wins: 40,
                    losses: 42,
                }),
            },
            home: NbaPregameTeam {
                abbreviation: "LAL".to_string(),
                colors: TeamColors {
                    primary: 0x552583,
                    alternate: 0xFDB927,
                },
                record: Some(NbaRecord {
                    wins: 50,
                    losses: 32,
                }),
            },
        }
    }

    const GOLDEN_NBA_PRE: &str = "02000328002a003200200028b2d9697a1229002060e5008325550027b9fd000934303138313130343003504858034c414c1063727970746f2e636f6d204172656e61";

    #[test]
    fn golden_nba_pregame_all_flags() {
        assert_eq!(
            hex::encode(encode_nba_game(&NbaGame::Pregame(nba_pregame_fixture()))),
            GOLDEN_NBA_PRE
        );
    }

    const GOLDEN_NBA_PRE_NO_RECORDS: &str = "020000000000000000000028b2d9697a1229002060e5008325550027b9fd000934303138313130343003504858034c414c1063727970746f2e636f6d204172656e61";

    #[test]
    fn golden_nba_pregame_no_records() {
        let mut game = nba_pregame_fixture();
        game.away.record = None;
        game.home.record = None;
        assert_eq!(
            hex::encode(encode_nba_game(&NbaGame::Pregame(game))),
            GOLDEN_NBA_PRE_NO_RECORDS
        );
    }

    fn nba_final_fixture() -> NbaFinalGame {
        NbaFinalGame {
            game_id: "401811026".to_string(),
            periods_played: 4,
            away: NbaFinalTeam {
                abbreviation: "DET".to_string(),
                score: 118,
                colors: TeamColors {
                    primary: 0x1D428A,
                    alternate: 0xC8102E,
                },
                line_score: vec![30, 28, 30, 30],
            },
            home: NbaFinalTeam {
                abbreviation: "CHA".to_string(),
                score: 100,
                colors: TeamColors {
                    primary: 0x008CA8,
                    alternate: 0x1D1160,
                },
                line_score: vec![25, 25, 25, 25],
            },
        }
    }

    const GOLDEN_NBA_FINAL: &str = "0202040404760064008a421d002e10c800a88c000060111d001e1c1e1e19191919093430313831313032360344455403434841";

    #[test]
    fn golden_nba_final_regulation() {
        assert_eq!(
            hex::encode(encode_nba_game(&NbaGame::Final(nba_final_fixture()))),
            GOLDEN_NBA_FINAL
        );
    }

    #[test]
    fn nba_final_overtime_extends_line_scores() {
        let mut game = nba_final_fixture();
        game.periods_played = 5;
        game.away.line_score.push(12);
        game.home.line_score.push(10);
        let bytes = encode_nba_game(&NbaGame::Final(game));
        // periods_played, nA, nH sit at offsets 2..5 (after version, state).
        assert_eq!(&bytes[2..5], &[5, 5, 5]);
    }

    // --- Football goldens (shared verbatim with tools/wire_format_check.py) ---

    use crate::football::{
        FootballFinalTeam, FootballGame, FootballLiveGame, FootballPregameGame,
        FootballPregameTeam, FootballSituation, Timeouts,
    };
    use crate::shared::game::{LivePhase as FbPhase, Record as FbRecord, Side as FbSide};

    fn fb_side(abbrev: &str, score: u32, primary: u32, alternate: u32) -> TeamState {
        TeamState {
            abbreviation: abbrev.to_string(),
            score,
            colors: TeamColors { primary, alternate },
        }
    }

    fn football_live_fixture() -> FootballLiveGame {
        FootballLiveGame {
            game_id: "401772510".to_string(),
            period: 3,
            clock: "8:24".to_string(),
            phase: FbPhase::InProgress,
            away: fb_side("BUF", 14, 0x00338D, 0xC60C30),
            home: fb_side("KC", 17, 0xE31837, 0xFFB81C),
            situation: Some(FootballSituation {
                down: 2,
                distance: 7,
                yard_line: 45,
                possession: FbSide::Home,
                red_zone: false,
            }),
            timeouts: Some(Timeouts { away: 2, home: 3 }),
            last_play: Some(LastPlay {
                id: "401772510105".to_string(),
                text: "P. Mahomes pass complete to T. Kelce for 8 yards".to_string(),
            }),
        }
    }

    const GOLDEN_FOOTBALL_LIVE: &str = "020117030002072d02030e0011008d330000300cc6003718e3001cb8ff000934303137373235313003425546024b4304383a32340c34303137373235313031303530502e204d61686f6d6573207061737320636f6d706c65746520746f20542e204b656c636520666f722038207961726473";

    #[test]
    fn golden_football_live_with_situation() {
        let game = FootballGame::Live(football_live_fixture());
        assert_eq!(
            hex::encode(encode_football_game(&game)),
            GOLDEN_FOOTBALL_LIVE
        );
    }

    const GOLDEN_FOOTBALL_LIVE_BREAK: &str = "020100020100000000000a000e008d330000300cc6003718e3001cb8ff000934303137373235313103425546024b4304303a3030";

    #[test]
    fn golden_football_live_break_no_situation() {
        // Halftime: flags 0 (no last play, no situation, no timeouts), phase 1,
        // down/distance/yard_line/timeouts all zero.
        let game = FootballGame::Live(FootballLiveGame {
            game_id: "401772511".to_string(),
            period: 2,
            clock: "0:00".to_string(),
            phase: FbPhase::Halftime,
            away: fb_side("BUF", 10, 0x00338D, 0xC60C30),
            home: fb_side("KC", 14, 0xE31837, 0xFFB81C),
            situation: None,
            timeouts: None,
            last_play: None,
        });
        assert_eq!(
            hex::encode(encode_football_game(&game)),
            GOLDEN_FOOTBALL_LIVE_BREAK
        );
    }

    fn football_pregame_fixture() -> FootballPregameGame {
        FootballPregameGame {
            game_id: "401772512".to_string(),
            start_time: 1_783_647_600,
            venue: "Arrowhead Stadium".to_string(),
            away: FootballPregameTeam {
                abbreviation: "BUF".to_string(),
                colors: TeamColors {
                    primary: 0x00338D,
                    alternate: 0xC60C30,
                },
                record: Some(FbRecord {
                    wins: 11,
                    losses: 3,
                }),
                rank_line: None,
            },
            home: FootballPregameTeam {
                abbreviation: "KC".to_string(),
                colors: TeamColors {
                    primary: 0xE31837,
                    alternate: 0xFFB81C,
                },
                record: Some(FbRecord {
                    wins: 13,
                    losses: 1,
                }),
                rank_line: None,
            },
        }
    }

    const GOLDEN_FOOTBALL_PRE: &str = "0200030b0003000d000100704d506a8d330000300cc6003718e3001cb8ff000934303137373235313203425546024b43114172726f7768656164205374616469756d";

    #[test]
    fn golden_football_pregame_nfl_records_no_ranks() {
        assert_eq!(
            hex::encode(encode_football_game(&FootballGame::Pregame(
                football_pregame_fixture()
            ))),
            GOLDEN_FOOTBALL_PRE
        );
    }

    const GOLDEN_FOOTBALL_PRE_RANKED: &str = "02000b0b0003000d000100704d506a8d330000300cc6003718e3001cb8ff0009343031373732353133044d494348034f53550c4f68696f205374616469756d0d2333204f48494f205354415445";

    #[test]
    fn golden_football_pregame_ncaaf_home_ranked() {
        // College: home #3 carries a rank line (rides the pitcher slot), away
        // unranked → absent. Records still travel numerically.
        let mut game = football_pregame_fixture();
        game.game_id = "401772513".to_string();
        game.away.abbreviation = "MICH".to_string();
        game.home.abbreviation = "OSU".to_string();
        game.venue = "Ohio Stadium".to_string();
        game.home.rank_line = Some("#3 OHIO STATE".to_string());
        assert_eq!(
            hex::encode(encode_football_game(&FootballGame::Pregame(game))),
            GOLDEN_FOOTBALL_PRE_RANKED
        );
    }

    fn football_final_fixture() -> FootballGame {
        FootballGame::Final(FootballFinalGame {
            game_id: "401772514".to_string(),
            periods_played: 5,
            away: FootballFinalTeam {
                abbreviation: "BUF".to_string(),
                score: 24,
                colors: TeamColors {
                    primary: 0x00338D,
                    alternate: 0xC60C30,
                },
                line_score: vec![7, 3, 7, 7, 0],
            },
            home: FootballFinalTeam {
                abbreviation: "KC".to_string(),
                score: 27,
                colors: TeamColors {
                    primary: 0xE31837,
                    alternate: 0xFFB81C,
                },
                line_score: vec![7, 7, 0, 10, 3],
            },
        })
    }

    const GOLDEN_FOOTBALL_FINAL_OT: &str = "020205050518001b008d330000300cc6003718e3001cb8ff0007030707000707000a030934303137373235313403425546024b43";

    #[test]
    fn golden_football_final_overtime() {
        assert_eq!(
            hex::encode(encode_football_game(&football_final_fixture())),
            GOLDEN_FOOTBALL_FINAL_OT
        );
    }

    const GOLDEN_FOOTBALL_FINAL: &str = "020204040418001b003718e3001cb8ff008d330000300cc60007030707000a070a09343031353437343137024b4303425546";

    #[test]
    fn golden_football_final_regulation() {
        let game = FootballGame::Final(FootballFinalGame {
            game_id: "401547417".to_string(),
            periods_played: 4,
            away: FootballFinalTeam {
                abbreviation: "KC".to_string(),
                score: 24,
                colors: TeamColors {
                    primary: 0xE31837,
                    alternate: 0xFFB81C,
                },
                line_score: vec![7, 3, 7, 7],
            },
            home: FootballFinalTeam {
                abbreviation: "BUF".to_string(),
                score: 27,
                colors: TeamColors {
                    primary: 0x00338D,
                    alternate: 0xC60C30,
                },
                line_score: vec![0, 10, 7, 10],
            },
        });
        assert_eq!(
            hex::encode(encode_football_game(&game)),
            GOLDEN_FOOTBALL_FINAL
        );
    }

    #[test]
    fn football_live_flags_encode_situation_possession_and_redzone() {
        // Away possession in the red zone with timeouts, no last play: bit1
        // (situation) + bit3 (red zone) + bit4 (timeouts) set, bit0 and bit2
        // (last play, possession-home) clear.
        let mut live = football_live_fixture();
        live.last_play = None;
        live.situation = Some(FootballSituation {
            down: 1,
            distance: 8,
            yard_line: 92,
            possession: FbSide::Away,
            red_zone: true,
        });
        let bytes = encode_football_game(&FootballGame::Live(live));
        // flags at offset 2.
        assert_eq!(
            bytes[2],
            FOOTBALL_FLAG_SITUATION | FOOTBALL_FLAG_RED_ZONE | FOOTBALL_FLAG_TIMEOUTS
        );
    }

    #[test]
    fn football_live_score_saturates_to_u16() {
        let mut live = football_live_fixture();
        live.home.score = 1_000_000;
        let bytes = encode_football_game(&FootballGame::Live(live));
        // home_score at offset 12..14 (see live spec table).
        assert_eq!(u16::from_le_bytes([bytes[12], bytes[13]]), u16::MAX);
    }

    #[test]
    fn football_final_overtime_extends_line_scores() {
        let bytes = encode_football_game(&football_final_fixture());
        // periods_played, nA, nH sit at offsets 2..5 (after version, state).
        assert_eq!(&bytes[2..5], &[5, 5, 5]);
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
