//! NBA domain model → [`scoreboard_wire::nba`] view. Layout lives in the
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
    NbaFinalGame, NbaFinalTeam, NbaGame, NbaLeader, NbaLedger, NbaLiveGame, NbaMake,
    NbaNewestScore, NbaPregameGame, NbaPregameTeam, NbaRun, NbaTeamLedger,
};
use crate::shared::wire::{encoded, line_score};

pub(crate) fn encode_game(game: &NbaGame, level: u8) -> Vec<u8> {
    match game {
        NbaGame::Pregame(game) => encode(&wire::nba::Game::Pregame(game.into()), level),
        NbaGame::Live(game) => {
            let makes: Vec<wire::nba::Make> = game.makes.iter().map(Into::into).collect();
            encode(&wire::nba::Game::Live(live(game, &makes)), level)
        }
        NbaGame::Final(game) => encode(&wire::nba::Game::Final(game.into()), level),
    }
}

/// A 4th-quarter live game at `ext=2` runs about 400 bytes (NBA.md §5); the
/// capacity is a hint, the `Vec` grows past it.
fn encode(game: &wire::nba::Game<'_>, level: u8) -> Vec<u8> {
    encoded(512, |out| wire::nba::encode_at(game, level, out))
}

fn live<'a>(game: &'a NbaLiveGame, makes: &'a [wire::nba::Make]) -> wire::nba::Live<'a> {
    wire::nba::Live {
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
        makes: wire::nba::Makes::new(makes),
    }
}

impl From<NbaLedger> for wire::nba::Ledger {
    fn from(ledger: NbaLedger) -> Self {
        Self {
            away: ledger.away.into(),
            home: ledger.home.into(),
        }
    }
}

impl From<NbaTeamLedger> for wire::nba::TeamLedger {
    fn from(column: NbaTeamLedger) -> Self {
        Self {
            fouls: column.fouls,
            in_bonus: column.in_bonus,
            timeouts: column.timeouts,
        }
    }
}

impl From<NbaRun> for wire::nba::Run {
    fn from(run: NbaRun) -> Self {
        Self {
            side: run.side.into(),
            points: run.points,
        }
    }
}

impl<'a> From<&'a NbaNewestScore> for wire::nba::NewestScore<'a> {
    fn from(score: &'a NbaNewestScore) -> Self {
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

impl From<&NbaMake> for wire::nba::Make {
    fn from(make: &NbaMake) -> Self {
        Self {
            side: make.side.into(),
            three: make.three,
            x: make.x,
            y: make.y,
        }
    }
}

impl<'a> From<&'a NbaPregameGame> for wire::nba::Pregame<'a> {
    fn from(game: &'a NbaPregameGame) -> Self {
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

impl<'a> From<&'a NbaPregameTeam> for wire::nba::PregameTeam<'a> {
    fn from(team: &'a NbaPregameTeam) -> Self {
        Self {
            abbreviation: &team.abbreviation,
            colors: (&team.colors).into(),
            record: team.record.as_ref().map(Into::into),
            line: team.line.as_deref(),
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
            leaders: game.leaders.as_ref().map(|leaders| wire::nba::Leaders {
                points: leader(&leaders.points),
                rebounds: leader(&leaders.rebounds),
                assists: leader(&leaders.assists),
            }),
            series: game.series.as_deref(),
            records: game.records.as_ref().map(|records| wire::nba::Records {
                away: (&records.away).into(),
                home: (&records.home).into(),
            }),
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

fn leader(leader: &NbaLeader) -> wire::nba::Leader<'_> {
    wire::nba::Leader {
        side: leader.side.into(),
        value: leader.value,
        name: &leader.name,
    }
}
