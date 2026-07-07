use crate::error::AppError;
use crate::shared::team::{TeamColors, order_home_away, parse_hex_rgb};

use super::types::{
    EspnCompetitor, EspnDetail, LastEvent, Side, SoccerGame, SoccerTeam, SoccerTeamState,
};

/// Parse a competitor's colors, shared by the pre and live builders.
fn competitor_colors(c: &EspnCompetitor) -> Result<TeamColors, AppError> {
    let primary = parse_hex_rgb(&c.team.color, &c.team.abbreviation)?;
    let alternate = parse_hex_rgb(&c.team.alternate_color, &c.team.abbreviation)?;
    Ok(TeamColors { primary, alternate })
}

fn competitor_to_team_state(c: &EspnCompetitor) -> Result<SoccerTeamState, AppError> {
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
    Ok(SoccerTeamState {
        abbreviation: c.team.abbreviation.clone(),
        score,
        colors: competitor_colors(c)?,
    })
}

/// The most recent goal or red card (yellow cards are ticker noise for a
/// 128x64 panel). Side is matched via the detail's team id against the two
/// competitors; an unmatched or missing id yields `team: None`.
fn last_event(details: &[EspnDetail], home_team_id: &str, away_team_id: &str) -> Option<LastEvent> {
    details
        .iter()
        .filter(|d| d.scoring_play || d.red_card)
        .max_by(|a, b| {
            a.clock
                .value
                .partial_cmp(&b.clock.value)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|d| {
            let text = match d.athletes_involved.first() {
                Some(athlete) => format!("{} - {}", d.r#type.text, athlete.short_name),
                None => d.r#type.text.clone(),
            };
            let team = d.team.as_ref().and_then(|t| {
                if t.id == home_team_id {
                    Some(Side::Home)
                } else if t.id == away_team_id {
                    Some(Side::Away)
                } else {
                    None
                }
            });
            LastEvent {
                text,
                clock: d.clock.display_value.clone(),
                team,
            }
        })
}

/// Transform a live competition into `SoccerGame::Live`. Callers must
/// pattern-match `EspnCompetition::Live` at the call site.
pub(crate) fn live_competition_to_game(
    event_id: String,
    competitors: [EspnCompetitor; 2],
    display_clock: String,
    period: u8,
    halftime: bool,
    details: Vec<EspnDetail>,
) -> Result<SoccerGame, AppError> {
    let (home_c, away_c) = order_home_away(
        &event_id,
        competitors,
        |c| c.home_away,
        |c| c.team.abbreviation.as_str(),
    )?;

    let last_event = last_event(&details, &home_c.team.id, &away_c.team.id);
    let home = competitor_to_team_state(&home_c)?;
    let away = competitor_to_team_state(&away_c)?;

    Ok(SoccerGame::Live {
        game_id: event_id,
        clock: display_clock,
        half: period,
        halftime,
        home,
        away,
        last_event,
    })
}

/// Transform a pre-game competition into `SoccerGame::Pregame`.
///
/// Not yet called by a handler — the games endpoints are live-only for now —
/// but modeled and tested so exposing pregame data later is a handler-only
/// change. Drop the cfg_attr when that handler lands.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn pregame_competition_to_game(
    event_id: String,
    date: String,
    competitors: [EspnCompetitor; 2],
) -> Result<SoccerGame, AppError> {
    let (home_c, away_c) = order_home_away(
        &event_id,
        competitors,
        |c| c.home_away,
        |c| c.team.abbreviation.as_str(),
    )?;
    let team = |c: &EspnCompetitor| -> Result<SoccerTeam, AppError> {
        Ok(SoccerTeam {
            abbreviation: c.team.abbreviation.clone(),
            colors: competitor_colors(c)?,
        })
    };
    Ok(SoccerGame::Pregame {
        game_id: event_id,
        date,
        home: team(&home_c)?,
        away: team(&away_c)?,
    })
}

#[cfg(test)]
mod tests {
    use super::super::types::{EspnCompetition, EspnEvent};
    use super::*;

    /// Real live-captured fixtures (see tools/extract_fixtures.py). The
    /// USA-BEL knockout provides pre, both halves, and halftime; POR-ESP
    /// provides full time.
    fn fixture(name: &str) -> EspnEvent {
        let path = format!(
            "{}/testdata/soccer/{}.json",
            env!("CARGO_MANIFEST_DIR"),
            name
        );
        let raw = std::fs::read_to_string(path).expect("fixture readable");
        serde_json::from_str(&raw).expect("fixture parses as a soccer event")
    }

    fn live_parts(event: EspnEvent) -> (String, [EspnCompetitor; 2], String, u8, bool, Vec<EspnDetail>) {
        let id = event.id;
        let Some(EspnCompetition::Live {
            competitors,
            display_clock,
            period,
            halftime,
            details,
        }) = event.competitions.into_iter().next()
        else {
            panic!("fixture must be a live competition");
        };
        (id, competitors, display_clock, period, halftime, details)
    }

    #[test]
    fn first_half_transforms_with_stoppage_clock() {
        let (id, competitors, clock, period, halftime, details) = live_parts(fixture("first_half"));
        let game =
            live_competition_to_game(id, competitors, clock, period, halftime, details).unwrap();
        let SoccerGame::Live {
            clock,
            half,
            halftime,
            home,
            away,
            last_event,
            ..
        } = game
        else {
            panic!("expected live");
        };
        assert_eq!(clock, "45'+6'");
        assert_eq!(half, 1);
        assert!(!halftime);
        assert_eq!((home.abbreviation.as_str(), home.score), ("USA", 1));
        assert_eq!((away.abbreviation.as_str(), away.score), ("BEL", 2));
        assert!(last_event.is_some());
    }

    #[test]
    fn halftime_is_distinguished_from_first_half_stoppage() {
        // Same clock ("45'+6'") and period (1) as the first_half fixture —
        // only status.type.description differs. This is the empirical reason
        // Live carries the halftime bool.
        let (id, competitors, clock, period, halftime, details) = live_parts(fixture("halftime"));
        assert_eq!(clock, "45'+6'");
        assert_eq!(period, 1);
        assert!(halftime);
        let game =
            live_competition_to_game(id, competitors, clock, period, halftime, details).unwrap();
        assert!(matches!(game, SoccerGame::Live { halftime: true, .. }));
    }

    #[test]
    fn second_half_stoppage_surfaces_latest_goal() {
        let (id, competitors, clock, period, halftime, details) =
            live_parts(fixture("second_half_stoppage"));
        let game =
            live_competition_to_game(id, competitors, clock, period, halftime, details).unwrap();
        let SoccerGame::Live {
            clock,
            half,
            away,
            last_event,
            ..
        } = game
        else {
            panic!("expected live");
        };
        assert_eq!(clock, "90'+4'");
        assert_eq!(half, 2);
        assert_eq!(away.score, 4);
        let event = last_event.expect("a goal was scored");
        assert_eq!(event.text, "Goal - R. Lukaku");
        assert_eq!(event.clock, "90'+3'");
        assert_eq!(event.team, Some(Side::Away));
    }

    #[test]
    fn pregame_fixture_transforms_through_du() {
        let event = fixture("pregame");
        let id = event.id;
        let date = event.date;
        let Some(EspnCompetition::PreGame { competitors }) =
            event.competitions.into_iter().next()
        else {
            panic!("fixture must be a pregame competition");
        };
        let game = pregame_competition_to_game(id, date, competitors).unwrap();
        let SoccerGame::Pregame {
            date, home, away, ..
        } = game
        else {
            panic!("expected pregame");
        };
        assert_eq!(date, "2026-07-07T00:00Z");
        assert_eq!(home.abbreviation, "USA");
        assert_eq!(away.abbreviation, "BEL");
    }

    #[test]
    fn full_time_fixture_maps_to_final_marker() {
        let event = fixture("full_time");
        assert!(matches!(
            event.competitions.into_iter().next(),
            Some(EspnCompetition::Final)
        ));
    }
}
