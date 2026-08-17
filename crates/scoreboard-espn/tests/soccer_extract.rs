//! Soccer lane corpus + semantics tests.
//!
//! Fixtures are single EVENT objects (`backend/testdata/soccer/**/*.json`),
//! wrapped here as `{"events":[…]}` scoreboard bodies; goldens mirror under
//! `backend/testdata/wire/soccer/`. The goldens were generated WITHOUT
//! summaries, so commentary is `None` on the golden path and byte equality
//! is asserted that way. Ruling 13's tie-break semantics are pinned by the
//! two named `ruling13_*` tests.

use std::fs;
use std::path::PathBuf;

use scoreboard_espn::common::{IgnoreQuirks, Quirk, Quirks};
use scoreboard_espn::soccer::{
    GameExtractor, GameOutcome, ListExtractor, SoccerExtract, SummaryExtractor, SummaryOutcome,
    parse_display_clock,
};
use scoreboard_wire::soccer::{self as wire_soccer, EventKind, FinalFlavor};
use scoreboard_wire::{GameState, Side, SliceSink};

const LEAGUE: &str = "fifa.world";

/// Every committed fixture, league-dir relative (single EVENT objects).
const FIXTURES: &[&str] = &[
    "end_of_regulation",
    "extra_time_halftime",
    "final_after_extra_time",
    "final_after_penalties",
    "first_half",
    "full_time",
    "full_time_home_multi_goal",
    "halftime",
    "live_red_card",
    "overtime",
    "pregame",
    "second_half_stoppage",
    "shootout",
];

fn testdata(rel: &str) -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../backend/testdata"
    ))
    .join(rel)
}

fn fixture(name: &str) -> String {
    let path = testdata(&format!("soccer/{LEAGUE}/{name}.json"));
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"))
}

fn golden(name: &str) -> Vec<u8> {
    let path = testdata(&format!("wire/soccer/{LEAGUE}/{name}.bin"));
    fs::read(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"))
}

fn wrap(events: &[&str]) -> String {
    format!("{{\"events\":[{}]}}", events.join(","))
}

fn id_of(event_json: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(event_json).expect("fixture parses");
    value["id"].as_str().expect("string id").to_string()
}

fn mutate(event_json: &str, f: impl FnOnce(&mut serde_json::Value)) -> String {
    let mut value: serde_json::Value = serde_json::from_str(event_json).expect("fixture parses");
    f(&mut value);
    value.to_string()
}

/// Records quirks for assertions.
#[derive(Debug, Default)]
struct RecQuirks(Vec<Quirk>);

impl Quirks for RecQuirks {
    fn quirk(&mut self, quirk: Quirk) {
        self.0.push(quirk);
    }
}

fn extract_chunked(body: &str, target: &str, chunk: usize) -> GameOutcome {
    let mut scratch = vec![0u8; 64 * 1024];
    let mut extractor = GameExtractor::new(target, IgnoreQuirks, &mut scratch).unwrap();
    for piece in body.as_bytes().chunks(chunk.max(1)) {
        extractor.write(piece).unwrap();
    }
    extractor.finish().unwrap().0
}

fn found(body: &str, target: &str) -> SoccerExtract {
    match extract_chunked(body, target, usize::MAX) {
        GameOutcome::Found(extract) => extract,
        other => panic!("expected Found for {target}, got {other:?}"),
    }
}

fn encode(extract: &SoccerExtract) -> Vec<u8> {
    let mut buf = [0u8; 4096];
    let mut sink = SliceSink::new(&mut buf);
    wire_soccer::encode(&extract.as_game(), &mut sink).expect("encode fits");
    sink.written().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn run_summary(body: &str, chunk: usize) -> SummaryOutcome {
    let mut scratch = vec![0u8; 64 * 1024];
    let mut extractor = SummaryExtractor::new(&mut scratch).unwrap();
    for piece in body.as_bytes().chunks(chunk.max(1)) {
        extractor.write(piece).unwrap();
    }
    extractor.finish().unwrap()
}

// ----------------------------------------------------------- golden parity

#[test]
fn every_fixture_encodes_to_its_committed_golden() {
    for name in FIXTURES {
        let event = fixture(name);
        let target = id_of(&event);
        let extract = found(&wrap(&[&event]), &target);
        assert_eq!(
            hex(&encode(&extract)),
            hex(&golden(name)),
            "{name} encodes differently than its golden"
        );
    }
}

#[test]
fn chunk_split_invariance_across_feed_shapes() {
    // A live shootout and the penalties final — the two byte-heaviest paths —
    // plus pregame for the third state.
    for name in ["shootout", "final_after_penalties", "pregame"] {
        let event = fixture(name);
        let target = id_of(&event);
        let body = wrap(&[&event]);
        let whole = match extract_chunked(&body, &target, usize::MAX) {
            GameOutcome::Found(extract) => encode(&extract),
            other => panic!("{name}: {other:?}"),
        };
        for chunk in [1, 7] {
            let split = match extract_chunked(&body, &target, chunk) {
                GameOutcome::Found(extract) => encode(&extract),
                other => panic!("{name} at {chunk}-byte chunks: {other:?}"),
            };
            assert_eq!(
                hex(&split),
                hex(&whole),
                "{name}: {chunk}-byte feed diverged"
            );
        }
    }
}

// -------------------------------------------------- ruling 13: tie-breaks

/// All seven penalty kicks in `shootout.json` share `clock.value == 7200.0`;
/// `Iterator::max_by` keeps the LAST of equal maxima, so the golden encodes
/// R. Vargas (array index 11). A first-of-ties implementation diverges here.
#[test]
fn ruling13_shootout_last_event_takes_the_last_of_equal_clock_maxima() {
    let event = fixture("shootout");
    let extract = found(&wrap(&[&event]), &id_of(&event));
    let SoccerExtract::Live(live) = &extract else {
        panic!("shootout is live");
    };
    assert_eq!(live.half, 5);
    assert!(!live.on_break, "\"Shootout\" is active play, not a break");
    let last = live.last_event.as_ref().expect("penalty kicks are events");
    assert_eq!(last.athlete.as_str(), "R. Vargas");
    assert_eq!(last.kind, EventKind::Goal);
    assert_eq!(last.side, Some(Side::Home)); // 475 = SUI = home
    assert_eq!(last.clock.as_str(), "120'");
    assert_eq!(hex(&encode(&extract)), hex(&golden("shootout")));
}

/// The four home scoring plays in `final_after_penalties.json` all sit at
/// 7200.0; the backend's STABLE `sort_by` preserves ESPN's array order. A
/// `sort_unstable` (or any non-stable ordering) silently breaks these bytes.
#[test]
fn ruling13_final_after_penalties_scorers_preserve_stable_sort_order() {
    let event = fixture("final_after_penalties");
    let extract = found(&wrap(&[&event]), &id_of(&event));
    let SoccerExtract::Final(game) = &extract else {
        panic!("final_after_penalties is final");
    };
    assert_eq!(game.flavor, FinalFlavor::AfterPenalties);
    // shootoutScore is deliberately not read: a penalties final encodes 0-0.
    assert_eq!((game.away.score, game.home.score), (0, 0));
    assert_eq!(
        game.home.scorers.as_str(),
        "G. Xhaka 120', Z. Amdouni 120', C. Itten 120', R. Vargas 120'"
    );
    assert_eq!(
        game.away.scorers.as_str(),
        "J. Quintero 120', J. Campaz 120', L. Díaz 120'"
    );
    assert_eq!(hex(&encode(&extract)), hex(&golden("final_after_penalties")));
}

/// A detail flagged both `scoringPlay` and `redCard` encodes as a red card.
#[test]
fn red_card_takes_precedence_over_goal_on_a_double_flagged_detail() {
    let event = fixture("second_half_stoppage");
    let body = mutate(&event, |v| {
        // Details[6] is the R. Lukaku goal at the maximum clock value.
        v["competitions"][0]["details"][6]["redCard"] = serde_json::json!(true);
    });
    let extract = found(&wrap(&[&body]), &id_of(&event));
    let SoccerExtract::Live(live) = &extract else {
        panic!("live fixture");
    };
    let last = live.last_event.as_ref().expect("event present");
    assert_eq!(last.kind, EventKind::RedCard);
    assert_eq!(last.athlete.as_str(), "R. Lukaku");
}

// ------------------------------------------------------- state semantics

/// A postponed match arrives as `pre` (description "Postponed") and stays a
/// pregame card — the pre arm never consults the description, so the bytes
/// equal the untouched pregame golden.
#[test]
fn postponed_arrives_as_pre_and_stays_a_pregame_card() {
    let event = fixture("pregame");
    let body = mutate(&event, |v| {
        v["competitions"][0]["status"]["type"]["description"] = serde_json::json!("Postponed");
    });
    let extract = found(&wrap(&[&body]), &id_of(&event));
    assert!(matches!(extract, SoccerExtract::Pregame(_)));
    assert_eq!(hex(&encode(&extract)), hex(&golden("pregame")));
}

#[test]
fn break_and_active_description_sets_match_the_backend() {
    let event = fixture("halftime");
    let target = id_of(&event);
    let with_desc = |desc: &str| {
        mutate(&event, |v| {
            v["competitions"][0]["status"]["type"]["description"] = serde_json::json!(desc);
        })
    };
    for desc in [
        "Halftime",
        "Extra Time Halftime",
        "End of Regulation",
        "End of Extra Time",
    ] {
        let SoccerExtract::Live(live) = found(&wrap(&[&with_desc(desc)]), &target) else {
            panic!("live");
        };
        assert!(live.on_break, "{desc} should be a break");
    }
    for desc in [
        "First Half",
        "Second Half",
        "In Progress",
        "Overtime",
        "Shootout",
    ] {
        let SoccerExtract::Live(live) = found(&wrap(&[&with_desc(desc)]), &target) else {
            panic!("live");
        };
        assert!(!live.on_break, "{desc} should be active play");
    }
    // Absent description is active play, silently.
    let absent = mutate(&event, |v| {
        v["competitions"][0]["status"]["type"]
            .as_object_mut()
            .unwrap()
            .remove("description");
    });
    let SoccerExtract::Live(live) = found(&wrap(&[&absent]), &target) else {
        panic!("live");
    };
    assert!(!live.on_break);
}

#[test]
fn unknown_break_description_degrades_to_active_play_with_a_quirk() {
    let event = fixture("halftime");
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["status"]["type"]["description"] =
            serde_json::json!("Penalty Shootout Pending");
    })]);
    let mut scratch = vec![0u8; 64 * 1024];
    let mut extractor =
        GameExtractor::new(&id_of(&event), RecQuirks::default(), &mut scratch).unwrap();
    extractor.write(body.as_bytes()).unwrap();
    let (outcome, quirks) = extractor.finish().unwrap();
    let GameOutcome::Found(SoccerExtract::Live(live)) = outcome else {
        panic!("live");
    };
    assert!(!live.on_break, "unknown descriptions never guess a break");
    assert_eq!(quirks.0, vec![Quirk::UnknownBreakDescription]);
}

#[test]
fn unknown_final_description_degrades_to_full_time_with_a_quirk() {
    let event = fixture("full_time");
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["status"]["type"]["description"] = serde_json::json!("Abandoned");
    })]);
    let mut scratch = vec![0u8; 64 * 1024];
    let mut extractor =
        GameExtractor::new(&id_of(&event), RecQuirks::default(), &mut scratch).unwrap();
    extractor.write(body.as_bytes()).unwrap();
    let (outcome, quirks) = extractor.finish().unwrap();
    let GameOutcome::Found(extract) = outcome else {
        panic!("found");
    };
    let SoccerExtract::Final(game) = &extract else {
        panic!("final");
    };
    assert_eq!(game.flavor, FinalFlavor::FullTime);
    assert_eq!(quirks.0, vec![Quirk::UnknownFinalFlavor]);
    // Flavor byte identical to the genuine "Full Time" golden.
    assert_eq!(hex(&encode(&extract)), hex(&golden("full_time")));
}

// -------------------------------------------------------- clock derivation

#[test]
fn display_clock_parses_floor_minutes_and_stoppage() {
    assert_eq!(parse_display_clock("23'", None), 1380);
    assert_eq!(parse_display_clock("45'+6'", None), 3060);
    assert_eq!(parse_display_clock("120'+4'", None), 7440);
    assert_eq!(parse_display_clock("0'", None), 0);
    // Segments are trimmed before the apostrophe strip.
    assert_eq!(parse_display_clock("45' + 6'", None), 3060);
    // The u16 cap.
    assert_eq!(parse_display_clock("1100'", None), u16::MAX);
}

#[test]
fn unparseable_display_clock_falls_back_to_the_numeric_clock() {
    assert_eq!(parse_display_clock("HT", Some(2700.0)), 2700);
    assert_eq!(parse_display_clock("HT", None), 0);
    assert_eq!(parse_display_clock("garbage", None), 0);
    // The fallback clamps rather than wraps.
    assert_eq!(parse_display_clock("HT", Some(1e9)), u16::MAX);
    assert_eq!(parse_display_clock("HT", Some(-5.0)), 0);
}

/// `Option<u32>: Sum` short-circuits: one unparseable `+`-segment poisons the
/// WHOLE string — no partial credit, straight to the numeric fallback.
#[test]
fn one_poisoned_clock_segment_falls_back_for_the_whole_string() {
    assert_eq!(parse_display_clock("45'+x'", Some(2700.0)), 2700);
    assert_eq!(parse_display_clock("45'+x'", None), 0);
}

// ----------------------------------------------------- last-event details

#[test]
fn unattributed_event_sets_neither_side_flag() {
    let event = fixture("second_half_stoppage");
    let body = mutate(&event, |v| {
        v["competitions"][0]["details"][6]["team"]["id"] = serde_json::json!("999");
    });
    let extract = found(&wrap(&[&body]), &id_of(&event));
    let bytes = encode(&extract);
    let decoded = wire_soccer::decode(&bytes).expect("round trip");
    let wire_soccer::Game::Live(live) = decoded else {
        panic!("live");
    };
    let last = live.last_event.expect("event present");
    assert_eq!(last.side, None);
    // FLAG_EVENT set, FLAG_EVENT_AWAY | FLAG_EVENT_HOME clear.
    assert_eq!(bytes[2] & 0x02, 0x02);
    assert_eq!(bytes[2] & 0x18, 0);
}

#[test]
fn athlete_less_event_falls_back_to_the_type_text() {
    let event = fixture("second_half_stoppage");
    let body = mutate(&event, |v| {
        v["competitions"][0]["details"][6]["athletesInvolved"] = serde_json::json!([]);
    });
    let SoccerExtract::Live(live) = found(&wrap(&[&body]), &id_of(&event)) else {
        panic!("live");
    };
    let last = live.last_event.as_ref().expect("event present");
    assert_eq!(last.athlete.as_str(), "");
    // The JSON-DTO-only composed line: no " - athlete" suffix.
    assert_eq!(last.text.as_str(), "Goal");
}

/// Scorer names fall back differently than the last event: a detail with NO
/// athletes uses `type.text` as the name ("Goal 90'+1'").
#[test]
fn athlete_less_scorer_falls_back_to_type_text_in_the_list() {
    let event = fixture("full_time");
    let body = mutate(&event, |v| {
        // Details[1] is the lone (away) goal, M. Merino.
        v["competitions"][0]["details"][1]
            .as_object_mut()
            .unwrap()
            .remove("athletesInvolved");
    });
    let SoccerExtract::Final(game) = found(&wrap(&[&body]), &id_of(&event)) else {
        panic!("final");
    };
    assert_eq!(game.away.scorers.as_str(), "Goal 90'+1'");
    assert_eq!(game.home.scorers.as_str(), "");
}

// ------------------------------------------------------- summary/commentary

#[test]
fn summary_highest_sequence_wins_and_junk_subtrees_are_skipped() {
    let body = r#"{
        "boxscore": {"teams": [{"statistics": [1, 2, {"deep": [null, true]}]}]},
        "commentary": [
            {"sequence": 3, "text": "old"},
            {"sequence": 9, "text": "newest"},
            {"sequence": 7, "text": "mid"}
        ],
        "standings": {"groups": []}
    }"#;
    for chunk in [usize::MAX, 1] {
        let outcome = run_summary(body, chunk);
        assert!(!outcome.malformed);
        let commentary = outcome.commentary.expect("commentary present");
        assert_eq!(commentary.id.as_str(), "9");
        assert_eq!(commentary.text.as_str(), "newest");
    }
}

/// `max_by_key` keeps the LAST of equal maxima — same rule as the details.
#[test]
fn summary_equal_sequences_keep_the_last_item() {
    let body = r#"{"commentary":[{"sequence":5,"text":"first"},{"sequence":5,"text":"second"}]}"#;
    let outcome = run_summary(body, usize::MAX);
    assert_eq!(outcome.commentary.unwrap().text.as_str(), "second");
}

/// Selection is max THEN filter: an empty-text HIGHEST sequence collapses the
/// whole thing to None — it does not fall through to the next non-empty line.
#[test]
fn summary_empty_text_highest_collapses_to_none() {
    let body = r#"{"commentary":[{"sequence":1,"text":"text"},{"sequence":5,"text":""}]}"#;
    let outcome = run_summary(body, usize::MAX);
    assert!(!outcome.malformed);
    assert_eq!(outcome.commentary, None);
}

#[test]
fn summary_sequence_is_stringified_for_the_wire_id() {
    let body = r#"{"commentary":[{"sequence":4294967295,"text":"cap"}]}"#;
    let outcome = run_summary(body, usize::MAX);
    assert_eq!(outcome.commentary.unwrap().id.as_str(), "4294967295");
}

#[test]
fn summary_absent_or_empty_commentary_is_none() {
    for body in [r#"{}"#, r#"{"commentary":[]}"#, r#"{"header":{"id":"1"}}"#] {
        let outcome = run_summary(body, usize::MAX);
        assert!(!outcome.malformed, "{body}");
        assert_eq!(outcome.commentary, None, "{body}");
    }
}

/// Any malformed item fails the WHOLE summary deserialize in the backend,
/// which degrades to no-commentary (best-effort) — even when another item
/// was fine.
#[test]
fn summary_malformed_items_degrade_the_whole_summary_to_none() {
    for body in [
        r#"{"commentary":[{"sequence":1,"text":"ok"},{"sequence":2}]}"#, // missing text
        r#"{"commentary":[{"sequence":1.5,"text":"ok"}]}"#,              // non-integer
        r#"{"commentary":[{"sequence":-1,"text":"ok"}]}"#,               // negative
        r#"{"commentary":[{"sequence":1,"text":"ok"},42]}"#,             // scalar item
        r#"{"commentary":null}"#,                                        // null array
    ] {
        let outcome = run_summary(body, usize::MAX);
        assert!(outcome.malformed, "{body}");
        assert_eq!(outcome.commentary, None, "{body}");
    }
}

#[test]
fn commentary_rides_the_live_extract_onto_the_wire() {
    use scoreboard_espn::soccer::CommentaryExtract;

    let event = fixture("first_half");
    let mut extract = found(&wrap(&[&event]), &id_of(&event));

    // The summary pass, over a hand-written body matching the inventory's
    // schema section.
    let line = "Goal!  Belgium 2, USA 1. Romelu Lukaku right footed shot to the bottom left corner.";
    let summary = format!(
        r#"{{"commentary":[{{"sequence":3,"text":"kickoff"}},{{"sequence":87,"text":"{line}"}}]}}"#
    );
    let outcome = run_summary(&summary, usize::MAX);
    let commentary: CommentaryExtract = outcome.commentary.expect("commentary present");
    assert_eq!(commentary.id.as_str(), "87");
    extract.set_commentary(Some(commentary));

    let bytes = encode(&extract);
    assert_eq!(bytes[2] & 0x20, 0x20, "FLAG_COMMENTARY set");
    let wire_soccer::Game::Live(live) = wire_soccer::decode(&bytes).expect("round trip") else {
        panic!("live");
    };
    let decoded = live.commentary.expect("commentary encoded");
    assert_eq!(decoded.id, "87");
    assert_eq!(decoded.text, line);

    // Cleared commentary returns to the exact golden bytes.
    extract.set_commentary(None);
    assert_eq!(hex(&encode(&extract)), hex(&golden("first_half")));
}

// ---------------------------------------------------------- list + finding

#[test]
fn games_list_reports_states_ids_and_exact_failed_counts() {
    let pregame = fixture("pregame"); // id 760507, pre
    let shootout = fixture("shootout"); // id 760508, in
    // DU-clean but competition-less: listed nowhere, still "ok".
    let no_comp = r#"{"id":"555","date":"2026-07-07T00:00Z","competitions":[]}"#;
    let body = wrap(&[&pregame, "{}", &shootout, no_comp]);

    let mut scratch = vec![0u8; 64 * 1024];
    let mut extractor = ListExtractor::new(&mut scratch).unwrap();
    extractor.write(body.as_bytes()).unwrap();
    let list = extractor.finish().unwrap();

    assert_eq!(list.ok, 3);
    assert_eq!(list.failed, 1);
    assert!(!list.overflowed);
    let entries: Vec<(GameState, &str)> = list
        .games
        .iter()
        .map(|entry| (entry.state, entry.id.as_str()))
        .collect();
    assert_eq!(
        entries,
        vec![
            (GameState::Pregame, "760507"),
            (GameState::Live, "760508"),
        ]
    );
}

#[test]
fn target_found_behind_skipped_events() {
    let shootout = fixture("shootout"); // 760508 — skipped at its id
    let first_half = fixture("first_half"); // 760507 — the target
    let body = wrap(&[&shootout, &first_half]);
    let extract = found(&body, "760507");
    assert_eq!(hex(&encode(&extract)), hex(&golden("first_half")));
}

/// The 404-vs-502 rule (ruling 13): an absent target on a CLEAN scoreboard is
/// "game ended" (failed == 0); a glitched scoreboard must never masquerade as
/// that, so provably-unparseable events are counted.
#[test]
fn absent_target_distinguishes_clean_from_glitched_scoreboards() {
    let pregame = fixture("pregame");

    let clean = wrap(&[&pregame]);
    let GameOutcome::NotFound { ok, failed, skipped } = extract_chunked(&clean, "999", usize::MAX)
    else {
        panic!("target 999 is absent");
    };
    assert_eq!((ok, failed, skipped), (0, 0, 1));

    let glitched = wrap(&[&pregame, "{}"]);
    let GameOutcome::NotFound { failed, skipped, .. } =
        extract_chunked(&glitched, "999", usize::MAX)
    else {
        panic!("target 999 is absent");
    };
    assert_eq!((failed, skipped), (1, 1));
}

/// A found event with an empty `competitions` array is the backend's
/// `GameNotFound` (404 on a clean parse), not an error.
#[test]
fn target_with_no_competition_is_not_found() {
    let body = wrap(&[r#"{"id":"555","date":"2026-07-07T00:00Z","competitions":[]}"#]);
    let GameOutcome::NotFound { ok, failed, skipped } = extract_chunked(&body, "555", usize::MAX)
    else {
        panic!("no competition to serve");
    };
    assert_eq!((ok, failed, skipped), (1, 0, 0));
}

// ------------------------------------------------------- rejection parity

/// Ruling 1: the backend's per-state required-field rules drop the event
/// exactly where the DU conversion would.
#[test]
fn live_event_missing_display_clock_is_dropped() {
    let event = fixture("first_half");
    let target = id_of(&event);
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["status"]
            .as_object_mut()
            .unwrap()
            .remove("displayClock");
    })]);
    let GameOutcome::NotFound { ok, failed, .. } = extract_chunked(&body, &target, usize::MAX)
    else {
        panic!("dropped event cannot be found");
    };
    assert_eq!((ok, failed), (0, 1));
}

#[test]
fn pregame_event_missing_venue_is_dropped() {
    let event = fixture("pregame");
    let target = id_of(&event);
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0].as_object_mut().unwrap().remove("venue");
    })]);
    let GameOutcome::NotFound { ok, failed, .. } = extract_chunked(&body, &target, usize::MAX)
    else {
        panic!("dropped event cannot be found");
    };
    assert_eq!((ok, failed), (0, 1));

    // The same event as live would NOT need the venue — but a venue object
    // missing fullName fails deserialization in every state.
    let broken_venue = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["venue"]
            .as_object_mut()
            .unwrap()
            .remove("fullName");
    })]);
    let GameOutcome::NotFound { failed, .. } = extract_chunked(&broken_venue, &target, usize::MAX)
    else {
        panic!("dropped event cannot be found");
    };
    assert_eq!(failed, 1);
}
