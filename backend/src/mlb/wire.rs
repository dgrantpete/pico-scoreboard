//! MLB domain model → [`scoreboard_wire::mlb`] view. Layout lives in the crate.

use scoreboard_wire as wire;

use super::types::{
    InningHalf, MlbFinalGame, MlbFinalTeam, MlbGame, MlbLiveGame, MlbPregameGame, MlbPregameTeam,
};
use crate::shared::wire::{encoded, line_score};

pub(crate) fn encode_game(game: &MlbGame) -> Vec<u8> {
    encoded(256, |out| wire::mlb::encode(&game.into(), out))
}

impl<'a> From<&'a MlbGame> for wire::mlb::Game<'a> {
    fn from(game: &'a MlbGame) -> Self {
        match game {
            MlbGame::Pregame(game) => Self::Pregame(game.into()),
            MlbGame::Live(game) => Self::Live(game.into()),
            MlbGame::Final(game) => Self::Final(game.into()),
        }
    }
}

impl<'a> From<&'a MlbLiveGame> for wire::mlb::Live<'a> {
    fn from(game: &'a MlbLiveGame) -> Self {
        Self {
            game_id: &game.game_id,
            inning: wire::mlb::Inning {
                number: game.inning.number,
                half: match game.inning.half {
                    InningHalf::Top => wire::mlb::InningHalf::Top,
                    InningHalf::Middle => wire::mlb::InningHalf::Middle,
                    InningHalf::Bottom => wire::mlb::InningHalf::Bottom,
                    InningHalf::End => wire::mlb::InningHalf::End,
                },
            },
            count: wire::mlb::Count {
                balls: game.count.balls,
                strikes: game.count.strikes,
                outs: game.count.outs,
            },
            bases: wire::mlb::Bases {
                first: game.bases.first,
                second: game.bases.second,
                third: game.bases.third,
            },
            away: (&game.away).into(),
            home: (&game.home).into(),
            at_bat: game.at_bat.as_ref().map(|at_bat| wire::mlb::AtBat {
                pitcher: &at_bat.pitcher,
                batter: &at_bat.batter,
            }),
            last_play: (&game.last_play).into(),
        }
    }
}

impl<'a> From<&'a MlbPregameGame> for wire::mlb::Pregame<'a> {
    fn from(game: &'a MlbPregameGame) -> Self {
        Self {
            game_id: &game.game_id,
            start_time: game.start_time,
            venue: &game.venue,
            weather: game.weather.as_ref().map(|weather| wire::mlb::Weather {
                condition: &weather.condition,
                temperature: wire::clamp_temperature(weather.temperature),
            }),
            away: (&game.away).into(),
            home: (&game.home).into(),
        }
    }
}

impl<'a> From<&'a MlbPregameTeam> for wire::mlb::PregameTeam<'a> {
    fn from(team: &'a MlbPregameTeam) -> Self {
        Self {
            abbreviation: &team.abbreviation,
            colors: (&team.colors).into(),
            record: team.record.as_ref().map(Into::into),
            probable_pitcher: team.probable_pitcher.as_deref(),
        }
    }
}

impl<'a> From<&'a MlbFinalGame> for wire::mlb::Final<'a> {
    fn from(game: &'a MlbFinalGame) -> Self {
        Self {
            game_id: &game.game_id,
            innings_played: game.innings_played,
            away: final_team(&game.away),
            home: final_team(&game.home),
        }
    }
}

fn final_team(team: &MlbFinalTeam) -> wire::FinalTeam<'_> {
    wire::FinalTeam {
        abbreviation: &team.abbreviation,
        score: wire::saturate_score(team.score),
        colors: (&team.colors).into(),
        line_score: line_score(&team.line_score),
    }
}
