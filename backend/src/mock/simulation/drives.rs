//! Drive logic: scoring, turnovers, possession changes, down/distance updates.

use rand::Rng;

use crate::game::types::{Down, PlayType, Possession};

use super::plays::{PlayOutcome, ScoringPlay};
use super::state::LiveState;

/// Apply the outcome of a play to the game state.
pub fn apply_play_outcome(state: &mut LiveState, outcome: &PlayOutcome) {
    // Handle scoring plays first
    if let Some(scoring) = &outcome.scoring {
        match scoring {
            ScoringPlay::Touchdown => handle_touchdown(state),
            ScoringPlay::FieldGoal => handle_field_goal(state),
            ScoringPlay::Safety => handle_safety(state),
        }
        return;
    }

    // Handle turnovers
    if outcome.turnover {
        handle_turnover(state, outcome);
        return;
    }

    // Handle kickoff
    if outcome.play_type == PlayType::Kickoff || outcome.play_type == PlayType::KickoffReturn {
        handle_kickoff_return(state, outcome);
        return;
    }

    // Regular play - update field position and down/distance
    update_field_position(state, outcome);
}

fn handle_touchdown(state: &mut LiveState) {
    // Add 6 points
    add_score(state, 6);

    // Extra point attempt (simplified: 94% success rate)
    if state.rng.gen_bool(0.94) {
        add_score(state, 1);
    }

    // Set up kickoff
    setup_kickoff_after_score(state);
}

fn handle_field_goal(state: &mut LiveState) {
    // Add 3 points
    add_score(state, 3);

    // Set up kickoff
    setup_kickoff_after_score(state);
}

fn handle_safety(state: &mut LiveState) {
    // Safety scores 2 points for the DEFENSE
    let scoring_team = opponent(state.possession);
    if scoring_team == Possession::Home {
        state.home_score += 2;
    } else {
        state.away_score += 2;
    }

    // After a safety, the team that was scored on kicks off (free kick)
    // This is a bit unusual - the team that got the safety kicks to the team that scored
    state.possession = opponent(state.possession);
    state.kickoff_pending = true;
    state.down = Down::First;
    state.distance = 10;
    state.yard_line = 20; // Free kick from own 20
}

fn handle_turnover(state: &mut LiveState, outcome: &PlayOutcome) {
    match outcome.play_type {
        PlayType::Interception => {
            // Change possession, opponent starts at their ~30-40
            flip_possession(state);
            state.yard_line = state.rng.gen_range(25..40);
            state.down = Down::First;
            state.distance = 10;
        }
        PlayType::FumbleRecoveryOpponent => {
            // Change possession at current spot
            flip_possession(state);
            // Flip field position
            state.yard_line = 100 - state.yard_line;
            state.down = Down::First;
            state.distance = 10;
        }
        PlayType::Punt => {
            // Change possession, net punt yardage
            flip_possession(state);
            // Flip and apply punt distance
            let punt_distance = (-outcome.yards_gained) as u8;
            state.yard_line = 100 - (state.yard_line + punt_distance).min(95);
            // Apply return yards (simple: 5-15 yards)
            let return_yards = state.rng.gen_range(5..15);
            state.yard_line = (state.yard_line + return_yards).min(99);
            state.down = Down::First;
            state.distance = 10;
        }
        PlayType::FieldGoalMissed | PlayType::BlockedFieldGoal => {
            // Opponent gets ball at spot of kick (roughly)
            flip_possession(state);
            state.yard_line = 100 - state.yard_line;
            state.down = Down::First;
            state.distance = 10;
        }
        _ => {
            // Turnover on downs
            flip_possession(state);
            state.yard_line = 100 - state.yard_line;
            state.down = Down::First;
            state.distance = 10;
        }
    }
}

fn handle_kickoff_return(state: &mut LiveState, outcome: &PlayOutcome) {
    state.kickoff_pending = false;

    if outcome.play_type == PlayType::Kickoff {
        // Touchback - start at 25
        state.yard_line = 25;
    } else {
        // Return - start at return spot
        // Kickoffs start from the 35, go to ~end zone, and return
        // So return yards from outcome is where they end up from their own goal line
        state.yard_line = outcome.yards_gained.max(20) as u8;
    }

    state.down = Down::First;
    state.distance = 10;
}

fn update_field_position(state: &mut LiveState, outcome: &PlayOutcome) {
    // Update yard line
    let new_yard_line = (state.yard_line as i16 + outcome.yards_gained as i16).clamp(1, 99) as u8;
    let yards_gained = outcome.yards_gained;

    // Check if first down was achieved
    let gained_first_down = yards_gained >= state.distance as i8;

    if gained_first_down {
        // First down!
        state.down = Down::First;
        state.distance = 10.min(100 - new_yard_line);
    } else if yards_gained >= 0 {
        // Positive yards but not enough for first down
        state.distance = (state.distance as i8 - yards_gained).max(1) as u8;
        state.down = next_down(state.down);

        // Check for turnover on downs
        if state.down == Down::First && !gained_first_down {
            // This means we cycled past Fourth - turnover on downs
            handle_turnover_on_downs(state);
            return;
        }
    } else {
        // Loss of yards
        state.distance = (state.distance as i8 - yards_gained).min(99) as u8;
        state.down = next_down(state.down);

        if state.down == Down::First {
            handle_turnover_on_downs(state);
            return;
        }
    }

    state.yard_line = new_yard_line;
}

fn handle_turnover_on_downs(state: &mut LiveState) {
    flip_possession(state);
    state.yard_line = 100 - state.yard_line;
    state.down = Down::First;
    state.distance = 10;
}

fn setup_kickoff_after_score(state: &mut LiveState) {
    // Opponent receives
    state.possession = opponent(state.possession);
    state.kickoff_pending = true;
    state.yard_line = 35; // Kickoff from 35
    state.down = Down::First;
    state.distance = 10;
}

fn add_score(state: &mut LiveState, points: u8) {
    if state.possession == Possession::Home {
        state.home_score = state.home_score.saturating_add(points);
    } else {
        state.away_score = state.away_score.saturating_add(points);
    }
}

fn flip_possession(state: &mut LiveState) {
    state.possession = opponent(state.possession);
}

fn opponent(possession: Possession) -> Possession {
    match possession {
        Possession::Home => Possession::Away,
        Possession::Away => Possession::Home,
    }
}

fn next_down(down: Down) -> Down {
    match down {
        Down::First => Down::Second,
        Down::Second => Down::Third,
        Down::Third => Down::Fourth,
        Down::Fourth => Down::First, // Will trigger turnover check
    }
}
