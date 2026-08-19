//! `scoreboard-espn::nba` extracts → the NBA domain DTOs, plus the two
//! streaming entry points the handlers call. The NBA lane exposes a bare
//! sink (`nba::Extractor`) rather than wrapped extractors, so this adapter
//! drives `StreamMatcher` itself.

use scoreboard_espn::StreamMatcher;
use scoreboard_espn::common::{ListRow, ListSink, LivePhase as ExtractPhase};
use scoreboard_espn::nba::{self, DetailOutcome, Extract, Kind, TransformError};

use crate::error::AppError;
use crate::espn::adapt::{
    SCRATCH_LEN, TracingQuirks, TransformKind, absent_verdict, domain_colors, domain_record,
    domain_state, events_malformed, stream_error, transform_error, warn_failed_events,
};
use crate::shared::game::{GameListEntry, LastPlay, LivePhase};
use crate::shared::team::TeamState;

use super::types::{
    NbaFinalGame, NbaFinalTeam, NbaGame, NbaLiveGame, NbaPregameGame, NbaPregameTeam,
};

#[derive(Default)]
struct Entries(Vec<GameListEntry>);

impl ListSink for Entries {
    fn row(&mut self, row: ListRow<'_>) {
        self.0.push(GameListEntry {
            id: row.id.to_string(),
            state: domain_state(row.state),
        });
    }
}

/// Games list: every clean event with a competition — NBA's list state is
/// total, no exclusions.
pub(crate) fn list_entries(bytes: &[u8], url: &str) -> Result<Vec<GameListEntry>, AppError> {
    let mut quirks = TracingQuirks::new("nba");
    let extractor = nba::Extractor::games_list(Entries::default(), &mut quirks);
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
    let entries = sink
        .into_list()
        .expect("extractor was constructed in list mode");
    Ok(entries.0)
}

/// Detail: extract one game or map the outcome to today's status codes.
pub(crate) fn detail_game(bytes: &[u8], game_id: &str, url: &str) -> Result<NbaGame, AppError> {
    let mut quirks = TracingQuirks::new("nba");
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
        DetailOutcome::Found(extract) => Ok(game_from_extract(&extract)),
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

/// Extract → DTO. Field-for-field; scores stay `u32` like the JSON always
/// carried them (the wire encoder does the saturating, not the DTO).
pub(crate) fn game_from_extract(extract: &Extract) -> NbaGame {
    let game_id = extract.game_id.as_str().to_owned();
    match &extract.kind {
        Kind::Pregame(game) => NbaGame::Pregame(NbaPregameGame {
            game_id,
            start_time: game.start_time,
            venue: game.venue.as_str().to_owned(),
            home: pregame_team(&game.home),
            away: pregame_team(&game.away),
        }),
        Kind::Live(game) => NbaGame::Live(NbaLiveGame {
            game_id,
            period: game.period,
            clock: game.clock.as_str().to_owned(),
            phase: match game.phase {
                ExtractPhase::InProgress => LivePhase::InProgress,
                ExtractPhase::Halftime => LivePhase::Halftime,
                ExtractPhase::EndOfPeriod => LivePhase::EndOfPeriod,
            },
            home: live_team(&game.home),
            away: live_team(&game.away),
            last_play: game.last_play.as_ref().map(|play| LastPlay {
                id: play.id.as_str().to_owned(),
                text: play.text.as_str().to_owned(),
            }),
        }),
        Kind::Final(game) => NbaGame::Final(NbaFinalGame {
            game_id,
            periods_played: game.periods_played,
            home: final_team(&game.home),
            away: final_team(&game.away),
        }),
    }
}

fn pregame_team(team: &nba::PregameTeam) -> NbaPregameTeam {
    NbaPregameTeam {
        abbreviation: team.abbreviation.as_str().to_owned(),
        colors: domain_colors(team.colors),
        record: team.record.map(domain_record),
    }
}

fn live_team(team: &nba::LiveTeam) -> TeamState {
    TeamState {
        abbreviation: team.abbreviation.as_str().to_owned(),
        score: team.score,
        colors: domain_colors(team.colors),
    }
}

fn final_team(team: &nba::FinalTeam) -> NbaFinalTeam {
    NbaFinalTeam {
        abbreviation: team.abbreviation.as_str().to_owned(),
        score: team.score,
        colors: domain_colors(team.colors),
        line_score: team.line_score.to_vec(),
    }
}

/// The `find_event` verdicts, re-pinned at the adapter seam (the serde-era
/// tests lived on the deleted `espn::types::find_event`).
#[cfg(test)]
mod tests {
    use super::*;

    const CLEAN_EVENT: &str = r#"{"id":"401","date":"2026-04-11T02:30Z","competitions":[]}"#;

    #[test]
    fn absent_id_with_clean_parse_is_not_found() {
        let body = format!(r#"{{"events":[{CLEAN_EVENT}]}}"#);
        let err = detail_game(body.as_bytes(), "999", "test://sb").err().unwrap();
        assert!(matches!(err, AppError::GameNotFound(id) if id == "999"));
    }

    #[test]
    fn absent_id_with_glitched_parse_is_upstream_error_not_404() {
        let body = format!(r#"{{"events":[{CLEAN_EVENT},{{}}]}}"#);
        let err = detail_game(body.as_bytes(), "999", "test://sb").err().unwrap();
        assert!(matches!(err, AppError::EspnDeserialize { .. }));
    }

    #[test]
    fn found_id_without_competition_is_404_even_when_glitched() {
        let body = format!(r#"{{"events":[{CLEAN_EVENT},{{}}]}}"#);
        let err = detail_game(body.as_bytes(), "401", "test://sb").err().unwrap();
        assert!(matches!(err, AppError::GameNotFound(id) if id == "401"));
    }

    #[test]
    fn events_not_an_array_is_upstream_error() {
        let body = br#"{"events":"glitch"}"#;
        let err = detail_game(body, "401", "test://sb").err().unwrap();
        assert!(matches!(err, AppError::EspnDeserialize { .. }));
        let err = list_entries(body, "test://sb").err().unwrap();
        assert!(matches!(err, AppError::EspnDeserialize { .. }));
    }
}
