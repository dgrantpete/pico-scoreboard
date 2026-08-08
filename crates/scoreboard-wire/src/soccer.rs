//! Soccer game detail (`GET /soccer/{league}/games/{game_id}`).
//!
//! The league's display name is firmware-side config — the device knows which
//! league it polled — so no payload carries it.
//!
//! # Live (state = 1)
//!
//! Fixed 24-byte section at offset 2 (`<BBHHHIIII`):
//!
//! | off | type | field                                            |
//! |-----|------|--------------------------------------------------|
//! | 2   | u8   | flags (see below)                                |
//! | 3   | u8   | half — ESPN's raw period: regulation halves 1/2, extra-time halves 3/4, shootout 5 |
//! | 4   | u16  | clock_seconds — elapsed match seconds, floor-minute convention (parsed from ESPN's displayClock; the firmware extrapolates forward from this anchor) |
//! | 6   | u16  | away_score                                       |
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
//! `home.abbreviation`, then **iff** bit1: `event.clock` (display-shaped, e.g.
//! "90'+3'") and `event.athlete` (short name, may be empty), then **iff** bit5:
//! `commentary.id` (ESPN sequence, change-detection key) and `commentary.text`
//! (latest play-by-play line — the firmware flashes it like MLB's play text).
//!
//! # Pregame (state = 0)
//!
//! Fixed 20-byte section at offset 2 (`<IIIII`): `start_time` (unix epoch
//! seconds UTC), then away primary/alternate + home primary/alternate colors.
//! Strings from offset 22: `game_id`, `away.abbreviation`,
//! `home.abbreviation`, `venue` (stadium `fullName`).
//!
//! # Final (state = 2)
//!
//! Fixed 21-byte section at offset 2 (`<BHHIIII`): `flavor` u8 (0 = full time,
//! 1 = after extra time, 2 = after penalties — how the match was decided),
//! `away_score` u16, `home_score` u16, then the four colors. Strings from
//! offset 23: `game_id`, `away.abbreviation`, `home.abbreviation`,
//! `away.scorers`, `home.scorers` (pre-formatted "M. Merino 90'+1', ..." lists,
//! always present, empty when scoreless).

use crate::codec::{Sink, SinkExt, color_pairs, le_u16, le_u32};
use crate::common::{payload_reader, read_header, write_header};
use crate::error::{BufferFull, DecodeError, DecodeErrorKind};
use crate::{GameState, HEADER_LEN, Side, TeamColors, TeamState};

const LIVE_LEN: usize = 24;
const PRE_LEN: usize = 20;
const FINAL_LEN: usize = 21;

const FLAG_BREAK: u8 = 0x01;
const FLAG_EVENT: u8 = 0x02;
const FLAG_EVENT_RED: u8 = 0x04;
const FLAG_EVENT_AWAY: u8 = 0x08;
const FLAG_EVENT_HOME: u8 = 0x10;
const FLAG_COMMENTARY: u8 = 0x20;

/// ESPN's period set: regulation halves 1/2, extra-time halves 3/4, shootout 5.
/// Decode rejects anything outside it (the same fail-loud policy as MLB's
/// inning half and NBA's phase); encode passes the upstream value through.
const HALF_FIRST: u8 = 1;
const HALF_SHOOTOUT: u8 = 5;

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
    /// ESPN's raw period (see [`HALF_FIRST`]..=[`HALF_SHOOTOUT`]).
    pub half: u8,
    /// Elapsed match seconds, floor-minute convention.
    pub clock_seconds: u16,
    /// True during a non-playing break — the clock alone cannot distinguish
    /// one from active stoppage time.
    pub on_break: bool,
    pub away: TeamState<'a>,
    pub home: TeamState<'a>,
    pub last_event: Option<Event<'a>>,
    pub commentary: Option<Commentary<'a>>,
}

/// The most recent goal or red card (yellow cards are ticker noise on a
/// 128x64 panel, so the backend never sends them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event<'a> {
    pub kind: EventKind,
    /// Which side it belongs to; `None` when ESPN omitted the team.
    pub side: Option<Side>,
    /// Display-shaped match clock, e.g. "90'+3'".
    pub clock: &'a str,
    /// Athlete short name; empty when ESPN lists none.
    pub athlete: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Goal,
    RedCard,
}

/// One play-by-play line; `id` is the ESPN sequence number the firmware
/// compares between polls (same contract as MLB's play id).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Commentary<'a> {
    pub id: &'a str,
    pub text: &'a str,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Final<'a> {
    pub game_id: &'a str,
    pub flavor: FinalFlavor,
    pub away: FinalTeam<'a>,
    pub home: FinalTeam<'a>,
}

/// How a finished match was decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalFlavor {
    FullTime,
    AfterExtraTime,
    AfterPenalties,
}

impl FinalFlavor {
    pub fn code(self) -> u8 {
        match self {
            FinalFlavor::FullTime => 0,
            FinalFlavor::AfterExtraTime => 1,
            FinalFlavor::AfterPenalties => 2,
        }
    }

    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(FinalFlavor::FullTime),
            1 => Some(FinalFlavor::AfterExtraTime),
            2 => Some(FinalFlavor::AfterPenalties),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FinalTeam<'a> {
    pub abbreviation: &'a str,
    pub score: u16,
    pub colors: TeamColors,
    /// Pre-formatted goal-scorer list ("M. Merino 90'+1', F. Torres 12'"),
    /// empty when the side didn't score. Built by the backend so the firmware
    /// never formats strings.
    pub scorers: &'a str,
}

pub fn encode<S: Sink + ?Sized>(game: &Game<'_>, out: &mut S) -> Result<(), BufferFull> {
    write_header(out, game.state())?;
    match game {
        Game::Pregame(pregame) => write_pregame(out, pregame),
        Game::Live(live) => write_live(out, live),
        Game::Final(game) => write_final(out, game),
    }
}

pub fn decode(buf: &[u8]) -> Result<Game<'_>, DecodeError> {
    match read_header(buf)? {
        GameState::Pregame => read_pregame(buf).map(Game::Pregame),
        GameState::Live => read_live(buf).map(Game::Live),
        GameState::Final => read_final(buf).map(Game::Final),
    }
}

fn write_live<S: Sink + ?Sized>(out: &mut S, game: &Live<'_>) -> Result<(), BufferFull> {
    let mut flags = 0u8;
    if game.on_break {
        flags |= FLAG_BREAK;
    }
    if let Some(event) = &game.last_event {
        flags |= FLAG_EVENT;
        if event.kind == EventKind::RedCard {
            flags |= FLAG_EVENT_RED;
        }
        match event.side {
            Some(Side::Away) => flags |= FLAG_EVENT_AWAY,
            Some(Side::Home) => flags |= FLAG_EVENT_HOME,
            None => {}
        }
    }
    if game.commentary.is_some() {
        flags |= FLAG_COMMENTARY;
    }

    out.u8(flags)?;
    out.u8(game.half)?;
    out.u16(game.clock_seconds)?;
    out.u16(game.away.score)?;
    out.u16(game.home.score)?;
    out.color_pairs(game.away.colors, game.home.colors)?;

    out.string(game.game_id)?;
    out.string(game.away.abbreviation)?;
    out.string(game.home.abbreviation)?;
    if let Some(event) = &game.last_event {
        out.string(event.clock)?;
        out.string(event.athlete)?;
    }
    if let Some(commentary) = &game.commentary {
        out.string(commentary.id)?;
        out.string(commentary.text)?;
    }
    Ok(())
}

fn read_live(buf: &[u8]) -> Result<Live<'_>, DecodeError> {
    let mut reader = payload_reader(buf);
    let fixed = reader.fixed::<LIVE_LEN>()?;
    let flags = fixed[0];
    let half = fixed[1];
    if !(HALF_FIRST..=HALF_SHOOTOUT).contains(&half) {
        return Err(DecodeError::at(
            HEADER_LEN + 1,
            DecodeErrorKind::InvalidCode {
                field: "soccer period",
                code: half,
            },
        ));
    }
    let (away_colors, home_colors) = color_pairs(fixed, 8);

    let game_id = reader.string("game_id")?;
    let away_abbreviation = reader.string("away abbreviation")?;
    let home_abbreviation = reader.string("home abbreviation")?;
    let last_event = if flags & FLAG_EVENT != 0 {
        Some(Event {
            kind: if flags & FLAG_EVENT_RED != 0 {
                EventKind::RedCard
            } else {
                EventKind::Goal
            },
            side: if flags & FLAG_EVENT_AWAY != 0 {
                Some(Side::Away)
            } else if flags & FLAG_EVENT_HOME != 0 {
                Some(Side::Home)
            } else {
                None
            },
            clock: reader.string("event clock")?,
            athlete: reader.string("event athlete")?,
        })
    } else {
        None
    };
    let commentary = if flags & FLAG_COMMENTARY != 0 {
        Some(Commentary {
            id: reader.string("commentary id")?,
            text: reader.string("commentary text")?,
        })
    } else {
        None
    };
    reader.finish()?;

    Ok(Live {
        game_id,
        half,
        clock_seconds: le_u16(fixed, 2),
        on_break: flags & FLAG_BREAK != 0,
        away: TeamState {
            abbreviation: away_abbreviation,
            score: le_u16(fixed, 4),
            colors: away_colors,
        },
        home: TeamState {
            abbreviation: home_abbreviation,
            score: le_u16(fixed, 6),
            colors: home_colors,
        },
        last_event,
        commentary,
    })
}

fn write_pregame<S: Sink + ?Sized>(out: &mut S, game: &Pregame<'_>) -> Result<(), BufferFull> {
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
    let (away_colors, home_colors) = color_pairs(fixed, 4);

    let game_id = reader.string("game_id")?;
    let away_abbreviation = reader.string("away abbreviation")?;
    let home_abbreviation = reader.string("home abbreviation")?;
    let venue = reader.string("venue")?;
    reader.finish()?;

    Ok(Pregame {
        game_id,
        start_time: le_u32(fixed, 0),
        venue,
        away: PregameTeam {
            abbreviation: away_abbreviation,
            colors: away_colors,
        },
        home: PregameTeam {
            abbreviation: home_abbreviation,
            colors: home_colors,
        },
    })
}

fn write_final<S: Sink + ?Sized>(out: &mut S, game: &Final<'_>) -> Result<(), BufferFull> {
    out.u8(game.flavor.code())?;
    out.u16(game.away.score)?;
    out.u16(game.home.score)?;
    out.color_pairs(game.away.colors, game.home.colors)?;

    out.string(game.game_id)?;
    out.string(game.away.abbreviation)?;
    out.string(game.home.abbreviation)?;
    out.string(game.away.scorers)?;
    out.string(game.home.scorers)
}

fn read_final(buf: &[u8]) -> Result<Final<'_>, DecodeError> {
    let mut reader = payload_reader(buf);
    let fixed = reader.fixed::<FINAL_LEN>()?;
    let flavor = FinalFlavor::from_code(fixed[0]).ok_or_else(|| {
        DecodeError::at(
            HEADER_LEN,
            DecodeErrorKind::InvalidCode {
                field: "full-time flavor",
                code: fixed[0],
            },
        )
    })?;
    let (away_colors, home_colors) = color_pairs(fixed, 5);

    let game_id = reader.string("game_id")?;
    let away_abbreviation = reader.string("away abbreviation")?;
    let home_abbreviation = reader.string("home abbreviation")?;
    let away_scorers = reader.string("away scorers")?;
    let home_scorers = reader.string("home scorers")?;
    reader.finish()?;

    Ok(Final {
        game_id,
        flavor,
        away: FinalTeam {
            abbreviation: away_abbreviation,
            score: le_u16(fixed, 1),
            colors: away_colors,
            scorers: away_scorers,
        },
        home: FinalTeam {
            abbreviation: home_abbreviation,
            score: le_u16(fixed, 3),
            colors: home_colors,
            scorers: home_scorers,
        },
    })
}
