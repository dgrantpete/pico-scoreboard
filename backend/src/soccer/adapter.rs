//! `scoreboard-espn::soccer` extracts → the soccer domain DTOs, plus the
//! streaming entry points the handlers call. Soccer is two documents: the
//! league scoreboard (list + detail) and, for live games only, the per-event
//! summary whose single wire-relevant field is the latest commentary line.

use scoreboard_espn::common::{ListRow, ListSink};
use scoreboard_espn::soccer::{
    self, CommentaryExtract, ExtractError, GameOutcome, SoccerExtract, TransformError,
};
use scoreboard_wire as wire;

use crate::error::AppError;
use crate::espn::adapt::{
    SCRATCH_LEN, TracingQuirks, TransformKind, absent_verdict, domain_colors, domain_state,
    events_malformed, stream_error, transform_error, warn_failed_events,
};
use crate::shared::game::{GameListEntry, Side};
use crate::shared::team::TeamState;

use super::types::{
    Commentary, EventKind, LastEvent, SoccerFinalFlavor, SoccerFinalGame, SoccerFinalTeam,
    SoccerGame, SoccerLiveGame, SoccerPregameGame, SoccerPregameTeam,
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

/// Games list: every clean event with a competition, in body order.
pub(crate) fn list_entries(bytes: &[u8], url: &str) -> Result<Vec<GameListEntry>, AppError> {
    let mut scratch = vec![0u8; SCRATCH_LEN];
    let mut extractor =
        soccer::ListExtractor::new(Entries::default(), TracingQuirks::new("soccer"), &mut scratch)
            .map_err(|e| stream_error(url, e))?;
    extractor.write(bytes).map_err(|e| stream_error(url, e))?;
    let report = extractor.finish().map_err(|e| match e {
        ExtractError::Stream(e) => stream_error(url, e),
        ExtractError::MalformedBody => events_malformed(url),
        // List mode has no target; the transform tier never fires.
        ExtractError::Transform(kind) => transform_error(transform_kind(kind), url),
    })?;
    warn_failed_events(url, u64::from(report.failed));
    Ok(report.entries.0)
}

/// Detail front half: extract the target event (the handler fetches the
/// summary afterwards for live games, then builds the DTO).
pub(crate) fn detail_extract(
    bytes: &[u8],
    game_id: &str,
    url: &str,
) -> Result<SoccerExtract, AppError> {
    let mut scratch = vec![0u8; SCRATCH_LEN];
    let mut extractor =
        soccer::GameExtractor::new(game_id, TracingQuirks::new("soccer"), &mut scratch)
            .map_err(|e| stream_error(url, e))?;
    extractor.write(bytes).map_err(|e| stream_error(url, e))?;
    let report = extractor.finish().map_err(|e| match e {
        ExtractError::Stream(e) => stream_error(url, e),
        ExtractError::MalformedBody => events_malformed(url),
        ExtractError::Transform(kind) => transform_error(transform_kind(kind), url),
    })?;
    warn_failed_events(url, u64::from(report.failed));
    match report.outcome {
        GameOutcome::Found(extract) => Ok(extract),
        // The id WAS on the board, just competition-less: 404 regardless of
        // the failure count, exactly like the handler's post-find check.
        GameOutcome::NoCompetition => Err(AppError::GameNotFound(game_id.to_string())),
        GameOutcome::Absent => Err(absent_verdict(game_id, u64::from(report.failed), url)),
    }
}

fn transform_kind(error: TransformError) -> TransformKind {
    match error {
        TransformError::Sides => TransformKind::HomeAway,
        TransformError::Color => TransformKind::Color,
        TransformError::Score => TransformKind::Score,
        TransformError::Date => TransformKind::StartTime,
    }
}

/// The latest commentary line from a summary body, best-effort: a malformed
/// summary degrades to `None` with a warn (the caller already treats a
/// failed fetch the same way).
pub(crate) fn summary_commentary(bytes: &[u8], url: &str) -> Option<CommentaryExtract> {
    let mut scratch = vec![0u8; SCRATCH_LEN];
    let mut extractor = match soccer::SummaryExtractor::new(&mut scratch) {
        Ok(extractor) => extractor,
        Err(e) => {
            tracing::warn!(url = %url, error = ?e, "soccer summary extractor failed to construct; serving live without commentary");
            return None;
        }
    };
    let outcome = extractor
        .write(bytes)
        .and_then(|()| extractor.finish());
    match outcome {
        Ok(outcome) => {
            if outcome.malformed {
                tracing::warn!(url = %url, "soccer summary failed the lenient parse; serving live without commentary");
            }
            outcome.commentary
        }
        Err(e) => {
            tracing::warn!(url = %url, error = ?e, "soccer summary body unreadable; serving live without commentary");
            None
        }
    }
}

/// Extract → DTO. `commentary` comes from the summary pass (live only) and
/// is `None` for pregame/final by construction.
pub(crate) fn game_from_extract(
    extract: &SoccerExtract,
    commentary: Option<CommentaryExtract>,
) -> SoccerGame {
    match extract {
        SoccerExtract::Pregame(game) => SoccerGame::Pregame(SoccerPregameGame {
            game_id: game.game_id.as_str().to_owned(),
            start_time: game.start_time,
            venue: game.venue.as_str().to_owned(),
            home: SoccerPregameTeam {
                abbreviation: game.home.abbreviation.as_str().to_owned(),
                colors: domain_colors(game.home.colors),
            },
            away: SoccerPregameTeam {
                abbreviation: game.away.abbreviation.as_str().to_owned(),
                colors: domain_colors(game.away.colors),
            },
        }),
        SoccerExtract::Live(game) => SoccerGame::Live(SoccerLiveGame {
            game_id: game.game_id.as_str().to_owned(),
            clock: game.clock.as_str().to_owned(),
            clock_seconds: game.clock_seconds,
            half: game.half,
            on_break: game.on_break,
            home: live_team(&game.home),
            away: live_team(&game.away),
            last_event: game.last_event.as_ref().map(|event| LastEvent {
                text: event.text.as_str().to_owned(),
                kind: match event.kind {
                    wire::soccer::EventKind::Goal => EventKind::Goal,
                    wire::soccer::EventKind::RedCard => EventKind::RedCard,
                },
                athlete: event.athlete.as_str().to_owned(),
                clock: event.clock.as_str().to_owned(),
                team: event.side.map(|side| match side {
                    wire::Side::Home => Side::Home,
                    wire::Side::Away => Side::Away,
                }),
            }),
            commentary: commentary.map(|commentary| Commentary {
                id: commentary.id.as_str().to_owned(),
                text: commentary.text.as_str().to_owned(),
            }),
        }),
        SoccerExtract::Final(game) => SoccerGame::Final(SoccerFinalGame {
            game_id: game.game_id.as_str().to_owned(),
            flavor: match game.flavor {
                wire::soccer::FinalFlavor::FullTime => SoccerFinalFlavor::FullTime,
                wire::soccer::FinalFlavor::AfterExtraTime => SoccerFinalFlavor::AfterExtraTime,
                wire::soccer::FinalFlavor::AfterPenalties => SoccerFinalFlavor::AfterPenalties,
            },
            home: final_team(&game.home),
            away: final_team(&game.away),
        }),
    }
}

fn live_team(team: &soccer::LiveTeamExtract) -> TeamState {
    TeamState {
        abbreviation: team.abbreviation.as_str().to_owned(),
        score: team.score,
        colors: domain_colors(team.colors),
    }
}

fn final_team(team: &soccer::FinalTeamExtract) -> SoccerFinalTeam {
    SoccerFinalTeam {
        abbreviation: team.abbreviation.as_str().to_owned(),
        score: team.score,
        colors: domain_colors(team.colors),
        scorers: team.scorers.as_str().to_owned(),
    }
}

/// The `find_event` verdicts, re-pinned at the adapter seam (the serde-era
/// tests lived on the deleted `espn::types::find_event`).
#[cfg(test)]
mod tests {
    use super::*;

    const CLEAN_EVENT: &str = r#"{"id":"401","date":"2026-06-27T19:00Z","competitions":[]}"#;

    #[test]
    fn absent_id_with_clean_parse_is_not_found() {
        let body = format!(r#"{{"events":[{CLEAN_EVENT}]}}"#);
        let err = detail_extract(body.as_bytes(), "999", "test://sb").err().unwrap();
        assert!(matches!(err, AppError::GameNotFound(id) if id == "999"));
    }

    #[test]
    fn absent_id_with_glitched_parse_is_upstream_error_not_404() {
        let body = format!(r#"{{"events":[{CLEAN_EVENT},{{}}]}}"#);
        let err = detail_extract(body.as_bytes(), "999", "test://sb").err().unwrap();
        assert!(matches!(err, AppError::EspnDeserialize { .. }));
    }

    #[test]
    fn found_id_without_competition_is_404_even_when_glitched() {
        let body = format!(r#"{{"events":[{CLEAN_EVENT},{{}}]}}"#);
        let err = detail_extract(body.as_bytes(), "401", "test://sb").err().unwrap();
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

    #[test]
    fn malformed_summary_degrades_to_no_commentary() {
        assert_eq!(
            summary_commentary(br#"{"commentary":"glitch"}"#, "test://summary"),
            None
        );
        assert_eq!(summary_commentary(br#"not json"#, "test://summary"), None);
    }
}
