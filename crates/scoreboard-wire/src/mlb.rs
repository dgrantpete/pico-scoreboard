//! MLB game detail (`GET /baseball/mlb/games/{game_id}`).
//!
//! # Live (state = 1)
//!
//! Fixed 27-byte section at offset 2 (`<BBBBBBBHHIIII`):
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
//! | 9   | u16  | away_score                                       |
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
//! # Pregame (state = 0)
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
//! # Final (state = 2)
//!
//! The shared final layout (see [`crate::common`] — `innings_played` is the
//! period count, the line scores are runs per inning).

use crate::codec::{Sink, SinkExt, color_pairs, le_u16, le_u32};
use crate::common::{
    FinalParts, payload_reader, read_final, read_header, write_final, write_header,
};
use crate::error::{BufferFull, DecodeError, DecodeErrorKind};
use crate::{FinalTeam, GameState, HEADER_LEN, LastPlay, Record, TeamColors, TeamState};

const LIVE_LEN: usize = 27;
const PRE_LEN: usize = 30;

const FLAG_AT_BAT: u8 = 0x01;

const PRE_FLAG_WEATHER: u8 = 0x01;
const PRE_FLAG_AWAY_RECORD: u8 = 0x02;
const PRE_FLAG_HOME_RECORD: u8 = 0x04;
const PRE_FLAG_AWAY_PROBABLE: u8 = 0x08;
const PRE_FLAG_HOME_PROBABLE: u8 = 0x10;

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
    pub inning: Inning,
    pub count: Count,
    pub bases: Bases,
    pub away: TeamState<'a>,
    pub home: TeamState<'a>,
    /// Absent between innings or before an at-bat starts.
    pub at_bat: Option<AtBat<'a>>,
    pub last_play: LastPlay<'a>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Inning {
    pub number: u8,
    pub half: InningHalf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InningHalf {
    Top,
    Middle,
    Bottom,
    End,
}

impl InningHalf {
    pub fn code(self) -> u8 {
        match self {
            InningHalf::Top => 0,
            InningHalf::Middle => 1,
            InningHalf::Bottom => 2,
            InningHalf::End => 3,
        }
    }

    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(InningHalf::Top),
            1 => Some(InningHalf::Middle),
            2 => Some(InningHalf::Bottom),
            3 => Some(InningHalf::End),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Count {
    pub balls: u8,
    pub strikes: u8,
    pub outs: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bases {
    pub first: bool,
    pub second: bool,
    pub third: bool,
}

impl Bases {
    fn bits(self) -> u8 {
        (self.first as u8) | ((self.second as u8) << 1) | ((self.third as u8) << 2)
    }

    fn from_bits(bits: u8) -> Self {
        Self {
            first: bits & 0x01 != 0,
            second: bits & 0x02 != 0,
            third: bits & 0x04 != 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtBat<'a> {
    pub pitcher: &'a str,
    pub batter: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pregame<'a> {
    pub game_id: &'a str,
    /// Unix epoch seconds, UTC. The firmware applies the device's offset.
    pub start_time: u32,
    pub venue: &'a str,
    pub weather: Option<Weather<'a>>,
    pub away: PregameTeam<'a>,
    pub home: PregameTeam<'a>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Weather<'a> {
    pub condition: &'a str,
    /// °F (see [`crate::clamp_temperature`]).
    pub temperature: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PregameTeam<'a> {
    pub abbreviation: &'a str,
    pub colors: TeamColors,
    pub record: Option<Record>,
    /// Probable starter's short name, e.g. "G. Marquez".
    pub probable_pitcher: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Final<'a> {
    pub game_id: &'a str,
    /// 9, or more for extras.
    pub innings_played: u8,
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
            game.innings_played,
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
                innings_played: periods,
                away,
                home,
            })
        }),
    }
}

fn write_live<S: Sink + ?Sized>(out: &mut S, game: &Live<'_>) -> Result<(), BufferFull> {
    out.u8(if game.at_bat.is_some() { FLAG_AT_BAT } else { 0 })?;
    out.u8(game.inning.number)?;
    out.u8(game.inning.half.code())?;
    out.u8(game.count.balls)?;
    out.u8(game.count.strikes)?;
    out.u8(game.count.outs)?;
    out.u8(game.bases.bits())?;
    out.u16(game.away.score)?;
    out.u16(game.home.score)?;
    out.color_pairs(game.away.colors, game.home.colors)?;

    out.string(game.game_id)?;
    out.string(game.away.abbreviation)?;
    out.string(game.home.abbreviation)?;
    if let Some(at_bat) = &game.at_bat {
        out.string(at_bat.pitcher)?;
        out.string(at_bat.batter)?;
    }
    out.string(game.last_play.id)?;
    out.string(game.last_play.text)
}

fn read_live(buf: &[u8]) -> Result<Live<'_>, DecodeError> {
    let mut reader = payload_reader(buf);
    let fixed = reader.fixed::<LIVE_LEN>()?;
    let flags = fixed[0];
    let half = InningHalf::from_code(fixed[2]).ok_or_else(|| {
        DecodeError::at(
            HEADER_LEN + 2,
            DecodeErrorKind::InvalidCode {
                field: "inning half",
                code: fixed[2],
            },
        )
    })?;
    let (away_colors, home_colors) = color_pairs(fixed, 11);

    let game_id = reader.string("game_id")?;
    let away_abbreviation = reader.string("away abbreviation")?;
    let home_abbreviation = reader.string("home abbreviation")?;
    let at_bat = if flags & FLAG_AT_BAT != 0 {
        Some(AtBat {
            pitcher: reader.string("pitcher")?,
            batter: reader.string("batter")?,
        })
    } else {
        None
    };
    let last_play = LastPlay {
        id: reader.string("last play id")?,
        text: reader.string("last play text")?,
    };
    reader.finish()?;

    Ok(Live {
        game_id,
        inning: Inning {
            number: fixed[1],
            half,
        },
        count: Count {
            balls: fixed[3],
            strikes: fixed[4],
            outs: fixed[5],
        },
        bases: Bases::from_bits(fixed[6]),
        away: TeamState {
            abbreviation: away_abbreviation,
            score: le_u16(fixed, 7),
            colors: away_colors,
        },
        home: TeamState {
            abbreviation: home_abbreviation,
            score: le_u16(fixed, 9),
            colors: home_colors,
        },
        at_bat,
        last_play,
    })
}

fn write_pregame<S: Sink + ?Sized>(out: &mut S, game: &Pregame<'_>) -> Result<(), BufferFull> {
    let mut flags = 0u8;
    if game.weather.is_some() {
        flags |= PRE_FLAG_WEATHER;
    }
    if game.away.record.is_some() {
        flags |= PRE_FLAG_AWAY_RECORD;
    }
    if game.home.record.is_some() {
        flags |= PRE_FLAG_HOME_RECORD;
    }
    if game.away.probable_pitcher.is_some() {
        flags |= PRE_FLAG_AWAY_PROBABLE;
    }
    if game.home.probable_pitcher.is_some() {
        flags |= PRE_FLAG_HOME_PROBABLE;
    }
    let away_record = game.away.record.unwrap_or(Record { wins: 0, losses: 0 });
    let home_record = game.home.record.unwrap_or(Record { wins: 0, losses: 0 });

    out.u8(flags)?;
    out.u8(game.weather.map_or(0, |w| w.temperature))?;
    out.u16(away_record.wins)?;
    out.u16(away_record.losses)?;
    out.u16(home_record.wins)?;
    out.u16(home_record.losses)?;
    out.u32(game.start_time)?;
    out.color_pairs(game.away.colors, game.home.colors)?;

    out.string(game.game_id)?;
    out.string(game.away.abbreviation)?;
    out.string(game.home.abbreviation)?;
    out.string(game.venue)?;
    if let Some(weather) = &game.weather {
        out.string(weather.condition)?;
    }
    if let Some(pitcher) = game.away.probable_pitcher {
        out.string(pitcher)?;
    }
    if let Some(pitcher) = game.home.probable_pitcher {
        out.string(pitcher)?;
    }
    Ok(())
}

fn read_pregame(buf: &[u8]) -> Result<Pregame<'_>, DecodeError> {
    let mut reader = payload_reader(buf);
    let fixed = reader.fixed::<PRE_LEN>()?;
    let flags = fixed[0];
    let (away_colors, home_colors) = color_pairs(fixed, 14);

    let game_id = reader.string("game_id")?;
    let away_abbreviation = reader.string("away abbreviation")?;
    let home_abbreviation = reader.string("home abbreviation")?;
    let venue = reader.string("venue")?;
    let weather = if flags & PRE_FLAG_WEATHER != 0 {
        Some(Weather {
            condition: reader.string("weather condition")?,
            temperature: fixed[1],
        })
    } else {
        None
    };
    let away_pitcher = if flags & PRE_FLAG_AWAY_PROBABLE != 0 {
        Some(reader.string("away probable pitcher")?)
    } else {
        None
    };
    let home_pitcher = if flags & PRE_FLAG_HOME_PROBABLE != 0 {
        Some(reader.string("home probable pitcher")?)
    } else {
        None
    };
    reader.finish()?;

    Ok(Pregame {
        game_id,
        start_time: le_u32(fixed, 10),
        venue,
        weather,
        away: PregameTeam {
            abbreviation: away_abbreviation,
            colors: away_colors,
            record: (flags & PRE_FLAG_AWAY_RECORD != 0).then(|| Record {
                wins: le_u16(fixed, 2),
                losses: le_u16(fixed, 4),
            }),
            probable_pitcher: away_pitcher,
        },
        home: PregameTeam {
            abbreviation: home_abbreviation,
            colors: home_colors,
            record: (flags & PRE_FLAG_HOME_RECORD != 0).then(|| Record {
                wins: le_u16(fixed, 6),
                losses: le_u16(fixed, 8),
            }),
            probable_pitcher: home_pitcher,
        },
    })
}
