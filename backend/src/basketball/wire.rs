//! NBA domain model → [`scoreboard_wire::basketball`] view. Layout lives in the
//! crate; how much of it a payload carries is the extension level the client
//! asked for (crate docs): 0 the frozen v2 bytes, 1 plus the pregame team
//! lines, 2 plus the live, pregame and final blocks.
//!
//! The live view borrows its makes as a slice of the crate's own `Make`
//! records, which the DTO does not hold, so [`encode_game`] builds that slice
//! first and lends it to the view for the one encode — hockey's goals, the
//! same way.

use scoreboard_wire as wire;

use super::types::{
    BasketballFinalGame, BasketballFinalTeam, BasketballGame, BasketballLeader, BasketballLedger,
    BasketballLiveGame, BasketballMake, BasketballNewestScore, BasketballPregameGame,
    BasketballPregameTeam, BasketballRun, BasketballTeamLedger,
};
use crate::shared::wire::{encoded, line_score};

pub(crate) fn encode_game(game: &BasketballGame, level: u8) -> Vec<u8> {
    match game {
        BasketballGame::Pregame(game) => {
            encode(&wire::basketball::Game::Pregame(game.into()), level)
        }
        BasketballGame::Live(game) => {
            let makes: Vec<wire::basketball::Make> = game.makes.iter().map(Into::into).collect();
            encode(&wire::basketball::Game::Live(live(game, &makes)), level)
        }
        BasketballGame::Final(game) => encode(&wire::basketball::Game::Final(game.into()), level),
    }
}

/// A 4th-quarter live game at `ext=2` runs about 400 bytes (NBA.md §5); the
/// capacity is a hint, the `Vec` grows past it.
fn encode(game: &wire::basketball::Game<'_>, level: u8) -> Vec<u8> {
    encoded(512, |out| wire::basketball::encode_at(game, level, out))
}

fn live<'a>(
    game: &'a BasketballLiveGame,
    makes: &'a [wire::basketball::Make],
) -> wire::basketball::Live<'a> {
    wire::basketball::Live {
        game_id: &game.game_id,
        period: game.period,
        phase: game.phase.into(),
        clock: &game.clock,
        away: (&game.away).into(),
        home: (&game.home).into(),
        last_play: game.last_play.as_ref().map(Into::into),
        ledger: game.ledger.map(Into::into),
        run: game.run.map(Into::into),
        newest_score: game.newest_score.as_ref().map(Into::into),
        makes: wire::basketball::Makes::new(makes),
    }
}

impl From<BasketballLedger> for wire::basketball::Ledger {
    fn from(ledger: BasketballLedger) -> Self {
        Self {
            away: ledger.away.into(),
            home: ledger.home.into(),
        }
    }
}

impl From<BasketballTeamLedger> for wire::basketball::TeamLedger {
    fn from(column: BasketballTeamLedger) -> Self {
        Self {
            fouls: column.fouls,
            in_bonus: column.in_bonus,
            timeouts: column.timeouts,
        }
    }
}

impl From<BasketballRun> for wire::basketball::Run {
    fn from(run: BasketballRun) -> Self {
        Self {
            side: run.side.into(),
            points: run.points,
        }
    }
}

impl<'a> From<&'a BasketballNewestScore> for wire::basketball::NewestScore<'a> {
    fn from(score: &'a BasketballNewestScore) -> Self {
        Self {
            side: score.side.into(),
            // The wire's 0 is "not known yet"; the DTO says so as an absence.
            scorer_points: score.scorer_points.unwrap_or(0),
            seq: score.seq,
            marked: score.marked,
            scorer: &score.scorer,
        }
    }
}

impl From<&BasketballMake> for wire::basketball::Make {
    fn from(make: &BasketballMake) -> Self {
        Self {
            side: make.side.into(),
            three: make.three,
            x: make.x,
            y: make.y,
        }
    }
}

impl<'a> From<&'a BasketballPregameGame> for wire::basketball::Pregame<'a> {
    fn from(game: &'a BasketballPregameGame) -> Self {
        Self {
            game_id: &game.game_id,
            start_time: game.start_time,
            venue: &game.venue,
            info_secondary: &game.info_secondary,
            away: (&game.away).into(),
            home: (&game.home).into(),
        }
    }
}

impl<'a> From<&'a BasketballPregameTeam> for wire::basketball::PregameTeam<'a> {
    fn from(team: &'a BasketballPregameTeam) -> Self {
        Self {
            abbreviation: &team.abbreviation,
            colors: (&team.colors).into(),
            record: team.record.as_ref().map(Into::into),
            line: team.line.as_deref(),
        }
    }
}

impl<'a> From<&'a BasketballFinalGame> for wire::basketball::Final<'a> {
    fn from(game: &'a BasketballFinalGame) -> Self {
        Self {
            game_id: &game.game_id,
            periods_played: game.periods_played,
            away: final_team(&game.away),
            home: final_team(&game.home),
            leaders: game
                .leaders
                .as_ref()
                .map(|leaders| wire::basketball::Leaders {
                    points: leader(&leaders.points),
                    rebounds: leader(&leaders.rebounds),
                    assists: leader(&leaders.assists),
                }),
            series: game.series.as_deref(),
            records: game
                .records
                .as_ref()
                .map(|records| wire::basketball::Records {
                    away: (&records.away).into(),
                    home: (&records.home).into(),
                }),
        }
    }
}

fn final_team(team: &BasketballFinalTeam) -> wire::FinalTeam<'_> {
    wire::FinalTeam {
        abbreviation: &team.abbreviation,
        score: wire::saturate_score(team.score),
        colors: (&team.colors).into(),
        line_score: line_score(&team.line_score),
    }
}

fn leader(leader: &BasketballLeader) -> wire::basketball::Leader<'_> {
    wire::basketball::Leader {
        side: leader.side.into(),
        value: leader.value,
        name: &leader.name,
    }
}
