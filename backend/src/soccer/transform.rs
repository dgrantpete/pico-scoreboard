use crate::error::AppError;
use crate::espn::types::parse_start_time;
use crate::shared::competitor::{
    competitor_colors, competitor_to_team_state, order_competitors, parse_score,
};

use super::types::{
    Commentary, EspnCompetitor, EspnDetail, EventKind, LastEvent, RawSummary, Side,
    SoccerFinalFlavor, SoccerFinalGame, SoccerFinalTeam, SoccerLiveGame, SoccerPregameGame,
    SoccerTeam,
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

/// Transform a live competition into a `SoccerLiveGame`. Callers must
/// pattern-match `EspnCompetition::Live` at the call site.
#[allow(clippy::too_many_arguments)]
pub(crate) fn live_competition_to_game(
    event_id: String,
    competitors: [EspnCompetitor; 2],
    display_clock: String,
    clock_seconds: u16,
    period: u8,
    on_break: bool,
    details: Vec<EspnDetail>,
    commentary: Option<Commentary>,
) -> Result<SoccerLiveGame, AppError> {
    let (home_c, away_c) = order_competitors(&event_id, competitors)?;

    let last_event = last_event(&details, &home_c.team.id, &away_c.team.id);
    let home = competitor_to_team_state(&home_c)?;
    let away = competitor_to_team_state(&away_c)?;

    Ok(SoccerLiveGame {
        game_id: event_id,
        clock: display_clock,
        clock_seconds,
        half: period,
        on_break,
        home,
        away,
        last_event,
        commentary,
    })
}

/// Transform a pre-game competition into a `SoccerPregameGame`. `venue_name`
/// comes from the competition (`venue.fullName`).
pub(crate) fn pregame_competition_to_game(
    event_id: String,
    date: &str,
    venue_name: String,
    competitors: [EspnCompetitor; 2],
) -> Result<SoccerPregameGame, AppError> {
    let start_time = parse_start_time(date)?;
    let (home_c, away_c) = order_competitors(&event_id, competitors)?;
    let team = |c: &EspnCompetitor| -> Result<SoccerTeam, AppError> {
        Ok(SoccerTeam {
            abbreviation: c.team.abbreviation.clone(),
            colors: competitor_colors(c)?,
        })
    };
    Ok(SoccerPregameGame {
        game_id: event_id,
        start_time,
        venue: venue_name,
        home: team(&home_c)?,
        away: team(&away_c)?,
    })
}

/// Transform a finished competition into a `SoccerFinalGame` with per-side
/// scores and pre-formatted scorer lists.
pub(crate) fn final_competition_to_game(
    event_id: String,
    competitors: [EspnCompetitor; 2],
    details: Vec<EspnDetail>,
    flavor: SoccerFinalFlavor,
) -> Result<SoccerFinalGame, AppError> {
    let (home_c, away_c) = order_competitors(&event_id, competitors)?;
    let (home_id, away_id) = (home_c.team.id.clone(), away_c.team.id.clone());
    let team = |c: &EspnCompetitor, side: Side| -> Result<SoccerFinalTeam, AppError> {
        Ok(SoccerFinalTeam {
            abbreviation: c.team.abbreviation.clone(),
            score: parse_score(c)?,
            colors: competitor_colors(c)?,
            scorers: scorers_for(&details, side, &home_id, &away_id),
        })
    };
    Ok(SoccerFinalGame {
        game_id: event_id,
        flavor,
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
        on_break: bool,
        details: Vec<EspnDetail>,
    }

    fn live_parts(event: EspnEvent) -> LiveParts {
        let id = event.id;
        let Some(EspnCompetition::Live {
            competitors,
            display_clock,
            clock_seconds,
            period,
            on_break,
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
            on_break,
            details,
        }
    }

    fn to_live(p: LiveParts) -> SoccerLiveGame {
        live_competition_to_game(
            p.id,
            p.competitors,
            p.display_clock,
            p.clock_seconds,
            p.period,
            p.on_break,
            p.details,
            None,
        )
        .unwrap()
    }

    /// Destructure a final fixture's competition into its parts (with the
    /// DU-derived `flavor`), then build the domain final.
    fn to_final(event: EspnEvent) -> SoccerFinalGame {
        let id = event.id;
        let Some(EspnCompetition::Final {
            competitors,
            details,
            flavor,
        }) = event.competitions.into_iter().next()
        else {
            panic!("fixture must be a final competition");
        };
        final_competition_to_game(id, competitors, details, flavor).unwrap()
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
        assert_eq!(game.clock, "45'+6'");
        assert_eq!(game.clock_seconds, 51 * 60);
        assert_eq!(game.half, 1);
        assert!(!game.on_break);
        assert_eq!(
            (game.home.abbreviation.as_str(), game.home.score),
            ("USA", 1)
        );
        assert_eq!(
            (game.away.abbreviation.as_str(), game.away.score),
            ("BEL", 2)
        );
        assert!(game.last_event.is_some());
    }

    #[test]
    fn halftime_is_distinguished_from_first_half_stoppage() {
        // Same clock ("45'+6'") and period (1) as the first_half fixture —
        // only status.type.description differs. This is the empirical reason
        // Live carries the break flag.
        let p = live_parts(fixture("halftime"));
        assert_eq!(p.display_clock, "45'+6'");
        assert_eq!(p.period, 1);
        assert!(p.on_break);
        assert!(to_live(p).on_break);
    }

    #[test]
    fn second_half_stoppage_surfaces_latest_goal() {
        let game = to_live(live_parts(fixture("second_half_stoppage")));
        assert_eq!(game.clock, "90'+4'");
        assert_eq!(game.half, 2);
        assert_eq!(game.away.score, 4);
        let event = game.last_event.expect("a goal was scored");
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
        let Some(EspnCompetition::PreGame {
            competitors,
            venue_name,
        }) = event.competitions.into_iter().next()
        else {
            panic!("fixture must be a pregame competition");
        };
        assert_eq!(venue_name, "Lumen Field");
        let game = pregame_competition_to_game(id, &date, venue_name, competitors).unwrap();
        assert_eq!(date, "2026-07-07T00:00Z");
        assert_eq!(game.start_time, parse_start_time(&date).unwrap());
        assert_eq!(game.venue, "Lumen Field");
        assert_eq!(game.home.abbreviation, "USA");
        assert_eq!(game.away.abbreviation, "BEL");
    }

    #[test]
    fn red_card_is_surfaced_as_last_event() {
        // Real ARG-SUI knockout details, truncated to the moment just after
        // the 72' red card so it is the latest surfaced event (later goals
        // would win the max otherwise).
        let mut p = live_parts(fixture("live_red_card"));
        p.details.retain(|d| d.clock.value <= 4278.0);
        let event = to_live(p).last_event.expect("red card present");
        assert_eq!(event.kind, EventKind::RedCard);
        assert_eq!(event.athlete, "B. Embolo");
        assert_eq!(event.clock, "72'");
        assert_eq!(event.team, Some(Side::Away));
        assert_eq!(event.text, "Red Card - B. Embolo");
    }

    #[test]
    fn overtime_live_parses_with_extended_clock() {
        // Knockout extra time serves as in-state with description "Overtime":
        // active play (not a break), running clock, period passed through.
        let game = to_live(live_parts(fixture("live_red_card")));
        assert_eq!(game.clock, "120'+4'");
        assert!(!game.on_break);
        assert_eq!(
            (game.home.abbreviation.as_str(), game.home.score),
            ("ARG", 3)
        );
        assert_eq!((game.away.abbreviation.as_str(), game.away.score), ("SUI", 1));
        // Latest event is the 120'+1' goal, not the earlier red card.
        let event = game.last_event.expect("late goal present");
        assert_eq!(event.kind, EventKind::Goal);
        assert_eq!(event.athlete, "L. Martínez");
        assert_eq!(event.team, Some(Side::Home));
    }

    #[test]
    fn home_side_multi_goal_scorers_are_ordered_and_separated() {
        // Same match, post state ("Final Score - After Extra Time"): three
        // home goals (one a header subtype — collapses to the name format)
        // in clock order, and the away side's lone goal kept separate.
        let game = to_final(fixture("full_time_home_multi_goal"));
        assert_eq!(game.flavor, SoccerFinalFlavor::AfterExtraTime);
        assert_eq!(
            (game.home.abbreviation.as_str(), game.home.score),
            ("ARG", 3)
        );
        assert_eq!(
            game.home.scorers,
            "A. Mac Allister 10', J. Álvarez 112', L. Martínez 120'+1'"
        );
        assert_eq!(game.away.scorers, "D. Ndoye 67'");
    }

    #[test]
    fn latest_commentary_picks_highest_sequence_and_skips_empty() {
        let summary = |items: Vec<(u32, &str)>| RawSummary {
            commentary: items
                .into_iter()
                .map(|(sequence, text)| super::super::types::EspnCommentaryItem {
                    sequence,
                    text: text.to_string(),
                })
                .collect(),
        };

        // Highest sequence wins regardless of order.
        let c = latest_commentary(summary(vec![(3, "old"), (9, "newest"), (7, "mid")]))
            .expect("commentary present");
        assert_eq!((c.id.as_str(), c.text.as_str()), ("9", "newest"));

        // An empty-text winner degrades to None (never flash a blank line)...
        assert!(latest_commentary(summary(vec![(1, "text"), (5, "")])).is_none());
        // ...and so does an empty feed — the no-commentary degradation path.
        assert!(latest_commentary(summary(vec![])).is_none());
    }

    #[test]
    fn full_time_fixture_builds_final_with_scorers() {
        let game = to_final(fixture("full_time"));
        assert_eq!(game.flavor, SoccerFinalFlavor::FullTime);
        assert_eq!(
            (game.home.abbreviation.as_str(), game.home.score),
            ("POR", 0)
        );
        assert_eq!(
            (game.away.abbreviation.as_str(), game.away.score),
            ("ESP", 1)
        );
        // Yellow cards are excluded; the lone goal formats as "name clock".
        assert_eq!(game.home.scorers, "");
        assert_eq!(game.away.scorers, "M. Merino 90'+1'");
    }

    // --- Phase H: knockout live states + final flavors (real corpus fixtures) ---

    #[test]
    fn extra_time_in_play_parses_as_active_play() {
        // "Overtime" at period 4 (extra time), running clock, not a break.
        let game = to_live(live_parts(fixture("overtime")));
        assert_eq!(game.half, 4);
        assert!(!game.on_break);
        assert_eq!(game.clock, "120'+4'");
    }

    #[test]
    fn shootout_parses_at_period_five_active() {
        // "Shootout" is period 5; the match clock is frozen but it is not a
        // break flag (the firmware renders the shootout by half == 5).
        let game = to_live(live_parts(fixture("shootout")));
        assert_eq!(game.half, 5);
        assert!(!game.on_break);
    }

    #[test]
    fn extra_time_halftime_is_a_break() {
        // The interval between the two extra-time halves (period 3).
        let game = to_live(live_parts(fixture("extra_time_halftime")));
        assert_eq!(game.half, 3);
        assert!(game.on_break);
    }

    #[test]
    fn end_of_regulation_is_a_break() {
        // The interval between second half and extra time (period 2).
        let game = to_live(live_parts(fixture("end_of_regulation")));
        assert_eq!(game.half, 2);
        assert!(game.on_break);
    }

    #[test]
    fn is_break_covers_the_full_observed_description_set() {
        use super::super::types::is_break;
        for d in ["Halftime", "Extra Time Halftime", "End of Regulation", "End of Extra Time"] {
            assert!(is_break(Some(d)), "{d} should be a break");
        }
        for d in ["First Half", "Second Half", "In Progress", "Overtime", "Shootout"] {
            assert!(!is_break(Some(d)), "{d} should be active play");
        }
        assert!(!is_break(None));
        // Unknown degrades to active play (warn-and-render), never guessed.
        assert!(!is_break(Some("Penalty Shootout Pending")));
    }

    #[test]
    fn final_after_extra_time_sets_aet_flavor() {
        let game = to_final(fixture("final_after_extra_time"));
        assert_eq!(game.flavor, SoccerFinalFlavor::AfterExtraTime);
        // Scorers still resolve from the same details path.
        assert!(!game.home.scorers.is_empty() || !game.away.scorers.is_empty());
    }

    #[test]
    fn final_after_penalties_sets_penalties_flavor() {
        let game = to_final(fixture("final_after_penalties"));
        assert_eq!(game.flavor, SoccerFinalFlavor::AfterPenalties);
    }

    #[test]
    fn final_flavor_unknown_description_defaults_full_time() {
        use super::super::types::final_flavor;
        assert_eq!(final_flavor(Some("Full Time")), SoccerFinalFlavor::FullTime);
        assert_eq!(final_flavor(None), SoccerFinalFlavor::FullTime);
        assert_eq!(final_flavor(Some("Abandoned")), SoccerFinalFlavor::FullTime);
    }
}
