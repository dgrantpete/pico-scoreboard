//! NBA domain model → [`scoreboard_wire::nba`] view. Layout lives in the crate.

use scoreboard_wire as wire;

use super::types::{
    NbaFinalGame, NbaFinalTeam, NbaGame, NbaLiveGame, NbaPregameGame, NbaPregameTeam,
};
use crate::shared::wire::{encoded, line_score};

pub(crate) fn encode_game(game: &NbaGame) -> Vec<u8> {
    encoded(128, |out| wire::nba::encode(&game.into(), out))
}

impl<'a> From<&'a NbaGame> for wire::nba::Game<'a> {
    fn from(game: &'a NbaGame) -> Self {
        match game {
            NbaGame::Pregame(game) => Self::Pregame(game.into()),
            NbaGame::Live(game) => Self::Live(game.into()),
            NbaGame::Final(game) => Self::Final(game.into()),
        }
    }
}

impl<'a> From<&'a NbaLiveGame> for wire::nba::Live<'a> {
    fn from(game: &'a NbaLiveGame) -> Self {
        Self {
            game_id: &game.game_id,
            period: game.period,
            phase: game.phase.into(),
            clock: &game.clock,
            away: (&game.away).into(),
            home: (&game.home).into(),
            last_play: game.last_play.as_ref().map(Into::into),
        }
    }
}

impl<'a> From<&'a NbaPregameGame> for wire::nba::Pregame<'a> {
    fn from(game: &'a NbaPregameGame) -> Self {
        Self {
            game_id: &game.game_id,
            start_time: game.start_time,
            venue: &game.venue,
            away: (&game.away).into(),
            home: (&game.home).into(),
        }
    }
}

impl<'a> From<&'a NbaPregameTeam> for wire::nba::PregameTeam<'a> {
    fn from(team: &'a NbaPregameTeam) -> Self {
        Self {
            abbreviation: &team.abbreviation,
            colors: (&team.colors).into(),
            record: team.record.as_ref().map(Into::into),
        }
    }
}

impl<'a> From<&'a NbaFinalGame> for wire::nba::Final<'a> {
    fn from(game: &'a NbaFinalGame) -> Self {
        Self {
            game_id: &game.game_id,
            periods_played: game.periods_played,
            away: final_team(&game.away),
            home: final_team(&game.home),
        }
    }
}

fn final_team(team: &NbaFinalTeam) -> wire::FinalTeam<'_> {
    wire::FinalTeam {
        abbreviation: &team.abbreviation,
        score: wire::saturate_score(team.score),
        colors: (&team.colors).into(),
        line_score: line_score(&team.line_score),
    }
}
