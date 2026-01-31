//! Play generation with situational weights and realistic yard distributions.

use rand::rngs::StdRng;
use rand::Rng;

use crate::game::types::{Down, PlayType, Possession, Quarter};

use super::state::{LiveState, SimulatedPlay};

/// The outcome of generating a play.
pub struct PlayOutcome {
    pub play_type: PlayType,
    pub yards_gained: i8,
    pub clock_elapsed: u16,
    pub description: String,
    /// If this play is a turnover
    pub turnover: bool,
    /// If this play scores points (touchdown, field goal, safety)
    pub scoring: Option<ScoringPlay>,
}

#[derive(Debug, Clone, Copy)]
pub enum ScoringPlay {
    Touchdown,
    FieldGoal,
    Safety,
}

/// Generate the next play based on game situation.
pub fn generate_play(state: &mut LiveState) -> PlayOutcome {
    // Extract the values we need before borrowing rng mutably
    let kickoff_pending = state.kickoff_pending;
    let down = state.down;
    let distance = state.distance;
    let yard_line = state.yard_line;
    let quarter = state.quarter;
    let clock_seconds = state.clock_seconds;
    let possession = state.possession;
    let home_score = state.home_score;
    let away_score = state.away_score;

    // Handle kickoff situation
    if kickoff_pending {
        return generate_kickoff(&mut state.rng);
    }

    // Fourth down decisions
    if down == Down::Fourth {
        return generate_fourth_down_play(
            &mut state.rng,
            down,
            distance,
            yard_line,
            quarter,
            clock_seconds,
            possession,
            home_score,
            away_score,
        );
    }

    // Regular play selection based on situation
    let play_type = select_play_type(&mut state.rng, down, distance, quarter, clock_seconds, yard_line);

    match play_type {
        PlayType::Rush => generate_rush_play(&mut state.rng, yard_line),
        PlayType::PassReception | PlayType::PassIncompletion => {
            generate_pass_play(&mut state.rng, yard_line, distance)
        }
        PlayType::Sack => generate_sack_play(&mut state.rng),
        _ => generate_rush_play(&mut state.rng, yard_line), // Fallback
    }
}

/// Select play type based on down, distance, and field position.
fn select_play_type(
    rng: &mut StdRng,
    down: Down,
    distance: u8,
    quarter: Quarter,
    clock_seconds: u16,
    yard_line: u8,
) -> PlayType {
    let roll: u8 = rng.gen_range(0..100);

    // Two-minute drill: more passing
    let in_two_minute =
        clock_seconds <= 120 && matches!(quarter, Quarter::Second | Quarter::Fourth);

    // Red zone adjustments
    let in_red_zone = yard_line >= 80;

    // Situational weights
    match (down, distance) {
        // 1st down: balanced
        (Down::First, _) => {
            if in_two_minute {
                if roll < 75 {
                    PlayType::PassReception
                } else {
                    PlayType::Rush
                }
            } else if roll < 45 {
                PlayType::Rush
            } else {
                PlayType::PassReception
            }
        }

        // 2nd and short (1-3): run-heavy
        (Down::Second, 1..=3) => {
            if roll < 55 {
                PlayType::Rush
            } else {
                PlayType::PassReception
            }
        }

        // 2nd and medium (4-7): balanced
        (Down::Second, 4..=7) => {
            if roll < 45 {
                PlayType::Rush
            } else {
                PlayType::PassReception
            }
        }

        // 2nd and long (8+): pass-heavy
        (Down::Second, _) => {
            if roll < 30 {
                PlayType::Rush
            } else {
                PlayType::PassReception
            }
        }

        // 3rd and short (1-3): power run or quick pass
        (Down::Third, 1..=3) => {
            if in_red_zone && roll < 65 {
                PlayType::Rush
            } else if roll < 50 {
                PlayType::Rush
            } else {
                PlayType::PassReception
            }
        }

        // 3rd and medium (4-7): passing
        (Down::Third, 4..=7) => {
            if roll < 25 {
                PlayType::Rush
            } else {
                PlayType::PassReception
            }
        }

        // 3rd and long (8+): passing heavy
        (Down::Third, _) => {
            if roll < 15 {
                PlayType::Rush
            } else if roll < 90 {
                PlayType::PassReception
            } else {
                // Rare draw play
                PlayType::Rush
            }
        }

        // 4th down is handled separately
        (Down::Fourth, _) => PlayType::Punt, // Shouldn't reach here
    }
}

fn generate_kickoff(rng: &mut StdRng) -> PlayOutcome {
    // Most kickoffs result in touchback
    let touchback = rng.gen_bool(0.65);

    if touchback {
        PlayOutcome {
            play_type: PlayType::Kickoff,
            yards_gained: 0,
            clock_elapsed: 5,
            description: "Kickoff, touchback.".to_string(),
            turnover: false,
            scoring: None,
        }
    } else {
        let return_yards: i8 = rng.gen_range(15..35);
        PlayOutcome {
            play_type: PlayType::KickoffReturn,
            yards_gained: return_yards,
            clock_elapsed: rng.gen_range(5..10),
            description: format!("Kickoff returned for {} yards.", return_yards),
            turnover: false,
            scoring: None,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn generate_fourth_down_play(
    rng: &mut StdRng,
    _down: Down,
    distance: u8,
    yard_line: u8,
    quarter: Quarter,
    clock_seconds: u16,
    possession: Possession,
    home_score: u8,
    away_score: u8,
) -> PlayOutcome {
    // Field goal range (roughly inside the 35 yard line, i.e., yard_line >= 65)
    let in_fg_range = yard_line >= 55;

    // Punt range (not in FG range and not desperate)
    let should_punt = !in_fg_range && yard_line < 60;

    // Very short yardage might go for it
    let go_for_it = distance <= 2 && yard_line >= 50;

    // Late game desperation
    let desperate = clock_seconds < 120
        && matches!(quarter, Quarter::Fourth)
        && ((possession == Possession::Home && home_score < away_score)
            || (possession == Possession::Away && away_score < home_score));

    if in_fg_range && !desperate {
        // Field goal attempt
        let fg_distance = 100 - yard_line + 17; // Add 17 for end zone + line of scrimmage
        let success_rate = match fg_distance {
            0..=30 => 0.95,
            31..=40 => 0.85,
            41..=50 => 0.70,
            51..=55 => 0.55,
            _ => 0.40,
        };

        if rng.gen_bool(success_rate) {
            PlayOutcome {
                play_type: PlayType::FieldGoalGood,
                yards_gained: 0,
                clock_elapsed: 5,
                description: format!("{} yard field goal is GOOD!", fg_distance),
                turnover: false,
                scoring: Some(ScoringPlay::FieldGoal),
            }
        } else {
            PlayOutcome {
                play_type: PlayType::FieldGoalMissed,
                yards_gained: 0,
                clock_elapsed: 5,
                description: format!("{} yard field goal is NO GOOD.", fg_distance),
                turnover: true, // Opponent gets ball
                scoring: None,
            }
        }
    } else if should_punt && !desperate && !go_for_it {
        // Punt
        let punt_distance: i8 = rng.gen_range(35..55);
        PlayOutcome {
            play_type: PlayType::Punt,
            yards_gained: -punt_distance, // Negative because it goes to opponent
            clock_elapsed: rng.gen_range(5..10),
            description: format!("Punt for {} yards.", punt_distance),
            turnover: true,
            scoring: None,
        }
    } else {
        // Go for it!
        if distance <= 2 {
            // Short yardage - try a run
            generate_rush_play(rng, yard_line)
        } else {
            // Need more yards - pass
            generate_pass_play(rng, yard_line, distance)
        }
    }
}

fn generate_rush_play(rng: &mut StdRng, yard_line: u8) -> PlayOutcome {
    // Fumble chance (~1%)
    if rng.gen_bool(0.01) {
        let fumble_recovered_by_opponent = rng.gen_bool(0.5);
        if fumble_recovered_by_opponent {
            return PlayOutcome {
                play_type: PlayType::FumbleRecoveryOpponent,
                yards_gained: 0,
                clock_elapsed: rng.gen_range(5..10),
                description: "FUMBLE! Recovered by the defense.".to_string(),
                turnover: true,
                scoring: None,
            };
        } else {
            return PlayOutcome {
                play_type: PlayType::FumbleRecoveryOwn,
                yards_gained: rng.gen_range(-3..=0),
                clock_elapsed: rng.gen_range(20..35),
                description: "Fumble, recovered by the offense.".to_string(),
                turnover: false,
                scoring: None,
            };
        }
    }

    // Generate yards with realistic distribution
    let yards = generate_rush_yards(rng, yard_line);

    // Check for touchdown
    let would_score = yard_line as i16 + yards as i16 >= 100;
    if would_score {
        return PlayOutcome {
            play_type: PlayType::RushingTouchdown,
            yards_gained: (100 - yard_line) as i8,
            clock_elapsed: rng.gen_range(5..15),
            description: format!("TOUCHDOWN! {} yard rushing TD!", 100 - yard_line),
            turnover: false,
            scoring: Some(ScoringPlay::Touchdown),
        };
    }

    // Check for safety
    let would_safety = yard_line as i16 + yards as i16 <= 0;
    if would_safety {
        return PlayOutcome {
            play_type: PlayType::Safety,
            yards_gained: -(yard_line as i8),
            clock_elapsed: rng.gen_range(5..10),
            description: "SAFETY! Tackled in the end zone!".to_string(),
            turnover: true,
            scoring: Some(ScoringPlay::Safety),
        };
    }

    let clock = if yards < 0 || rng.gen_bool(0.3) {
        // Out of bounds or tackle for loss
        rng.gen_range(5..15)
    } else {
        rng.gen_range(25..45)
    };

    PlayOutcome {
        play_type: PlayType::Rush,
        yards_gained: yards,
        clock_elapsed: clock,
        description: if yards > 0 {
            format!("Rush for {} yards.", yards)
        } else if yards == 0 {
            "Rush for no gain.".to_string()
        } else {
            format!("Rush for a loss of {} yards.", -yards)
        },
        turnover: false,
        scoring: None,
    }
}

fn generate_pass_play(rng: &mut StdRng, yard_line: u8, distance: u8) -> PlayOutcome {
    // Sack chance (~7%)
    if rng.gen_bool(0.07) {
        return generate_sack_play(rng);
    }

    // Interception chance (~2.5%)
    if rng.gen_bool(0.025) {
        return PlayOutcome {
            play_type: PlayType::Interception,
            yards_gained: 0,
            clock_elapsed: rng.gen_range(5..10),
            description: "INTERCEPTED!".to_string(),
            turnover: true,
            scoring: None,
        };
    }

    // Incompletion chance (~35%)
    if rng.gen_bool(0.35) {
        return PlayOutcome {
            play_type: PlayType::PassIncompletion,
            yards_gained: 0,
            clock_elapsed: rng.gen_range(5..10),
            description: "Pass incomplete.".to_string(),
            turnover: false,
            scoring: None,
        };
    }

    // Completed pass
    let yards = generate_pass_yards(rng, yard_line, distance);

    // Check for touchdown
    let would_score = yard_line as i16 + yards as i16 >= 100;
    if would_score {
        return PlayOutcome {
            play_type: PlayType::PassingTouchdown,
            yards_gained: (100 - yard_line) as i8,
            clock_elapsed: rng.gen_range(5..15),
            description: format!("TOUCHDOWN! {} yard passing TD!", 100 - yard_line),
            turnover: false,
            scoring: Some(ScoringPlay::Touchdown),
        };
    }

    let clock = if rng.gen_bool(0.25) {
        // Out of bounds
        rng.gen_range(5..15)
    } else {
        rng.gen_range(25..45)
    };

    PlayOutcome {
        play_type: PlayType::PassReception,
        yards_gained: yards,
        clock_elapsed: clock,
        description: if yards > 0 {
            format!("Pass complete for {} yards.", yards)
        } else {
            format!("Pass complete for a loss of {} yards.", -yards)
        },
        turnover: false,
        scoring: None,
    }
}

fn generate_sack_play(rng: &mut StdRng) -> PlayOutcome {
    let yards_lost: i8 = rng.gen_range(3..=10);
    PlayOutcome {
        play_type: PlayType::Sack,
        yards_gained: -yards_lost,
        clock_elapsed: rng.gen_range(25..40),
        description: format!("SACKED for a loss of {} yards!", yards_lost),
        turnover: false,
        scoring: None,
    }
}

/// Generate rushing yards with realistic distribution.
fn generate_rush_yards(rng: &mut StdRng, yard_line: u8) -> i8 {
    let roll: u8 = rng.gen_range(0..100);

    // Distribution: -3 to +75 with mean ~4.3
    let yards = if roll < 15 {
        // Loss or no gain (15%)
        rng.gen_range(-3..=0)
    } else if roll < 55 {
        // Short gain 1-4 (40%)
        rng.gen_range(1..=4)
    } else if roll < 85 {
        // Medium gain 5-9 (30%)
        rng.gen_range(5..=9)
    } else if roll < 95 {
        // Big play 10-19 (10%)
        rng.gen_range(10..=19)
    } else {
        // Breakaway 20-75 (5%)
        rng.gen_range(20..=75)
    };

    // Cap at remaining yards to goal (can't gain more than needed for TD)
    let max_yards = (100 - yard_line) as i8;
    yards.min(max_yards)
}

/// Generate passing yards with realistic distribution.
fn generate_pass_yards(rng: &mut StdRng, yard_line: u8, distance: u8) -> i8 {
    let roll: u8 = rng.gen_range(0..100);

    // Adjust based on needed distance (tendency to throw for the first down)
    let target_boost = if distance >= 5 { 3 } else { 0 };

    let yards = if roll < 10 {
        // Screen/dump off or loss (10%)
        rng.gen_range(-2..=2)
    } else if roll < 35 {
        // Short pass 3-7 (25%)
        rng.gen_range(3..=7) + target_boost / 2
    } else if roll < 70 {
        // Medium pass 8-15 (35%)
        rng.gen_range(8..=15) + target_boost
    } else if roll < 90 {
        // Deep pass 16-30 (20%)
        rng.gen_range(16..=30)
    } else {
        // Big play 31-75 (10%)
        rng.gen_range(31..=75)
    };

    // Cap at remaining yards
    let max_yards = (100 - yard_line) as i8;
    yards.min(max_yards)
}

/// Convert PlayOutcome to SimulatedPlay.
pub fn outcome_to_play(outcome: &PlayOutcome) -> SimulatedPlay {
    SimulatedPlay {
        play_type: outcome.play_type,
        yards_gained: outcome.yards_gained,
        description: outcome.description.clone(),
        clock_elapsed: outcome.clock_elapsed,
    }
}
