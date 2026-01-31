//! Simulation engine: time advancement, quarter transitions, state management.

use crate::game::types::Quarter;

use super::drives::apply_play_outcome;
use super::plays::{generate_play, outcome_to_play};
use super::state::LiveState;

/// Advance the game state to the current wall-clock time.
///
/// This is called when a game is fetched, to simulate all plays
/// that should have occurred since the last access.
pub fn advance_to_now(state: &mut LiveState) {
    let real_elapsed = state.game_start_instant.elapsed();
    let target_game_seconds = (real_elapsed.as_secs_f64() * state.time_scale) as u64;

    // Only advance if we're behind the target time
    if target_game_seconds > state.simulated_game_seconds {
        advance_to_target(state, target_game_seconds);
    }
}

/// Advance the game until we've simulated up to the target game-seconds.
fn advance_to_target(state: &mut LiveState, target_game_seconds: u64) {
    // Cap to prevent runaway simulation
    const MAX_GAME_SECONDS: u64 = 3600 * 4; // 4 hours of game time max
    let target = target_game_seconds.min(state.simulated_game_seconds + MAX_GAME_SECONDS);

    while state.simulated_game_seconds < target && !is_game_over(state) {
        // Handle halftime
        if is_halftime(state) {
            handle_halftime(state);
            continue;
        }

        // Handle quarter transitions
        if state.clock_seconds == 0 {
            if !handle_quarter_end(state) {
                // Game over
                break;
            }
            continue;
        }

        // Generate and execute a play
        let outcome = generate_play(state);
        let play_duration = outcome.clock_elapsed.min(state.clock_seconds);

        // Apply the play
        apply_play_outcome(state, &outcome);

        // Record the play
        let play = outcome_to_play(&outcome);
        state.last_play = Some(play.clone());
        state.play_history.push(play);

        // Update game clock
        if should_clock_run(&outcome) {
            state.clock_seconds = state.clock_seconds.saturating_sub(play_duration);
        } else {
            // Clock stopped - minimal time passes
            state.clock_seconds = state.clock_seconds.saturating_sub(5.min(play_duration));
        }

        // Update clock running status for display
        state.clock_running = should_clock_run(&outcome);

        // Track ACTUAL simulated game time (the full play duration)
        state.simulated_game_seconds += play_duration as u64;

        // Handle two-minute warning
        if state.clock_seconds <= 120
            && state.clock_seconds > 115
            && matches!(state.quarter, Quarter::Second | Quarter::Fourth)
        {
            state.clock_running = false;
        }
    }
}

/// Check if the game is over.
fn is_game_over(state: &LiveState) -> bool {
    state.is_game_over()
}

/// Check if we're at halftime.
fn is_halftime(state: &LiveState) -> bool {
    state.quarter == Quarter::Second && state.clock_seconds == 0
}

/// Handle halftime transition.
fn handle_halftime(state: &mut LiveState) {
    state.quarter = Quarter::Third;
    state.clock_seconds = 900; // 15:00

    // Second half kickoff - team that didn't receive first gets it
    // For simplicity, just flip possession
    state.possession = match state.possession {
        crate::game::types::Possession::Home => crate::game::types::Possession::Away,
        crate::game::types::Possession::Away => crate::game::types::Possession::Home,
    };
    state.kickoff_pending = true;

    // Reset timeouts
    state.home_timeouts = 3;
    state.away_timeouts = 3;
}

/// Handle end of quarter. Returns false if game is over.
fn handle_quarter_end(state: &mut LiveState) -> bool {
    match state.quarter {
        Quarter::First => {
            state.quarter = Quarter::Second;
            state.clock_seconds = 900;
            true
        }
        Quarter::Second => {
            // Halftime is handled separately
            true
        }
        Quarter::Third => {
            state.quarter = Quarter::Fourth;
            state.clock_seconds = 900;
            true
        }
        Quarter::Fourth => {
            // Check for overtime
            if state.home_score == state.away_score {
                state.quarter = Quarter::Overtime;
                state.clock_seconds = 600; // 10-minute OT in regular season
                state.kickoff_pending = true;
                // Reset timeouts for OT
                state.home_timeouts = 2;
                state.away_timeouts = 2;
                true
            } else {
                // Game over
                false
            }
        }
        Quarter::Overtime => {
            // In NFL OT, first score wins (simplified)
            // If still tied, go to double OT
            if state.home_score == state.away_score {
                state.quarter = Quarter::DoubleOvertime;
                state.clock_seconds = 600;
                true
            } else {
                false
            }
        }
        Quarter::DoubleOvertime => {
            // Game over, even if tied (tie game)
            false
        }
    }
}

/// Determine if clock should be running based on play outcome.
fn should_clock_run(outcome: &super::plays::PlayOutcome) -> bool {
    use crate::game::types::PlayType;

    // Clock stops for:
    // - Incomplete passes
    // - Out of bounds (handled probabilistically in play generation)
    // - Scores
    // - Turnovers
    // - Penalties
    // - Timeouts

    if outcome.scoring.is_some() {
        return false;
    }

    if outcome.turnover {
        return false;
    }

    match outcome.play_type {
        PlayType::PassIncompletion
        | PlayType::Interception
        | PlayType::Timeout
        | PlayType::OfficialTimeout
        | PlayType::TwoMinuteWarning
        | PlayType::Penalty
        | PlayType::Punt
        | PlayType::Kickoff
        | PlayType::KickoffReturn
        | PlayType::FieldGoalGood
        | PlayType::FieldGoalMissed
        | PlayType::EndPeriod => false,

        // These generally keep clock running (in-bounds tackle)
        PlayType::Rush
        | PlayType::PassReception
        | PlayType::Sack
        | PlayType::FumbleRecoveryOwn => true,

        _ => true,
    }
}
