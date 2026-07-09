use crate::error::AppError;
use crate::mlb::transform::parse_start_time;
use crate::shared::team::{TeamColors, order_home_away, parse_hex_rgb};

use super::types::{
    Commentary, EspnCompetitor, EspnDetail, EventKind, LastEvent, RawSummary, Side,
    SoccerFinalTeam, SoccerGame, SoccerTeam, SoccerTeamState,
};

/// The latest commentary line of a summary payload (highest sequence).
pub(crate) fn latest_commentary(summary: RawSummary) -> Option<Commentary> {
    summary
        .commentary
        .into_iter()
        .max_by_key(|item| item.sequence)
        .filter(|item| !item.text.is_empty())
        .map(|item| Commentary {
            id: item.sequence.to_string(),
            text: item.text,
        })
}

/// Parse a competitor's colors, shared by the pre, live, and final builders.
fn competitor_colors(c: &EspnCompetitor) -> Result<TeamColors, AppError> {
    let primary = parse_hex_rgb(&c.team.color, &c.team.abbreviation)?;
    let alternate = parse_hex_rgb(&c.team.alternate_color, &c.team.abbreviation)?;
    Ok(TeamColors { primary, alternate })
}

fn parse_score(c: &EspnCompetitor) -> Result<u32, AppError> {
    c.score.parse::<u32>().map_err(|e| {
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
    })
}

fn competitor_to_team_state(c: &EspnCompetitor) -> Result<SoccerTeamState, AppError> {
    Ok(SoccerTeamState {
        abbreviation: c.team.abbreviation.clone(),
        score: parse_score(c)?,
        colors: competitor_colors(c)?,
    })
}

fn detail_side(d: &EspnDetail, home_team_id: &str, away_team_id: &str) -> Option<Side> {
    d.team.as_ref().and_then(|t| {
        if t.id == home_team_id {
            Some(Side::Home)
        } else if t.id == away_team_id {
            Some(Side::Away)
        } else {
            None
        }
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
            let athlete = d
                .athletes_involved
                .first()
                .map(|a| a.short_name.clone())
                .unwrap_or_default();
            let text = if athlete.is_empty() {
                d.r#type.text.clone()
            } else {
                format!("{} - {}", d.r#type.text, athlete)
            };
            LastEvent {
                text,
                kind: if d.red_card {
                    EventKind::RedCard
                } else {
                    EventKind::Goal
                },
                athlete,
                clock: d.clock.display_value.clone(),
                team: detail_side(d, home_team_id, away_team_id),
            }
        })
}

/// One side's pre-formatted scorer list ("M. Merino 90'+1', F. Torres 12'"),
/// in match order. An athlete-less goal falls back to the detail's type text
/// ("Goal 45'"). Own goals arrive attributed to the benefiting side by ESPN.
fn scorers_for(details: &[EspnDetail], side: Side, home_id: &str, away_id: &str) -> String {
    let mut scoring: Vec<&EspnDetail> = details
        .iter()
        .filter(|d| d.scoring_play && detail_side(d, home_id, away_id) == Some(side))
        .collect();
    scoring.sort_by(|a, b| {
        a.clock
            .value
            .partial_cmp(&b.clock.value)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scoring
        .iter()
        .map(|d| {
            let name = d
                .athletes_involved
                .first()
                .map(|a| a.short_name.as_str())
                .unwrap_or(d.r#type.text.as_str());
            format!("{} {}", name, d.clock.display_value)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Transform a live competition into `SoccerGame::Live`. Callers must
/// pattern-match `EspnCompetition::Live` at the call site.
#[allow(clippy::too_many_arguments)]
pub(crate) fn live_competition_to_game(
    event_id: String,
    competitors: [EspnCompetitor; 2],
    display_clock: String,
    clock_seconds: u16,
    period: u8,
    halftime: bool,
    details: Vec<EspnDetail>,
    commentary: Option<Commentary>,
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
        clock_seconds,
        half: period,
        halftime,
        home,
        away,
        last_event,
        commentary,
    })
}

/// Transform a pre-game competition into `SoccerGame::Pregame`.
pub(crate) fn pregame_competition_to_game(
    event_id: String,
    date: String,
    competitors: [EspnCompetitor; 2],
) -> Result<SoccerGame, AppError> {
    let start_time = parse_start_time(&date)?;
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
        start_time,
        home: team(&home_c)?,
        away: team(&away_c)?,
    })
}

/// Transform a finished competition into `SoccerGame::Final` with per-side
/// scores and pre-formatted scorer lists.
pub(crate) fn final_competition_to_game(
    event_id: String,
    competitors: [EspnCompetitor; 2],
    details: Vec<EspnDetail>,
) -> Result<SoccerGame, AppError> {
    let (home_c, away_c) = order_home_away(
        &event_id,
        competitors,
        |c| c.home_away,
        |c| c.team.abbreviation.as_str(),
    )?;
    let (home_id, away_id) = (home_c.team.id.clone(), away_c.team.id.clone());
    let team = |c: &EspnCompetitor, side: Side| -> Result<SoccerFinalTeam, AppError> {
        Ok(SoccerFinalTeam {
            abbreviation: c.team.abbreviation.clone(),
            score: parse_score(c)?,
            colors: competitor_colors(c)?,
            scorers: scorers_for(&details, side, &home_id, &away_id),
        })
    };
    Ok(SoccerGame::Final {
        game_id: event_id,
        home: team(&home_c, Side::Home)?,
        away: team(&away_c, Side::Away)?,
    })
}

#[cfg(test)]
mod tests {
    use super::super::types::{EspnCompetition, EspnEvent, parse_display_clock};
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

    struct LiveParts {
        id: String,
        competitors: [EspnCompetitor; 2],
        display_clock: String,
        clock_seconds: u16,
        period: u8,
        halftime: bool,
        details: Vec<EspnDetail>,
    }

    fn live_parts(event: EspnEvent) -> LiveParts {
        let id = event.id;
        let Some(EspnCompetition::Live {
            competitors,
            display_clock,
            clock_seconds,
            period,
            halftime,
            details,
        }) = event.competitions.into_iter().next()
        else {
            panic!("fixture must be a live competition");
        };
        LiveParts {
            id,
            competitors,
            display_clock,
            clock_seconds,
            period,
            halftime,
            details,
        }
    }

    fn to_live(p: LiveParts) -> SoccerGame {
        live_competition_to_game(
            p.id,
            p.competitors,
            p.display_clock,
            p.clock_seconds,
            p.period,
            p.halftime,
            p.details,
            None,
        )
        .unwrap()
    }

    #[test]
    fn display_clock_parses_floor_minutes_and_stoppage() {
        assert_eq!(parse_display_clock("23'", None), 23 * 60);
        assert_eq!(parse_display_clock("0'", None), 0);
        assert_eq!(parse_display_clock("45'+6'", None), 51 * 60);
        assert_eq!(parse_display_clock("90'+4'", None), 94 * 60);
        // Unparseable degrades to the numeric fallback (capped at regulation).
        assert_eq!(parse_display_clock("HT", Some(2700.0)), 2700);
        assert_eq!(parse_display_clock("garbage", None), 0);
    }

    #[test]
    fn first_half_transforms_with_stoppage_clock() {
        let game = to_live(live_parts(fixture("first_half")));
        let SoccerGame::Live {
            clock,
            clock_seconds,
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
        assert_eq!(clock_seconds, 51 * 60);
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
        let p = live_parts(fixture("halftime"));
        assert_eq!(p.display_clock, "45'+6'");
        assert_eq!(p.period, 1);
        assert!(p.halftime);
        let game = to_live(p);
        assert!(matches!(game, SoccerGame::Live { halftime: true, .. }));
    }

    #[test]
    fn second_half_stoppage_surfaces_latest_goal() {
        let game = to_live(live_parts(fixture("second_half_stoppage")));
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
        assert_eq!(event.kind, EventKind::Goal);
        assert_eq!(event.athlete, "R. Lukaku");
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
            date,
            start_time,
            home,
            away,
            ..
        } = game
        else {
            panic!("expected pregame");
        };
        assert_eq!(date, "2026-07-07T00:00Z");
        assert_eq!(start_time, parse_start_time("2026-07-07T00:00Z").unwrap());
        assert_eq!(home.abbreviation, "USA");
        assert_eq!(away.abbreviation, "BEL");
    }

    #[test]
    fn full_time_fixture_builds_final_with_scorers() {
        let event = fixture("full_time");
        let id = event.id;
        let Some(EspnCompetition::Final {
            competitors,
            details,
        }) = event.competitions.into_iter().next()
        else {
            panic!("fixture must be a final competition");
        };
        let game = final_competition_to_game(id, competitors, details).unwrap();
        let SoccerGame::Final { home, away, .. } = game else {
            panic!("expected final");
        };
        assert_eq!((home.abbreviation.as_str(), home.score), ("POR", 0));
        assert_eq!((away.abbreviation.as_str(), away.score), ("ESP", 1));
        // Yellow cards are excluded; the lone goal formats as "name clock".
        assert_eq!(home.scorers, "");
        assert_eq!(away.scorers, "M. Merino 90'+1'");
    }
}
