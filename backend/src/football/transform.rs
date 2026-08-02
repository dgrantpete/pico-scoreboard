use crate::error::AppError;
use crate::espn::types::parse_start_time;
use crate::shared::competitor::{
    competitor_colors, competitor_to_team_state, linescore_bytes, order_competitors, parse_record,
    parse_score,
};
use crate::shared::game::{LastPlay, LivePhase, Side};

use super::types::{
    EspnCompetitor, EspnSituation, FootballFinalGame, FootballFinalTeam, FootballLiveGame,
    FootballPregameGame, FootballPregameTeam, FootballSituation, Timeouts, rank_line,
};

/// Transform a pre-game competition into a `FootballPregameGame`. `is_college`
/// gates the rank line (see [`rank_line`]).
pub(crate) fn pregame_competition_to_game(
    event_id: String,
    date: &str,
    venue_name: String,
    competitors: [EspnCompetitor; 2],
    is_college: bool,
) -> Result<FootballPregameGame, AppError> {
    let start_time = parse_start_time(date)?;
    let (home_c, away_c) = order_competitors(&event_id, competitors)?;

    let team = |c: &EspnCompetitor| -> Result<FootballPregameTeam, AppError> {
        Ok(FootballPregameTeam {
            abbreviation: c.team.abbreviation.clone(),
            colors: competitor_colors(c)?,
            record: parse_record(&c.records),
            rank_line: rank_line(c, is_college),
        })
    };

    Ok(FootballPregameGame {
        game_id: event_id,
        start_time,
        venue: venue_name,
        home: team(&home_c)?,
        away: team(&away_c)?,
    })
}

/// Transform a live competition into a `FootballLiveGame`. Callers must
/// pattern-match `EspnCompetition::Live` at the call site.
pub(crate) fn live_competition_to_game(
    event_id: String,
    competitors: [EspnCompetitor; 2],
    period: u8,
    display_clock: String,
    phase: LivePhase,
    situation: EspnSituation,
) -> Result<FootballLiveGame, AppError> {
    let (home_c, away_c) = order_competitors(&event_id, competitors)?;

    let football_situation = validate_situation(&situation, &home_c.team.id, &away_c.team.id);
    let timeouts = parse_timeouts(&situation);
    let last_play = situation.last_play.map(|p| LastPlay {
        id: p.id,
        text: p.text,
    });

    let home = competitor_to_team_state(&home_c)?;
    let away = competitor_to_team_state(&away_c)?;

    Ok(FootballLiveGame {
        game_id: event_id,
        period,
        clock: display_clock,
        phase,
        home,
        away,
        situation: football_situation,
        timeouts,
        last_play,
    })
}

/// Validate ESPN's raw situation into a display-ready `FootballSituation`, or
/// `None` when it isn't a well-formed offensive snap. ESPN uses `-1` (and
/// omission) as the "not applicable" sentinel between plays, so we keep the
/// situation only when the down is a real 1st–4th, the yard line sits on the
/// field (0–100), and the possession team id resolves to one of the two
/// competitors — otherwise we warn and drop it, because a half-formed situation
/// would misdraw the ball and first-down markers.
///
/// yardLine semantics — the 0–100 range and which end zone is 0 — are excavated
/// from the removed working code and remain unverified against live data; this
/// is the single highest-risk assumption in the module (see the BACKLOG
/// live-validation item).
fn validate_situation(
    s: &EspnSituation,
    home_id: &str,
    away_id: &str,
) -> Option<FootballSituation> {
    if !(1..=4).contains(&s.down) {
        // `-1` is the ordinary between-plays sentinel; anything else is a glitch.
        if s.down != -1 {
            tracing::warn!(
                down = s.down,
                "football situation down outside 1..=4 — dropping situation"
            );
        }
        return None;
    }
    if !(0..=100).contains(&s.yard_line) {
        tracing::warn!(
            yard_line = s.yard_line,
            "football situation yardLine outside 0..=100 — dropping situation"
        );
        return None;
    }
    let possession = match s.possession.as_deref() {
        Some(id) if id == home_id => Side::Home,
        Some(id) if id == away_id => Side::Away,
        other => {
            tracing::warn!(
                possession = ?other,
                "football situation possession id resolves to neither competitor — dropping situation"
            );
            return None;
        }
    };

    Some(FootballSituation {
        down: s.down as u8,
        distance: s.distance.clamp(0, u8::MAX as i16) as u8,
        yard_line: s.yard_line as u8,
        possession,
        red_zone: s.is_red_zone,
    })
}

/// Both sides' remaining timeouts, or `None` when ESPN hasn't populated them
/// (the `-1` sentinel / omission between plays). All-or-nothing: the wire
/// carries a single "timeouts present" flag covering both counts.
fn parse_timeouts(s: &EspnSituation) -> Option<Timeouts> {
    if s.away_timeouts < 0 || s.home_timeouts < 0 {
        return None;
    }
    Some(Timeouts {
        away: s.away_timeouts.clamp(0, u8::MAX as i16) as u8,
        home: s.home_timeouts.clamp(0, u8::MAX as i16) as u8,
    })
}

/// Transform a final competition into a `FootballFinalGame`.
pub(crate) fn final_competition_to_game(
    event_id: String,
    competitors: [EspnCompetitor; 2],
    period: u8,
) -> Result<FootballFinalGame, AppError> {
    let (home_c, away_c) = order_competitors(&event_id, competitors)?;

    let team = |c: &EspnCompetitor| -> Result<FootballFinalTeam, AppError> {
        Ok(FootballFinalTeam {
            abbreviation: c.team.abbreviation.clone(),
            score: parse_score(c)?,
            colors: competitor_colors(c)?,
            line_score: linescore_bytes(&c.linescores),
        })
    };

    Ok(FootballFinalGame {
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
    use crate::espn::types::parse_live_phase;

    /// Synthetic ESPN fixtures modeled on the excavated football situation
    /// shape (no live NFL/NCAAF bodies until preseason — see the BACKLOG
    /// live-validation item). Fixtures nest per ESPN league slug: `nfl/`,
    /// `college-football/`.
    fn fixture(name: &str) -> EspnEvent {
        let path = format!(
            "{}/testdata/football/{}.json",
            env!("CARGO_MANIFEST_DIR"),
            name
        );
        let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        serde_json::from_str(&raw).expect("fixture parses as a football event")
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

    fn to_live(p: LiveParts) -> FootballLiveGame {
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

    fn pregame(event: EspnEvent, is_college: bool) -> FootballPregameGame {
        let id = event.id;
        let date = event.date;
        let Some(EspnCompetition::PreGame {
            competitors,
            venue_name,
        }) = event.competitions.into_iter().next()
        else {
            panic!("fixture must be a pregame competition");
        };
        pregame_competition_to_game(id, &date, venue_name, competitors, is_college).unwrap()
    }

    fn to_final(event: EspnEvent) -> FootballFinalGame {
        let id = event.id;
        let Some(EspnCompetition::Final {
            competitors,
            period,
        }) = event.competitions.into_iter().next()
        else {
            panic!("fixture must be a final competition");
        };
        final_competition_to_game(id, competitors, period).unwrap()
    }

    #[test]
    fn parse_live_phase_maps_known_descriptions_and_the_quarter_alias() {
        assert_eq!(
            parse_live_phase(Some("In Progress"), "football"),
            LivePhase::InProgress
        );
        assert_eq!(
            parse_live_phase(Some("Halftime"), "football"),
            LivePhase::Halftime
        );
        assert_eq!(
            parse_live_phase(Some("End of Period"), "football"),
            LivePhase::EndOfPeriod
        );
        // Football labels the quarter break both ways — both are the same phase.
        assert_eq!(
            parse_live_phase(Some("End of Quarter"), "football"),
            LivePhase::EndOfPeriod
        );
        assert_eq!(parse_live_phase(None, "football"), LivePhase::InProgress);
    }

    #[test]
    fn parse_live_phase_unknown_degrades_to_in_progress() {
        assert_eq!(
            parse_live_phase(Some("Delayed"), "football"),
            LivePhase::InProgress
        );
        assert_eq!(
            parse_live_phase(Some("Overtime"), "football"),
            LivePhase::InProgress
        );
    }

    #[test]
    fn pregame_nfl_transforms_through_du_without_rank() {
        // NFL: records present, never a rank line (is_college false).
        let game = pregame(fixture("nfl/pregame"), false);
        assert_eq!(game.venue, "Arrowhead Stadium");
        assert_eq!(game.home.abbreviation, "KC");
        assert_eq!(game.away.abbreviation, "BUF");
        assert!(game.home.record.is_some());
        assert!(game.home.rank_line.is_none());
        assert!(game.away.rank_line.is_none());
    }

    #[test]
    fn pregame_ncaaf_ranked_builds_rank_line_only_for_ranked_side() {
        // Home #3 is ranked; away is rank 99 (unranked) → no line.
        let game = pregame(fixture("college-football/pregame_ranked"), true);
        assert_eq!(game.home.rank_line.as_deref(), Some("#3 OHIO STATE"));
        assert!(game.away.rank_line.is_none());
    }

    #[test]
    fn pregame_ncaaf_rank_absent_when_polled_as_nfl() {
        // Same college fixture, but is_college false suppresses every rank line.
        let game = pregame(fixture("college-football/pregame_ranked"), false);
        assert!(game.home.rank_line.is_none());
        assert!(game.away.rank_line.is_none());
    }

    #[test]
    fn in_progress_transforms_with_full_situation() {
        let game = to_live(live_parts(fixture("nfl/in_progress")));
        assert_eq!(game.phase, LivePhase::InProgress);
        assert_eq!(game.period, 3);
        assert_eq!(game.clock, "8:24");
        assert_eq!(
            (game.home.abbreviation.as_str(), game.home.score),
            ("KC", 17)
        );
        assert_eq!(
            (game.away.abbreviation.as_str(), game.away.score),
            ("BUF", 14)
        );
        let s = game.situation.expect("well-formed snap");
        assert_eq!((s.down, s.distance, s.yard_line), (2, 7, 45));
        assert_eq!(s.possession, Side::Home);
        assert!(!s.red_zone);
        let t = game.timeouts.expect("timeouts populated");
        assert_eq!((t.away, t.home), (2, 3));
        let play = game.last_play.expect("live game has a last play");
        assert_eq!(play.id, "401772510105");
        assert!(!play.text.is_empty());
    }

    #[test]
    fn in_progress_redzone_sets_red_zone_and_away_possession() {
        let game = to_live(live_parts(fixture("nfl/in_progress_redzone")));
        let s = game.situation.expect("well-formed snap");
        assert!(s.red_zone);
        assert_eq!(s.possession, Side::Away);
        assert_eq!((s.down, s.distance, s.yard_line), (1, 8, 92));
    }

    #[test]
    fn in_progress_empty_situation_drops_situation_and_timeouts() {
        // Pre-snap glitch: state "in" with an empty situation {}. Score and
        // colors still transform; situation/timeouts/last_play are absent.
        let game = to_live(live_parts(fixture("nfl/in_progress_empty_situation")));
        assert!(game.situation.is_none());
        assert!(game.timeouts.is_none());
        assert!(game.last_play.is_none());
        assert_eq!(game.home.score, 0);
        assert_eq!(game.away.score, 0);
    }

    #[test]
    fn halftime_sets_phase_and_drops_situation() {
        let game = to_live(live_parts(fixture("nfl/halftime")));
        assert_eq!(game.phase, LivePhase::Halftime);
        assert_eq!(game.period, 2);
        // Between-plays: down -1 → no situation, but no warning-worthy glitch.
        assert!(game.situation.is_none());
    }

    #[test]
    fn end_of_period_sets_phase() {
        let game = to_live(live_parts(fixture("nfl/end_of_period")));
        assert_eq!(game.phase, LivePhase::EndOfPeriod);
        assert_eq!(game.period, 1);
    }

    #[test]
    fn final_has_line_scores_and_score() {
        let game = to_final(fixture("nfl/final"));
        assert_eq!(game.periods_played, 4);
        assert_eq!(
            (game.home.abbreviation.as_str(), game.home.score),
            ("KC", 27)
        );
        assert_eq!(
            (game.away.abbreviation.as_str(), game.away.score),
            ("BUF", 24)
        );
        assert_eq!(game.home.line_score.len(), 4);
        assert_eq!(game.away.line_score.len(), 4);
    }

    #[test]
    fn final_overtime_extends_periods_and_line_scores() {
        let game = to_final(fixture("nfl/final_ot"));
        assert_eq!(game.periods_played, 5);
        assert_eq!(game.home.line_score.len(), 5);
        assert_eq!(game.away.line_score.len(), 5);
    }

    // --- Situation validation edge cases (synthetic competitions) ---

    fn competitor(id: &str, abbrev: &str, home_away: &str) -> String {
        format!(
            r#"{{"homeAway":"{home_away}","score":"10",
                "team":{{"id":"{id}","abbreviation":"{abbrev}","color":"e31837","alternateColor":"ffb81c"}}}}"#
        )
    }

    fn live_with_situation(situation: &str) -> FootballLiveGame {
        let json = format!(
            r#"{{"competitors":[{away},{home}],
                "status":{{"type":{{"state":"in","description":"In Progress"}},"period":1,"displayClock":"10:00"}},
                "situation":{situation}}}"#,
            away = competitor("2", "BUF", "away"),
            home = competitor("12", "KC", "home"),
        );
        let competition: EspnCompetition =
            serde_json::from_str(&json).expect("live competition parses through the DU");
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
        live_competition_to_game(
            "401772599".to_string(),
            competitors,
            period,
            display_clock,
            phase,
            situation,
        )
        .unwrap()
    }

    #[test]
    fn situation_dropped_when_down_is_zero() {
        let game =
            live_with_situation(r#"{"down":0,"distance":10,"yardLine":50,"possession":"12"}"#);
        assert!(game.situation.is_none());
    }

    #[test]
    fn situation_dropped_when_yardline_out_of_range() {
        let game =
            live_with_situation(r#"{"down":1,"distance":10,"yardLine":101,"possession":"12"}"#);
        assert!(game.situation.is_none());
    }

    #[test]
    fn situation_dropped_when_possession_matches_neither_team() {
        let game =
            live_with_situation(r#"{"down":1,"distance":10,"yardLine":50,"possession":"999"}"#);
        assert!(game.situation.is_none());
    }

    #[test]
    fn situation_dropped_when_possession_absent() {
        let game = live_with_situation(r#"{"down":1,"distance":10,"yardLine":50}"#);
        assert!(game.situation.is_none());
    }

    #[test]
    fn timeouts_present_independently_of_situation() {
        // A dropped situation must not drop the timeouts (separate wire flag).
        let game = live_with_situation(
            r#"{"down":-1,"distance":-1,"yardLine":-1,"homeTimeouts":1,"awayTimeouts":0}"#,
        );
        assert!(game.situation.is_none());
        let t = game.timeouts.expect("timeouts populated even with no snap");
        assert_eq!((t.away, t.home), (0, 1));
    }

    #[test]
    fn goal_to_go_snap_is_valid_at_the_one() {
        let game = live_with_situation(
            r#"{"down":3,"distance":1,"yardLine":99,"possession":"2","isRedZone":true}"#,
        );
        let s = game.situation.expect("goal-to-go is a valid snap");
        assert_eq!((s.down, s.distance, s.yard_line), (3, 1, 99));
        assert_eq!(s.possession, Side::Away);
        assert!(s.red_zone);
    }
}
