//! Football domain model → [`scoreboard_wire::football`] view. Layout lives in
//! the crate.

use scoreboard_wire as wire;

use super::types::{
    FootballFinalGame, FootballFinalTeam, FootballGame, FootballLiveGame, FootballPregameGame,
    FootballPregameTeam,
};
use crate::shared::wire::{encoded, line_score};

pub(crate) fn encode_game(game: &FootballGame) -> Vec<u8> {
    encoded(128, |out| wire::football::encode(&game.into(), out))
}

impl<'a> From<&'a FootballGame> for wire::football::Game<'a> {
    fn from(game: &'a FootballGame) -> Self {
        match game {
            FootballGame::Pregame(game) => Self::Pregame(game.into()),
            FootballGame::Live(game) => Self::Live(game.into()),
            FootballGame::Final(game) => Self::Final(game.into()),
        }
    }
}

impl<'a> From<&'a FootballLiveGame> for wire::football::Live<'a> {
    fn from(game: &'a FootballLiveGame) -> Self {
        Self {
            game_id: &game.game_id,
            period: game.period,
            phase: game.phase.into(),
            clock: &game.clock,
            away: (&game.away).into(),
            home: (&game.home).into(),
            situation: game
                .situation
                .as_ref()
                .map(|situation| wire::football::Situation {
                    down: situation.down,
                    distance: situation.distance,
                    yard_line: situation.yard_line,
                    possession: situation.possession.into(),
                    red_zone: situation.red_zone,
                }),
            timeouts: game.timeouts.map(|timeouts| wire::football::Timeouts {
                away: timeouts.away,
                home: timeouts.home,
            }),
            last_play: game.last_play.as_ref().map(Into::into),
        }
    }
}

impl<'a> From<&'a FootballPregameGame> for wire::football::Pregame<'a> {
    fn from(game: &'a FootballPregameGame) -> Self {
        Self {
            game_id: &game.game_id,
            start_time: game.start_time,
            venue: &game.venue,
            away: (&game.away).into(),
            home: (&game.home).into(),
        }
    }
}

impl<'a> From<&'a FootballPregameTeam> for wire::football::PregameTeam<'a> {
    fn from(team: &'a FootballPregameTeam) -> Self {
        Self {
            abbreviation: &team.abbreviation,
            colors: (&team.colors).into(),
            record: team.record.as_ref().map(Into::into),
            rank_line: team.rank_line.as_deref(),
        }
    }
}

impl<'a> From<&'a FootballFinalGame> for wire::football::Final<'a> {
    fn from(game: &'a FootballFinalGame) -> Self {
        Self {
            game_id: &game.game_id,
            periods_played: game.periods_played,
            away: final_team(&game.away),
            home: final_team(&game.home),
        }
    }
}

fn final_team(team: &FootballFinalTeam) -> wire::FinalTeam<'_> {
    wire::FinalTeam {
        abbreviation: &team.abbreviation,
        score: wire::saturate_score(team.score),
        colors: (&team.colors).into(),
        line_score: line_score(&team.line_score),
    }
}
