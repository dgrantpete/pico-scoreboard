//! `scoreboard-espn::football` extracts → the football domain DTOs, plus
//! the two streaming entry points the handlers call. The football extract
//! keeps its storage private and exposes the borrowed wire view
//! (`GameExtract::as_game`), so the DTO is built from that view — it covers
//! every football DTO field.

use scoreboard_espn::common::{ListRow, ListSink};
use scoreboard_espn::football::{self, DetailOutcome, ExtractError, FootballError, GameExtract};
use scoreboard_wire as wire;

use crate::error::AppError;
use crate::espn::adapt::{
    SCRATCH_LEN, TracingQuirks, TransformKind, absent_verdict, domain_colors, domain_record,
    domain_state, events_malformed, stream_error, transform_error, warn_failed_events,
};
use crate::shared::game::{GameListEntry, LastPlay, LivePhase, Side};
use crate::shared::team::TeamState;

use super::types::{
    FootballFinalGame, FootballFinalTeam, FootballGame, FootballLiveGame, FootballPregameGame,
    FootballPregameTeam, FootballSituation, Timeouts,
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

/// Games list: every clean event with a competition, in event order.
pub(crate) fn list_entries(bytes: &[u8], url: &str) -> Result<Vec<GameListEntry>, AppError> {
    let mut scratch = vec![0u8; SCRATCH_LEN];
    let mut extractor =
        football::ListExtractor::new(Entries::default(), TracingQuirks::new("football"), &mut scratch)
            .map_err(|e| stream_error(url, e))?;
    extractor.write(bytes).map_err(|e| stream_error(url, e))?;
    let report = extractor.finish().map_err(|e| match e {
        FootballError::Stream(e) => stream_error(url, e),
        FootballError::MalformedEvents => events_malformed(url),
        // List mode has no target; the transform tier never fires.
        FootballError::Extract(e) => transform_error(transform_kind(e), url),
    })?;
    warn_failed_events(url, report.counts.failed as u64);
    Ok(report.entries.0)
}

/// Detail: extract one game or map the outcome to today's status codes.
/// `is_college` gates the pregame rank line (DESIGN.md ruling 8).
pub(crate) fn detail_game(
    bytes: &[u8],
    game_id: &str,
    is_college: bool,
    url: &str,
) -> Result<FootballGame, AppError> {
    let mut scratch = vec![0u8; SCRATCH_LEN];
    let mut extractor = football::DetailExtractor::new(
        game_id,
        is_college,
        TracingQuirks::new("football"),
        &mut scratch,
    )
    .map_err(|e| stream_error(url, e))?;
    extractor.write(bytes).map_err(|e| stream_error(url, e))?;
    let report = extractor.finish().map_err(|e| match e {
        FootballError::Stream(e) => stream_error(url, e),
        FootballError::MalformedEvents => events_malformed(url),
        FootballError::Extract(e) => transform_error(transform_kind(e), url),
    })?;
    warn_failed_events(url, report.counts.failed as u64);
    match report.outcome {
        DetailOutcome::Found(extract) => Ok(game_from_extract(&extract)),
        // The id WAS on the board, just competition-less: 404 regardless of
        // the failure count, exactly like the handler's post-find check.
        DetailOutcome::NoCompetitions => Err(AppError::GameNotFound(game_id.to_string())),
        DetailOutcome::Absent => Err(absent_verdict(game_id, report.counts.failed as u64, url)),
    }
}

fn transform_kind(error: ExtractError) -> TransformKind {
    match error {
        ExtractError::HomeAwayConflict => TransformKind::HomeAway,
        ExtractError::BadColor => TransformKind::Color,
        ExtractError::BadScore => TransformKind::Score,
        ExtractError::BadStartTime => TransformKind::StartTime,
    }
}

/// Extract → DTO through the borrowed wire view (the extract's fields are
/// private by design; the view carries every football DTO field).
pub(crate) fn game_from_extract(extract: &GameExtract) -> FootballGame {
    match extract.as_game() {
        wire::football::Game::Pregame(game) => FootballGame::Pregame(FootballPregameGame {
            game_id: game.game_id.to_owned(),
            start_time: game.start_time,
            venue: game.venue.to_owned(),
            home: pregame_team(&game.home),
            away: pregame_team(&game.away),
        }),
        wire::football::Game::Live(game) => FootballGame::Live(FootballLiveGame {
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
            situation: game.situation.map(|situation| FootballSituation {
                down: situation.down,
                distance: situation.distance,
                yard_line: situation.yard_line,
                possession: match situation.possession {
                    wire::Side::Home => Side::Home,
                    wire::Side::Away => Side::Away,
                },
                red_zone: situation.red_zone,
            }),
            timeouts: game.timeouts.map(|timeouts| Timeouts {
                away: timeouts.away,
                home: timeouts.home,
            }),
            last_play: game.last_play.map(|play| LastPlay {
                id: play.id.to_owned(),
                text: play.text.to_owned(),
            }),
        }),
        wire::football::Game::Final(game) => FootballGame::Final(FootballFinalGame {
            game_id: game.game_id.to_owned(),
            periods_played: game.periods_played,
            home: final_team(&game.home),
            away: final_team(&game.away),
        }),
    }
}

fn pregame_team(team: &wire::football::PregameTeam<'_>) -> FootballPregameTeam {
    FootballPregameTeam {
        abbreviation: team.abbreviation.to_owned(),
        colors: domain_colors(team.colors),
        record: team.record.map(domain_record),
        rank_line: team.rank_line.map(str::to_owned),
    }
}

fn live_team(team: &wire::TeamState<'_>) -> TeamState {
    TeamState {
        abbreviation: team.abbreviation.to_owned(),
        // The football extract narrows to the wire's u16 at store time; the
        // JSON DTO keeps its historical u32 field.
        score: u32::from(team.score),
        colors: domain_colors(team.colors),
    }
}

fn final_team(team: &wire::FinalTeam<'_>) -> FootballFinalTeam {
    FootballFinalTeam {
        abbreviation: team.abbreviation.to_owned(),
        score: u32::from(team.score),
        colors: domain_colors(team.colors),
        line_score: team.line_score.to_vec(),
    }
}

/// The `find_event` verdicts, re-pinned at the adapter seam (the serde-era
/// tests lived on the deleted `espn::types::find_event`).
#[cfg(test)]
mod tests {
    use super::*;

    const CLEAN_EVENT: &str = r#"{"id":"401","date":"2026-01-12T21:30Z","competitions":[]}"#;

    // The crate gap this file originally noted is closed: the football lane
    // gained its bare events probe and the engine ContainerKind, so scalar
    // AND object shells 502 like every other sport.
    #[test]
    fn events_not_an_array_is_upstream_error() {
        for body in [
            r#"{"events":42}"#,
            r#"{"events":null}"#,
            r#"{"events":{"x":1}}"#,
        ] {
            let err = detail_game(body.as_bytes(), "999", false, "test://sb").err().unwrap();
            assert!(matches!(err, AppError::EspnDeserialize { .. }), "detail {body}");
            let err = list_entries(body.as_bytes(), "test://sb").err().unwrap();
            assert!(matches!(err, AppError::EspnDeserialize { .. }), "list {body}");
        }
    }

    #[test]
    fn absent_id_with_clean_parse_is_not_found() {
        let body = format!(r#"{{"events":[{CLEAN_EVENT}]}}"#);
        let err = detail_game(body.as_bytes(), "999", false, "test://sb").err().unwrap();
        assert!(matches!(err, AppError::GameNotFound(id) if id == "999"));
    }

    #[test]
    fn absent_id_with_glitched_parse_is_upstream_error_not_404() {
        let body = format!(r#"{{"events":[{CLEAN_EVENT},{{}}]}}"#);
        let err = detail_game(body.as_bytes(), "999", false, "test://sb").err().unwrap();
        assert!(matches!(err, AppError::EspnDeserialize { .. }));
    }

    #[test]
    fn found_id_without_competition_is_404_even_when_glitched() {
        let body = format!(r#"{{"events":[{CLEAN_EVENT},{{}}]}}"#);
        let err = detail_game(body.as_bytes(), "401", false, "test://sb").err().unwrap();
        assert!(matches!(err, AppError::GameNotFound(id) if id == "401"));
    }
}
