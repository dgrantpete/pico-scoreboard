//! NBA game detail (`GET /basketball/nba/games/{game_id}`).
//!
//! # Live (state = 1)
//!
//! Fixed 23-byte section at offset 2 (`<BBBHHIIII`):
//!
//! | off | type | field                                            |
//! |-----|------|--------------------------------------------------|
//! | 2   | u8   | flags (bit0 = last play present)                 |
//! | 3   | u8   | period (quarter 1–4; overtime = 5+)              |
//! | 4   | u8   | phase (0=in progress, 1=halftime, 2=end of period) |
//! | 5   | u16  | away_score                                       |
//! | 7   | u16  | home_score                                       |
//! | 9   | u32  | away_colors.primary (0x00RRGGBB)                 |
//! | 13  | u32  | away_colors.alternate                            |
//! | 17  | u32  | home_colors.primary                              |
//! | 21  | u32  | home_colors.alternate                            |
//!
//! Strings from offset 25: `game_id`, `away.abbreviation`,
//! `home.abbreviation`, `clock` (display-shaped: "10:08"; "53.0" under a
//! minute; meaningless during breaks — render by `phase`; never extrapolated,
//! the firmware re-renders it each poll), then **iff** bit0: `last_play.id`
//! (change-detection key for the flash) and `last_play.text`.
//!
//! # Pregame (state = 0)
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
//! # Final (state = 2)
//!
//! The shared final layout (see [`crate::common`]) with per-quarter points in
//! the line scores; overtime extends both past 4.

use crate::codec::{Sink, SinkExt, color_pairs, le_u16, le_u32};
use crate::common::{
    FinalParts, payload_reader, read_final, read_header, write_final, write_header,
};
use crate::error::{BufferFull, DecodeError, DecodeErrorKind};
use crate::{FinalTeam, GameState, HEADER_LEN, LastPlay, LivePhase, Record, TeamColors, TeamState};

const LIVE_LEN: usize = 23;
const PRE_LEN: usize = 29;

const FLAG_LAST_PLAY: u8 = 0x01;

const PRE_FLAG_AWAY_RECORD: u8 = 0x01;
const PRE_FLAG_HOME_RECORD: u8 = 0x02;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Game<'a> {
    Pregame(Pregame<'a>),
    Live(Live<'a>),
    Final(Final<'a>),
}

impl Game<'_> {
    pub fn state(&self) -> GameState {
        match self {
            Game::Pregame(_) => GameState::Pregame,
            Game::Live(_) => GameState::Live,
            Game::Final(_) => GameState::Final,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Live<'a> {
    pub game_id: &'a str,
    /// Quarter 1–4; overtime periods pass through as 5+.
    pub period: u8,
    pub phase: LivePhase,
    /// Display-shaped and exact at poll time — never extrapolated.
    pub clock: &'a str,
    pub away: TeamState<'a>,
    pub home: TeamState<'a>,
    /// Absent before the opening tip.
    pub last_play: Option<LastPlay<'a>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pregame<'a> {
    pub game_id: &'a str,
    /// Unix epoch seconds, UTC.
    pub start_time: u32,
    pub venue: &'a str,
    pub away: PregameTeam<'a>,
    pub home: PregameTeam<'a>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PregameTeam<'a> {
    pub abbreviation: &'a str,
    pub colors: TeamColors,
    pub record: Option<Record>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Final<'a> {
    pub game_id: &'a str,
    /// 4, or more with overtime.
    pub periods_played: u8,
    pub away: FinalTeam<'a>,
    pub home: FinalTeam<'a>,
}

pub fn encode<S: Sink + ?Sized>(game: &Game<'_>, out: &mut S) -> Result<(), BufferFull> {
    write_header(out, game.state())?;
    match game {
        Game::Pregame(pregame) => write_pregame(out, pregame),
        Game::Live(live) => write_live(out, live),
        Game::Final(game) => write_final(
            out,
            game.game_id,
            game.periods_played,
            &game.away,
            &game.home,
        ),
    }
}

pub fn decode(buf: &[u8]) -> Result<Game<'_>, DecodeError> {
    match read_header(buf)? {
        GameState::Pregame => read_pregame(buf).map(Game::Pregame),
        GameState::Live => read_live(buf).map(Game::Live),
        GameState::Final => read_final(buf).map(|parts| {
            let FinalParts {
                game_id,
                periods,
                away,
                home,
            } = parts;
            Game::Final(Final {
                game_id,
                periods_played: periods,
                away,
                home,
            })
        }),
    }
}

fn write_live<S: Sink + ?Sized>(out: &mut S, game: &Live<'_>) -> Result<(), BufferFull> {
    out.u8(if game.last_play.is_some() {
        FLAG_LAST_PLAY
    } else {
        0
    })?;
    out.u8(game.period)?;
    out.u8(game.phase.code())?;
    out.u16(game.away.score)?;
    out.u16(game.home.score)?;
    out.color_pairs(game.away.colors, game.home.colors)?;

    out.string(game.game_id)?;
    out.string(game.away.abbreviation)?;
    out.string(game.home.abbreviation)?;
    out.string(game.clock)?;
    if let Some(play) = &game.last_play {
        out.string(play.id)?;
        out.string(play.text)?;
    }
    Ok(())
}

fn read_live(buf: &[u8]) -> Result<Live<'_>, DecodeError> {
    let mut reader = payload_reader(buf);
    let fixed = reader.fixed::<LIVE_LEN>()?;
    let flags = fixed[0];
    let phase = LivePhase::from_code(fixed[2]).ok_or_else(|| {
        DecodeError::at(
            HEADER_LEN + 2,
            DecodeErrorKind::InvalidCode {
                field: "live phase",
                code: fixed[2],
            },
        )
    })?;
    let (away_colors, home_colors) = color_pairs(fixed, 7);

    let game_id = reader.string("game_id")?;
    let away_abbreviation = reader.string("away abbreviation")?;
    let home_abbreviation = reader.string("home abbreviation")?;
    let clock = reader.string("clock")?;
    let last_play = if flags & FLAG_LAST_PLAY != 0 {
        Some(LastPlay {
            id: reader.string("last play id")?,
            text: reader.string("last play text")?,
        })
    } else {
        None
    };
    reader.finish()?;

    Ok(Live {
        game_id,
        period: fixed[1],
        phase,
        clock,
        away: TeamState {
            abbreviation: away_abbreviation,
            score: le_u16(fixed, 3),
            colors: away_colors,
        },
        home: TeamState {
            abbreviation: home_abbreviation,
            score: le_u16(fixed, 5),
            colors: home_colors,
        },
        last_play,
    })
}

fn write_pregame<S: Sink + ?Sized>(out: &mut S, game: &Pregame<'_>) -> Result<(), BufferFull> {
    let mut flags = 0u8;
    if game.away.record.is_some() {
        flags |= PRE_FLAG_AWAY_RECORD;
    }
    if game.home.record.is_some() {
        flags |= PRE_FLAG_HOME_RECORD;
    }
    let away_record = game.away.record.unwrap_or(Record { wins: 0, losses: 0 });
    let home_record = game.home.record.unwrap_or(Record { wins: 0, losses: 0 });

    out.u8(flags)?;
    out.u16(away_record.wins)?;
    out.u16(away_record.losses)?;
    out.u16(home_record.wins)?;
    out.u16(home_record.losses)?;
    out.u32(game.start_time)?;
    out.color_pairs(game.away.colors, game.home.colors)?;

    out.string(game.game_id)?;
    out.string(game.away.abbreviation)?;
    out.string(game.home.abbreviation)?;
    out.string(game.venue)
}

fn read_pregame(buf: &[u8]) -> Result<Pregame<'_>, DecodeError> {
    let mut reader = payload_reader(buf);
    let fixed = reader.fixed::<PRE_LEN>()?;
    let flags = fixed[0];
    let (away_colors, home_colors) = color_pairs(fixed, 13);

    let game_id = reader.string("game_id")?;
    let away_abbreviation = reader.string("away abbreviation")?;
    let home_abbreviation = reader.string("home abbreviation")?;
    let venue = reader.string("venue")?;
    reader.finish()?;

    Ok(Pregame {
        game_id,
        start_time: le_u32(fixed, 9),
        venue,
        away: PregameTeam {
            abbreviation: away_abbreviation,
            colors: away_colors,
            record: (flags & PRE_FLAG_AWAY_RECORD != 0).then(|| Record {
                wins: le_u16(fixed, 1),
                losses: le_u16(fixed, 3),
            }),
        },
        home: PregameTeam {
            abbreviation: home_abbreviation,
            colors: home_colors,
            record: (flags & PRE_FLAG_HOME_RECORD != 0).then(|| Record {
                wins: le_u16(fixed, 5),
                losses: le_u16(fixed, 7),
            }),
        },
    })
}
