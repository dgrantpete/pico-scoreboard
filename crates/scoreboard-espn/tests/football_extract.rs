//! Football lane acceptance: byte parity against the committed wire
//! goldens over every fixture, chunk-split invariance, the Unicode
//! uppercase property (ruling 10), and rejection/leniency parity with the
//! backend's lenient event parse (ruling 1).

use std::fs;
use std::path::PathBuf;

use scoreboard_espn::common::{Crests, IgnoreQuirks, ListRow, ListSink, Quirk, Quirks};
use scoreboard_espn::football::{
    Counts, DetailExtractor, DetailOutcome, FootballError, GameExtract, ListExtractor,
};
use scoreboard_wire::{GameState, SliceSink, football as wire};

// ---------------------------------------------------------------- helpers

fn testdata() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../backend/testdata"))
}

/// Fixture stems under `backend/testdata/football`, league subdirs
/// included (`nfl/pregame`, `college-football/pregame_ranked`), sorted.
fn fixture_names() -> Vec<String> {
    fn walk(dir: &PathBuf, prefix: &str, out: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap_or_else(|e| panic!("read {dir:?}: {e}")) {
            let entry = entry.unwrap();
            let name = entry.file_name().into_string().unwrap();
            if entry.file_type().unwrap().is_dir() {
                walk(&entry.path(), &format!("{prefix}{name}/"), out);
            } else if let Some(stem) = name.strip_suffix(".json") {
                out.push(format!("{prefix}{stem}"));
            }
        }
    }
    let mut names = Vec::new();
    walk(&testdata().join("football"), "", &mut names);
    names.sort();
    assert!(names.len() >= 9, "football corpus missing? found {names:?}");
    names
}

fn fixture_json(name: &str) -> String {
    let path = testdata().join("football").join(format!("{name}.json"));
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"))
}

fn golden(name: &str) -> Vec<u8> {
    let path = testdata().join("wire/football").join(format!("{name}.bin"));
    fs::read(&path).unwrap_or_else(|e| panic!("read golden {path:?}: {e}"))
}

/// A fixture file is a single scoreboard EVENT; the extractor consumes the
/// scoreboard body shape.
fn wrap(event_json: &str) -> String {
    format!(r#"{{"events":[{event_json}]}}"#)
}

fn event_id(event_json: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(event_json).expect("fixture parses");
    value["id"].as_str().expect("fixture has a string id").to_string()
}

/// `wire_corpus.rs:174`: the `college-football/` path prefix is what
/// drives `is_college`.
fn is_college(name: &str) -> bool {
    name.starts_with("college-football/")
}

fn run_detail(
    body: &str,
    target: &str,
    college: bool,
    chunk: usize,
) -> (DetailOutcome, Counts) {
    let mut scratch = vec![0u8; 16 * 1024];
    let mut extractor =
        DetailExtractor::new(target, college, IgnoreQuirks, &mut scratch).expect("table valid");
    for piece in body.as_bytes().chunks(chunk.max(1)) {
        extractor.write(piece).expect("clean parse");
    }
    let report = extractor.finish().expect("no transform error");
    (report.outcome, report.counts)
}

fn found(body: &str, target: &str, college: bool, chunk: usize) -> GameExtract {
    match run_detail(body, target, college, chunk) {
        (DetailOutcome::Found(game), _) => game,
        (other, counts) => panic!("expected Found, got {other:?} with {counts:?}"),
    }
}

fn encode(game: &wire::Game<'_>) -> Vec<u8> {
    let mut buf = [0u8; 4096];
    let mut sink = SliceSink::new(&mut buf);
    wire::encode(game, &mut sink).expect("payload fits");
    sink.written().to_vec()
}

fn extract_bytes(name: &str, college: bool, chunk: usize) -> Vec<u8> {
    let event = fixture_json(name);
    let extract = found(&wrap(&event), &event_id(&event), college, chunk);
    encode(&extract.as_game())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// One [`ListRow`] copied into owned storage. A row borrows the extractor's
/// per-event scratch, so nothing survives the callback uncopied — which is
/// the property these tests exist to keep honest.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    id: String,
    state: GameState,
    away_abbreviation: Option<String>,
    home_abbreviation: Option<String>,
    away_crest: Option<String>,
    home_crest: Option<String>,
}

fn owned_row(row: ListRow<'_>) -> Row {
    Row {
        id: row.id.to_string(),
        state: row.state,
        away_abbreviation: row.away.abbreviation.map(str::to_string),
        home_abbreviation: row.home.abbreviation.map(str::to_string),
        away_crest: row.away.crest.map(str::to_string),
        home_crest: row.home.crest.map(str::to_string),
    }
}

impl Row {
    /// The `(id, state)` pair the list has always delivered — what the
    /// membership assertions compare, unchanged by the extras.
    fn pair(&self) -> (String, GameState) {
        (self.id.clone(), self.state)
    }
}

#[derive(Default)]
struct Collect(Vec<Row>);

impl ListSink for Collect {
    fn row(&mut self, row: ListRow<'_>) {
        self.0.push(owned_row(row));
    }
}

fn run_list_rows(body: &str, chunk: usize) -> (Vec<Row>, Counts) {
    let mut scratch = vec![0u8; 16 * 1024];
    let mut extractor =
        ListExtractor::new(Collect::default(), IgnoreQuirks, &mut scratch).expect("table valid");
    for piece in body.as_bytes().chunks(chunk.max(1)) {
        extractor.write(piece).expect("clean parse");
    }
    let report = extractor.finish().expect("list never transforms");
    (report.entries.0, report.counts)
}

fn run_list(body: &str, chunk: usize) -> (Vec<(String, GameState)>, Counts) {
    let (rows, counts) = run_list_rows(body, chunk);
    (rows.iter().map(Row::pair).collect(), counts)
}

// ---------------------------------------------------------- byte parity

/// The parity gate (ruling 9): every fixture through extract → `as_game`
/// → wire encode must be byte-identical to its committed golden.
#[test]
fn corpus_byte_parity_against_goldens() {
    for name in fixture_names() {
        let bytes = extract_bytes(&name, is_college(&name), usize::MAX);
        assert_eq!(
            hex(&bytes),
            hex(&golden(&name)),
            "{name} diverges from its golden"
        );
    }
}

/// Flag-byte canaries: live `0x17` (last play | situation | possession
/// home | timeouts) and pregame_ranked `0x0b` (both records | home rank).
#[test]
fn flag_byte_canaries() {
    let live = extract_bytes("nfl/in_progress", false, usize::MAX);
    assert_eq!(live[2], 0x17, "live flags: {}", hex(&live));

    let ranked = extract_bytes("college-football/pregame_ranked", true, usize::MAX);
    assert_eq!(ranked[2], 0x0b, "pregame flags: {}", hex(&ranked));
}

/// Whole-buffer vs 1-byte-at-a-time (and a mid-size chunking) must produce
/// identical bytes — the extract-level chunk-split invariance.
#[test]
fn chunk_split_invariance() {
    for name in ["nfl/in_progress", "college-football/pregame_ranked"] {
        let college = is_college(name);
        let whole = extract_bytes(name, college, usize::MAX);
        assert_eq!(whole, golden(name), "{name} whole-buffer");
        assert_eq!(
            extract_bytes(name, college, 1),
            whole,
            "{name}: 1-byte feed diverged"
        );
        assert_eq!(
            extract_bytes(name, college, 7),
            whole,
            "{name}: 7-byte feed diverged"
        );
    }
}

// ------------------------------------------------------- rank line rules

/// Ruling 10: the crate's `core`-based per-char uppercasing must be
/// byte-identical to `str::to_uppercase` — including `ß` → `SS`, which
/// grows the byte length.
#[test]
fn rank_line_uppercase_matches_std() {
    let event = fixture_json("college-football/pregame_ranked");
    for name in [
        "Ohio State",
        "École Polytechnique",
        "Piñata Tech",
        "Großglockner Straße",
        "ǆungla ǅombe",
    ] {
        let mut value: serde_json::Value = serde_json::from_str(&event).unwrap();
        value["competitions"][0]["competitors"][0]["team"]["shortDisplayName"] =
            serde_json::Value::String(name.to_string());
        let body = wrap(&serde_json::to_string(&value).unwrap());
        let extract = found(&body, &event_id(&event), true, usize::MAX);
        let wire::Game::Pregame(pregame) = extract.as_game() else {
            panic!("ranked fixture is pregame");
        };
        // Home (#3) carries the line; away is ESPN's 99 unranked sentinel.
        assert_eq!(
            pregame.home.rank_line,
            Some(format!("#3 {}", name.to_uppercase()).as_str()),
            "uppercase diverged for {name:?}"
        );
        assert_eq!(pregame.away.rank_line, None);
    }
}

/// Pinned upstream as `pregame_ncaaf_rank_absent_when_polled_as_nfl`: the
/// `is_college` call parameter gates the rank line, so the same college
/// fixture polled as NFL emits none — and the flag byte drops to just the
/// two record bits.
#[test]
fn college_fixture_polled_as_nfl_has_no_rank_line() {
    let event = fixture_json("college-football/pregame_ranked");
    let extract = found(&wrap(&event), &event_id(&event), false, usize::MAX);
    let bytes = encode(&extract.as_game());
    assert_eq!(bytes[2], 0x03, "flags: {}", hex(&bytes));
    let wire::Game::Pregame(pregame) = wire::decode(&bytes).unwrap() else {
        panic!("pregame payload");
    };
    assert_eq!(pregame.home.rank_line, None);
    assert_eq!(pregame.away.rank_line, None);
    assert!(pregame.home.record.is_some());
}

// ------------------------------------------------------ rejection parity

/// `status.displayClock` is required in every state; an event without it
/// fails the deserialize tier and is counted failed — so the absent target
/// reports `failed > 0` (the caller's 502, never a 404).
#[test]
fn missing_display_clock_rejects_and_counts_failed() {
    let event = fixture_json("nfl/in_progress");
    let mut value: serde_json::Value = serde_json::from_str(&event).unwrap();
    value["competitions"][0]["status"]
        .as_object_mut()
        .unwrap()
        .remove("displayClock");
    let body = wrap(&serde_json::to_string(&value).unwrap());
    let (outcome, counts) = run_detail(&body, &event_id(&event), false, usize::MAX);
    assert!(matches!(outcome, DetailOutcome::Absent), "{outcome:?}");
    assert_eq!(counts, Counts { ok: 0, failed: 1 });
}

/// `status.period` is likewise required even where its value is unread —
/// a pregame without it must be rejected (inventory §5.12).
#[test]
fn missing_period_rejects_even_pregame() {
    let event = fixture_json("nfl/pregame");
    let mut value: serde_json::Value = serde_json::from_str(&event).unwrap();
    value["competitions"][0]["status"]
        .as_object_mut()
        .unwrap()
        .remove("period");
    let body = wrap(&serde_json::to_string(&value).unwrap());
    let (outcome, counts) = run_detail(&body, &event_id(&event), false, usize::MAX);
    assert!(matches!(outcome, DetailOutcome::Absent), "{outcome:?}");
    assert_eq!(counts, Counts { ok: 0, failed: 1 });
}

/// The venue is demanded only by the pregame arm: removing it fails a
/// pregame event but leaves a live one extractable.
#[test]
fn venue_required_only_pregame() {
    for (name, expect_found) in [("nfl/pregame", false), ("nfl/in_progress", true)] {
        let event = fixture_json(name);
        let mut value: serde_json::Value = serde_json::from_str(&event).unwrap();
        value["competitions"][0].as_object_mut().unwrap().remove("venue");
        let body = wrap(&serde_json::to_string(&value).unwrap());
        let (outcome, counts) = run_detail(&body, &event_id(&event), false, usize::MAX);
        if expect_found {
            assert!(matches!(outcome, DetailOutcome::Found(_)), "{name}: {outcome:?}");
        } else {
            assert!(matches!(outcome, DetailOutcome::Absent), "{name}: {outcome:?}");
            assert_eq!(counts.failed, 1, "{name}");
        }
    }
}

/// A scoreboard shell whose `events` is a scalar or null fails the
/// backend's whole-body deserialize — a 502 before any event. Neither
/// extractor may launder it into a clean 404 / empty list (ruling 13's
/// glitch-vs-ended rule at body scope).
#[test]
fn scalar_events_shell_is_malformed_for_detail_and_list() {
    for body in [r#"{"events":42}"#, r#"{"events":null}"#, r#"{"events":"x"}"#] {
        let mut scratch = vec![0u8; 1024];
        let mut detail =
            DetailExtractor::new("77", false, IgnoreQuirks, &mut scratch).expect("table valid");
        detail.write(body.as_bytes()).expect("clean parse");
        assert!(
            matches!(detail.finish(), Err(FootballError::MalformedEvents)),
            "detail must reject {body}"
        );

        let mut scratch = vec![0u8; 1024];
        let mut list = ListExtractor::new(Collect::default(), IgnoreQuirks, &mut scratch)
            .expect("table valid");
        list.write(body.as_bytes()).expect("clean parse");
        assert!(
            matches!(list.finish(), Err(FootballError::MalformedEvents)),
            "list must reject {body}"
        );
    }
}

/// KNOWN RESIDUE, pinned so it flips loudly if the engine ever reports
/// container kinds: an `events` OBJECT is invisible at the sink API —
/// its members arrive as keys, which match no pattern — so
/// `{"events":{…}}` scans exactly like the LEGAL empty scoreboard
/// `{"events":[]}` and parses clean (backend: whole-body 502). Flagging
/// "no event elements seen" instead would 502 every real no-games day.
/// Identical residue in all four sport lanes.
#[test]
fn object_events_shell_is_malformed_now_that_enter_reports_kind() {
    // Formerly the documented residue: the engine's ContainerKind addition
    // makes `{"events":{…}}` detectable, closing the last 404-masquerade.
    for body in [r#"{"events":{"x":1}}"#, r#"{"events":{}}"#] {
        let mut scratch = vec![0u8; 1024];
        let mut detail =
            DetailExtractor::new("77", false, IgnoreQuirks, &mut scratch).expect("table valid");
        detail.write(body.as_bytes()).expect("clean parse");
        assert!(
            matches!(detail.finish(), Err(FootballError::MalformedEvents)),
            "detail must reject {body}"
        );

        let mut scratch = vec![0u8; 1024];
        let mut list = ListExtractor::new(Collect::default(), IgnoreQuirks, &mut scratch)
            .expect("table valid");
        list.write(body.as_bytes()).expect("clean parse");
        assert!(
            matches!(list.finish(), Err(FootballError::MalformedEvents)),
            "list must reject {body}"
        );
    }
    // The legal empty scoreboard stays clean.
    let (outcome, counts) = run_detail(r#"{"events":[]}"#, "77", false, usize::MAX);
    assert!(matches!(outcome, DetailOutcome::Absent));
    assert_eq!(counts, Counts { ok: 0, failed: 0 });
}

/// A found id with an empty competitions array is the backend's direct
/// `GameNotFound`, distinct from the absent-id path.
#[test]
fn empty_competitions_is_no_competitions() {
    let body = r#"{"events":[{"id":"77","date":"2026-01-12T21:30Z","competitions":[]}]}"#;
    let (outcome, counts) = run_detail(body, "77", false, usize::MAX);
    assert!(matches!(outcome, DetailOutcome::NoCompetitions), "{outcome:?}");
    assert_eq!(counts, Counts { ok: 1, failed: 0 });
}

/// Absent target over a glitched board: the ok/failed tally is exact so
/// the caller can distinguish "ended" (404) from "glitched" (502).
#[test]
fn absent_target_reports_exact_counts() {
    let event = fixture_json("nfl/pregame");
    let body = format!(r#"{{"events":[{event},{{}}]}}"#);
    let (outcome, counts) = run_detail(&body, "does-not-exist", false, usize::MAX);
    assert!(matches!(outcome, DetailOutcome::Absent), "{outcome:?}");
    assert_eq!(counts, Counts { ok: 1, failed: 1 });
}

/// Once the target is found the remaining events are fast-forwarded with
/// `SkipElement` (and deliberately left out of the counts — they are only
/// consumed when the target is absent, in which case nothing was skipped).
/// Events *before* the target are fully validated.
#[test]
fn events_after_found_target_are_skipped() {
    let event = fixture_json("nfl/in_progress");
    let id = event_id(&event);
    let garbage = r#"{"id":42,"date":false,"competitions":"nope"}"#;

    let after = format!(r#"{{"events":[{event},{garbage}]}}"#);
    let (outcome, counts) = run_detail(&after, &id, false, usize::MAX);
    assert!(matches!(outcome, DetailOutcome::Found(_)), "{outcome:?}");
    assert_eq!(counts, Counts { ok: 1, failed: 0 }, "post-target skipped");

    let before = format!(r#"{{"events":[{garbage},{event}]}}"#);
    let (outcome, counts) = run_detail(&before, &id, false, usize::MAX);
    assert!(matches!(outcome, DetailOutcome::Found(_)), "{outcome:?}");
    assert_eq!(counts, Counts { ok: 1, failed: 1 }, "pre-target validated");
}

// ------------------------------------------------------------- list mode

/// The games list is the same parse: per-event state + the lenient
/// ok/failed tally, with ESPN's observed all-`{}` glitch event counted
/// failed and excluded.
#[test]
fn list_mode_over_multi_event_body() {
    let body = format!(
        r#"{{"events":[{pre},{live},{fin},{{}}]}}"#,
        pre = fixture_json("nfl/pregame"),
        live = fixture_json("nfl/in_progress"),
        fin = fixture_json("nfl/final"),
    );
    let (entries, counts) = run_list(&body, usize::MAX);
    assert_eq!(
        entries,
        vec![
            ("401772512".to_string(), GameState::Pregame),
            ("401772510".to_string(), GameState::Live),
            ("401772514".to_string(), GameState::Final),
        ]
    );
    assert_eq!(counts, Counts { ok: 3, failed: 1 });

    let (one_byte, counts_1) = run_list(&body, 1);
    assert_eq!(one_byte, entries, "1-byte list feed diverged");
    assert_eq!(counts_1, counts);
}

// ------------------------------------------------- situation edge parity

/// A synthetic live competition around one situation block, KC (id 12)
/// home vs BUF (id 2) away — the backend transform tests' shape.
fn live_body(situation: &str) -> String {
    format!(
        r#"{{"events":[{{"id":"401772599","date":"2026-01-12T21:30Z","competitions":[{{
            "competitors":[
                {{"homeAway":"away","score":"10","team":{{"id":"2","abbreviation":"BUF","color":"00338d","alternateColor":"c60c30"}}}},
                {{"homeAway":"home","score":"10","team":{{"id":"12","abbreviation":"KC","color":"e31837","alternateColor":"ffb81c"}}}}
            ],
            "status":{{"type":{{"state":"in","description":"In Progress"}},"period":1,"displayClock":"10:00"}},
            "situation":{situation}
        }}]}}]}}"#
    )
}

fn live_of(body: &str) -> Vec<u8> {
    let extract = found(body, "401772599", false, usize::MAX);
    encode(&extract.as_game())
}

fn decode_live(bytes: &[u8]) -> (Option<wire::Situation>, Option<wire::Timeouts>) {
    let wire::Game::Live(live) = wire::decode(bytes).unwrap() else {
        panic!("live payload");
    };
    (live.situation, live.timeouts)
}

/// The backend's all-or-nothing situation validation, in its order: a bad
/// down, a bad yardLine, or an unresolvable possession each drop the whole
/// situation — while `distance` alone is clamped, never validated.
#[test]
fn situation_validation_is_all_or_nothing() {
    for bad in [
        r#"{"down":0,"distance":10,"yardLine":50,"possession":"12"}"#,
        r#"{"down":1,"distance":10,"yardLine":101,"possession":"12"}"#,
        r#"{"down":1,"distance":10,"yardLine":50,"possession":"999"}"#,
        r#"{"down":1,"distance":10,"yardLine":50}"#,
    ] {
        let (situation, _) = decode_live(&live_of(&live_body(bad)));
        assert!(situation.is_none(), "situation should drop for {bad}");
    }

    // distance: -1 beside a valid snap silently clamps to 0 (§2.3a).
    let (situation, _) = decode_live(&live_of(&live_body(
        r#"{"down":2,"distance":-1,"yardLine":45,"possession":"12"}"#,
    )));
    let s = situation.expect("valid snap");
    assert_eq!((s.down, s.distance, s.yard_line), (2, 0, 45));
    assert_eq!(s.possession, scoreboard_wire::Side::Home);
}

/// Timeouts ride a separate flag: a dropped situation must not drop them
/// (the backend's `timeouts_present_independently_of_situation`).
#[test]
fn timeouts_survive_a_dropped_situation() {
    let bytes = live_of(&live_body(
        r#"{"down":-1,"distance":-1,"yardLine":-1,"homeTimeouts":1,"awayTimeouts":0}"#,
    ));
    let (situation, timeouts) = decode_live(&bytes);
    assert!(situation.is_none());
    let t = timeouts.expect("timeouts populated with no snap");
    assert_eq!((t.away, t.home), (0, 1));
    assert_eq!(bytes[2], 0x10, "flags: timeouts only");
}

#[derive(Default)]
struct QuirkLog(Vec<Quirk>);

impl Quirks for QuirkLog {
    fn quirk(&mut self, quirk: Quirk) {
        self.0.push(quirk);
    }
}

fn quirks_of(body: &str, target: &str) -> Vec<Quirk> {
    let mut scratch = vec![0u8; 16 * 1024];
    let mut extractor =
        DetailExtractor::new(target, false, QuirkLog::default(), &mut scratch).expect("table valid");
    extractor.write(body.as_bytes()).expect("clean parse");
    extractor.finish().expect("no transform error").quirks.0
}

/// `Quirk::SituationDropped` fires exactly where the backend warns: a
/// glitched down (anything but the `-1` sentinel), an out-of-range
/// yardLine, or an unresolvable possession — while the ordinary
/// between-plays situation stays silent.
#[test]
fn situation_dropped_quirk_fires_where_the_backend_warns() {
    let silent = live_body(r#"{"down":-1,"distance":-1,"yardLine":-1}"#);
    assert_eq!(quirks_of(&silent, "401772599"), vec![]);

    for glitch in [
        r#"{"down":0,"distance":10,"yardLine":50,"possession":"12"}"#,
        r#"{"down":1,"distance":10,"yardLine":101,"possession":"12"}"#,
        r#"{"down":1,"distance":10,"yardLine":50,"possession":"999"}"#,
        r#"{"down":1,"distance":10,"yardLine":50}"#,
    ] {
        assert_eq!(
            quirks_of(&live_body(glitch), "401772599"),
            vec![Quirk::SituationDropped],
            "one warn expected for {glitch}"
        );
    }
}

/// Ruling 16: compare keys refuse to truncate. A team id + possession
/// pair beyond the compare-key bound would string-match in the backend's
/// unbounded compare; here both keys invalidate and the situation drops
/// (the safe direction) with the same warn-quirk — never a prefix match.
/// An over-bound detail target likewise matches nothing: Absent over a
/// clean board, not the wrong game.
#[test]
fn oversized_compare_keys_refuse_instead_of_prefix_matching() {
    let long = "9".repeat(30);
    let body = format!(
        r#"{{"events":[{{"id":"401772599","date":"2026-01-12T21:30Z","competitions":[{{
            "competitors":[
                {{"homeAway":"away","score":"10","team":{{"id":"2","abbreviation":"BUF","color":"00338d","alternateColor":"c60c30"}}}},
                {{"homeAway":"home","score":"10","team":{{"id":"{long}","abbreviation":"KC","color":"e31837","alternateColor":"ffb81c"}}}}
            ],
            "status":{{"type":{{"state":"in","description":"In Progress"}},"period":1,"displayClock":"10:00"}},
            "situation":{{"down":1,"distance":10,"yardLine":50,"possession":"{long}"}}
        }}]}}]}}"#
    );
    assert_eq!(quirks_of(&body, "401772599"), vec![Quirk::SituationDropped]);
    let (situation, _) = decode_live(&live_of(&body));
    assert!(situation.is_none(), "over-bound keys must not resolve a side");

    let clean = live_body(r#"{}"#);
    let (outcome, counts) = run_detail(&clean, &"4".repeat(30), false, usize::MAX);
    assert!(matches!(outcome, DetailOutcome::Absent), "{outcome:?}");
    assert_eq!(counts, Counts { ok: 1, failed: 0 });
}

/// Ruling 4: possession resolves at finalize from buffered ids, so a body
/// that emits `situation` before `competitors` produces identical bytes.
#[test]
fn possession_resolves_independent_of_field_order() {
    let normal = live_body(r#"{"down":3,"distance":1,"yardLine":99,"possession":"2","isRedZone":true}"#);
    let reordered = r#"{"events":[{"date":"2026-01-12T21:30Z","competitions":[{
            "situation":{"down":3,"distance":1,"yardLine":99,"possession":"2","isRedZone":true},
            "status":{"type":{"state":"in","description":"In Progress"},"period":1,"displayClock":"10:00"},
            "competitors":[
                {"homeAway":"away","score":"10","team":{"id":"2","abbreviation":"BUF","color":"00338d","alternateColor":"c60c30"}},
                {"homeAway":"home","score":"10","team":{"id":"12","abbreviation":"KC","color":"e31837","alternateColor":"ffb81c"}}
            ]
        }],"id":"401772599"}]}"#;
    let a = live_of(&normal);
    let b = live_of(reordered);
    assert_eq!(hex(&a), hex(&b), "field order changed the bytes");
    let (situation, _) = decode_live(&a);
    let s = situation.expect("goal-to-go is a valid snap");
    assert_eq!(s.possession, scoreboard_wire::Side::Away);
    assert!(s.red_zone);
}

// ------------------------------------------------------------ crest paths

/// The committed football fixtures are trimmed projections — their team
/// objects carry only what the wire needs, with no `logo` at all. Absence
/// has to be free: no crest, no failure, no change to the bytes.
#[test]
fn trimmed_fixtures_yield_no_crest_and_no_failure() {
    for name in fixture_names() {
        let event = fixture_json(&name);
        let value: serde_json::Value = serde_json::from_str(&event).expect("fixture parses");
        assert!(
            value["competitions"][0]["competitors"][0]["team"]["logo"].is_null(),
            "{name}: fixture unexpectedly grew a logo — assert its value instead"
        );

        let body = wrap(&event);
        let game = found(&body, &event_id(&event), is_college(&name), body.len());
        assert_eq!(
            game.crests(),
            &Crests::default(),
            "{name}: no logo in, no crest out"
        );
    }
}

/// A real college-football slate keys its crests by numeric team id
/// (`/i/teamlogos/ncaa/500/{id}.png`), not by abbreviation — the shape
/// difference the construct-vs-expose ruling turned on. Hrefs are quoted
/// from `firmware-rs/bench/assets/body-cfb-live.json`, where all 198 teams
/// carry them.
#[test]
fn ncaa_crests_ride_the_home_away_ordering() {
    const AWAY: &str = "https://a.espncdn.com/i/teamlogos/ncaa/500/194.png";
    const HOME: &str = "https://a.espncdn.com/i/teamlogos/ncaa/500/130.png";

    let name = "college-football/pregame_ranked";
    let mut value: serde_json::Value =
        serde_json::from_str(&fixture_json(name)).expect("fixture parses");
    {
        let competitors = value["competitions"][0]["competitors"]
            .as_array_mut()
            .expect("competitors array");
        for competitor in competitors.iter_mut() {
            let href = match competitor["homeAway"].as_str().expect("marker") {
                "away" => AWAY,
                _ => HOME,
            };
            competitor["team"]["logo"] = serde_json::Value::String(href.into());
        }
    }
    let event = value.to_string();
    let body = wrap(&event);
    let game = found(&body, &event_id(&event), true, body.len());

    assert_eq!(
        game.crests().away.as_ref().map(|p| p.as_str()),
        Some("/i/teamlogos/ncaa/500/194.png")
    );
    assert_eq!(
        game.crests().home.as_ref().map(|p| p.as_str()),
        Some("/i/teamlogos/ncaa/500/130.png")
    );
    assert_eq!(
        hex(&extract_bytes(name, true, body.len())),
        hex(&golden(name)),
        "the wire bytes are untouched by the crests"
    );
}

#[test]
fn a_malformed_crest_never_costs_the_event() {
    let name = "nfl/pregame";
    for junk in [
        serde_json::json!(42),
        serde_json::json!(null),
        serde_json::json!("https://evil.example.com/i/teamlogos/nfl/500/kc.png"),
    ] {
        let mut value: serde_json::Value =
            serde_json::from_str(&fixture_json(name)).expect("fixture parses");
        value["competitions"][0]["competitors"][0]["team"]["logo"] = junk.clone();
        let event = value.to_string();
        let body = wrap(&event);

        let (outcome, counts) = run_detail(&body, &event_id(&event), false, body.len());
        let game = match outcome {
            DetailOutcome::Found(game) => game,
            other => panic!("{junk}: expected Found, got {other:?}"),
        };
        assert_eq!(counts.failed, 0, "{junk}: the event still parses");
        assert_eq!(game.crests(), &Crests::default(), "{junk}: no crest");
        assert_eq!(
            hex(&encode(&game.as_game())),
            hex(&golden(name)),
            "{junk}: the wire bytes are untouched"
        );
    }
}

// ------------------------------------------------------- list-row extras

/// The `homeAway`-keyed abbreviations, read straight out of the fixture so
/// the expectation cannot drift from it.
fn expected_abbreviations(event_json: &str) -> (Option<String>, Option<String>) {
    let value: serde_json::Value = serde_json::from_str(event_json).expect("fixture parses");
    let mut away = None;
    let mut home = None;
    for competitor in value["competitions"][0]["competitors"]
        .as_array()
        .expect("competitors array")
    {
        let abbreviation = competitor["team"]["abbreviation"]
            .as_str()
            .map(str::to_string);
        match competitor["homeAway"].as_str().expect("marker") {
            "away" => away = abbreviation,
            "home" => home = abbreviation,
            other => panic!("unexpected marker {other}"),
        }
    }
    (away, home)
}

/// CORPUS GAP, pinned so it flips loudly if the fixtures are ever recaptured:
/// the committed football fixtures are hand-authored down to the fields the
/// backend reads, so no `team.logo` exists to extract. Real slates are the
/// opposite — every one of the 198 teams in
/// `firmware-rs/bench/assets/body-cfb-live.json` carries one — so this test
/// asserts the ABBREVIATIONS arrive in full and the crests are absent
/// *because the input has none*, not because the list pass drops them.
#[test]
fn list_rows_name_both_teams_and_the_trimmed_fixtures_carry_no_crest() {
    for name in fixture_names() {
        let event = fixture_json(&name);
        let value: serde_json::Value = serde_json::from_str(&event).expect("fixture parses");
        assert!(
            value["competitions"][0]["competitors"][0]["team"]["logo"].is_null(),
            "{name}: fixture unexpectedly grew a logo — assert its value instead"
        );

        let body = wrap(&event);
        let (rows, _) = run_list_rows(&body, body.len());
        let [row] = rows.as_slice() else {
            panic!("{name}: expected exactly one listed row, got {rows:?}");
        };
        let (away, home) = expected_abbreviations(&event);
        assert_eq!(row.away_abbreviation, away, "{name}: away abbr");
        assert_eq!(row.home_abbreviation, home, "{name}: home abbr");
        assert!(away.is_some() && home.is_some(), "{name}: both named");
        assert_eq!(row.away_crest, None, "{name}: no logo in, no crest out");
        assert_eq!(row.home_crest, None, "{name}: no logo in, no crest out");
    }
}

/// The list twin of [`ncaa_crests_ride_the_home_away_ordering`]: given a
/// payload that carries the artwork, the list row resolves it to the same
/// paths, on the same sides, as the detail extract — one `homeAway`
/// discipline, two readers.
#[test]
fn ncaa_crests_ride_the_list_row_too() {
    const AWAY: &str = "https://a.espncdn.com/i/teamlogos/ncaa/500/194.png";
    const HOME: &str = "https://a.espncdn.com/i/teamlogos/ncaa/500/130.png";

    let name = "college-football/pregame_ranked";
    let mut value: serde_json::Value =
        serde_json::from_str(&fixture_json(name)).expect("fixture parses");
    {
        let competitors = value["competitions"][0]["competitors"]
            .as_array_mut()
            .expect("competitors array");
        for competitor in competitors.iter_mut() {
            let href = match competitor["homeAway"].as_str().expect("marker") {
                "away" => AWAY,
                _ => HOME,
            };
            competitor["team"]["logo"] = serde_json::Value::String(href.into());
        }
    }
    let event = value.to_string();
    let body = wrap(&event);

    let (rows, counts) = run_list_rows(&body, body.len());
    assert_eq!(counts, Counts { ok: 1, failed: 0 });
    let [row] = rows.as_slice() else {
        panic!("expected exactly one listed row, got {rows:?}");
    };
    assert_eq!(row.away_abbreviation.as_deref(), Some("MICH"));
    assert_eq!(row.home_abbreviation.as_deref(), Some("OSU"));
    assert_eq!(row.away_crest.as_deref(), Some("/i/teamlogos/ncaa/500/194.png"));
    assert_eq!(row.home_crest.as_deref(), Some("/i/teamlogos/ncaa/500/130.png"));

    let game = found(&body, &event_id(&event), true, body.len());
    assert_eq!(
        row.away_crest.as_deref(),
        game.crests().away.as_deref(),
        "list and detail disagree on the away crest"
    );
    assert_eq!(
        row.home_crest.as_deref(),
        game.crests().home.as_deref(),
        "list and detail disagree on the home crest"
    );
}

/// Tolerance: an event whose two competitors claim the same side still
/// LISTS — marker conflicts are transform-tier, and the list never runs the
/// transform. The extras go empty on both sides rather than guessing from
/// array position.
#[test]
fn conflicting_markers_still_list_with_empty_extras() {
    let mut value: serde_json::Value =
        serde_json::from_str(&fixture_json("nfl/pregame")).expect("fixture parses");
    for competitor in value["competitions"][0]["competitors"]
        .as_array_mut()
        .expect("competitors array")
    {
        competitor["homeAway"] = serde_json::json!("home");
    }
    let event = value.to_string();
    let body = wrap(&event);

    let (rows, counts) = run_list_rows(&body, body.len());
    assert_eq!(counts, Counts { ok: 1, failed: 0 }, "still a clean parse");
    let [row] = rows.as_slice() else {
        panic!("the event must still list, got {rows:?}");
    };
    assert_eq!(row.state, GameState::Pregame);
    assert_eq!(row.away_abbreviation, None, "unresolvable side");
    assert_eq!(row.home_abbreviation, None, "unresolvable side");
    assert_eq!(row.away_crest, None, "unresolvable side");
    assert_eq!(row.home_crest, None, "unresolvable side");
}
