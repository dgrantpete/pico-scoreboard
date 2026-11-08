//! `scoreboard-espn::nba` extracts → the NBA domain DTOs, plus the streaming
//! entry points the handlers call. The NBA is two documents like hockey: the
//! league scoreboard (list + detail) and, for live games only, the per-event
//! summary (NBA.md D3). The extractor combines the two itself
//! (`Extract::as_game`) — the one-source flash, which summary groups a
//! payload carries — so the DTO is built from that combined view rather than
//! from the two extracts, and cannot disagree with the wire. The scoreboard
//! lane exposes a bare sink (`nba::Extractor`) rather than wrapped
//! extractors, so this adapter drives `StreamMatcher` itself for it.

use scoreboard_espn::StreamMatcher;
use scoreboard_espn::nba::{
    self, DetailOutcome, Extract, Kind, Summary, SummaryExtractor, TransformError,
};
use scoreboard_wire as wire;

use crate::error::AppError;
use crate::espn::adapt::{
    SCRATCH_LEN, TracingQuirks, TransformKind, absent_verdict, domain_colors, domain_record,
    domain_side, domain_state, events_malformed, stream_error, transform_error, warn_failed_events,
};
use crate::shared::game::{GameListEntry, LastPlay, LivePhase};
use crate::shared::team::TeamState;

use super::types::{
    NbaFinalGame, NbaFinalTeam, NbaGame, NbaLeader, NbaLeaders, NbaLedger, NbaLiveGame, NbaMake,
    NbaNewestScore, NbaPregameGame, NbaPregameTeam, NbaRecords, NbaRun, NbaTeamLedger,
};

/// Games list: every clean event with a competition — NBA's list state is
/// total, no exclusions.
pub(crate) fn list_entries(bytes: &[u8], url: &str) -> Result<Vec<GameListEntry>, AppError> {
    let mut entries: Vec<GameListEntry> = Vec::new();
    let mut on_game = |id: &str, state: wire::GameState| {
        entries.push(GameListEntry {
            id: id.to_string(),
            state: domain_state(state),
        });
    };
    let mut quirks = TracingQuirks::new("nba");
    let extractor = nba::Extractor::games_list(&mut on_game, &mut quirks);
    let mut scratch = vec![0u8; SCRATCH_LEN];
    let mut matcher =
        StreamMatcher::new(nba::PATHS, extractor, &mut scratch).map_err(|e| stream_error(url, e))?;
    matcher.write(bytes).map_err(|e| stream_error(url, e))?;
    let sink = matcher.finish().map_err(|e| stream_error(url, e))?;
    let stats = sink.stats();
    drop(sink);
    if stats.events_malformed {
        return Err(events_malformed(url));
    }
    warn_failed_events(url, stats.failed as u64);
    Ok(entries)
}

/// Detail front half: extract the target event or map the outcome to today's
/// status codes. The handler then fetches the summary when [`wants_summary`]
/// says so and builds the DTO.
pub(crate) fn detail_extract(bytes: &[u8], game_id: &str, url: &str) -> Result<Extract, AppError> {
    let mut quirks = TracingQuirks::for_event("nba", game_id);
    let extractor = nba::Extractor::game_detail(game_id, &mut quirks);
    let mut scratch = vec![0u8; SCRATCH_LEN];
    let mut matcher =
        StreamMatcher::new(nba::PATHS, extractor, &mut scratch).map_err(|e| stream_error(url, e))?;
    matcher.write(bytes).map_err(|e| stream_error(url, e))?;
    let sink = matcher.finish().map_err(|e| stream_error(url, e))?;
    let stats = sink.stats();
    if stats.events_malformed {
        return Err(events_malformed(url));
    }
    warn_failed_events(url, stats.failed as u64);
    let outcome = sink
        .into_detail()
        .expect("extractor was constructed in detail mode");
    match outcome {
        DetailOutcome::Found(extract) => Ok(extract),
        DetailOutcome::Rejected(kind) => Err(transform_error(
            match kind {
                TransformError::Color => TransformKind::Color,
                TransformError::Score => TransformKind::Score,
                TransformError::StartTime => TransformKind::StartTime,
                TransformError::HomeAway => TransformKind::HomeAway,
            },
            url,
        )),
        // The target exists with an empty competitions array: 404 regardless
        // of the failure count, exactly like the handler's post-find check.
        DetailOutcome::NoCompetition => Err(AppError::GameNotFound(game_id.to_string())),
        DetailOutcome::NotFound => Err(absent_verdict(game_id, stats.failed as u64, url)),
    }
}

/// D3: a live game reads the summary — the ledger, the run, the makes, the
/// newest score and the flash's second source. A final's leaders, records and
/// series and a pregame's second line are on the scoreboard, so neither
/// fetches the 110–513 KB body for nothing.
pub(crate) fn wants_summary(extract: &Extract) -> bool {
    matches!(extract.kind, Kind::Live(_))
}

/// The summary's contribution, best-effort: an unreadable body degrades to
/// `None` with a warn, as a failed fetch does, and the game is served from the
/// scoreboard alone. Only the stream itself can fail; a malformed region
/// inside a readable summary already degrades field by field in the crate.
pub(crate) fn summary(bytes: &[u8], extract: &Extract, url: &str) -> Option<Summary> {
    let mut scratch = vec![0u8; SCRATCH_LEN];
    let mut extractor = match SummaryExtractor::new(
        extract.summary_context(),
        TracingQuirks::for_event("nba", extract.game_id.as_str()),
        &mut scratch,
    ) {
        Ok(extractor) => extractor,
        Err(e) => {
            tracing::warn!(url = %url, error = ?e, "nba summary extractor failed to construct; serving the scoreboard alone");
            return None;
        }
    };
    let report = extractor.write(bytes).and_then(|()| extractor.finish());
    match report {
        Ok(report) => Some(report.summary),
        Err(e) => {
            tracing::warn!(url = %url, error = ?e, "nba summary body unreadable; serving the scoreboard alone");
            None
        }
    }
}

/// Extract (+ summary) → DTO, through the crate's own combiner.
pub(crate) fn game_from_extract(extract: &Extract, summary: Option<&Summary>) -> NbaGame {
    game_from_view(&extract.as_game(summary))
}

/// The combined wire view → DTO, field for field. `wire.rs` is the inverse,
/// and the corpus harness checks the two compose to the identity. Scores stay
/// `u32` like the JSON always carried them (the wire view saturates, so the
/// widening here is exact).
fn game_from_view(game: &wire::nba::Game<'_>) -> NbaGame {
    match game {
        wire::nba::Game::Pregame(game) => NbaGame::Pregame(NbaPregameGame {
            game_id: game.game_id.to_owned(),
            start_time: game.start_time,
            venue: game.venue.to_owned(),
            info_secondary: game.info_secondary.to_owned(),
            home: pregame_team(&game.home),
            away: pregame_team(&game.away),
        }),
        wire::nba::Game::Live(game) => NbaGame::Live(NbaLiveGame {
            game_id: game.game_id.to_owned(),
            period: game.period,
            clock: game.clock.to_owned(),
            phase: match game.phase {
                wire::LivePhase::InProgress => LivePhase::InProgress,
                wire::LivePhase::Halftime => LivePhase::Halftime,
                wire::LivePhase::EndOfPeriod => LivePhase::EndOfPeriod,
            },
            home: live_team(&game.home),
            away: live_team(&game.away),
            last_play: game.last_play.map(|play| LastPlay {
                id: play.id.to_owned(),
                text: play.text.to_owned(),
            }),
            ledger: game.ledger.map(|ledger| NbaLedger {
                away: team_ledger(ledger.away),
                home: team_ledger(ledger.home),
            }),
            run: game.run.map(|run| NbaRun {
                side: domain_side(run.side),
                points: run.points,
            }),
            newest_score: game.newest_score.map(|score| NbaNewestScore {
                side: domain_side(score.side),
                scorer: score.scorer.to_owned(),
                // The wire's 0 is "the boxscore has not listed the player".
                scorer_points: (score.scorer_points != 0).then_some(score.scorer_points),
                seq: score.seq,
                marked: score.marked,
            }),
            makes: game.makes.iter().map(make).collect(),
        }),
        wire::nba::Game::Final(game) => NbaGame::Final(NbaFinalGame {
            game_id: game.game_id.to_owned(),
            periods_played: game.periods_played,
            home: final_team(&game.home),
            away: final_team(&game.away),
            leaders: game.leaders.map(|leaders| NbaLeaders {
                points: leader(leaders.points),
                rebounds: leader(leaders.rebounds),
                assists: leader(leaders.assists),
            }),
            series: game.series.map(str::to_owned),
            records: game.records.map(|records| NbaRecords {
                away: domain_record(records.away),
                home: domain_record(records.home),
            }),
        }),
    }
}

fn pregame_team(team: &wire::nba::PregameTeam<'_>) -> NbaPregameTeam {
    NbaPregameTeam {
        abbreviation: team.abbreviation.to_owned(),
        colors: domain_colors(team.colors),
        record: team.record.map(domain_record),
        line: team.line.map(str::to_owned),
    }
}

fn live_team(team: &wire::TeamState<'_>) -> TeamState {
    TeamState {
        abbreviation: team.abbreviation.to_owned(),
        score: u32::from(team.score),
        colors: domain_colors(team.colors),
    }
}

fn final_team(team: &wire::FinalTeam<'_>) -> NbaFinalTeam {
    NbaFinalTeam {
        abbreviation: team.abbreviation.to_owned(),
        score: u32::from(team.score),
        colors: domain_colors(team.colors),
        line_score: team.line_score.to_vec(),
    }
}

fn team_ledger(column: wire::nba::TeamLedger) -> NbaTeamLedger {
    NbaTeamLedger {
        fouls: column.fouls,
        in_bonus: column.in_bonus,
        timeouts: column.timeouts,
    }
}

fn make(make: wire::nba::Make) -> NbaMake {
    NbaMake {
        side: domain_side(make.side),
        three: make.three,
        x: make.x,
        y: make.y,
    }
}

fn leader(leader: wire::nba::Leader<'_>) -> NbaLeader {
    NbaLeader {
        side: domain_side(leader.side),
        value: leader.value,
        name: leader.name.to_owned(),
    }
}

/// The `find_event` verdicts and the summary degrade, pinned at the adapter
/// seam like the other sports' (the serde-era verdict tests lived on the
/// deleted `espn::types::find_event`).
#[cfg(test)]
mod tests {
    use super::*;

    const CLEAN_EVENT: &str = r#"{"id":"401","date":"2026-04-11T02:30Z","competitions":[]}"#;

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

    /// One corpus event, as the handler would have it from the board.
    fn corpus_extract(stem: &str, id: &str) -> Extract {
        let path = format!("{}/testdata/nba/{stem}.json", env!("CARGO_MANIFEST_DIR"));
        let event = std::fs::read_to_string(path).expect("corpus fixture");
        let body = format!(r#"{{"events":[{event}]}}"#);
        detail_extract(body.as_bytes(), id, "test://sb").expect("corpus event")
    }

    #[test]
    fn an_unreadable_summary_degrades_to_the_scoreboard_alone() {
        let extract = corpus_extract("in_progress", "401909940");
        assert!(wants_summary(&extract));
        assert!(summary(br#"not json"#, &extract, "test://summary").is_none());
        // Truncated mid-document: the stream never closes.
        assert!(summary(br#"{"plays":[{"id":"1""#, &extract, "test://summary").is_none());
    }

    #[test]
    fn a_readable_summary_with_no_facts_still_serves() {
        let extract = corpus_extract("in_progress", "401909940");
        let summary = summary(br#"{"plays":"glitch"}"#, &extract, "test://summary")
            .expect("a readable body is a summary, however thin");
        let NbaGame::Live(game) = game_from_extract(&extract, Some(&summary)) else {
            panic!("in_progress is live");
        };
        assert!(game.ledger.is_none() && game.run.is_none());
        assert!(game.newest_score.is_none() && game.makes.is_empty());
    }

    /// D3: the summary is a live game's fetch only. The final screen's
    /// leaders, records and series and the pregame's second line are on the
    /// scoreboard, so neither state pays for the body.
    #[test]
    fn pregames_and_finals_read_no_summary() {
        assert!(!wants_summary(&corpus_extract("pregame", "401909944")));
        assert!(!wants_summary(&corpus_extract("final", "401909938")));
    }
}
