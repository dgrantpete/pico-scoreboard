use crate::error::AppError;
use crate::espn::types::HomeAway;
use crate::shared::team::{TeamColors, parse_hex_rgb};

use super::types::{
    AtBat, Bases, Count, EspnCompetitor, EspnSituation, Inning, InningHalf, LastPlay, LiveGame,
    TeamState,
};

/// Parse the inning half from ESPN's `shortDetail` prefix.
///
/// Returns `None` for prefixes outside the four in-play states — e.g.
/// "Delayed", "Suspended", or "Rain Delay". Those games are technically
/// `state: "in"` but have nothing displayable; callers treat them as
/// not-found so the firmware skips the slot instead of erroring.
fn parse_inning_half(short_detail: &str) -> Option<InningHalf> {
    match short_detail.split_whitespace().next().unwrap_or("") {
        "Top" => Some(InningHalf::Top),
        "Mid" => Some(InningHalf::Middle),
        "Bot" => Some(InningHalf::Bottom),
        "End" => Some(InningHalf::End),
        other => {
            tracing::warn!(
                short_detail = %short_detail,
                prefix = %other,
                "ESPN shortDetail has non-inning prefix (delay/suspension?) — treating game as not displayable"
            );
            None
        }
    }
}

/// Build a `TeamState` from an ESPN competitor, parsing the score and colors.
fn competitor_to_team_state(c: &EspnCompetitor) -> Result<TeamState, AppError> {
    let score = c.score.parse::<u32>().map_err(|e| {
        let json_path = format!(
            "events[?].competitions[0].competitors[{}].score",
            c.team.abbreviation
        );
        tracing::error!(
            json_path = %json_path,
            team = %c.team.abbreviation,
            raw_score = %c.score,
            error = %e,
            "ESPN competitor score failed to parse as u32"
        );
        AppError::EspnDeserialize {
            url: String::new(),
            json_path,
            message: format!("invalid score '{}': {}", c.score, e),
        }
    })?;

    let primary = parse_hex_rgb(&c.team.color, &c.team.abbreviation).inspect_err(|e| {
        tracing::error!(
            team = %c.team.abbreviation,
            raw_color = %c.team.color,
            error = ?e,
            "ESPN primary team color failed to parse"
        );
    })?;
    let alternate = parse_hex_rgb(&c.team.alternate_color, &c.team.abbreviation).inspect_err(
        |e| {
            tracing::error!(
                team = %c.team.abbreviation,
                raw_color = %c.team.alternate_color,
                error = ?e,
                "ESPN alternate team color failed to parse"
            );
        },
    )?;

    Ok(TeamState {
        abbreviation: c.team.abbreviation.clone(),
        score,
        colors: TeamColors { primary, alternate },
    })
}

/// Transform a live competition into a `LiveGame`. Callers must pattern-match
/// `EspnCompetition::Live` at the call site, so no runtime state check lives
/// inside this function.
pub(crate) fn live_competition_to_game(
    event_id: String,
    competitors: [EspnCompetitor; 2],
    situation: EspnSituation,
    period: u8,
    short_detail: String,
) -> Result<LiveGame, AppError> {
    // A live game in a non-inning state (rain delay, suspension) has nothing
    // to display — surface it exactly like a game that isn't live.
    let Some(half) = parse_inning_half(&short_detail) else {
        return Err(AppError::GameNotFound(event_id));
    };

    let [a, b] = competitors;
    let (home, away) = match (a.home_away, b.home_away) {
        (HomeAway::Home, HomeAway::Away) => (competitor_to_team_state(&a)?, competitor_to_team_state(&b)?),
        (HomeAway::Away, HomeAway::Home) => (competitor_to_team_state(&b)?, competitor_to_team_state(&a)?),
        _ => {
            let json_path = format!(
                "events[?].competitions[0].competitors (event_id={})",
                event_id
            );
            tracing::error!(
                json_path = %json_path,
                event_id = %event_id,
                first_team = %a.team.abbreviation,
                second_team = %b.team.abbreviation,
                "ESPN competitors did not split into exactly one home and one away"
            );
            return Err(AppError::EspnDeserialize {
                url: String::new(),
                json_path,
                message: format!(
                    "expected one home and one away competitor, got {}/{}",
                    a.team.abbreviation, b.team.abbreviation
                ),
            });
        }
    };

    let count = Count {
        balls: situation.balls,
        strikes: situation.strikes,
        outs: situation.outs,
    };
    let bases = Bases {
        first: situation.on_first,
        second: situation.on_second,
        third: situation.on_third,
    };
    let at_bat = match (situation.pitcher, situation.batter) {
        (Some(pitcher), Some(batter)) => Some(AtBat {
            pitcher: pitcher.athlete.short_name,
            batter: batter.athlete.short_name,
        }),
        _ => None,
    };
    let last_play = LastPlay {
        id: situation.last_play.id,
        text: situation.last_play.text,
    };

    let inning = Inning {
        number: period,
        half,
    };

    Ok(LiveGame {
        game_id: event_id,
        inning,
        home,
        away,
        count,
        bases,
        at_bat,
        last_play,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_inning_half_accepts_in_play_prefixes() {
        assert!(matches!(parse_inning_half("Top 3rd"), Some(InningHalf::Top)));
        assert!(matches!(parse_inning_half("Mid 5th"), Some(InningHalf::Middle)));
        assert!(matches!(parse_inning_half("Bot 9th"), Some(InningHalf::Bottom)));
        assert!(matches!(parse_inning_half("End 1st"), Some(InningHalf::End)));
    }

    #[test]
    fn parse_inning_half_rejects_delay_states() {
        assert!(parse_inning_half("Delayed").is_none());
        assert!(parse_inning_half("Rain Delay").is_none());
        assert!(parse_inning_half("Suspended").is_none());
        assert!(parse_inning_half("").is_none());
    }
}
