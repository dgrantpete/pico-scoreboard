use crate::error::AppError;
use crate::espn::types::parse_start_time;
use crate::shared::competitor::{
    competitor_colors, competitor_to_team_state, linescore_bytes, order_competitors, parse_record,
    parse_score,
};
use crate::shared::game::{LastPlay, LivePhase};

use super::types::{
    EspnCompetitor, EspnSituation, NbaFinalGame, NbaFinalTeam, NbaLiveGame, NbaPregameGame,
    NbaPregameTeam,
};

/// Transform a pre-game competition into an `NbaPregameGame`. `date` comes
/// from the event level; `venue_name` from the competition.
pub(crate) fn pregame_competition_to_game(
    event_id: String,
    date: &str,
    venue_name: String,
    competitors: [EspnCompetitor; 2],
) -> Result<NbaPregameGame, AppError> {
    let start_time = parse_start_time(date)?;
    let (home_c, away_c) = order_competitors(&event_id, competitors)?;

    let team = |c: &EspnCompetitor| -> Result<NbaPregameTeam, AppError> {
        Ok(NbaPregameTeam {
            abbreviation: c.team.abbreviation.clone(),
            colors: competitor_colors(c)?,
            record: parse_record(&c.records),
        })
    };

    Ok(NbaPregameGame {
        game_id: event_id,
        start_time,
        venue: venue_name,
        home: team(&home_c)?,
        away: team(&away_c)?,
    })
}

/// Transform a live competition into an `NbaLiveGame`. Callers must
/// pattern-match `EspnCompetition::Live` at the call site.
pub(crate) fn live_competition_to_game(
    event_id: String,
    competitors: [EspnCompetitor; 2],
    period: u8,
    display_clock: String,
    phase: LivePhase,
    situation: EspnSituation,
) -> Result<NbaLiveGame, AppError> {
    let (home_c, away_c) = order_competitors(&event_id, competitors)?;
    let home = competitor_to_team_state(&home_c)?;
    let away = competitor_to_team_state(&away_c)?;

    Ok(NbaLiveGame {
        game_id: event_id,
        period,
        clock: display_clock,
        phase,
        home,
        away,
        last_play: situation.last_play.map(|p| LastPlay {
            id: p.id,
            text: p.text,
        }),
    })
}

/// Transform a final competition into an `NbaFinalGame`.
pub(crate) fn final_competition_to_game(
    event_id: String,
    competitors: [EspnCompetitor; 2],
    period: u8,
) -> Result<NbaFinalGame, AppError> {
    let (home_c, away_c) = order_competitors(&event_id, competitors)?;

    let team = |c: &EspnCompetitor| -> Result<NbaFinalTeam, AppError> {
        Ok(NbaFinalTeam {
            abbreviation: c.team.abbreviation.clone(),
            score: parse_score(c)?,
            colors: competitor_colors(c)?,
            line_score: linescore_bytes(&c.linescores),
        })
    };

    Ok(NbaFinalGame {
        game_id: event_id,
        periods_played: period,
        home: team(&home_c)?,
        away: team(&away_c)?,
    })
}

#[cfg(test)]
mod tests {
    use super::super::types::{EspnCompetition, EspnEvent};
    use super::*;
    use crate::espn::types::{EspnRecord, parse_live_phase};

    /// Real live-captured NBA fixtures (see tools/extract_fixtures.py), from
    /// the April 2026 end-of-season/playoff collection.
    fn fixture(name: &str) -> EspnEvent {
        let path = format!("{}/testdata/nba/{}.json", env!("CARGO_MANIFEST_DIR"), name);
        let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        serde_json::from_str(&raw).expect("fixture parses as an NBA event")
    }

    struct LiveParts {
        id: String,
        competitors: [EspnCompetitor; 2],
        period: u8,
        display_clock: String,
        phase: LivePhase,
        situation: EspnSituation,
    }

    fn live_parts(event: EspnEvent) -> LiveParts {
        let id = event.id;
        let Some(EspnCompetition::Live {
            competitors,
            period,
            display_clock,
            phase,
            situation,
        }) = event.competitions.into_iter().next()
        else {
            panic!("fixture must be a live competition");
        };
        LiveParts {
            id,
            competitors,
            period,
            display_clock,
            phase,
            situation,
        }
    }

    fn to_live(p: LiveParts) -> NbaLiveGame {
        live_competition_to_game(
            p.id,
            p.competitors,
            p.period,
            p.display_clock,
            p.phase,
            p.situation,
        )
        .unwrap()
    }

    #[test]
    fn parse_live_phase_maps_known_descriptions() {
        assert_eq!(
            parse_live_phase(Some("In Progress"), "nba"),
            LivePhase::InProgress
        );
        assert_eq!(
            parse_live_phase(Some("Halftime"), "nba"),
            LivePhase::Halftime
        );
        assert_eq!(
            parse_live_phase(Some("End of Period"), "nba"),
            LivePhase::EndOfPeriod
        );
        assert_eq!(parse_live_phase(None, "nba"), LivePhase::InProgress);
    }

    #[test]
    fn parse_live_phase_unknown_degrades_to_in_progress() {
        // An OT-specific or delay label has never been observed live; the
        // contract is warn-and-render rather than guess a break state.
        assert_eq!(
            parse_live_phase(Some("Overtime"), "nba"),
            LivePhase::InProgress
        );
        assert_eq!(
            parse_live_phase(Some("Delayed"), "nba"),
            LivePhase::InProgress
        );
    }

    #[test]
    fn pregame_fixture_transforms_through_du() {
        let event = fixture("pregame");
        let id = event.id;
        let date = event.date;
        let Some(EspnCompetition::PreGame {
            competitors,
            venue_name,
        }) = event.competitions.into_iter().next()
        else {
            panic!("fixture must be a pregame competition");
        };
        let game = pregame_competition_to_game(id, &date, venue_name, competitors).unwrap();
        assert_eq!(game.venue, "crypto.com Arena");
        assert_eq!(game.start_time, parse_start_time(&date).unwrap());
        assert_eq!(game.home.abbreviation, "LAL");
        assert_eq!(game.away.abbreviation, "PHX");
        assert_eq!(game.home.colors.primary, 0x552583);
        assert!(game.home.record.is_some());
        assert!(game.away.record.is_some());
    }

    #[test]
    fn in_progress_fixture_transforms() {
        let game = to_live(live_parts(fixture("in_progress")));
        assert_eq!(game.phase, LivePhase::InProgress);
        assert_eq!(game.period, 3);
        assert_eq!(game.clock, "4:37");
        assert_eq!(
            (game.home.abbreviation.as_str(), game.home.score),
            ("DEN", 77)
        );
        assert_eq!(
            (game.away.abbreviation.as_str(), game.away.score),
            ("OKC", 75)
        );
        let play = game.last_play.expect("live game has a last play");
        assert_eq!(play.id, "401811037411");
        assert!(!play.text.is_empty());
    }

    #[test]
    fn subminute_clock_fixture_transforms() {
        // Under a minute the clock switches from "M:SS" to "SS.d" form; the
        // string passes through untouched.
        let p = live_parts(fixture("in_progress_subminute"));
        assert!(!p.display_clock.contains(':'));
        let game = to_live(p);
        assert_eq!(game.phase, LivePhase::InProgress);
        assert_eq!(game.period, 2);
    }

    #[test]
    fn live_without_last_play_transforms() {
        // Real pre-tip glitch payload: state "in" with an empty situation {}.
        let game = to_live(live_parts(fixture("in_progress_no_last_play")));
        assert!(game.last_play.is_none());
        assert_eq!(game.home.score, 0);
        assert_eq!(game.away.score, 0);
    }

    #[test]
    fn halftime_fixture_sets_phase() {
        // Clock reads "0.0" at the break — description is the only signal,
        // which is the empirical reason Live carries the phase.
        let p = live_parts(fixture("halftime"));
        assert_eq!(p.display_clock, "0.0");
        let game = to_live(p);
        assert_eq!(game.phase, LivePhase::Halftime);
        assert_eq!(game.period, 2);
        assert_eq!(
            (game.home.abbreviation.as_str(), game.home.score),
            ("UTAH", 74)
        );
    }

    #[test]
    fn end_of_period_fixture_sets_phase() {
        let game = to_live(live_parts(fixture("end_of_period")));
        assert_eq!(game.phase, LivePhase::EndOfPeriod);
        assert_eq!(game.period, 4);
    }

    #[test]
    fn final_fixture_has_line_scores_and_score() {
        let event = fixture("final");
        let id = event.id;
        let Some(EspnCompetition::Final {
            competitors,
            period,
        }) = event.competitions.into_iter().next()
        else {
            panic!("fixture must be a final competition");
        };
        let game = final_competition_to_game(id, competitors, period).unwrap();
        assert_eq!(game.periods_played, 4);
        assert_eq!(
            (game.home.abbreviation.as_str(), game.home.score),
            ("CHA", 100)
        );
        assert_eq!(
            (game.away.abbreviation.as_str(), game.away.score),
            ("DET", 118)
        );
        assert_eq!(game.home.line_score.len(), 4);
        assert_eq!(game.away.line_score.len(), 4);
    }

    fn competitor(abbrev: &str, home_away: &str, linescores: &str) -> String {
        format!(
            r#"{{"homeAway":"{home_away}","score":"120",
                "team":{{"id":"1","abbreviation":"{abbrev}","color":"552583","alternateColor":"FDB927"}},
                "linescores":[{linescores}]}}"#
        )
    }

    #[test]
    fn overtime_period_passes_through_du() {
        // Never observed in the playoff corpus (max period 4) — pinned
        // synthetically so a first OT game cannot panic or truncate.
        let quarters = r#"{"value":30.0,"period":1},{"value":25.0,"period":2},
            {"value":20.0,"period":3},{"value":25.0,"period":4},{"value":20.0,"period":5}"#;
        let json = format!(
            r#"{{"competitors":[{away},{home}],
                "status":{{"type":{{"state":"in","description":"In Progress"}},"period":5,"displayClock":"2:11"}},
                "situation":{{"lastPlay":{{"id":"p1","text":"Jump ball"}}}}}}"#,
            away = competitor("OKC", "away", quarters),
            home = competitor("DEN", "home", quarters),
        );
        let competition: EspnCompetition =
            serde_json::from_str(&json).expect("OT live competition parses through the DU");
        let EspnCompetition::Live {
            competitors,
            period,
            display_clock,
            phase,
            situation,
        } = competition
        else {
            panic!("state 'in' must map to the Live variant");
        };
        let game = live_competition_to_game(
            "401811099".to_string(),
            competitors,
            period,
            display_clock,
            phase,
            situation,
        )
        .unwrap();
        assert_eq!(game.period, 5);

        let json = format!(
            r#"{{"competitors":[{away},{home}],
                "status":{{"type":{{"state":"post","description":"Final"}},"period":5,"displayClock":"0.0"}}}}"#,
            away = competitor("OKC", "away", quarters),
            home = competitor("DEN", "home", quarters),
        );
        let competition: EspnCompetition =
            serde_json::from_str(&json).expect("OT final competition parses through the DU");
        let EspnCompetition::Final {
            competitors,
            period,
        } = competition
        else {
            panic!("state 'post' must map to the Final variant");
        };
        let game = final_competition_to_game("401811099".to_string(), competitors, period).unwrap();
        assert_eq!(game.periods_played, 5);
        assert_eq!(game.home.line_score.len(), 5);
    }

    #[test]
    fn parse_record_reads_total_entry() {
        let records = vec![
            EspnRecord {
                r#type: "home".to_string(),
                summary: "28-13".to_string(),
            },
            EspnRecord {
                r#type: "total".to_string(),
                summary: "51-29".to_string(),
            },
        ];
        let record = parse_record(&records).expect("total record present");
        assert_eq!((record.wins, record.losses), (51, 29));
    }

    #[test]
    fn parse_record_absent_or_malformed_is_none() {
        assert!(parse_record(&[]).is_none());
        let bad = vec![EspnRecord {
            r#type: "total".to_string(),
            summary: "TBD".to_string(),
        }];
        assert!(parse_record(&bad).is_none());
    }

    #[test]
    fn pregame_transform_splits_home_away_and_parses_colors() {
        let c = |abbrev: &str, home_away: &str| -> EspnCompetitor {
            serde_json::from_str(&competitor(abbrev, home_away, "")).expect("competitor parses")
        };
        let game = pregame_competition_to_game(
            "401811040".to_string(),
            "2026-04-11T02:30Z",
            "crypto.com Arena".to_string(),
            [c("PHX", "away"), c("LAL", "home")],
        )
        .unwrap();
        assert_eq!(game.game_id, "401811040");
        assert_eq!(game.home.abbreviation, "LAL");
        assert_eq!(game.away.abbreviation, "PHX");
        assert_eq!(game.home.colors.primary, 0x552583);
        assert!(game.home.record.is_none());
    }

    #[test]
    fn pregame_transform_rejects_two_home_teams() {
        let c = |abbrev: &str, home_away: &str| -> EspnCompetitor {
            serde_json::from_str(&competitor(abbrev, home_away, "")).expect("competitor parses")
        };
        let result = pregame_competition_to_game(
            "401811040".to_string(),
            "2026-04-11T02:30Z",
            "crypto.com Arena".to_string(),
            [c("PHX", "home"), c("LAL", "home")],
        );
        assert!(matches!(result, Err(AppError::EspnDeserialize { .. })));
    }

    #[test]
    fn pre_competition_without_venue_is_rejected() {
        let json = format!(
            r#"{{"competitors":[{away},{home}],
                "status":{{"type":{{"state":"pre","description":"Scheduled"}},"period":0,"displayClock":"0.0"}}}}"#,
            away = competitor("PHX", "away", ""),
            home = competitor("LAL", "home", ""),
        );
        let result: Result<EspnCompetition, _> = serde_json::from_str(&json);
        assert!(result.is_err(), "pregame without venue must fail the DU");
    }
}
