use chrono::NaiveDateTime;

use crate::error::AppError;
use crate::espn::types::EspnTeam;
use crate::shared::team::{TeamColors, order_home_away, parse_hex_rgb};

use super::types::{
    AtBat, Bases, Count, EspnCompetitor, EspnSituation, EspnWeather, FinalGame, FinalTeam, Inning,
    InningHalf, LastPlay, LiveGame, PregameGame, PregameTeam, Record, TeamState, Weather,
};

/// Parse the inning half from ESPN's `shortDetail` prefix.
///
/// Returns `None` for prefixes outside the four in-play states — e.g.
/// "Delayed", "Suspended", or "Rain Delay, Top 1st". Those games are
/// technically `state: "in"` but have nothing displayable; callers treat them
/// as not-found (detail) or exclude them (list) so the firmware never advertises
/// a live game it can't render.
pub(crate) fn parse_inning_half(short_detail: &str) -> Option<InningHalf> {
    match short_detail.split_whitespace().next().unwrap_or("") {
        "Top" => Some(InningHalf::Top),
        "Mid" => Some(InningHalf::Middle),
        "Bot" => Some(InningHalf::Bottom),
        "End" => Some(InningHalf::End),
        other => {
            tracing::warn!(
                short_detail = %short_detail,
                prefix = %other,
                "ESPN shortDetail has non-inning prefix (delay/suspension?) — treating game as not displayable"
            );
            None
        }
    }
}

/// Parse ESPN's `event.date` into a unix epoch (seconds, UTC).
///
/// The corpus is uniformly `%Y-%m-%dT%H:%MZ` (no seconds); a with-seconds
/// variant is accepted as a fallback. The field is 100%-present, so a parse
/// failure is a glitch, not an expected state — surfaced as `EspnDeserialize`.
pub(crate) fn parse_start_time(date: &str) -> Result<u32, AppError> {
    let parsed = NaiveDateTime::parse_from_str(date, "%Y-%m-%dT%H:%MZ")
        .or_else(|_| NaiveDateTime::parse_from_str(date, "%Y-%m-%dT%H:%M:%SZ"));
    let naive = parsed.map_err(|e| {
        tracing::error!(raw_date = %date, error = %e, "ESPN event.date failed to parse");
        AppError::EspnDeserialize {
            url: String::new(),
            json_path: "events[?].date".to_string(),
            message: format!("invalid date '{date}': {e}"),
        }
    })?;
    // ESPN dates are UTC; the epoch is non-negative for any real event.
    Ok(naive.and_utc().timestamp().max(0) as u32)
}

/// Normalize ESPN's swap-prone weather block into a display-ready `Weather`.
///
/// ESPN randomly swaps `displayValue`/`conditionId` between polls, so the
/// human condition is identified structurally: it is the member that does not
/// parse as a number. When both parse as numbers (or both are missing) the
/// condition is unknown and the whole block degrades to `None`. `Some` is
/// returned only when a condition text and a temperature both resolve.
pub(crate) fn normalize_weather(weather: &EspnWeather) -> Option<Weather> {
    fn non_numeric(s: &Option<String>) -> Option<&str> {
        let text = s.as_deref()?;
        // A pure number is the conditionId code, never the condition text.
        if text.trim().parse::<f64>().is_ok() {
            None
        } else {
            Some(text)
        }
    }

    let condition = non_numeric(&weather.display_value).or_else(|| non_numeric(&weather.condition_id));
    match (condition, weather.temperature) {
        (Some(condition), Some(temperature)) => Some(Weather {
            condition: condition.to_string(),
            temperature,
        }),
        _ => {
            tracing::warn!(
                display_value = ?weather.display_value,
                condition_id = ?weather.condition_id,
                temperature = ?weather.temperature,
                "ESPN weather block unusable (no non-numeric condition or no temperature) — dropping weather"
            );
            None
        }
    }
}

/// Parse the overall win-loss record from a competitor's records list.
///
/// The `type=="total"` entry carries the season record as "47-42"
/// (`abbreviation` varies — "Game", "Total" — so it is not matched on). A
/// missing or malformed entry yields `None` (the field is decorative).
pub(crate) fn parse_record(records: &[super::types::EspnRecord]) -> Option<Record> {
    let summary = records.iter().find(|r| r.r#type == "total")?;
    let parsed = summary
        .summary
        .split_once('-')
        .and_then(|(w, l)| Some((w.parse::<u16>().ok()?, l.parse::<u16>().ok()?)));
    match parsed {
        Some((wins, losses)) => Some(Record { wins, losses }),
        None => {
            tracing::warn!(
                summary = %summary.summary,
                "ESPN total record not in 'W-L' form — dropping record"
            );
            None
        }
    }
}

/// Runs per inning for one team, ordered by inning and clamped to `u8`.
///
/// ESPN sends floats (`value`); a single inning never scores past 255, so the
/// clamp only guards against corrupt data. Missing innings (a short home line
/// after a walk-off) simply produce a shorter vector.
fn linescore_runs(linescores: &[super::types::EspnLinescore]) -> Vec<u8> {
    let mut ordered: Vec<&super::types::EspnLinescore> = linescores.iter().collect();
    ordered.sort_by_key(|l| l.period);
    ordered
        .into_iter()
        .map(|l| l.value.clamp(0.0, u8::MAX as f64) as u8)
        .collect()
}

/// Parse a competitor's primary and alternate colors.
fn parse_team_colors(team: &EspnTeam) -> Result<TeamColors, AppError> {
    let primary = parse_hex_rgb(&team.color, &team.abbreviation).inspect_err(|e| {
        tracing::error!(
            team = %team.abbreviation,
            raw_color = %team.color,
            error = ?e,
            "ESPN primary team color failed to parse"
        );
    })?;
    let alternate = parse_hex_rgb(&team.alternate_color, &team.abbreviation).inspect_err(|e| {
        tracing::error!(
            team = %team.abbreviation,
            raw_color = %team.alternate_color,
            error = ?e,
            "ESPN alternate team color failed to parse"
        );
    })?;
    Ok(TeamColors { primary, alternate })
}

/// Parse a competitor's score string into a `u32`.
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

/// Build a live `TeamState` from an ESPN competitor (score + colors).
fn competitor_to_team_state(c: &EspnCompetitor) -> Result<TeamState, AppError> {
    Ok(TeamState {
        abbreviation: c.team.abbreviation.clone(),
        score: parse_score(c)?,
        colors: parse_team_colors(&c.team)?,
    })
}

/// Order two competitors into (home, away).
fn order_competitors(
    event_id: &str,
    competitors: [EspnCompetitor; 2],
) -> Result<(EspnCompetitor, EspnCompetitor), AppError> {
    order_home_away(
        event_id,
        competitors,
        |c| c.home_away,
        |c| c.team.abbreviation.as_str(),
    )
}

/// Transform a pre-game competition into a `PregameGame`. `date` and `weather`
/// come from the event level; `venue_name` from the competition.
pub(crate) fn pregame_competition_to_game(
    event_id: String,
    date: &str,
    weather: Option<&EspnWeather>,
    venue_name: String,
    competitors: [EspnCompetitor; 2],
) -> Result<PregameGame, AppError> {
    let start_time = parse_start_time(date)?;
    let (home_c, away_c) = order_competitors(&event_id, competitors)?;

    let team = |c: &EspnCompetitor| -> Result<PregameTeam, AppError> {
        Ok(PregameTeam {
            abbreviation: c.team.abbreviation.clone(),
            colors: parse_team_colors(&c.team)?,
            record: parse_record(&c.records),
            probable_pitcher: c.probables.first().map(|p| p.athlete.short_name.clone()),
        })
    };

    Ok(PregameGame {
        game_id: event_id,
        start_time,
        venue: venue_name,
        weather: weather.and_then(normalize_weather),
        home: team(&home_c)?,
        away: team(&away_c)?,
    })
}

/// Transform a final competition into a `FinalGame`.
pub(crate) fn final_competition_to_game(
    event_id: String,
    competitors: [EspnCompetitor; 2],
    period: u8,
) -> Result<FinalGame, AppError> {
    let (home_c, away_c) = order_competitors(&event_id, competitors)?;

    let team = |c: &EspnCompetitor| -> Result<FinalTeam, AppError> {
        Ok(FinalTeam {
            abbreviation: c.team.abbreviation.clone(),
            score: parse_score(c)?,
            colors: parse_team_colors(&c.team)?,
            line_score: linescore_runs(&c.linescores),
        })
    };

    Ok(FinalGame {
        game_id: event_id,
        innings_played: period,
        home: team(&home_c)?,
        away: team(&away_c)?,
    })
}

/// Transform a live competition into a `LiveGame`. Callers must pattern-match
/// `EspnCompetition::Live` at the call site, so no runtime state check lives
/// inside this function.
pub(crate) fn live_competition_to_game(
    event_id: String,
    competitors: [EspnCompetitor; 2],
    situation: EspnSituation,
    period: u8,
    short_detail: String,
) -> Result<LiveGame, AppError> {
    // A live game in a non-inning state (rain delay, suspension) has nothing
    // to display — surface it exactly like a game that isn't live.
    let Some(half) = parse_inning_half(&short_detail) else {
        return Err(AppError::GameNotFound(event_id));
    };

    let (home_c, away_c) = order_competitors(&event_id, competitors)?;
    let home = competitor_to_team_state(&home_c)?;
    let away = competitor_to_team_state(&away_c)?;

    let count = Count {
        balls: situation.balls,
        strikes: situation.strikes,
        outs: situation.outs,
    };
    let bases = Bases {
        first: situation.on_first,
        second: situation.on_second,
        third: situation.on_third,
    };
    let at_bat = match (situation.pitcher, situation.batter) {
        (Some(pitcher), Some(batter)) => Some(AtBat {
            pitcher: pitcher.athlete.short_name,
            batter: batter.athlete.short_name,
        }),
        _ => None,
    };
    let last_play = LastPlay {
        id: situation.last_play.id,
        text: situation.last_play.text,
    };

    let inning = Inning {
        number: period,
        half,
    };

    Ok(LiveGame {
        game_id: event_id,
        inning,
        home,
        away,
        count,
        bases,
        at_bat,
        last_play,
    })
}

#[cfg(test)]
mod tests {
    use super::super::types::{EspnCompetition, EspnEvent};
    use super::*;

    /// Real live-captured MLB fixtures (see tools/extract_fixtures.py).
    fn fixture(name: &str) -> EspnEvent {
        let path = format!("{}/testdata/mlb/{}.json", env!("CARGO_MANIFEST_DIR"), name);
        let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        serde_json::from_str(&raw).expect("fixture parses as an MLB event")
    }

    fn pregame_from(event: EspnEvent) -> PregameGame {
        let id = event.id;
        let date = event.date;
        let weather = event.weather;
        let Some(EspnCompetition::PreGame {
            competitors,
            venue_name,
        }) = event.competitions.into_iter().next()
        else {
            panic!("fixture must be a pregame competition");
        };
        pregame_competition_to_game(id, &date, weather.as_ref(), venue_name, competitors).unwrap()
    }

    #[test]
    fn parse_start_time_reads_minute_precision_utc() {
        // 2026-07-08T01:40Z == 1783474800 (verified against a UTC epoch table).
        assert_eq!(parse_start_time("2026-07-08T01:40Z").unwrap(), 1_783_474_800);
    }

    #[test]
    fn parse_start_time_accepts_seconds_fallback() {
        assert_eq!(
            parse_start_time("2026-07-08T01:40:30Z").unwrap(),
            1_783_474_830
        );
    }

    #[test]
    fn parse_start_time_rejects_garbage() {
        assert!(matches!(
            parse_start_time("not-a-date"),
            Err(AppError::EspnDeserialize { .. })
        ));
    }

    #[test]
    fn parse_record_reads_total_entry() {
        let records = vec![
            super::super::types::EspnRecord {
                r#type: "home".to_string(),
                summary: "23-23".to_string(),
            },
            super::super::types::EspnRecord {
                r#type: "total".to_string(),
                summary: "47-42".to_string(),
            },
        ];
        let record = parse_record(&records).expect("total record present");
        assert_eq!((record.wins, record.losses), (47, 42));
    }

    #[test]
    fn parse_record_absent_or_malformed_is_none() {
        assert!(parse_record(&[]).is_none());
        let bad = vec![super::super::types::EspnRecord {
            r#type: "total".to_string(),
            summary: "TBD".to_string(),
        }];
        assert!(parse_record(&bad).is_none());
    }

    fn weather(display: Option<&str>, condition: Option<&str>, temp: Option<i16>) -> EspnWeather {
        EspnWeather {
            display_value: display.map(str::to_string),
            condition_id: condition.map(str::to_string),
            temperature: temp,
        }
    }

    #[test]
    fn normalize_weather_reads_normal_and_swapped_identically() {
        // Normal orientation: displayValue is the text, conditionId a code.
        let normal = normalize_weather(&weather(Some("Mostly sunny"), Some("2"), Some(72)))
            .expect("normal weather resolves");
        // Swapped orientation: the two fields are transposed by ESPN.
        let swapped = normalize_weather(&weather(Some("7"), Some("Cloudy"), Some(78)))
            .expect("swapped weather resolves");
        assert_eq!(normal.condition, "Mostly sunny");
        assert_eq!(normal.temperature, 72);
        assert_eq!(swapped.condition, "Cloudy");
        assert_eq!(swapped.temperature, 78);
    }

    #[test]
    fn normalize_weather_degrades_when_unusable() {
        // Both numeric → no condition text.
        assert!(normalize_weather(&weather(Some("7"), Some("2"), Some(70))).is_none());
        // No temperature.
        assert!(normalize_weather(&weather(Some("Sunny"), Some("2"), None)).is_none());
    }

    #[test]
    fn pregame_fixture_swapped_weather_transforms() {
        let game = pregame_from(fixture("pregame"));
        // The pregame fixture carries the SWAPPED weather orientation.
        let w = game.weather.expect("weather present pregame");
        assert!(w.condition.trim().parse::<f64>().is_err());
        assert!(game.away.record.is_some());
        assert!(game.away.probable_pitcher.is_some());
    }

    #[test]
    fn pregame_fixture_both_orientations_agree() {
        let swapped = pregame_from(fixture("pregame"));
        let normal = pregame_from(fixture("pregame_weather_normal"));
        // Both fixtures resolve to a non-numeric condition and a real temp.
        assert!(swapped.weather.is_some());
        assert!(normal.weather.is_some());
    }

    #[test]
    fn final_fixture_has_line_scores_and_score() {
        let event = fixture("final");
        let id = event.id;
        let Some(EspnCompetition::Final { competitors, period }) =
            event.competitions.into_iter().next()
        else {
            panic!("fixture must be a final competition");
        };
        let game = final_competition_to_game(id, competitors, period).unwrap();
        assert_eq!(game.innings_played, 9);
        assert_eq!(game.away.line_score.len(), 9);
        assert!(!game.home.line_score.is_empty());
    }

    #[test]
    fn live_fixture_transforms_through_du() {
        let event = fixture("live_inning");
        let id = event.id;
        let Some(EspnCompetition::Live {
            competitors,
            situation,
            period,
            short_detail,
        }) = event.competitions.into_iter().next()
        else {
            panic!("fixture must be a live competition");
        };
        let game =
            live_competition_to_game(id, competitors, situation, period, short_detail).unwrap();
        assert!(game.inning.number >= 1);
    }

    #[test]
    fn rain_delay_fixture_is_game_not_found() {
        let event = fixture("rain_delay");
        let id = event.id.clone();
        let Some(EspnCompetition::Live {
            competitors,
            situation,
            period,
            short_detail,
        }) = event.competitions.into_iter().next()
        else {
            panic!("rain-delay fixture is still state 'in'");
        };
        let Err(err) =
            live_competition_to_game(id.clone(), competitors, situation, period, short_detail)
        else {
            panic!("rain-delayed live game must not transform");
        };
        assert!(matches!(err, AppError::GameNotFound(g) if g == id));
    }

    #[test]
    fn parse_inning_half_accepts_in_play_prefixes() {
        assert!(matches!(parse_inning_half("Top 3rd"), Some(InningHalf::Top)));
        assert!(matches!(parse_inning_half("Mid 5th"), Some(InningHalf::Middle)));
        assert!(matches!(parse_inning_half("Bot 9th"), Some(InningHalf::Bottom)));
        assert!(matches!(parse_inning_half("End 1st"), Some(InningHalf::End)));
    }

    #[test]
    fn parse_inning_half_rejects_delay_states() {
        assert!(parse_inning_half("Delayed").is_none());
        assert!(parse_inning_half("Rain Delay, Top 1st").is_none());
        assert!(parse_inning_half("Suspended").is_none());
        assert!(parse_inning_half("").is_none());
    }

    fn competitor(abbrev: &str, home_away: &str) -> EspnCompetitor {
        serde_json::from_str(&format!(
            r#"{{"homeAway":"{home_away}","score":"0",
                "team":{{"id":"1","abbreviation":"{abbrev}","color":"0C2340","alternateColor":"BD3039"}}}}"#
        ))
        .expect("test competitor json parses")
    }

    #[test]
    fn pregame_transform_splits_home_away_and_parses_colors() {
        let game = pregame_competition_to_game(
            "401570001".to_string(),
            "2026-07-07T23:10Z",
            None,
            "Fenway Park".to_string(),
            [competitor("NYY", "away"), competitor("BOS", "home")],
        )
        .unwrap();
        assert_eq!(game.game_id, "401570001");
        assert_eq!(game.venue, "Fenway Park");
        assert!(game.weather.is_none());
        assert_eq!(game.home.abbreviation, "BOS");
        assert_eq!(game.away.abbreviation, "NYY");
        assert_eq!(game.home.colors.primary, 0x0C2340);
    }

    #[test]
    fn pregame_transform_rejects_two_home_teams() {
        let result = pregame_competition_to_game(
            "401570001".to_string(),
            "2026-07-07T23:10Z",
            None,
            "Fenway Park".to_string(),
            [competitor("NYY", "home"), competitor("BOS", "home")],
        );
        assert!(matches!(result, Err(AppError::EspnDeserialize { .. })));
    }

    #[test]
    fn pre_competition_requires_venue_through_du() {
        use super::super::types::EspnCompetition;
        // status.period is 1 pregame in the corpus (never 0) — R6.
        let json = r#"{"competitors":[
            {"homeAway":"away","score":"0","team":{"id":"10","abbreviation":"NYY","color":"003087","alternateColor":"E4002C"}},
            {"homeAway":"home","score":"0","team":{"id":"2","abbreviation":"BOS","color":"0C2340","alternateColor":"BD3039"}}
        ],"status":{"type":{"state":"pre","shortDetail":"7/7 - 7:10 PM EDT"},"period":1},"venue":{"fullName":"Fenway Park"}}"#;
        let competition: EspnCompetition =
            serde_json::from_str(json).expect("pre competition parses through the DU");
        let EspnCompetition::PreGame {
            competitors,
            venue_name,
        } = competition
        else {
            panic!("state 'pre' must map to the PreGame variant");
        };
        assert_eq!(venue_name, "Fenway Park");
        let game = pregame_competition_to_game(
            "401570001".to_string(),
            "2026-07-07T23:10Z",
            None,
            venue_name,
            competitors,
        )
        .unwrap();
        assert_eq!(game.away.colors.primary, 0x003087);
    }

    #[test]
    fn pre_competition_without_venue_is_rejected() {
        use super::super::types::EspnCompetition;
        let json = r#"{"competitors":[
            {"homeAway":"away","score":"0","team":{"id":"10","abbreviation":"NYY","color":"003087","alternateColor":"E4002C"}},
            {"homeAway":"home","score":"0","team":{"id":"2","abbreviation":"BOS","color":"0C2340","alternateColor":"BD3039"}}
        ],"status":{"type":{"state":"pre","shortDetail":"7/7"},"period":1}}"#;
        let result: Result<EspnCompetition, _> = serde_json::from_str(json);
        assert!(result.is_err(), "pregame without venue must fail the DU");
    }
}
