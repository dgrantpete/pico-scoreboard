//! NHL domain model → [`scoreboard_wire::hockey`] view. Layout lives in the
//! crate; hockey defines no `ext=1` extension (its endpoint postdates the
//! frozen format, so every field rides plain v2).
//!
//! The live view borrows its goals as a slice of the crate's own `Goal`
//! records, which the DTO does not hold, so [`encode_game`] builds that slice
//! first and lends it to the view for the one encode.

use scoreboard_wire as wire;

use super::types::{
    GoalTag, HockeyFinalGame, HockeyFinalTeam, HockeyGame, HockeyGoal, HockeyLiveGame,
    HockeyPregameGame, HockeyPregameTeam, HockeyShootout, HockeyShots, HockeyStar, ShootoutAttempt,
    StrengthSituation,
};
use crate::shared::wire::{encoded, line_score};

pub(crate) fn encode_game(game: &HockeyGame) -> Vec<u8> {
    match game {
        HockeyGame::Pregame(game) => encode(&wire::hockey::Game::Pregame(game.into())),
        HockeyGame::Live(game) => {
            let goals: Vec<wire::hockey::Goal> = game.goals.iter().map(Into::into).collect();
            encode(&wire::hockey::Game::Live(live(game, &goals)))
        }
        HockeyGame::Final(game) => encode(&wire::hockey::Game::Final(game.into())),
    }
}

fn encode(game: &wire::hockey::Game<'_>) -> Vec<u8> {
    encoded(256, |out| wire::hockey::encode(game, out))
}

fn live<'a>(game: &'a HockeyLiveGame, goals: &'a [wire::hockey::Goal]) -> wire::hockey::Live<'a> {
    wire::hockey::Live {
        game_id: &game.game_id,
        period: game.period,
        phase: game.phase.into(),
        clock: &game.clock,
        playoffs: game.playoffs,
        away: (&game.away).into(),
        home: (&game.home).into(),
        shots: game.shots.map(Into::into),
        strength: game.strength.map(|strength| wire::hockey::Strength {
            kind: match strength.situation {
                StrengthSituation::Even => wire::hockey::StrengthKind::Even,
                StrengthSituation::PowerPlay { side } => {
                    wire::hockey::StrengthKind::PowerPlay(side.into())
                }
                StrengthSituation::Matched => wire::hockey::StrengthKind::Matched,
                StrengthSituation::EmptyNet { side } => {
                    wire::hockey::StrengthKind::EmptyNet(side.into())
                }
            },
            away_skaters: strength.away_skaters,
            home_skaters: strength.home_skaters,
        }),
        goals: wire::hockey::Goals::new(goals),
        last_scorer: &game.last_scorer,
        shootout: game.shootout.as_ref().map(Into::into),
        last_play: game.last_play.as_ref().map(Into::into),
    }
}

impl From<HockeyShots> for wire::hockey::Shots {
    fn from(shots: HockeyShots) -> Self {
        Self {
            away: shots.away,
            home: shots.home,
        }
    }
}

impl From<&HockeyGoal> for wire::hockey::Goal {
    fn from(goal: &HockeyGoal) -> Self {
        Self {
            side: goal.side.into(),
            tag: match goal.tag {
                GoalTag::Goal => wire::hockey::GoalTag::Goal,
                GoalTag::PowerPlay => wire::hockey::GoalTag::PowerPlay,
                GoalTag::ShortHanded => wire::hockey::GoalTag::ShortHanded,
                GoalTag::EmptyNet => wire::hockey::GoalTag::EmptyNet,
            },
            period: goal.period,
            elapsed: goal.elapsed,
            coordinate: goal.coordinate.map(|coordinate| wire::hockey::Coordinate {
                x: coordinate.x,
                y: coordinate.y,
            }),
        }
    }
}

impl From<&HockeyShootout> for wire::hockey::Shootout {
    fn from(shootout: &HockeyShootout) -> Self {
        Self {
            away: attempts(&shootout.away),
            home: attempts(&shootout.home),
            next_shooter: shootout.next_shooter.map(Into::into),
        }
    }
}

/// The attempts folded back into a count and a goal mask. Past the mask's
/// width the encoder would clamp anyway; capping here keeps the shift in
/// range.
fn attempts(attempts: &[ShootoutAttempt]) -> wire::hockey::ShootoutAttempts {
    let kept = &attempts[..attempts.len().min(wire::hockey::MAX_SHOOTOUT_ATTEMPTS)];
    wire::hockey::ShootoutAttempts {
        taken: kept.len() as u8,
        scored: kept
            .iter()
            .enumerate()
            .filter(|(_, attempt)| attempt.scored)
            .fold(0, |mask, (index, _)| mask | 1 << index),
    }
}

impl<'a> From<&'a HockeyPregameGame> for wire::hockey::Pregame<'a> {
    fn from(game: &'a HockeyPregameGame) -> Self {
        Self {
            game_id: &game.game_id,
            start_time: game.start_time,
            venue: &game.venue,
            info_secondary: &game.info_secondary,
            playoffs: game.playoffs,
            away: (&game.away).into(),
            home: (&game.home).into(),
        }
    }
}

impl<'a> From<&'a HockeyPregameTeam> for wire::hockey::PregameTeam<'a> {
    fn from(team: &'a HockeyPregameTeam) -> Self {
        Self {
            abbreviation: &team.abbreviation,
            colors: (&team.colors).into(),
            record: team.record.as_ref().map(|record| wire::hockey::Record {
                wins: record.wins,
                losses: record.losses,
                ot_losses: record.ot_losses,
            }),
            goalie: team.goalie.as_deref(),
        }
    }
}

impl<'a> From<&'a HockeyFinalGame> for wire::hockey::Final<'a> {
    fn from(game: &'a HockeyFinalGame) -> Self {
        Self {
            game_id: &game.game_id,
            periods_played: game.periods_played,
            playoffs: game.playoffs,
            away: final_team(&game.away),
            home: final_team(&game.home),
            shots: game.shots.map(Into::into),
            stars: game
                .stars
                .as_ref()
                .map(|stars| [star(&stars.first), star(&stars.second), star(&stars.third)]),
            series: game.series.as_deref(),
        }
    }
}

fn final_team(team: &HockeyFinalTeam) -> wire::FinalTeam<'_> {
    wire::FinalTeam {
        abbreviation: &team.abbreviation,
        score: wire::saturate_score(team.score),
        colors: (&team.colors).into(),
        line_score: line_score(&team.line_score),
    }
}

fn star(star: &HockeyStar) -> wire::hockey::Star<'_> {
    wire::hockey::Star {
        name: &star.name,
        side: star.side.map(Into::into),
    }
}
