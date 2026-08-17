//! NBA parity gate: every fixture in `backend/testdata/nba/` streamed
//! through the path table must encode byte-identically to the committed
//! golden in `backend/testdata/wire/nba/` (DESIGN.md ruling 9), whatever
//! the chunking. Plus the rejection-parity and two-tier-error pins the
//! backend's serde + transform split implies (ruling 1).

use scoreboard_espn::StreamMatcher;
use scoreboard_espn::common::{IgnoreQuirks, Quirk, Quirks};
use scoreboard_espn::nba::{self, DetailOutcome, Extractor, Kind, ScanStats, TransformError};
use scoreboard_wire::{GameState, SliceSink};
use std::fs;
use std::path::PathBuf;

const STEMS: [&str; 7] = [
    "end_of_period",
    "final",
    "halftime",
    "in_progress",
    "in_progress_no_last_play",
    "in_progress_subminute",
    "pregame",
];

fn testdata(rel: &str) -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../backend/testdata")).join(rel)
}

/// Raw fixture text — one EVENT object, exactly as captured (key order
/// preserved, unlike a serde_json round trip).
fn fixture(stem: &str) -> String {
    fs::read_to_string(testdata(&format!("nba/{stem}.json"))).expect("fixture readable")
}

fn golden(stem: &str) -> Vec<u8> {
    fs::read(testdata(&format!("wire/nba/{stem}.bin"))).expect("golden readable")
}

/// The scoreboard body shape the table matches: `{"events":[...]}`.
fn wrap(events: &[&str]) -> String {
    format!("{{\"events\":[{}]}}", events.join(","))
}

fn event_id(event_json: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(event_json).expect("fixture is JSON");
    value["id"].as_str().expect("event has a string id").to_string()
}

fn feed<'c, Q: Quirks>(
    body: &str,
    mut extractor: Extractor<'c, Q>,
    sizes: &[usize],
) -> Extractor<'c, Q> {
    // extractor moves into the matcher and comes back out of finish().
    let mut scratch = vec![0u8; 16 * 1024];
    let matcher = StreamMatcher::new(nba::PATHS, extractor, &mut scratch);
    let mut matcher = matcher.expect("table within engine limits");
    let input = body.as_bytes();
    let mut pos = 0;
    let mut i = 0;
    while pos < input.len() {
        let n = sizes[i % sizes.len()].max(1);
        let end = (pos + n).min(input.len());
        matcher.write(&input[pos..end]).expect("clean JSON parses");
        pos = end;
        i += 1;
    }
    extractor = matcher.finish().expect("document completes");
    extractor
}

fn run_detail(body: &str, target: &str, sizes: &[usize]) -> (DetailOutcome, ScanStats) {
    let mut quirks = IgnoreQuirks;
    let extractor = feed(body, Extractor::game_detail(target, &mut quirks), sizes);
    let stats = extractor.stats();
    (extractor.into_detail().expect("detail mode"), stats)
}

fn run_list(body: &str, sizes: &[usize]) -> (Vec<(String, GameState)>, ScanStats) {
    let mut entries: Vec<(String, GameState)> = Vec::new();
    let mut on_game = |id: &str, state: GameState| entries.push((id.to_string(), state));
    let mut quirks = IgnoreQuirks;
    let extractor = feed(body, Extractor::games_list(&mut on_game, &mut quirks), sizes);
    let stats = extractor.stats();
    drop(extractor);
    (entries, stats)
}

fn encode(extract: &nba::Extract) -> Vec<u8> {
    let mut buf = [0u8; 4096];
    let mut sink = SliceSink::new(&mut buf);
    scoreboard_wire::nba::encode(&extract.as_game(), &mut sink).expect("payload fits");
    sink.written().to_vec()
}

fn found(outcome: DetailOutcome) -> nba::Extract {
    match outcome {
        DetailOutcome::Found(extract) => extract,
        other => panic!("expected Found, got {other:?}"),
    }
}

#[derive(Default)]
struct RecQuirks(Vec<Quirk>);

impl Quirks for RecQuirks {
    fn quirk(&mut self, quirk: Quirk) {
        self.0.push(quirk);
    }
}

// ----------------------------------------------------------- byte parity

#[test]
fn every_fixture_encodes_byte_identical_to_its_golden() {
    for stem in STEMS {
        let event = fixture(stem);
        let body = wrap(&[&event]);
        let id = event_id(&event);
        let (outcome, stats) = run_detail(&body, &id, &[body.len()]);
        let extract = found(outcome);
        assert_eq!(
            hex(&encode(&extract)),
            hex(&golden(stem)),
            "{stem} diverged from its golden"
        );
        assert_eq!((stats.ok, stats.failed), (1, 0), "{stem} counts");
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn chunk_split_invariance_at_the_extract_level() {
    // Whole-buffer vs 1-byte vs ragged feeds must land the same bytes
    // (three fixtures cover all three wire states).
    for stem in ["in_progress", "pregame", "final"] {
        let event = fixture(stem);
        let body = wrap(&[&event]);
        let id = event_id(&event);

        let (whole, _) = run_detail(&body, &id, &[body.len()]);
        let reference = encode(&found(whole));
        assert_eq!(reference, golden(stem), "{stem} whole-buffer parity");

        let (one, _) = run_detail(&body, &id, &[1]);
        assert_eq!(encode(&found(one)), reference, "{stem} 1-byte feed");

        let (ragged, _) = run_detail(&body, &id, &[7, 3, 17, 1, 64]);
        assert_eq!(encode(&found(ragged)), reference, "{stem} ragged feed");
    }
}

#[test]
fn pre_tip_empty_situation_glitch_is_byte_equal_with_no_last_play() {
    // Real capture: state "in", situation == {} — the last-play flag bit
    // must clear and the two strings must not be appended.
    let event = fixture("in_progress_no_last_play");
    let body = wrap(&[&event]);
    let (outcome, _) = run_detail(&body, &event_id(&event), &[body.len()]);
    let extract = found(outcome);
    match &extract.kind {
        Kind::Live(live) => assert!(live.last_play.is_none(), "no last play before the tip"),
        other => panic!("expected live, got {other:?}"),
    }
    let bytes = encode(&extract);
    assert_eq!(bytes[2] & 0x01, 0, "flags bit0 must be clear");
    let expected = golden("in_progress_no_last_play");
    assert_eq!(expected.len(), 49, "the smallest golden in the corpus");
    assert_eq!(bytes, expected);
}

// ------------------------------------------------------- rejection parity

/// Load a fixture as a mutable serde_json tree. NOTE: re-serializing
/// reorders keys alphabetically (BTreeMap), which doubles as a field-order
/// independence check on every mutation test (ruling 4).
fn fixture_value(stem: &str) -> serde_json::Value {
    serde_json::from_str(&fixture(stem)).expect("fixture is JSON")
}

#[test]
fn pregame_missing_display_clock_is_rejected_even_though_nothing_reads_it() {
    let mut event = fixture_value("pregame");
    let removed = event["competitions"][0]["status"]
        .as_object_mut()
        .expect("status object")
        .remove("displayClock");
    assert!(removed.is_some(), "fixture carried displayClock");
    let mutated = event.to_string();
    let live = fixture("in_progress");
    let body = wrap(&[&mutated, &live]);

    // List: the pregame event is dropped, the failure is counted, the
    // healthy live event still serves — parse_events semantics.
    let (entries, stats) = run_list(&body, &[body.len()]);
    assert_eq!(
        entries,
        vec![(event_id(&live), GameState::Live)],
        "only the healthy event lists"
    );
    assert_eq!((stats.ok, stats.failed), (1, 1));

    // Detail for the broken event: not found, and — with every event
    // validated on the way (ruling 14) — the exact counts of the
    // backend's 502 shape, never a 404.
    let (outcome, stats) = run_detail(&body, &event_id(&mutated), &[body.len()]);
    assert!(matches!(outcome, DetailOutcome::NotFound), "got {outcome:?}");
    assert_eq!((stats.ok, stats.failed), (1, 1));
}

#[test]
fn list_mode_counts_an_empty_object_event_as_failed() {
    let events: Vec<String> = STEMS.iter().map(|stem| fixture(stem)).collect();
    let mut refs: Vec<&str> = events.iter().map(String::as_str).collect();
    refs.push("{}");
    let body = wrap(&refs);

    let (entries, stats) = run_list(&body, &[body.len()]);
    let expected: Vec<(String, GameState)> = [
        ("end_of_period", GameState::Live),
        ("final", GameState::Final),
        ("halftime", GameState::Live),
        ("in_progress", GameState::Live),
        ("in_progress_no_last_play", GameState::Live),
        ("in_progress_subminute", GameState::Live),
        ("pregame", GameState::Pregame),
    ]
    .into_iter()
    .map(|(stem, state)| (event_id(&fixture(stem)), state))
    .collect();
    assert_eq!(entries, expected);
    assert_eq!((stats.ok, stats.failed), (7, 1));
    assert!(!stats.events_malformed);
}

#[test]
fn empty_competitions_event_lists_nothing_and_details_no_competition() {
    let body = r#"{"events":[{"id":"7777","date":"2026-04-11T02:30Z","competitions":[]}]}"#;
    let (entries, stats) = run_list(body, &[body.len()]);
    assert!(entries.is_empty());
    // The event deserializes clean — it is ok, not failed (the backend's
    // list filter_map drops it silently).
    assert_eq!((stats.ok, stats.failed), (1, 0));

    let (outcome, stats) = run_detail(body, "7777", &[body.len()]);
    assert!(matches!(outcome, DetailOutcome::NoCompetition), "got {outcome:?}");
    assert_eq!(stats.failed, 0);
}

// --------------------------------------------------- two-tier error model

#[test]
fn bad_hex_color_still_lists_but_detail_rejects() {
    let mut event = fixture_value("in_progress");
    event["competitions"][0]["competitors"][0]["team"]["color"] =
        serde_json::Value::String("not-hex".to_string());
    let body = wrap(&[&event.to_string()]);
    let id = event_id(&event.to_string());

    // Deserialize tier passes — the games list never parses colors.
    let (entries, stats) = run_list(&body, &[body.len()]);
    assert_eq!(entries, vec![(id.clone(), GameState::Live)]);
    assert_eq!((stats.ok, stats.failed), (1, 0));

    // Transform tier is the backend's hard 5xx.
    let (outcome, stats) = run_detail(&body, &id, &[body.len()]);
    assert!(
        matches!(outcome, DetailOutcome::Rejected(TransformError::Color)),
        "got {outcome:?}"
    );
    assert_eq!((stats.ok, stats.failed), (1, 0));
}

#[test]
fn two_home_markers_still_list_but_detail_rejects() {
    let mut event = fixture_value("in_progress");
    event["competitions"][0]["competitors"][1]["homeAway"] =
        serde_json::Value::String("home".to_string());
    let body = wrap(&[&event.to_string()]);
    let id = event_id(&event.to_string());

    let (entries, _) = run_list(&body, &[body.len()]);
    assert_eq!(entries.len(), 1, "marker conflicts are transform-tier");

    let (outcome, _) = run_detail(&body, &id, &[body.len()]);
    assert!(
        matches!(outcome, DetailOutcome::Rejected(TransformError::HomeAway)),
        "got {outcome:?}"
    );
}

#[test]
fn unknown_home_away_marker_is_deserialize_tier_and_drops_the_event() {
    // Strict serde enum: "neutral" fails deserialization — a skip, not a
    // transform error, unlike the two-homes case above.
    let mut event = fixture_value("in_progress");
    event["competitions"][0]["competitors"][0]["homeAway"] =
        serde_json::Value::String("neutral".to_string());
    let body = wrap(&[&event.to_string()]);

    let (entries, stats) = run_list(&body, &[body.len()]);
    assert!(entries.is_empty());
    assert_eq!((stats.ok, stats.failed), (0, 1));
}

// ------------------------------------------------- ordering and skipping

#[test]
fn home_away_resolves_by_marker_even_with_the_array_flipped() {
    // All 7 fixtures put home at index 0 — flip the array and the bytes
    // must not move. (The serde_json rebuild also shuffles every object's
    // key order, so this doubles as the field-order independence proof.)
    let mut event = fixture_value("in_progress");
    let competitors = event["competitions"][0]["competitors"]
        .as_array_mut()
        .expect("competitors array");
    competitors.reverse();
    assert_eq!(
        competitors[0]["homeAway"], "away",
        "away really is first now"
    );
    let mutated = event.to_string();
    let body = wrap(&[&mutated]);

    let (outcome, _) = run_detail(&body, &event_id(&mutated), &[body.len()]);
    assert_eq!(encode(&found(outcome)), golden("in_progress"));
}

#[test]
fn detail_validates_until_its_target_then_skips_the_rest() {
    let final_event = fixture("final");
    let live_event = fixture("in_progress");
    let pregame_event = fixture("pregame");
    let body = wrap(&[&final_event, &live_event, &pregame_event]);

    // Ruling 14: events before the target validate and count; events
    // after it are skipped and uncounted.
    let (outcome, stats) = run_detail(&body, &event_id(&live_event), &[body.len()]);
    assert_eq!(encode(&found(outcome)), golden("in_progress"));
    assert_eq!((stats.ok, stats.failed), (2, 0), "final + live counted");

    let (outcome, stats) = run_detail(&body, &event_id(&pregame_event), &[body.len()]);
    assert_eq!(encode(&found(outcome)), golden("pregame"));
    assert_eq!((stats.ok, stats.failed), (3, 0), "everything preceded the target");

    // Absent id over a clean scoreboard: NotFound with exact counts —
    // nothing was skipped, so failed == 0 really is the 404 case.
    let (outcome, stats) = run_detail(&body, "000000000", &[body.len()]);
    assert!(matches!(outcome, DetailOutcome::NotFound), "got {outcome:?}");
    assert_eq!((stats.ok, stats.failed), (3, 0));

    // And the post-target skip must survive a 1-byte feed.
    let (outcome, stats) = run_detail(&body, &event_id(&final_event), &[1]);
    assert_eq!(encode(&found(outcome)), golden("final"));
    assert_eq!((stats.ok, stats.failed), (1, 0), "the two later events skipped");
}

/// Ruling 14's pin, mirroring football's `events_after_found_target_are_skipped`:
/// garbage after the target is skipped and uncounted; garbage before it is
/// validated and counted — which is what makes a NotFound verdict's counts
/// exact for the 404-vs-502 rule.
#[test]
fn events_shell_must_be_an_array() {
    // Scalar/null shells at the value callback; object shells via the
    // engine's ContainerKind at enter (the closed residue).
    for body in [
        r#"{"events":42}"#,
        r#"{"events":null}"#,
        r#"{"events":"x"}"#,
        r#"{"events":{"x":1}}"#,
        r#"{"events":{}}"#,
    ] {
        let mut quirks = scoreboard_espn::common::IgnoreQuirks;
        let extractor = feed(
            body,
            Extractor::game_detail("77", &mut quirks),
            &[usize::MAX],
        );
        assert!(extractor.stats().events_malformed, "{body}");
    }
    let mut quirks = scoreboard_espn::common::IgnoreQuirks;
    let extractor = feed("{\"events\":[]}", Extractor::game_detail("77", &mut quirks), &[usize::MAX]);
    assert!(!extractor.stats().events_malformed, "legal empty slate");
}

#[test]
fn events_after_found_target_are_skipped() {
    let event = fixture("in_progress");
    let id = event_id(&event);
    let garbage = r#"{"id":42,"date":false,"competitions":"nope"}"#;

    let after = wrap(&[&event, garbage]);
    let (outcome, stats) = run_detail(&after, &id, &[after.len()]);
    assert!(matches!(outcome, DetailOutcome::Found(_)), "got {outcome:?}");
    assert_eq!((stats.ok, stats.failed), (1, 0), "post-target skipped");

    let before = wrap(&[garbage, &event]);
    let (outcome, stats) = run_detail(&before, &id, &[before.len()]);
    assert!(matches!(outcome, DetailOutcome::Found(_)), "got {outcome:?}");
    assert_eq!((stats.ok, stats.failed), (1, 1), "pre-target validated");

    // Absent target with a glitched sibling: the backend's 502 shape —
    // NotFound plus a nonzero, exact failure count.
    let (outcome, stats) = run_detail(&before, "000000000", &[before.len()]);
    assert!(matches!(outcome, DetailOutcome::NotFound), "got {outcome:?}");
    assert_eq!((stats.ok, stats.failed), (1, 1));
}

/// Ruling 16: the target compare must never run on a truncated key —
/// distinct ids sharing a 255-byte prefix are different games, and the
/// prefix itself matches neither.
#[test]
fn long_ids_never_match_on_a_truncated_prefix() {
    let prefix = "9".repeat(255);
    let mut live = fixture_value("in_progress");
    live["id"] = serde_json::Value::String(format!("{prefix}1"));
    let mut fin = fixture_value("final");
    fin["id"] = serde_json::Value::String(format!("{prefix}2"));
    let body = wrap(&[&live.to_string(), &fin.to_string()]);

    // The second long id must find the second event, not the first.
    let target = format!("{prefix}2");
    let (outcome, stats) = run_detail(&body, &target, &[body.len()]);
    match &found(outcome).kind {
        Kind::Final(_) => {}
        other => panic!("prefix collision served the wrong game: {other:?}"),
    }
    assert_eq!((stats.ok, stats.failed), (2, 0));

    // The bare 255-byte prefix — exactly what a truncated compare would
    // see — matches neither event.
    let (outcome, stats) = run_detail(&body, &prefix, &[body.len()]);
    assert!(matches!(outcome, DetailOutcome::NotFound), "got {outcome:?}");
    assert_eq!((stats.ok, stats.failed), (2, 0), "clean scoreboard: a real 404");
}

// --------------------------------------------------------- field semantics

#[test]
fn clock_passes_through_verbatim_including_the_colonless_shape() {
    let event = fixture("in_progress_subminute");
    let body = wrap(&[&event]);
    let (outcome, _) = run_detail(&body, &event_id(&event), &[body.len()]);
    match &found(outcome).kind {
        Kind::Live(live) => {
            assert_eq!(live.clock.as_str(), "0.0", "colonless crunch signal intact");
        }
        other => panic!("expected live, got {other:?}"),
    }
}

#[test]
fn linescores_sort_stably_by_period() {
    // Shuffle the away side's linescores out of order and give the home
    // side duplicate periods: sort key is the period, equal keys keep
    // arrival order (max_by/sort stability, ruling 13).
    let mut event = fixture_value("final");
    let competitors = event["competitions"][0]["competitors"]
        .as_array_mut()
        .expect("competitors");
    // Fixture order is [home, away].
    competitors[1]["linescores"] = serde_json::json!([
        {"value": 25.0, "period": 3},
        {"value": 36.0, "period": 1},
        {"value": 25.0, "period": 4},
        {"value": 32.0, "period": 2},
    ]);
    competitors[0]["linescores"] = serde_json::json!([
        {"value": 30.0, "period": 1},
        {"value": 35.0, "period": 1},
        {"value": 25.0, "period": 1},
        {"value": 10.0, "period": 1},
    ]);
    let mutated = event.to_string();
    let body = wrap(&[&mutated]);
    let (outcome, _) = run_detail(&body, &event_id(&mutated), &[body.len()]);
    match &found(outcome).kind {
        Kind::Final(game) => {
            assert_eq!(game.away.line_score.as_slice(), &[36, 32, 25, 25]);
            assert_eq!(
                game.home.line_score.as_slice(),
                &[30, 35, 25, 10],
                "duplicate periods keep arrival order"
            );
        }
        other => panic!("expected final, got {other:?}"),
    }
}

#[test]
fn unknown_live_description_degrades_to_in_progress_with_a_quirk() {
    let mut event = fixture_value("in_progress");
    event["competitions"][0]["status"]["type"]["description"] =
        serde_json::Value::String("Overtime Break".to_string());
    let mutated = event.to_string();
    let body = wrap(&[&mutated]);

    let mut quirks = RecQuirks::default();
    let target = event_id(&mutated);
    let extractor = feed(
        &body,
        Extractor::game_detail(&target, &mut quirks),
        &[body.len()],
    );
    let outcome = extractor.into_detail().expect("detail mode");
    match &found(outcome).kind {
        Kind::Live(live) => assert_eq!(
            live.phase,
            scoreboard_espn::common::LivePhase::InProgress,
            "never guess a break"
        ),
        other => panic!("expected live, got {other:?}"),
    }
    assert_eq!(quirks.0, vec![Quirk::UnknownLivePhase]);
}

#[test]
fn malformed_total_record_drops_to_none_with_a_quirk() {
    let mut event = fixture_value("pregame");
    // Fixture order is [home, away]; break the home side's total.
    event["competitions"][0]["competitors"][0]["records"][0]["summary"] =
        serde_json::Value::String("TBD".to_string());
    let mutated = event.to_string();
    let body = wrap(&[&mutated]);

    let mut quirks = RecQuirks::default();
    let target = event_id(&mutated);
    let extractor = feed(
        &body,
        Extractor::game_detail(&target, &mut quirks),
        &[body.len()],
    );
    let outcome = extractor.into_detail().expect("detail mode");
    match &found(outcome).kind {
        Kind::Pregame(game) => {
            assert!(game.home.record.is_none(), "malformed total drops");
            assert!(game.away.record.is_some(), "the other side is untouched");
        }
        other => panic!("expected pregame, got {other:?}"),
    }
    assert_eq!(quirks.0, vec![Quirk::MalformedRecord]);
}
