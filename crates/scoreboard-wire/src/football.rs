//! Football game detail — NFL and NCAAF (`GET /football/{league}/games/{game_id}`).
//!
//! # Live (state = 1)
//!
//! Fixed 28-byte section at offset 2 (`<BBBBBBBBHHIIII`):
//!
//! | off | type | field                                            |
//! |-----|------|--------------------------------------------------|
//! | 2   | u8   | flags (see below)                                |
//! | 3   | u8   | period (quarter 1–4; overtime = 5+)              |
//! | 4   | u8   | phase (0=in progress, 1=halftime, 2=end of period) |
//! | 5   | u8   | down (1–4; 0 when no situation)                  |
//! | 6   | u8   | distance — yards to the first-down line (0 when no situation) |
//! | 7   | u8   | yard_line — absolute ball spot 0–100 (0 when no situation) |
//! | 8   | u8   | away_timeouts (0 when timeouts absent)           |
//! | 9   | u8   | home_timeouts (0 when timeouts absent)           |
//! | 10  | u16  | away_score                                       |
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
//! during breaks — render by `phase`; never extrapolated), then **iff** bit0:
//! `last_play.id` and `last_play.text`.
//!
//! # Pregame (state = 0)
//!
//! Byte-identical to the [NBA pregame](crate::nba) (`<BHHHHIIIII`, 29 bytes at
//! offset 2: flags, away wins/losses, home wins/losses, start_time, then the
//! four colors) plus two rank flag bits. Flags: bit0 = away record, bit1 = home
//! record, bit2 = away rank line present, bit3 = home rank line present.
//! Numeric record fields whose flag is unset are zero.
//!
//! Strings from offset 31, in order: `game_id`, `away.abbreviation`,
//! `home.abbreviation`, `venue`, then `away.rank_line` **iff** bit2 and
//! `home.rank_line` **iff** bit3 (display-shaped "#3 OHIO STATE" — college only,
//! present only when ranked). Mirrors MLB's probable-pitcher flag/string
//! pattern: the rank line rides the pitcher slot.
//!
//! # Final (state = 2)
//!
//! The shared final layout (see [`crate::common`]) with per-quarter points;
//! overtime extends both line scores past 4.

use crate::codec::{Sink, SinkExt, color_pairs, le_u16, le_u32};
use crate::common::{
    FinalParts, payload_reader, read_final, read_header, write_final, write_header,
};
use crate::error::{BufferFull, DecodeError, DecodeErrorKind};
use crate::{
    FinalTeam, GameState, HEADER_LEN, LastPlay, LivePhase, Record, Side, TeamColors, TeamState,
};

const LIVE_LEN: usize = 28;
const PRE_LEN: usize = 29;

const FLAG_LAST_PLAY: u8 = 0x01;
const FLAG_SITUATION: u8 = 0x02;
const FLAG_POSSESSION_HOME: u8 = 0x04;
const FLAG_RED_ZONE: u8 = 0x08;
const FLAG_TIMEOUTS: u8 = 0x10;

const PRE_FLAG_AWAY_RECORD: u8 = 0x01;
const PRE_FLAG_HOME_RECORD: u8 = 0x02;
const PRE_FLAG_AWAY_RANK: u8 = 0x04;
const PRE_FLAG_HOME_RANK: u8 = 0x08;

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
    /// Absent between plays and whenever the upstream situation fails
    /// validation — a half-formed one would misdraw the field markers.
    pub situation: Option<Situation>,
    /// All-or-nothing: one wire flag covers both counts.
    pub timeouts: Option<Timeouts>,
    /// Absent before the opening snap.
    pub last_play: Option<LastPlay<'a>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Situation {
    /// 1st–4th down.
    pub down: u8,
    pub distance: u8,
    /// Absolute 0–100 yard line.
    pub yard_line: u8,
    pub possession: Side,
    pub red_zone: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    pub away: u8,
    pub home: u8,
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
    /// "#3 OHIO STATE" — college only, and only when ranked.
    pub rank_line: Option<&'a str>,
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
    let mut flags = 0u8;
    if game.last_play.is_some() {
        flags |= FLAG_LAST_PLAY;
    }
    if let Some(situation) = &game.situation {
        flags |= FLAG_SITUATION;
        if situation.possession == Side::Home {
            flags |= FLAG_POSSESSION_HOME;
        }
        if situation.red_zone {
            flags |= FLAG_RED_ZONE;
        }
    }
    if game.timeouts.is_some() {
        flags |= FLAG_TIMEOUTS;
    }
    let situation = game.situation.unwrap_or(Situation {
        down: 0,
        distance: 0,
        yard_line: 0,
        possession: Side::Away,
        red_zone: false,
    });
    let timeouts = game.timeouts.unwrap_or(Timeouts { away: 0, home: 0 });

    out.u8(flags)?;
    out.u8(game.period)?;
    out.u8(game.phase.code())?;
    out.u8(situation.down)?;
    out.u8(situation.distance)?;
    out.u8(situation.yard_line)?;
    out.u8(timeouts.away)?;
    out.u8(timeouts.home)?;
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
    // Absent situation reads as zeroed drive fields regardless of what the
    // fixed section carries, so a stale spot can never reach the display.
    let situation = (flags & FLAG_SITUATION != 0).then(|| Situation {
        down: fixed[3],
        distance: fixed[4],
        yard_line: fixed[5],
        possession: if flags & FLAG_POSSESSION_HOME != 0 {
            Side::Home
        } else {
            Side::Away
        },
        red_zone: flags & FLAG_RED_ZONE != 0,
    });
    let timeouts = (flags & FLAG_TIMEOUTS != 0).then(|| Timeouts {
        away: fixed[6],
        home: fixed[7],
    });
    let (away_colors, home_colors) = color_pairs(fixed, 12);

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
            score: le_u16(fixed, 8),
            colors: away_colors,
        },
        home: TeamState {
            abbreviation: home_abbreviation,
            score: le_u16(fixed, 10),
            colors: home_colors,
        },
        situation,
        timeouts,
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
    if game.away.rank_line.is_some() {
        flags |= PRE_FLAG_AWAY_RANK;
    }
    if game.home.rank_line.is_some() {
        flags |= PRE_FLAG_HOME_RANK;
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
    out.string(game.venue)?;
    if let Some(rank) = game.away.rank_line {
        out.string(rank)?;
    }
    if let Some(rank) = game.home.rank_line {
        out.string(rank)?;
    }
    Ok(())
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
    let away_rank = if flags & PRE_FLAG_AWAY_RANK != 0 {
        Some(reader.string("away rank line")?)
    } else {
        None
    };
    let home_rank = if flags & PRE_FLAG_HOME_RANK != 0 {
        Some(reader.string("home rank line")?)
    } else {
        None
    };
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
            rank_line: away_rank,
        },
        home: PregameTeam {
            abbreviation: home_abbreviation,
            colors: home_colors,
            record: (flags & PRE_FLAG_HOME_RECORD != 0).then(|| Record {
                wins: le_u16(fixed, 5),
                losses: le_u16(fixed, 7),
            }),
            rank_line: home_rank,
        },
    })
}
