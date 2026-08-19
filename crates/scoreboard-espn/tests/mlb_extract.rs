//! The MLB parity gate: every committed fixture streamed through the path
//! table must encode byte-identically to its committed wire golden, the
//! rain-delay fixture must be vetoed (that asymmetry is the point — it has
//! no golden on purpose), and the extract must be invariant under chunk
//! splits and JSON key order.
//!
//! Fixtures are single EVENT objects; the extractor sees the real
//! scoreboard shape, so every test wraps them as `{"events":[…]}`.

use scoreboard_espn::common::{ListRow, ListSink, Quirk, Quirks};
use scoreboard_espn::mlb::{
    Counts, DetailError, DetailExtractor, Extract, ListExtractor, TransformError,
};
use scoreboard_wire::{GameState, SliceSink};
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;

// ---------------------------------------------------------------- harness

fn testdata() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../backend/testdata"
    ))
}

fn fixture(name: &str) -> String {
    let path = testdata().join("mlb").join(format!("{name}.json"));
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"))
}

fn golden(name: &str) -> Vec<u8> {
    let path = testdata().join("wire/mlb").join(format!("{name}.bin"));
    fs::read(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"))
}

fn parse(raw: &str) -> Value {
    serde_json::from_str(raw).expect("fixture parses as JSON")
}

fn event_id(raw: &str) -> String {
    parse(raw)["id"].as_str().expect("event id").to_string()
}

/// `{"events":[…]}` — the scoreboard body the engine actually walks.
fn wrap(events: &[&str]) -> Vec<u8> {
    format!("{{\"events\":[{}]}}", events.join(",")).into_bytes()
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
struct Entries(Vec<Row>);

impl ListSink for Entries {
    fn row(&mut self, row: ListRow<'_>) {
        self.0.push(owned_row(row));
    }
}

#[derive(Default)]
struct Recorded(Vec<Quirk>);

impl Quirks for Recorded {
    fn quirk(&mut self, quirk: Quirk) {
        self.0.push(quirk);
    }
}

fn detail_chunked(
    body: &[u8],
    id: &str,
    chunk: usize,
    quirks: &mut Recorded,
) -> Result<(Extract, Counts), DetailError> {
    let mut scratch = vec![0u8; 16 * 1024];
    let mut extractor = DetailExtractor::new(id, quirks, &mut scratch).expect("table validates");
    for piece in body.chunks(chunk.max(1)) {
        extractor.write(piece).expect("chunk accepted");
    }
    extractor.finish()
}

fn detail(body: &[u8], id: &str) -> Result<Extract, DetailError> {
    detail_counts(body, id).map(|(extract, _)| extract)
}

fn detail_counts(body: &[u8], id: &str) -> Result<(Extract, Counts), DetailError> {
    detail_chunked(body, id, usize::MAX, &mut Recorded::default())
}

fn list_rows(body: &[u8]) -> (Vec<Row>, Counts, Vec<Quirk>) {
    let mut scratch = vec![0u8; 16 * 1024];
    let mut quirks = Recorded::default();
    let mut extractor = ListExtractor::new(Entries::default(), &mut quirks, &mut scratch)
        .expect("table validates");
    extractor.write(body).expect("body accepted");
    let (entries, counts) = extractor.finish().expect("scoreboard shape valid");
    (entries.0, counts, quirks.0)
}

fn list(body: &[u8]) -> (Vec<(String, GameState)>, Counts, Vec<Quirk>) {
    let (rows, counts, quirks) = list_rows(body);
    (rows.iter().map(Row::pair).collect(), counts, quirks)
}

fn encode(extract: &Extract) -> Vec<u8> {
    let mut buf = [0u8; 4096];
    let mut sink = SliceSink::new(&mut buf);
    scoreboard_wire::mlb::encode(&extract.as_game(), &mut sink).expect("payload fits");
    sink.written().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------- golden parity

/// Every fixture with a committed golden: stream → extract → `as_game()` →
/// `scoreboard_wire::mlb::encode` must reproduce the golden byte for byte.
#[test]
fn corpus_fixtures_encode_to_their_goldens() {
    for name in ["final", "live_inning", "pregame", "pregame_weather_normal"] {
        let raw = fixture(name);
        let id = event_id(&raw);
        let body = wrap(&[&raw]);
        let extract =
            detail(&body, &id).unwrap_or_else(|e| panic!("{name}: extract failed: {e:?}"));
        assert_eq!(
            hex(&encode(&extract)),
            hex(&golden(name)),
            "{name} does not match its wire golden"
        );
    }
}

/// `rain_delay.json` has NO golden on purpose: still `state:"in"`, still a
/// full situation, and still excluded — the detail path answers NotFound
/// (today's 404, never a 502) and the list drops the entry while counting
/// the event as parsed.
#[test]
fn rain_delay_is_vetoed_not_encoded() {
    let raw = fixture("rain_delay");
    let id = event_id(&raw);
    let body = wrap(&[&raw]);

    let mut quirks = Recorded::default();
    let result = detail_chunked(&body, &id, usize::MAX, &mut quirks);
    assert_eq!(result, Err(DetailError::NotFound));
    assert_eq!(quirks.0, vec![Quirk::UnknownInningHalf]);

    let (entries, counts, quirks) = list(&body);
    assert!(entries.is_empty(), "vetoed game must not be advertised");
    assert_eq!(counts, Counts { ok: 1, failed: 0 });
    assert_eq!(quirks, vec![Quirk::UnknownInningHalf]);
}

// ------------------------------------------------------ split invariance

/// Whole-buffer vs 1-byte-at-a-time (and an uneven prime) must produce the
/// identical extract and identical encoded bytes — the S0 methodology at
/// the extract level.
#[test]
fn chunk_split_invariance_at_the_extract_level() {
    for name in ["pregame", "live_inning", "final"] {
        let raw = fixture(name);
        let id = event_id(&raw);
        let body = wrap(&[&raw]);
        let whole = detail(&body, &id).expect("whole-buffer extract");
        for chunk in [1usize, 7] {
            let (split, _) = detail_chunked(&body, &id, chunk, &mut Recorded::default())
                .unwrap_or_else(|e| panic!("{name}: {chunk}-byte feed failed: {e:?}"));
            assert_eq!(split, whole, "{name}: {chunk}-byte feed extract diverged");
            assert_eq!(
                hex(&encode(&split)),
                hex(&encode(&whole)),
                "{name}: {chunk}-byte feed bytes diverged"
            );
        }
    }
}

/// Ruling 4: no table may assume ESPN's emission order. serde_json's
/// default map is sorted, so a parse → serialize round trip rewrites every
/// object with alphabetical keys (`competitions` before `id`, `situation`
/// before `status` — the discriminant moves), and the golden must survive.
#[test]
fn key_order_does_not_matter() {
    for name in ["pregame", "live_inning", "final"] {
        let raw = fixture(name);
        let id = event_id(&raw);
        let reordered = parse(&raw).to_string();
        assert_ne!(raw.trim(), reordered, "round trip should reorder keys");
        let extract = detail(&wrap(&[&reordered]), &id)
            .unwrap_or_else(|e| panic!("{name} reordered: {e:?}"));
        assert_eq!(
            hex(&encode(&extract)),
            hex(&golden(name)),
            "{name}: reordered keys changed the wire bytes"
        );
    }
}

// ---------------------------------------------------------------- weather

/// The corpus carries both orientations: `pregame.json` is transposed
/// (`displayValue:"6"`, `conditionId:"Mostly cloudy"`) and
/// `pregame_weather_normal.json` is not (`displayValue:"Sunny"`,
/// `conditionId:"1"`). The condition is whichever member does not parse as
/// a number.
#[test]
fn weather_transposition_resolves_both_orientations() {
    let cases = [
        ("pregame", "Mostly cloudy", 58i16),
        ("pregame_weather_normal", "Sunny", 77i16),
    ];
    for (name, condition, temperature) in cases {
        let raw = fixture(name);
        let extract = detail(&wrap(&[&raw]), &event_id(&raw)).expect("pregame extracts");
        let Extract::Pregame(game) = extract else {
            panic!("{name} must extract as pregame");
        };
        let weather = game.weather.expect("weather resolves");
        assert_eq!(weather.condition.as_str(), condition, "{name}");
        assert_eq!(weather.temperature, temperature, "{name}");
    }
}

/// Both members numeric → no condition text → the whole block drops
/// (all-or-nothing with temperature), with the quirk, and the wire flags
/// bit stays clear.
#[test]
fn weather_with_no_resolvable_condition_drops_whole_block() {
    let raw = fixture("pregame_weather_normal");
    let id = event_id(&raw);
    let mut event = parse(&raw);
    event["weather"]["displayValue"] = json!("7"); // conditionId is already "1"
    let mut quirks = Recorded::default();
    let (extract, _) = detail_chunked(&wrap(&[&event.to_string()]), &id, usize::MAX, &mut quirks)
        .expect("event still extracts");
    let Extract::Pregame(ref game) = extract else {
        panic!("must stay pregame");
    };
    assert!(game.weather.is_none());
    assert!(quirks.0.contains(&Quirk::WeatherDropped));
    let bytes = encode(&extract);
    assert_eq!(bytes[2] & 0x01, 0, "weather flag bit must be clear");
    assert_eq!(bytes[3], 0, "temperature byte zeroed by the encoder");
}

/// Missing temperature also drops the block even though a condition
/// resolved — the flag is all-or-nothing.
#[test]
fn weather_without_temperature_drops_whole_block() {
    let raw = fixture("pregame_weather_normal");
    let id = event_id(&raw);
    let mut event = parse(&raw);
    event["weather"]
        .as_object_mut()
        .expect("weather object")
        .remove("temperature");
    let mut quirks = Recorded::default();
    let (extract, _) = detail_chunked(&wrap(&[&event.to_string()]), &id, usize::MAX, &mut quirks)
        .expect("event still extracts");
    let Extract::Pregame(game) = extract else {
        panic!("must stay pregame");
    };
    assert!(game.weather.is_none());
    assert!(quirks.0.contains(&Quirk::WeatherDropped));
}

// -------------------------------------------------------------- list mode

/// Two real fixtures plus a synthetic `{}` (the observed 2026-07-06 glitch
/// shape) in one events array: entries in document order, the glitch
/// counted as failed exactly like `parse_events`.
#[test]
fn list_walks_every_event_and_counts_failures() {
    let pregame = fixture("pregame");
    let live = fixture("live_inning");
    let body = wrap(&[&pregame, "{}", &live]);
    let (entries, counts, quirks) = list(&body);
    assert_eq!(
        entries,
        vec![
            (event_id(&pregame), GameState::Pregame),
            (event_id(&live), GameState::Live),
        ]
    );
    assert_eq!(counts, Counts { ok: 2, failed: 1 });
    assert!(quirks.is_empty());
}

/// The per-state required-field rules are the same parse the backend list
/// runs: pregame requires the venue, live requires the full situation
/// (including the no-default base bools), every state requires
/// `shortDetail`, and a float temperature kills the event.
#[test]
fn list_applies_per_state_required_field_rules() {
    // Pregame without a competition venue: DU-tier reject.
    let mut event = parse(&fixture("pregame"));
    event["competitions"][0]
        .as_object_mut()
        .unwrap()
        .remove("venue");
    let (entries, counts, _) = list(&wrap(&[&event.to_string()]));
    assert!(entries.is_empty());
    assert_eq!(counts, Counts { ok: 0, failed: 1 });

    // Final without a venue: fine — only the pregame arm requires it.
    let mut event = parse(&fixture("final"));
    event["competitions"][0]
        .as_object_mut()
        .unwrap()
        .remove("venue");
    let (entries, counts, _) = list(&wrap(&[&event.to_string()]));
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].1, GameState::Final);
    assert_eq!(counts, Counts { ok: 1, failed: 0 });

    // Live without a situation: DU-tier reject.
    let mut event = parse(&fixture("live_inning"));
    event["competitions"][0]
        .as_object_mut()
        .unwrap()
        .remove("situation");
    let (entries, counts, _) = list(&wrap(&[&event.to_string()]));
    assert!(entries.is_empty());
    assert_eq!(counts, Counts { ok: 0, failed: 1 });

    // Live missing one required base bool (`onSecond` has no default).
    let mut event = parse(&fixture("live_inning"));
    event["competitions"][0]["situation"]
        .as_object_mut()
        .unwrap()
        .remove("onSecond");
    let (entries, counts, _) = list(&wrap(&[&event.to_string()]));
    assert!(entries.is_empty());
    assert_eq!(counts, Counts { ok: 0, failed: 1 });

    // `shortDetail` is deserialized in every state, final included.
    let mut event = parse(&fixture("final"));
    event["competitions"][0]["status"]["type"]
        .as_object_mut()
        .unwrap()
        .remove("shortDetail");
    let (entries, counts, _) = list(&wrap(&[&event.to_string()]));
    assert!(entries.is_empty());
    assert_eq!(counts, Counts { ok: 0, failed: 1 });

    // A float temperature fails `i16` deserialization and kills the event.
    let mut event = parse(&fixture("pregame"));
    event["weather"]["temperature"] = json!(58.5);
    let (entries, counts, _) = list(&wrap(&[&event.to_string()]));
    assert!(entries.is_empty());
    assert_eq!(counts, Counts { ok: 0, failed: 1 });
}

// ------------------------------------------------------------ detail mode

/// Ruling 14: every event before the target is fully validated (and
/// counted), and the result is identical to extracting the fixture alone.
#[test]
fn detail_validates_preceding_events_and_matches_alone() {
    let pregame = fixture("pregame");
    let live = fixture("live_inning");
    let target = event_id(&live);
    let (multi, counts) =
        detail_counts(&wrap(&[&pregame, &live]), &target).expect("target extracts");
    let alone = detail(&wrap(&[&live]), &target).expect("target extracts alone");
    assert_eq!(multi, alone);
    assert_eq!(hex(&encode(&multi)), hex(&golden("live_inning")));
    assert_eq!(
        counts,
        Counts { ok: 2, failed: 0 },
        "the preceding non-target event is validated and counted"
    );
}

/// Ruling 14's pin (mirroring football's): garbage AFTER the found target
/// is skipped and uncounted — the verdict is already Found; garbage BEFORE
/// the target is validated and counted. Absent targets get exact counts
/// because nothing was ever skipped.
#[test]
fn events_after_found_target_are_skipped() {
    let live = fixture("live_inning");
    let target = event_id(&live);

    // Garbage after the target: skipped, uncounted.
    let (extract, counts) =
        detail_counts(&wrap(&[&live, "{}"]), &target).expect("target extracts");
    assert_eq!(hex(&encode(&extract)), hex(&golden("live_inning")));
    assert_eq!(counts, Counts { ok: 1, failed: 0 });

    // Garbage before the target: validated, counted, target still found.
    let (extract, counts) =
        detail_counts(&wrap(&["{}", &live]), &target).expect("target extracts");
    assert_eq!(hex(&encode(&extract)), hex(&golden("live_inning")));
    assert_eq!(counts, Counts { ok: 1, failed: 1 });
}

/// `find_event`'s asymmetry: an absent id on a clean scoreboard is
/// NotFound (the firmware's "game ended"); with any unparseable event it
/// must be Glitched instead — a glitch must never look like "game ended".
#[test]
fn events_shell_must_be_an_array() {
    // Scalar/null shells at the value callback; object shells via the
    // engine's ContainerKind at enter. Either way: never a clean 404/empty
    // slate out of a glitched scoreboard body.
    for body in [
        r#"{"events":42}"#,
        r#"{"events":null}"#,
        r#"{"events":"x"}"#,
        r#"{"events":{"x":1}}"#,
        r#"{"events":{}}"#,
    ] {
        let mut quirks = Recorded::default();
        let result = detail_chunked(body.as_bytes(), "77", usize::MAX, &mut quirks);
        assert!(matches!(result, Err(DetailError::Events)), "{body}");
    }
    // The legal no-games day keeps resolving clean.
    let mut quirks = Recorded::default();
    let result = detail_chunked(br#"{"events":[]}"#, "77", usize::MAX, &mut quirks);
    assert!(matches!(result, Err(DetailError::NotFound)), "legal empty slate");
}

#[test]
fn detail_absent_id_clean_vs_glitched() {
    let pregame = fixture("pregame");
    assert_eq!(
        detail(&wrap(&[&pregame]), "000000000"),
        Err(DetailError::NotFound)
    );
    assert_eq!(
        detail(&wrap(&[&pregame, "{}"]), "000000000"),
        Err(DetailError::Glitched)
    );
    // The target's own event failing to parse is the same 502-class case.
    let mut broken = parse(&pregame);
    broken.as_object_mut().unwrap().remove("date");
    let id = event_id(&pregame);
    assert_eq!(
        detail(&wrap(&[&broken.to_string()]), &id),
        Err(DetailError::Glitched)
    );
    // Ruling 14 (changed expectation): a malformed sibling whose id is
    // readable and non-target is now validated and counted, so the absent
    // target resolves Glitched exactly like the backend — under the old
    // skip-at-id policy this case wrongly answered NotFound.
    assert_eq!(
        detail(&wrap(&[&broken.to_string()]), "000000000"),
        Err(DetailError::Glitched)
    );
}

/// The rain-delay veto is a property of the TARGET event, not of sibling
/// accounting (inventory §2.1, unchanged by ruling 14): the vetoed target
/// answers NotFound even when other events failed to parse — the backend
/// finds the event despite `failed > 0`, then `parse_inning_half` 404s it.
#[test]
fn veto_semantics_survive_sibling_failures() {
    let rain = fixture("rain_delay");
    let id = event_id(&rain);
    for body in [wrap(&["{}", &rain]), wrap(&[&rain, "{}"])] {
        let mut quirks = Recorded::default();
        let result = detail_chunked(&body, &id, usize::MAX, &mut quirks);
        assert_eq!(result, Err(DetailError::NotFound));
        assert_eq!(quirks.0, vec![Quirk::UnknownInningHalf]);
    }
}

/// The two-tier split, date edition: an unparseable `date` still lists
/// (deserialization only wants a string) but the pregame detail transform
/// fails — the backend's 502, not a dropped event.
#[test]
fn unparseable_date_lists_but_fails_detail_transform() {
    let mut event = parse(&fixture("pregame"));
    event["date"] = json!("garbage");
    let body = wrap(&[&event.to_string()]);
    let id = event_id(&fixture("pregame"));

    let (entries, counts, _) = list(&body);
    assert_eq!(entries.len(), 1, "still advertised on the list");
    assert_eq!(counts, Counts { ok: 1, failed: 0 });

    assert_eq!(
        detail(&body, &id),
        Err(DetailError::Transform(TransformError::Date))
    );
}

/// More transform-tier cases: two home markers, a bad color, a bad live
/// score — all list fine, all fail detail with the matching error.
#[test]
fn transform_tier_failures_list_but_fail_detail() {
    // Two homes: `order_home_away` is by marker, never index.
    let mut event = parse(&fixture("live_inning"));
    event["competitions"][0]["competitors"][1]["homeAway"] = json!("home");
    let body = wrap(&[&event.to_string()]);
    let id = event_id(&fixture("live_inning"));
    let (entries, counts, _) = list(&body);
    assert_eq!(entries.len(), 1);
    assert_eq!(counts, Counts { ok: 1, failed: 0 });
    assert_eq!(
        detail(&body, &id),
        Err(DetailError::Transform(TransformError::HomeAway))
    );

    // Bad hex color (final).
    let mut event = parse(&fixture("final"));
    event["competitions"][0]["competitors"][0]["team"]["color"] = json!("zzz");
    let body = wrap(&[&event.to_string()]);
    let id = event_id(&fixture("final"));
    let (entries, counts, _) = list(&body);
    assert_eq!(entries.len(), 1);
    assert_eq!(counts, Counts { ok: 1, failed: 0 });
    assert_eq!(
        detail(&body, &id),
        Err(DetailError::Transform(TransformError::Color))
    );

    // Unparseable live score (checked before colors, like the backend).
    let mut event = parse(&fixture("live_inning"));
    event["competitions"][0]["competitors"][0]["score"] = json!("TBD");
    event["competitions"][0]["competitors"][0]["team"]["color"] = json!("zzz");
    let body = wrap(&[&event.to_string()]);
    let id = event_id(&fixture("live_inning"));
    let (entries, counts, _) = list(&body);
    assert_eq!(entries.len(), 1);
    assert_eq!(counts, Counts { ok: 1, failed: 0 });
    assert_eq!(
        detail(&body, &id),
        Err(DetailError::Transform(TransformError::Score))
    );
}

/// A pregame score is deserialized but never parsed — "TBD" survives and
/// the wire bytes are untouched (scores are not encoded pregame).
#[test]
fn pregame_score_is_not_parsed() {
    let mut event = parse(&fixture("pregame"));
    event["competitions"][0]["competitors"][0]["score"] = json!("TBD");
    event["competitions"][0]["competitors"][1]["score"] = json!("TBD");
    let id = event_id(&fixture("pregame"));
    let extract = detail(&wrap(&[&event.to_string()]), &id).expect("pregame extracts");
    assert_eq!(hex(&encode(&extract)), hex(&golden("pregame")));
}

// --------------------------------------------------------- field details

/// At-bat is all-or-nothing: one side alone yields `None`, which clears
/// wire flag bit0 and removes both strings from the payload.
#[test]
fn at_bat_is_all_or_nothing() {
    let mut event = parse(&fixture("live_inning"));
    event["competitions"][0]["situation"]
        .as_object_mut()
        .unwrap()
        .remove("batter");
    let id = event_id(&fixture("live_inning"));
    let extract = detail(&wrap(&[&event.to_string()]), &id).expect("live extracts");
    let Extract::Live(ref game) = extract else {
        panic!("must extract live");
    };
    assert!(game.at_bat.is_none(), "pitcher alone must not make an at-bat");
    let bytes = encode(&extract);
    assert_eq!(bytes[2], 0, "at-bat flag bit must be clear");
    assert!(
        bytes.len() < golden("live_inning").len(),
        "both at-bat strings must vanish from the payload"
    );
}

/// A malformed `type=="total"` record summary degrades that team's record
/// to `None` with the quirk; the other team keeps its record.
#[test]
fn malformed_total_record_degrades_with_quirk() {
    let mut event = parse(&fixture("pregame"));
    // Home (SF) overall record entry is records[0] in the fixture.
    event["competitions"][0]["competitors"][0]["records"][0]["summary"] = json!("TBD");
    let id = event_id(&fixture("pregame"));
    let mut quirks = Recorded::default();
    let (extract, _) = detail_chunked(&wrap(&[&event.to_string()]), &id, usize::MAX, &mut quirks)
        .expect("pregame extracts");
    let Extract::Pregame(game) = extract else {
        panic!("must extract pregame");
    };
    assert!(game.home.record.is_none(), "malformed record drops");
    assert_eq!(
        game.away.record,
        Some(scoreboard_wire::Record {
            wins: 42,
            losses: 49
        }),
        "the other team's record is untouched"
    );
    assert_eq!(quirks.0, vec![Quirk::MalformedRecord]);
}

// ------------------------------------------------------------ crest paths

/// The `homeAway`-keyed crest hrefs the backend would have resolved, read
/// straight out of the fixture so the expectation cannot drift from it.
fn expected_crests(raw: &str) -> (Option<String>, Option<String>) {
    let event = parse(raw);
    let mut away = None;
    let mut home = None;
    for competitor in event["competitions"][0]["competitors"]
        .as_array()
        .expect("competitors array")
    {
        let logo = competitor["team"]["logo"].as_str().map(|href| {
            href.strip_prefix("https://a.espncdn.com")
                .expect("fixture logos are on the ESPN CDN")
                .to_string()
        });
        match competitor["homeAway"].as_str().expect("marker") {
            "away" => away = logo,
            "home" => home = logo,
            other => panic!("unexpected marker {other}"),
        }
    }
    (away, home)
}

#[test]
fn every_fixture_exposes_both_crests_ordered_by_home_away() {
    for name in ["final", "live_inning", "pregame", "pregame_weather_normal"] {
        let raw = fixture(name);
        let body = wrap(&[&raw]);
        let extract = detail(&body, &event_id(&raw)).expect("extract");

        let (away, home) = expected_crests(&raw);
        let crests = extract.crests();
        assert_eq!(
            crests.away.as_ref().map(|p| p.as_str().to_string()),
            away,
            "{name}: away crest"
        );
        assert_eq!(
            crests.home.as_ref().map(|p| p.as_str().to_string()),
            home,
            "{name}: home crest"
        );
        assert!(
            crests.away.is_some() && crests.home.is_some(),
            "{name}: MLB fixtures all carry logos"
        );
    }
}

#[test]
fn a_malformed_crest_never_costs_the_event() {
    for junk in [
        serde_json::json!(42),
        serde_json::json!(null),
        serde_json::json!("https://evil.example.com/i/teamlogos/mlb/500/scoreboard/sf.png"),
    ] {
        let mut event = parse(&fixture("pregame"));
        event["competitions"][0]["competitors"][0]["team"]["logo"] = junk.clone();
        let mutated = event.to_string();
        let body = wrap(&[&mutated]);

        let (extract, counts) =
            detail_counts(&body, &event_id(&mutated)).expect("event still extracts");
        assert_eq!(counts.failed, 0, "{junk}: the event still parses");
        assert_eq!(
            hex(&encode(&extract)),
            hex(&golden("pregame")),
            "{junk}: the wire bytes are untouched"
        );
    }
}

// ------------------------------------------------------- list-row extras

/// The `homeAway`-keyed abbreviations, read straight out of the fixture so
/// the expectation cannot drift from it (the crest twin of this is
/// [`expected_crests`]).
fn expected_abbreviations(raw: &str) -> (Option<String>, Option<String>) {
    let event = parse(raw);
    let mut away = None;
    let mut home = None;
    for competitor in event["competitions"][0]["competitors"]
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

/// The probe-elimination contract: everything the crest warmer needs is on
/// the list row the poller already pays for, `homeAway`-ordered exactly like
/// the detail extract's own crests.
#[test]
fn list_rows_carry_both_abbreviations_and_both_crests() {
    for name in ["final", "live_inning", "pregame", "pregame_weather_normal"] {
        let raw = fixture(name);
        let (rows, _, _) = list_rows(&wrap(&[&raw]));
        let [row] = rows.as_slice() else {
            panic!("{name}: expected exactly one listed row, got {rows:?}");
        };

        let (away_abbreviation, home_abbreviation) = expected_abbreviations(&raw);
        let (away_crest, home_crest) = expected_crests(&raw);
        assert_eq!(row.away_abbreviation, away_abbreviation, "{name}: away abbr");
        assert_eq!(row.home_abbreviation, home_abbreviation, "{name}: home abbr");
        assert_eq!(row.away_crest, away_crest, "{name}: away crest");
        assert_eq!(row.home_crest, home_crest, "{name}: home crest");
        assert!(
            row.away_crest.is_some() && row.home_crest.is_some(),
            "{name}: MLB fixtures all carry logos"
        );

        // The row and the detail extract must not disagree about which
        // artwork belongs to whom — one `homeAway` discipline, two readers.
        let extract = detail(&wrap(&[&raw]), &event_id(&raw)).expect("extract");
        assert_eq!(
            row.away_crest.as_deref(),
            extract.crests().away.as_deref(),
            "{name}: list and detail disagree on the away crest"
        );
        assert_eq!(
            row.home_crest.as_deref(),
            extract.crests().home.as_deref(),
            "{name}: list and detail disagree on the home crest"
        );
    }
}

/// Tolerance: an event whose two competitors claim the same side still
/// LISTS — marker conflicts are transform-tier, and the list never runs the
/// transform. The extras go empty on both sides rather than guessing from
/// array position.
#[test]
fn conflicting_markers_still_list_with_empty_extras() {
    let mut event = parse(&fixture("pregame"));
    for competitor in event["competitions"][0]["competitors"]
        .as_array_mut()
        .expect("competitors array")
    {
        competitor["homeAway"] = serde_json::json!("home");
    }
    let mutated = event.to_string();
    let (rows, counts, _) = list_rows(&wrap(&[&mutated]));

    assert_eq!(counts, Counts { ok: 1, failed: 0 }, "still a clean parse");
    let [row] = rows.as_slice() else {
        panic!("the event must still list, got {rows:?}");
    };
    assert_eq!(row.state, GameState::Pregame);
    assert_eq!(row.away_abbreviation, None, "unresolvable side");
    assert_eq!(row.home_abbreviation, None, "unresolvable side");
    assert_eq!(row.away_crest, None, "unresolvable side");
    assert_eq!(row.home_crest, None, "unresolvable side");

    // Detail mode, over the same body, reports the conflict as it always did.
    assert_eq!(
        detail(&wrap(&[&mutated]), &event_id(&mutated)),
        Err(DetailError::Transform(TransformError::HomeAway))
    );
}

/// Tolerance: a payload with no `team.logo` at all still lists, still names
/// both teams, and simply has no artwork — the extras are best-effort, never
/// a gate.
#[test]
fn a_logo_less_event_lists_with_abbreviations_and_no_crests() {
    let mut event = parse(&fixture("final"));
    for competitor in event["competitions"][0]["competitors"]
        .as_array_mut()
        .expect("competitors array")
    {
        competitor["team"]
            .as_object_mut()
            .expect("team object")
            .remove("logo");
    }
    let mutated = event.to_string();
    let (rows, counts, _) = list_rows(&wrap(&[&mutated]));

    assert_eq!(counts, Counts { ok: 1, failed: 0 });
    let [row] = rows.as_slice() else {
        panic!("the event must still list, got {rows:?}");
    };
    assert_eq!(row.away_abbreviation.as_deref(), Some("MIL"));
    assert_eq!(row.home_abbreviation.as_deref(), Some("STL"));
    assert_eq!(row.away_crest, None);
    assert_eq!(row.home_crest, None);
}
