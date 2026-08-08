//! Soccer domain model → [`scoreboard_wire::soccer`] view. Layout lives in the
//! crate.
//!
//! `LastEvent::text` has no wire slot: the firmware composes its own line from
//! the kind, athlete and clock, so only those travel.

use scoreboard_wire as wire;

use super::types::{
    EventKind, SoccerFinalFlavor, SoccerFinalGame, SoccerFinalTeam, SoccerGame, SoccerLiveGame,
    SoccerPregameGame, SoccerPregameTeam,
};
use crate::shared::wire::encoded;

pub(crate) fn encode_game(game: &SoccerGame) -> Vec<u8> {
    encoded(128, |out| wire::soccer::encode(&game.into(), out))
}

impl<'a> From<&'a SoccerGame> for wire::soccer::Game<'a> {
    fn from(game: &'a SoccerGame) -> Self {
        match game {
            SoccerGame::Pregame(game) => Self::Pregame(game.into()),
            SoccerGame::Live(game) => Self::Live(game.into()),
            SoccerGame::Final(game) => Self::Final(game.into()),
        }
    }
}

impl<'a> From<&'a SoccerLiveGame> for wire::soccer::Live<'a> {
    fn from(game: &'a SoccerLiveGame) -> Self {
        Self {
            game_id: &game.game_id,
            half: game.half,
            clock_seconds: game.clock_seconds,
            on_break: game.on_break,
            away: (&game.away).into(),
            home: (&game.home).into(),
            last_event: game.last_event.as_ref().map(|event| wire::soccer::Event {
                kind: match event.kind {
                    EventKind::Goal => wire::soccer::EventKind::Goal,
                    EventKind::RedCard => wire::soccer::EventKind::RedCard,
                },
                side: event.team.map(Into::into),
                clock: &event.clock,
                athlete: &event.athlete,
            }),
            commentary: game
                .commentary
                .as_ref()
                .map(|commentary| wire::soccer::Commentary {
                    id: &commentary.id,
                    text: &commentary.text,
                }),
        }
    }
}

impl<'a> From<&'a SoccerPregameGame> for wire::soccer::Pregame<'a> {
    fn from(game: &'a SoccerPregameGame) -> Self {
        Self {
            game_id: &game.game_id,
            start_time: game.start_time,
            venue: &game.venue,
            away: (&game.away).into(),
            home: (&game.home).into(),
        }
    }
}

impl<'a> From<&'a SoccerPregameTeam> for wire::soccer::PregameTeam<'a> {
    fn from(team: &'a SoccerPregameTeam) -> Self {
        Self {
            abbreviation: &team.abbreviation,
            colors: (&team.colors).into(),
        }
    }
}

impl<'a> From<&'a SoccerFinalGame> for wire::soccer::Final<'a> {
    fn from(game: &'a SoccerFinalGame) -> Self {
        Self {
            game_id: &game.game_id,
            flavor: match game.flavor {
                SoccerFinalFlavor::FullTime => wire::soccer::FinalFlavor::FullTime,
                SoccerFinalFlavor::AfterExtraTime => wire::soccer::FinalFlavor::AfterExtraTime,
                SoccerFinalFlavor::AfterPenalties => wire::soccer::FinalFlavor::AfterPenalties,
            },
            away: (&game.away).into(),
            home: (&game.home).into(),
        }
    }
}

impl<'a> From<&'a SoccerFinalTeam> for wire::soccer::FinalTeam<'a> {
    fn from(team: &'a SoccerFinalTeam) -> Self {
        Self {
            abbreviation: &team.abbreviation,
            score: wire::saturate_score(team.score),
            colors: (&team.colors).into(),
            scorers: &team.scorers,
        }
    }
}
