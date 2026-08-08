//! The bridge from the owned domain model (which also serializes to JSON) to
//! the borrowed views [`scoreboard_wire`] encodes. Each sport's half lives in
//! its own module; this is what they share, plus the games list, whose payload
//! is sport-agnostic.

use scoreboard_wire as wire;

use crate::shared::game::{GameListEntry, GameState, LastPlay, LivePhase, Record, Side};
use crate::shared::team::{TeamColors, TeamState};

/// Encode into a fresh `Vec`, which cannot fill — the encoder's buffer-full
/// error only exists for the firmware's fixed-size sinks.
pub(crate) fn encoded(
    capacity: usize,
    write: impl FnOnce(&mut Vec<u8>) -> Result<(), wire::BufferFull>,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(capacity);
    write(&mut out).expect("a Vec sink never fills");
    out
}

/// Clip a line score to what the wire's `u8` length prefix can describe. The
/// cap is unreachable for a real game, so crossing it is worth a log line.
pub(crate) fn line_score(line: &[u8]) -> &[u8] {
    if line.len() > wire::MAX_LINE_SCORE {
        tracing::warn!(len = line.len(), "line score exceeds wire cap; truncating");
        &line[..wire::MAX_LINE_SCORE]
    } else {
        line
    }
}

impl From<&TeamColors> for wire::TeamColors {
    fn from(colors: &TeamColors) -> Self {
        Self {
            primary: colors.primary,
            alternate: colors.alternate,
        }
    }
}

impl<'a> From<&'a TeamState> for wire::TeamState<'a> {
    fn from(team: &'a TeamState) -> Self {
        Self {
            abbreviation: &team.abbreviation,
            score: wire::saturate_score(team.score),
            colors: (&team.colors).into(),
        }
    }
}

impl From<&Record> for wire::Record {
    fn from(record: &Record) -> Self {
        Self {
            wins: record.wins,
            losses: record.losses,
        }
    }
}

impl<'a> From<&'a LastPlay> for wire::LastPlay<'a> {
    fn from(play: &'a LastPlay) -> Self {
        Self {
            id: &play.id,
            text: &play.text,
        }
    }
}

impl From<LivePhase> for wire::LivePhase {
    fn from(phase: LivePhase) -> Self {
        match phase {
            LivePhase::InProgress => wire::LivePhase::InProgress,
            LivePhase::Halftime => wire::LivePhase::Halftime,
            LivePhase::EndOfPeriod => wire::LivePhase::EndOfPeriod,
        }
    }
}

impl From<Side> for wire::Side {
    fn from(side: Side) -> Self {
        match side {
            Side::Away => wire::Side::Away,
            Side::Home => wire::Side::Home,
        }
    }
}

impl From<GameState> for wire::GameState {
    fn from(state: GameState) -> Self {
        match state {
            GameState::Pregame => wire::GameState::Pregame,
            GameState::Live => wire::GameState::Live,
            GameState::Final => wire::GameState::Final,
        }
    }
}

/// Encode the games list. Entries past the wire's `u8` count are dropped by the
/// encoder — unreachable for any real slate, but never silently.
pub(crate) fn encode_game_list(entries: &[GameListEntry]) -> Vec<u8> {
    if entries.len() > wire::MAX_GAMES {
        tracing::warn!(
            count = entries.len(),
            "game list exceeds wire cap; truncating to {}",
            wire::MAX_GAMES
        );
    }
    let entries: Vec<wire::list::Entry<'_>> = entries
        .iter()
        .map(|entry| wire::list::Entry {
            state: entry.state.into(),
            id: &entry.id,
        })
        .collect();
    encoded(2 + entries.len() * 13, |out| {
        wire::list::encode(&entries, out)
    })
}
