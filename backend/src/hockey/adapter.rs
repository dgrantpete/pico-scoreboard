//! `scoreboard-espn::hockey` extracts → the NHL domain DTOs, plus the
//! streaming entry points the handlers call. Hockey is two documents like
//! soccer: the league scoreboard (list + detail) and, for live and final
//! games, the per-event summary (NHL.md D3). The extractor combines the two
//! itself (`Extract::as_game`) — the shootout's tied score, which shots
//! source wins, the one-source flash — so the DTO is built from that combined
//! view rather than from the two extracts, and cannot disagree with the wire.

use scoreboard_espn::hockey::{
    Extract, ExtractError, GameExtractor, GameOutcome, Kind, ListExtractor, Summary,
    SummaryExtractor, TransformError,
};
use scoreboard_wire as wire;

use crate::error::AppError;
use crate::espn::adapt::{
    SCRATCH_LEN, TracingQuirks, TransformKind, absent_verdict, domain_colors, domain_state,
    events_malformed, stream_error, transform_error, warn_failed_events,
};
use crate::shared::game::{GameListEntry, LastPlay, LivePhase, Side};
use crate::shared::team::TeamState;

use super::types::{
    GoalTag, HockeyCoordinate, HockeyFinalGame, HockeyFinalTeam, HockeyGame, HockeyGoal,
    HockeyLiveGame, HockeyPregameGame, HockeyPregameTeam, HockeyRecord, HockeyShootout,
    HockeyShots, HockeyStar, HockeyStars, HockeyStrength, ShootoutAttempt, StrengthSituation,
};

/// Games list: every clean event with a competition, in body order. A delayed
/// start that has not dropped the puck lists as a pregame (the extractor's
/// resolved state).
pub(crate) fn list_entries(bytes: &[u8], url: &str) -> Result<Vec<GameListEntry>, AppError> {
    let mut scratch = vec![0u8; SCRATCH_LEN];
    let mut extractor = ListExtractor::new(&mut scratch).map_err(|e| stream_error(url, e))?;
    extractor.write(bytes).map_err(|e| stream_error(url, e))?;
    let list = extractor.finish().map_err(|e| extract_error(e, url))?;
    if list.overflowed {
        tracing::warn!(url = %url, "hockey games list overflowed the extractor's bound — excess dropped");
    }
    warn_failed_events(url, u64::from(list.failed));
    Ok(list
        .games
        .iter()
        .map(|entry| GameListEntry {
            id: entry.id.as_str().to_owned(),
            state: domain_state(entry.state),
        })
        .collect())
}

/// Detail front half: extract the target event. The handler then fetches the
/// summary when [`wants_summary`] says so and builds the DTO.
pub(crate) fn detail_extract(bytes: &[u8], game_id: &str, url: &str) -> Result<Extract, AppError> {
    let mut scratch = vec![0u8; SCRATCH_LEN];
    let mut extractor = GameExtractor::new(game_id, TracingQuirks::new("hockey"), &mut scratch)
        .map_err(|e| stream_error(url, e))?;
    extractor.write(bytes).map_err(|e| stream_error(url, e))?;
    let report = extractor.finish().map_err(|e| extract_error(e, url))?;
    warn_failed_events(url, u64::from(report.failed));
    match report.outcome {
        GameOutcome::Found(extract) => Ok(extract),
        // The id WAS on the board, just competition-less: 404 regardless of
        // the failure count, exactly like the other sports.
        GameOutcome::NoCompetition => Err(AppError::GameNotFound(game_id.to_string())),
        GameOutcome::Absent => Err(absent_verdict(game_id, u64::from(report.failed), url)),
    }
}

fn extract_error(error: ExtractError, url: &str) -> AppError {
    match error {
        ExtractError::Stream(e) => stream_error(url, e),
        ExtractError::MalformedBody => events_malformed(url),
        ExtractError::Transform(kind) => transform_error(
            match kind {
                TransformError::Sides => TransformKind::HomeAway,
                TransformError::Color => TransformKind::Color,
                TransformError::Score => TransformKind::Score,
                TransformError::Date => TransformKind::StartTime,
            },
            url,
        ),
    }
}

/// D3: live and final games read the summary — strength, goals, the flash
/// and the shootout live, the reliable shots on a final. A pregame has
/// nothing in it the payload uses.
pub(crate) fn wants_summary(extract: &Extract) -> bool {
    !matches!(extract.kind, Kind::Pregame(_))
}

/// The summary's contribution, best-effort: an unreadable body degrades to
/// `None` with a warn, as a failed fetch does, and the game is served from the
/// scoreboard alone. Only the stream itself can fail; a malformed region
/// inside a readable summary already degrades field by field in the crate.
pub(crate) fn summary(bytes: &[u8], extract: &Extract, url: &str) -> Option<Summary> {
    let mut scratch = vec![0u8; SCRATCH_LEN];
    let mut extractor = match SummaryExtractor::new(
        extract.summary_context(),
        TracingQuirks::new("hockey"),
        &mut scratch,
    ) {
        Ok(extractor) => extractor,
        Err(e) => {
            tracing::warn!(url = %url, error = ?e, "hockey summary extractor failed to construct; serving the scoreboard alone");
            return None;
        }
    };
    let report = extractor.write(bytes).and_then(|()| extractor.finish());
    match report {
        Ok(report) => Some(report.summary),
        Err(e) => {
            tracing::warn!(url = %url, error = ?e, "hockey summary body unreadable; serving the scoreboard alone");
            None
        }
    }
}

/// Extract (+ summary) → DTO, through the crate's own combiner.
pub(crate) fn game_from_extract(extract: &Extract, summary: Option<&Summary>) -> HockeyGame {
    game_from_view(&extract.as_game(summary))
}

/// The combined wire view → DTO, field for field. `wire.rs` is the inverse,
/// and the corpus harness checks the two compose to the identity.
fn game_from_view(game: &wire::hockey::Game<'_>) -> HockeyGame {
    match game {
        wire::hockey::Game::Pregame(game) => HockeyGame::Pregame(HockeyPregameGame {
            game_id: game.game_id.to_owned(),
            start_time: game.start_time,
            venue: game.venue.to_owned(),
            info_secondary: game.info_secondary.to_owned(),
            playoffs: game.playoffs,
            home: pregame_team(&game.home),
            away: pregame_team(&game.away),
        }),
        wire::hockey::Game::Live(game) => HockeyGame::Live(HockeyLiveGame {
            game_id: game.game_id.to_owned(),
            period: game.period,
            phase: match game.phase {
                wire::LivePhase::InProgress => LivePhase::InProgress,
                wire::LivePhase::Halftime => LivePhase::Halftime,
                wire::LivePhase::EndOfPeriod => LivePhase::EndOfPeriod,
            },
            clock: game.clock.to_owned(),
            playoffs: game.playoffs,
            home: live_team(&game.home),
            away: live_team(&game.away),
            shots: game.shots.map(shots),
            strength: game.strength.map(|strength| HockeyStrength {
                situation: match strength.kind {
                    wire::hockey::StrengthKind::Even => StrengthSituation::Even,
                    wire::hockey::StrengthKind::PowerPlay(side) => StrengthSituation::PowerPlay {
                        side: domain_side(side),
                    },
                    wire::hockey::StrengthKind::Matched => StrengthSituation::Matched,
                    wire::hockey::StrengthKind::EmptyNet(side) => StrengthSituation::EmptyNet {
                        side: domain_side(side),
                    },
                },
                away_skaters: strength.away_skaters,
                home_skaters: strength.home_skaters,
            }),
            goals: game.goals.iter().map(goal).collect(),
            last_scorer: game.last_scorer.to_owned(),
            shootout: game.shootout.map(|shootout| HockeyShootout {
                away: attempts(shootout.away),
                home: attempts(shootout.home),
                next_shooter: shootout.next_shooter.map(domain_side),
            }),
            last_play: game.last_play.map(|play| LastPlay {
                id: play.id.to_owned(),
                text: play.text.to_owned(),
            }),
        }),
        wire::hockey::Game::Final(game) => HockeyGame::Final(HockeyFinalGame {
            game_id: game.game_id.to_owned(),
            periods_played: game.periods_played,
            playoffs: game.playoffs,
            home: final_team(&game.home),
            away: final_team(&game.away),
            shots: game.shots.map(shots),
            stars: game.stars.map(|[first, second, third]| HockeyStars {
                first: star(first),
                second: star(second),
                third: star(third),
            }),
            series: game.series.map(str::to_owned),
        }),
    }
}

fn domain_side(side: wire::Side) -> Side {
    match side {
        wire::Side::Home => Side::Home,
        wire::Side::Away => Side::Away,
    }
}

fn pregame_team(team: &wire::hockey::PregameTeam<'_>) -> HockeyPregameTeam {
    HockeyPregameTeam {
        abbreviation: team.abbreviation.to_owned(),
        colors: domain_colors(team.colors),
        record: team.record.map(|record| HockeyRecord {
            wins: record.wins,
            losses: record.losses,
            ot_losses: record.ot_losses,
        }),
        goalie: team.goalie.map(str::to_owned),
    }
}

fn live_team(team: &wire::TeamState<'_>) -> TeamState {
    TeamState {
        abbreviation: team.abbreviation.to_owned(),
        score: u32::from(team.score),
        colors: domain_colors(team.colors),
    }
}

fn final_team(team: &wire::FinalTeam<'_>) -> HockeyFinalTeam {
    HockeyFinalTeam {
        abbreviation: team.abbreviation.to_owned(),
        score: u32::from(team.score),
        colors: domain_colors(team.colors),
        line_score: team.line_score.to_vec(),
    }
}

fn shots(shots: wire::hockey::Shots) -> HockeyShots {
    HockeyShots {
        away: shots.away,
        home: shots.home,
    }
}

fn goal(goal: wire::hockey::Goal) -> HockeyGoal {
    HockeyGoal {
        side: domain_side(goal.side),
        tag: match goal.tag {
            wire::hockey::GoalTag::Goal => GoalTag::Goal,
            wire::hockey::GoalTag::PowerPlay => GoalTag::PowerPlay,
            wire::hockey::GoalTag::ShortHanded => GoalTag::ShortHanded,
            wire::hockey::GoalTag::EmptyNet => GoalTag::EmptyNet,
        },
        period: goal.period,
        elapsed: goal.elapsed,
        coordinate: goal.coordinate.map(|coordinate| HockeyCoordinate {
            x: coordinate.x,
            y: coordinate.y,
        }),
    }
}

/// A side's goal mask unrolled into its attempts. The wire carries at most
/// [`wire::hockey::MAX_SHOOTOUT_ATTEMPTS`] per side — the mask's width — so
/// that is all the JSON shows too.
fn attempts(side: wire::hockey::ShootoutAttempts) -> Vec<ShootoutAttempt> {
    let taken = usize::from(side.taken).min(wire::hockey::MAX_SHOOTOUT_ATTEMPTS);
    (0..taken)
        .map(|attempt| ShootoutAttempt {
            scored: side.scored & (1 << attempt) != 0,
        })
        .collect()
}

fn star(star: wire::hockey::Star<'_>) -> HockeyStar {
    HockeyStar {
        name: star.name.to_owned(),
        side: star.side.map(domain_side),
    }
}

/// The `find_event` verdicts and the summary degrade, pinned at the adapter
/// seam like the other sports'.
#[cfg(test)]
mod tests {
    use super::*;

    const CLEAN_EVENT: &str = r#"{"id":"401","date":"2026-10-14T23:00Z","competitions":[]}"#;

    #[test]
    fn absent_id_with_clean_parse_is_not_found() {
        let body = format!(r#"{{"events":[{CLEAN_EVENT}]}}"#);
        let err = detail_extract(body.as_bytes(), "999", "test://sb")
            .err()
            .unwrap();
        assert!(matches!(err, AppError::GameNotFound(id) if id == "999"));
    }

    #[test]
    fn absent_id_with_glitched_parse_is_upstream_error_not_404() {
        let body = format!(r#"{{"events":[{CLEAN_EVENT},{{}}]}}"#);
        let err = detail_extract(body.as_bytes(), "999", "test://sb")
            .err()
            .unwrap();
        assert!(matches!(err, AppError::EspnDeserialize { .. }));
    }

    #[test]
    fn found_id_without_competition_is_404_even_when_glitched() {
        let body = format!(r#"{{"events":[{CLEAN_EVENT},{{}}]}}"#);
        let err = detail_extract(body.as_bytes(), "401", "test://sb")
            .err()
            .unwrap();
        assert!(matches!(err, AppError::GameNotFound(id) if id == "401"));
    }

    #[test]
    fn events_not_an_array_is_upstream_error() {
        let body = br#"{"events":"glitch"}"#;
        let err = detail_extract(body, "401", "test://sb").err().unwrap();
        assert!(matches!(err, AppError::EspnDeserialize { .. }));
        let err = list_entries(body, "test://sb").err().unwrap();
        assert!(matches!(err, AppError::EspnDeserialize { .. }));
    }

    /// One live corpus event, as the handler would have it from the board.
    fn live_extract() -> Extract {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/hockey/nhl/in_progress.json"
        );
        let event = std::fs::read_to_string(path).expect("in_progress fixture");
        let body = format!(r#"{{"events":[{event}]}}"#);
        detail_extract(body.as_bytes(), "401892503", "test://sb").expect("live event")
    }

    #[test]
    fn an_unreadable_summary_degrades_to_the_scoreboard_alone() {
        let extract = live_extract();
        assert!(wants_summary(&extract));
        assert!(summary(br#"not json"#, &extract, "test://summary").is_none());
        // Truncated mid-document: the stream never closes.
        assert!(summary(br#"{"plays":[{"id":"1""#, &extract, "test://summary").is_none());
    }

    #[test]
    fn a_readable_summary_with_no_facts_still_serves() {
        let extract = live_extract();
        let summary = summary(br#"{"plays":"glitch"}"#, &extract, "test://summary")
            .expect("a readable body is a summary, however thin");
        let HockeyGame::Live(game) = game_from_extract(&extract, Some(&summary)) else {
            panic!("in_progress is live");
        };
        assert!(game.goals.is_empty());
    }

    #[test]
    fn a_pregame_reads_no_summary() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/hockey/nhl/pregame.json"
        );
        let event = std::fs::read_to_string(path).expect("pregame fixture");
        let body = format!(r#"{{"events":[{event}]}}"#);
        let extract = detail_extract(body.as_bytes(), "401892505", "test://sb").unwrap();
        assert!(!wants_summary(&extract));
    }
}
