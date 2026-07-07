use crate::error::AppError;
use crate::espn::types::HomeAway;
use crate::shared::team::{TeamColors, parse_hex_rgb};

use super::types::{
    AtBat, Bases, Count, EspnCompetitor, EspnSituation, Inning, InningHalf, LastPlay, LiveGame,
    PregameGame, TeamState,
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

/// Split two competitors into (home, away) by their `homeAway` markers.
fn split_home_away(
    event_id: &str,
    competitors: [EspnCompetitor; 2],
) -> Result<(TeamState, TeamState), AppError> {
    let [a, b] = competitors;
    match (a.home_away, b.home_away) {
        (HomeAway::Home, HomeAway::Away) => {
            Ok((competitor_to_team_state(&a)?, competitor_to_team_state(&b)?))
        }
        (HomeAway::Away, HomeAway::Home) => {
            Ok((competitor_to_team_state(&b)?, competitor_to_team_state(&a)?))
        }
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
            Err(AppError::EspnDeserialize {
                url: String::new(),
                json_path,
                message: format!(
                    "expected one home and one away competitor, got {}/{}",
                    a.team.abbreviation, b.team.abbreviation
                ),
            })
        }
    }
}

/// Transform a pre-game competition into a `PregameGame`.
///
/// `date` is the event-level scheduled start (ISO 8601), passed in by the
/// caller since it lives on the event, not the competition. Not yet called
/// by a handler — the games endpoints are live-only for now — but modeled
/// and tested so exposing pregame data later is a handler-only change.
/// Drop the cfg_attr when that handler lands.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn pregame_competition_to_game(
    event_id: String,
    date: String,
    competitors: [EspnCompetitor; 2],
) -> Result<PregameGame, AppError> {
    let (home, away) = split_home_away(&event_id, competitors)?;
    Ok(PregameGame {
        game_id: event_id,
        date,
        home,
        away,
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

    let (home, away) = split_home_away(&event_id, competitors)?;

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

    fn competitor(abbrev: &str, home_away: &str) -> EspnCompetitor {
        serde_json::from_str(&format!(
            r#"{{"homeAway":"{home_away}","score":"0",
                "team":{{"abbreviation":"{abbrev}","color":"0C2340","alternateColor":"BD3039"}}}}"#
        ))
        .expect("test competitor json parses")
    }

    #[test]
    fn pre_competition_deserializes_through_du_and_transforms() {
        use super::super::types::EspnCompetition;

        let competition: EspnCompetition = serde_json::from_str(
            r#"{"competitors":[
                {"homeAway":"away","score":"0","team":{"abbreviation":"NYY","color":"003087","alternateColor":"E4002C"}},
                {"homeAway":"home","score":"0","team":{"abbreviation":"BOS","color":"0C2340","alternateColor":"BD3039"}}
            ],"status":{"type":{"state":"pre","shortDetail":"7/7 - 7:10 PM EDT"},"period":0}}"#,
        )
        .expect("pre competition parses through the DU");
        let EspnCompetition::PreGame { competitors } = competition else {
            panic!("state 'pre' must map to the PreGame variant");
        };
        let game = pregame_competition_to_game(
            "401570001".to_string(),
            "2026-07-07T23:10Z".to_string(),
            competitors,
        )
        .unwrap();
        assert_eq!(game.away.abbreviation, "NYY");
        assert_eq!(game.away.colors.primary, 0x003087);
    }

    #[test]
    fn pregame_transform_splits_home_away_and_parses_colors() {
        let game = pregame_competition_to_game(
            "401570001".to_string(),
            "2026-07-07T23:10Z".to_string(),
            [competitor("NYY", "away"), competitor("BOS", "home")],
        )
        .unwrap();
        assert_eq!(game.game_id, "401570001");
        assert_eq!(game.date, "2026-07-07T23:10Z");
        assert_eq!(game.home.abbreviation, "BOS");
        assert_eq!(game.away.abbreviation, "NYY");
        assert_eq!(game.home.colors.primary, 0x0C2340);
        assert_eq!(game.home.score, 0);
    }

    #[test]
    fn pregame_transform_rejects_two_home_teams() {
        let result = pregame_competition_to_game(
            "401570001".to_string(),
            "2026-07-07T23:10Z".to_string(),
            [competitor("NYY", "home"), competitor("BOS", "home")],
        );
        let Err(err) = result else {
            panic!("two home teams must not transform");
        };
        assert!(matches!(err, AppError::EspnDeserialize { .. }));
    }
}
