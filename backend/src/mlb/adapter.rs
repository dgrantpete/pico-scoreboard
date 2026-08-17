//! `scoreboard-espn::mlb` extracts → the MLB domain DTOs, plus the two
//! streaming entry points the handlers call. The 404-vs-502 verdicts here
//! reproduce `find_event` + the rain-delay veto exactly (DESIGN.md rulings
//! 1 and 14); the extraction itself is the shared crate's.

use scoreboard_espn::mlb::{self, DetailError, Extract, ListError, TransformError};

use crate::error::AppError;
use crate::espn::adapt::{
    SCRATCH_LEN, TracingQuirks, TransformKind, domain_colors, domain_record, domain_state,
    events_malformed, stream_error, transform_error, warn_failed_events,
};
use crate::shared::game::{GameListEntry, LastPlay};
use crate::shared::team::TeamState;

use super::types::{
    InningHalf, MlbAtBat, MlbBases, MlbCount, MlbFinalGame, MlbFinalTeam, MlbGame, MlbInning,
    MlbLiveGame, MlbPregameGame, MlbPregameTeam, MlbWeather,
};

struct Entries(Vec<GameListEntry>);

impl mlb::ListSink for Entries {
    fn entry(&mut self, id: &str, state: scoreboard_wire::GameState) {
        self.0.push(GameListEntry {
            id: id.to_string(),
            state: domain_state(state),
        });
    }
}

/// Games list: one entry per event that passes today's lenient parse. The
/// rain-delay veto and the empty-competitions skip happen inside the crate.
pub(crate) fn list_entries(bytes: &[u8], url: &str) -> Result<Vec<GameListEntry>, AppError> {
    let mut entries = Entries(Vec::new());
    let mut quirks = TracingQuirks::new("mlb");
    let mut scratch = vec![0u8; SCRATCH_LEN];
    let mut extractor = mlb::ListExtractor::new(&mut entries, &mut quirks, &mut scratch)
        .map_err(|e| stream_error(url, e))?;
    extractor.write(bytes).map_err(|e| stream_error(url, e))?;
    let counts = extractor.finish().map_err(|e| match e {
        ListError::Stream(e) => stream_error(url, e),
        ListError::Events => events_malformed(url),
    })?;
    warn_failed_events(url, counts.failed as u64);
    Ok(entries.0)
}

/// Detail: extract one game or map the outcome to today's status codes.
pub(crate) fn detail_game(bytes: &[u8], game_id: &str, url: &str) -> Result<MlbGame, AppError> {
    let mut quirks = TracingQuirks::new("mlb");
    let mut scratch = vec![0u8; SCRATCH_LEN];
    let mut extractor = mlb::DetailExtractor::new(game_id, &mut quirks, &mut scratch)
        .map_err(|e| stream_error(url, e))?;
    extractor.write(bytes).map_err(|e| stream_error(url, e))?;
    match extractor.finish() {
        Ok((extract, counts)) => {
            warn_failed_events(url, counts.failed as u64);
            Ok(game_from_extract(&extract))
        }
        Err(DetailError::Stream(e)) => Err(stream_error(url, e)),
        Err(DetailError::Events) => Err(events_malformed(url)),
        // Absent from a clean scoreboard, an event with no competition, or
        // the rain-delay veto — all today's 404.
        Err(DetailError::NotFound) => Err(AppError::GameNotFound(game_id.to_string())),
        // Id absent AND at least one event unparseable: the game may be
        // inside the glitched subset, so 502 — never "game ended". (The MLB
        // lane folds the verdict itself, so the exact count isn't carried.)
        Err(DetailError::Glitched) => Err(AppError::EspnDeserialize {
            url: url.to_string(),
            json_path: "events".to_string(),
            message: "event(s) unparseable; cannot distinguish 'ended' from 'glitched'"
                .to_string(),
        }),
        Err(DetailError::Transform(kind)) => Err(transform_error(
            match kind {
                TransformError::Date => TransformKind::StartTime,
                TransformError::HomeAway => TransformKind::HomeAway,
                TransformError::Score => TransformKind::Score,
                TransformError::Color => TransformKind::Color,
            },
            url,
        )),
    }
}

/// Extract → DTO. Field-for-field; the JSON keeps its wider numeric types
/// (`u32` scores, `i16` temperature) straight from the extract.
pub(crate) fn game_from_extract(extract: &Extract) -> MlbGame {
    match extract {
        Extract::Pregame(game) => MlbGame::Pregame(MlbPregameGame {
            game_id: game.game_id.as_str().to_owned(),
            start_time: game.start_time,
            venue: game.venue.as_str().to_owned(),
            weather: game.weather.as_ref().map(|weather| MlbWeather {
                condition: weather.condition.as_str().to_owned(),
                temperature: weather.temperature,
            }),
            home: pregame_team(&game.home),
            away: pregame_team(&game.away),
        }),
        Extract::Live(game) => MlbGame::Live(MlbLiveGame {
            game_id: game.game_id.as_str().to_owned(),
            inning: MlbInning {
                number: game.inning.number,
                half: match game.inning.half {
                    scoreboard_wire::mlb::InningHalf::Top => InningHalf::Top,
                    scoreboard_wire::mlb::InningHalf::Middle => InningHalf::Middle,
                    scoreboard_wire::mlb::InningHalf::Bottom => InningHalf::Bottom,
                    scoreboard_wire::mlb::InningHalf::End => InningHalf::End,
                },
            },
            home: live_team(&game.home),
            away: live_team(&game.away),
            count: MlbCount {
                balls: game.count.balls,
                strikes: game.count.strikes,
                outs: game.count.outs,
            },
            bases: MlbBases {
                first: game.bases.first,
                second: game.bases.second,
                third: game.bases.third,
            },
            at_bat: game.at_bat.as_ref().map(|at_bat| MlbAtBat {
                pitcher: at_bat.pitcher.as_str().to_owned(),
                batter: at_bat.batter.as_str().to_owned(),
            }),
            last_play: LastPlay {
                id: game.last_play.id.as_str().to_owned(),
                text: game.last_play.text.as_str().to_owned(),
            },
        }),
        Extract::Final(game) => MlbGame::Final(MlbFinalGame {
            game_id: game.game_id.as_str().to_owned(),
            innings_played: game.innings_played,
            home: final_team(&game.home),
            away: final_team(&game.away),
        }),
    }
}

fn pregame_team(team: &mlb::PregameTeam) -> MlbPregameTeam {
    MlbPregameTeam {
        abbreviation: team.abbreviation.as_str().to_owned(),
        colors: domain_colors(team.colors),
        record: team.record.map(domain_record),
        probable_pitcher: team
            .probable_pitcher
            .as_ref()
            .map(|name| name.as_str().to_owned()),
    }
}

fn live_team(team: &mlb::LiveTeam) -> TeamState {
    TeamState {
        abbreviation: team.abbreviation.as_str().to_owned(),
        score: team.score,
        colors: domain_colors(team.colors),
    }
}

fn final_team(team: &mlb::FinalTeam) -> MlbFinalTeam {
    MlbFinalTeam {
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

    const CLEAN_EVENT: &str = r#"{"id":"401","date":"2026-07-08T01:40Z","competitions":[]}"#;

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
