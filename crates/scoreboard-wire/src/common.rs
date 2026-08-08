//! The vocabulary every sport payload shares: the 2-byte header, the state and
//! phase codes, and the team/play value types.

use crate::codec::{Reader, Sink, SinkExt, color_pairs, le_u16};
use crate::error::{BufferFull, DecodeError, DecodeErrorKind};
use crate::{HEADER_LEN, MAX_LINE_SCORE, WIRE_VERSION};

/// Which payload follows the header. Also the per-entry tag in the games list
/// and the state half of the backend's ETag cache tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameState {
    Pregame,
    Live,
    Final,
}

impl GameState {
    pub fn code(self) -> u8 {
        match self {
            GameState::Pregame => 0,
            GameState::Live => 1,
            GameState::Final => 2,
        }
    }

    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(GameState::Pregame),
            1 => Some(GameState::Live),
            2 => Some(GameState::Final),
            _ => None,
        }
    }
}

/// The live sub-state of the clock-stopping sports (NBA + football): during a
/// break the clock string is meaningless, so the phase decides the view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LivePhase {
    InProgress,
    Halftime,
    EndOfPeriod,
}

impl LivePhase {
    pub fn code(self) -> u8 {
        match self {
            LivePhase::InProgress => 0,
            LivePhase::Halftime => 1,
            LivePhase::EndOfPeriod => 2,
        }
    }

    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(LivePhase::InProgress),
            1 => Some(LivePhase::Halftime),
            2 => Some(LivePhase::EndOfPeriod),
            _ => None,
        }
    }
}

/// Home or away. Carried as a flag bit (football: who has the ball; soccer:
/// whose event it was).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Away,
    Home,
}

/// RGB888 packed as `0x00RRGGBB` for cheap parsing on the Pico.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TeamColors {
    pub primary: u32,
    pub alternate: u32,
}

/// A live team: identity, current score, colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TeamState<'a> {
    /// e.g. "BOS" — the firmware's logo key.
    pub abbreviation: &'a str,
    pub score: u16,
    pub colors: TeamColors,
}

/// A finished team: score plus the per-period line score (runs, points).
/// Per-team lengths are independent — a walk-off leaves the home line short,
/// extras/overtime run past the regulation count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FinalTeam<'a> {
    pub abbreviation: &'a str,
    pub score: u16,
    pub colors: TeamColors,
    pub line_score: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    pub wins: u16,
    pub losses: u16,
}

/// The most recent play: display text plus the `id` the firmware compares
/// between polls to trigger its flash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LastPlay<'a> {
    pub id: &'a str,
    pub text: &'a str,
}

/// Scores travel as `u16`; anything larger is a data glitch and saturates
/// rather than wrapping into a plausible-looking small number.
pub fn saturate_score(score: u32) -> u16 {
    score.min(u16::MAX as u32) as u16
}

/// Temperature travels as one unsigned byte of °F.
pub fn clamp_temperature(fahrenheit: i16) -> u8 {
    fahrenheit.clamp(0, u8::MAX as i16) as u8
}

pub(crate) fn write_header<S: Sink + ?Sized>(
    out: &mut S,
    state: GameState,
) -> Result<(), BufferFull> {
    out.u8(WIRE_VERSION)?;
    out.u8(state.code())
}

/// Fail loud on anything that isn't this wire version — a stray JSON body
/// (`{` / `[` at offset 0) trips this immediately.
pub(crate) fn read_header_version(buf: &[u8]) -> Result<(), DecodeError> {
    match buf.first() {
        None => Err(DecodeError::at(0, DecodeErrorKind::Empty)),
        Some(&version) if version != WIRE_VERSION => Err(DecodeError::at(
            0,
            DecodeErrorKind::UnsupportedVersion(version),
        )),
        Some(_) => Ok(()),
    }
}

/// Validate the 2-byte header and return the state the payload claims.
pub(crate) fn read_header(buf: &[u8]) -> Result<GameState, DecodeError> {
    read_header_version(buf)?;
    let state = *buf
        .get(1)
        .ok_or_else(|| DecodeError::at(1, DecodeErrorKind::Truncated("state byte")))?;
    GameState::from_code(state)
        .ok_or_else(|| DecodeError::at(1, DecodeErrorKind::UnknownState(state)))
}

/// Start a payload reader positioned just past the validated header.
pub(crate) fn payload_reader(buf: &[u8]) -> Reader<'_> {
    let mut reader = Reader::new(buf);
    reader.skip_header();
    debug_assert_eq!(reader.offset(), HEADER_LEN);
    reader
}

/// The final payload, identical across MLB, NBA and football: a fixed 23-byte
/// section at offset 2 (`<BBBHHIIII`) — period count, the two line-score
/// lengths, the two scores, the two color pairs — then the away and home
/// line-score bytes, then `game_id`, `away.abbreviation`, `home.abbreviation`.
/// Only the period's name differs between the sports (innings vs quarters),
/// which is a field name, not a byte.
const FINAL_LEN: usize = 23;

pub(crate) struct FinalParts<'a> {
    pub(crate) game_id: &'a str,
    pub(crate) periods: u8,
    pub(crate) away: FinalTeam<'a>,
    pub(crate) home: FinalTeam<'a>,
}

pub(crate) fn write_final<S: Sink + ?Sized>(
    out: &mut S,
    game_id: &str,
    periods: u8,
    away: &FinalTeam<'_>,
    home: &FinalTeam<'_>,
) -> Result<(), BufferFull> {
    let away_line = &away.line_score[..away.line_score.len().min(MAX_LINE_SCORE)];
    let home_line = &home.line_score[..home.line_score.len().min(MAX_LINE_SCORE)];

    out.u8(periods)?;
    out.u8(away_line.len() as u8)?;
    out.u8(home_line.len() as u8)?;
    out.u16(away.score)?;
    out.u16(home.score)?;
    out.color_pairs(away.colors, home.colors)?;

    out.write_bytes(away_line)?;
    out.write_bytes(home_line)?;

    out.string(game_id)?;
    out.string(away.abbreviation)?;
    out.string(home.abbreviation)
}

pub(crate) fn read_final(buf: &[u8]) -> Result<FinalParts<'_>, DecodeError> {
    let mut reader = payload_reader(buf);
    let fixed = reader.fixed::<FINAL_LEN>()?;
    let (away_colors, home_colors) = color_pairs(fixed, 7);
    let (away_line, home_line) = reader.line_scores(fixed[1] as usize, fixed[2] as usize)?;

    let game_id = reader.string("game_id")?;
    let away_abbreviation = reader.string("away abbreviation")?;
    let home_abbreviation = reader.string("home abbreviation")?;
    reader.finish()?;

    Ok(FinalParts {
        game_id,
        periods: fixed[0],
        away: FinalTeam {
            abbreviation: away_abbreviation,
            score: le_u16(fixed, 3),
            colors: away_colors,
            line_score: away_line,
        },
        home: FinalTeam {
            abbreviation: home_abbreviation,
            score: le_u16(fixed, 5),
            colors: home_colors,
            line_score: home_line,
        },
    })
}
