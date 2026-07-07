//! Inbound ESPN types shared by every sport, plus the lenient scoreboard
//! shell. Sport-specific composites (competitions, situations) live in each
//! sport's module and are built from these leaves.

use serde::Deserialize;

use crate::error::AppError;

/// The cross-sport game-state discriminant (`status.type.state`).
///
/// Empirically 100%-covered with exactly these three values in every league
/// sampled (see tools/espn discover) — safe to deserialize strictly.
#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CompetitionState {
    Pre,
    In,
    Post,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum HomeAway {
    Home,
    Away,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspnTeam {
    /// ESPN's numeric team id as a string; 100%-present in every sampled
    /// sport. Soccer matches scoring details to a side through it.
    pub(crate) id: String,
    pub(crate) abbreviation: String,
    pub(crate) color: String,
    pub(crate) alternate_color: String,
}

/// Outer scoreboard shell, deliberately lenient: events are held as raw JSON
/// and parsed individually by `parse_events`. ESPN has been observed serving
/// a 200 scoreboard whose events were all empty objects — a strict
/// `Vec<EspnEvent>` turns one glitch poll into a whole-scoreboard failure.
#[derive(Deserialize)]
pub(crate) struct RawScoreboard {
    #[serde(default)]
    pub(crate) events: Vec<serde_json::Value>,
}

/// Parse each event individually, skipping (and logging) unparseable ones.
///
/// Returns the parsed events and the number that failed. Callers that map
/// "id not found" to 404 MUST treat a nonzero failure count as an upstream
/// error instead — a glitched scoreboard must never masquerade as
/// "game ended" to the firmware.
pub(crate) fn parse_events<E: serde::de::DeserializeOwned>(
    raw: RawScoreboard,
    url: &str,
) -> (Vec<E>, usize) {
    let mut parsed = Vec::with_capacity(raw.events.len());
    let mut failed = 0usize;
    for (index, value) in raw.events.into_iter().enumerate() {
        match serde_path_to_error::deserialize::<_, E>(value) {
            Ok(event) => parsed.push(event),
            Err(err) => {
                failed += 1;
                let json_path = err.path().to_string();
                tracing::warn!(
                    url = %url,
                    event_index = index,
                    json_path = %json_path,
                    error = %err.into_inner(),
                    "skipping unparseable ESPN event"
                );
            }
        }
    }
    (parsed, failed)
}

/// Find one event by id in a leniently-parsed scoreboard.
///
/// 404 (`GameNotFound`) is the firmware's "game ended, drop it" signal, so an
/// absent id only maps to it when the scoreboard parsed clean: if any events
/// failed, the game may be inside the glitched subset and the caller gets an
/// upstream error (502) instead.
pub(crate) fn find_event<E>(
    events: Vec<E>,
    failed: usize,
    game_id: &str,
    url: &str,
    id_of: impl Fn(&E) -> &str,
) -> Result<E, AppError> {
    match events.into_iter().find(|e| id_of(e) == game_id) {
        Some(event) => Ok(event),
        None if failed > 0 => Err(AppError::EspnDeserialize {
            url: url.to_string(),
            json_path: "events".to_string(),
            message: format!(
                "{failed} event(s) unparseable; cannot distinguish 'ended' from 'glitched'"
            ),
        }),
        None => Err(AppError::GameNotFound(game_id.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize, Debug)]
    struct MinimalEvent {
        id: String,
    }

    fn raw(json: &str) -> RawScoreboard {
        serde_json::from_str(json).expect("test scoreboard json parses")
    }

    #[test]
    fn all_empty_events_parse_to_zero_with_failures_counted() {
        // Shape observed live from ESPN (MLB, 2026-07-06): 200 response,
        // every event an empty object.
        let sb = raw(r#"{"events":[{},{},{}]}"#);
        let (events, failed) = parse_events::<MinimalEvent>(sb, "test://sb");
        assert!(events.is_empty());
        assert_eq!(failed, 3);
    }

    #[test]
    fn mixed_scoreboard_keeps_good_events_and_counts_bad() {
        let sb = raw(r#"{"events":[{"id":"401"},{},{"id":"402"}]}"#);
        let (events, failed) = parse_events::<MinimalEvent>(sb, "test://sb");
        assert_eq!(
            events.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            ["401", "402"]
        );
        assert_eq!(failed, 1);
    }

    #[test]
    fn missing_events_key_is_an_empty_scoreboard() {
        let sb = raw(r#"{}"#);
        let (events, failed) = parse_events::<MinimalEvent>(sb, "test://sb");
        assert!(events.is_empty());
        assert_eq!(failed, 0);
    }

    #[test]
    fn absent_id_with_clean_parse_is_not_found() {
        let sb = raw(r#"{"events":[{"id":"401"}]}"#);
        let (events, failed) = parse_events::<MinimalEvent>(sb, "test://sb");
        let err = find_event(events, failed, "999", "test://sb", |e| &e.id).unwrap_err();
        assert!(matches!(err, AppError::GameNotFound(id) if id == "999"));
    }

    #[test]
    fn absent_id_with_glitched_parse_is_upstream_error_not_404() {
        let sb = raw(r#"{"events":[{"id":"401"},{}]}"#);
        let (events, failed) = parse_events::<MinimalEvent>(sb, "test://sb");
        assert_eq!(failed, 1);
        let err = find_event(events, failed, "999", "test://sb", |e| &e.id).unwrap_err();
        assert!(matches!(err, AppError::EspnDeserialize { .. }));
    }

    #[test]
    fn present_id_is_found_even_when_other_events_glitched() {
        let sb = raw(r#"{"events":[{"id":"401"},{}]}"#);
        let (events, failed) = parse_events::<MinimalEvent>(sb, "test://sb");
        let event = find_event(events, failed, "401", "test://sb", |e| &e.id).unwrap();
        assert_eq!(event.id, "401");
    }
}
