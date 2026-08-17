//! The lenient scoreboard shell for payload-driven lookups (the logo
//! endpoint). The sport transforms no longer deserialize here — they stream
//! the raw body through `crates/scoreboard-espn` — so this module keeps only
//! the shell + per-event lenient parse that non-transform consumers use.

use serde::Deserialize;

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
/// The warn carries the offending event's full JSON — that payload is the
/// artifact needed to fix the model when live data grows a new shape.
///
/// Returns the parsed events and the number that failed.
pub(crate) fn parse_events<E: serde::de::DeserializeOwned>(
    raw: RawScoreboard,
    url: &str,
) -> (Vec<E>, usize) {
    let mut parsed = Vec::with_capacity(raw.events.len());
    let mut failed = 0usize;
    for (index, value) in raw.events.into_iter().enumerate() {
        // Deserialize from a reference so the value survives for the failure
        // log; costs nothing on the success path.
        match serde_path_to_error::deserialize::<_, E>(&value) {
            Ok(event) => parsed.push(event),
            Err(err) => {
                failed += 1;
                let json_path = err.path().to_string();
                tracing::warn!(
                    url = %url,
                    event_index = index,
                    json_path = %json_path,
                    error = %err.into_inner(),
                    payload = %value,
                    "skipping unparseable ESPN event"
                );
            }
        }
    }
    (parsed, failed)
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
}
