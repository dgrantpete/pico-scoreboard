//! NBA summary lane: the corpus pairs and NBA.md §3's rules.
//!
//! Fixtures are single EVENT objects (`backend/testdata/basketball/nba/*.json`),
//! wrapped here as `{"events":[…]}` scoreboard bodies, each paired with the
//! raw summary body of the same moment (`*.summary.json`; pregame has none).
//! The expected values are `PROVENANCE.md`'s evidence plus a reference walk
//! of §3 over the pairs; every wire value the lane produces is encoded at
//! `ext=2` and decoded back through `scoreboard_wire::basketball`. The scoreboard
//! half's parity gate is `basketball_extract.rs`; this file is the summary half and
//! the fields the court screens added to the scoreboard.

use std::fs;
use std::path::PathBuf;

use scoreboard_espn::basketball::{
    self, DetailOutcome, Extract, Extractor, Kind, Summary, SummaryContext, SummaryExtractor,
    SummaryReport,
};
use scoreboard_espn::common::{Quirk, Quirks};
use scoreboard_espn::StreamMatcher;
use scoreboard_wire::basketball::{self as wire, Game, Ledger, Make, Run, TeamLedger};
use scoreboard_wire::{Record, Side, SliceSink};

/// Every committed pair, in `PROVENANCE.md` order.
const FIXTURES: &[&str] = &[
    "pregame",
    "tip",
    "in_progress",
    "run",
    "bonus",
    "timeout",
    "end_of_period",
    "halftime",
    "end_of_fourth",
    "overtime",
    "frozen_clock",
    "ledger_glitch",
    "placeholder_play",
    "in_progress_no_last_play",
    "in_progress_subminute",
    "final",
    "final_ot",
    "final_2ot",
    "final_playoff",
    "bonus_both",
];

fn testdata(rel: &str) -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../backend/testdata/basketball/nba"
    ))
    .join(rel)
}

fn fixture(name: &str) -> String {
    let path = testdata(&format!("{name}.json"));
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"))
}

fn summary_text(name: &str) -> Option<String> {
    let path = testdata(&format!("{name}.summary.json"));
    path.exists()
        .then(|| fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}")))
}

fn wrap(events: &[&str]) -> String {
    format!("{{\"events\":[{}]}}", events.join(","))
}

fn parse(json: &str) -> serde_json::Value {
    serde_json::from_str(json).expect("fixture parses")
}

fn id_of(event_json: &str) -> String {
    parse(event_json)["id"]
        .as_str()
        .expect("string id")
        .to_string()
}

fn mutate(json: &str, f: impl FnOnce(&mut serde_json::Value)) -> String {
    let mut value = parse(json);
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

fn chunks(len: usize, sizes: &[usize]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let (mut pos, mut i) = (0, 0);
    while pos < len {
        let end = (pos + sizes[i % sizes.len()].max(1)).min(len);
        out.push((pos, end));
        pos = end;
        i += 1;
    }
    out
}

fn extract_sized(body: &str, target: &str, sizes: &[usize]) -> (DetailOutcome, RecQuirks) {
    let mut quirks = RecQuirks::default();
    let outcome = {
        let extractor = Extractor::game_detail(target, &mut quirks);
        let mut scratch = vec![0u8; 64 * 1024];
        let mut matcher = StreamMatcher::new(basketball::PATHS, extractor, &mut scratch).unwrap();
        for (start, end) in chunks(body.len(), sizes) {
            matcher.write(&body.as_bytes()[start..end]).unwrap();
        }
        matcher
            .finish()
            .unwrap()
            .into_detail()
            .expect("detail mode")
    };
    (outcome, quirks)
}

fn detail(body: &str, target: &str) -> (Extract, RecQuirks) {
    let (outcome, quirks) = extract_sized(body, target, &[usize::MAX]);
    match outcome {
        DetailOutcome::Found(extract) => (extract, quirks),
        other => panic!("expected Found for {target}, got {other:?}"),
    }
}

fn found(body: &str, target: &str) -> Extract {
    detail(body, target).0
}

fn summarize_sized(
    text: &str,
    context: SummaryContext,
    sizes: &[usize],
) -> SummaryReport<RecQuirks> {
    let mut scratch = vec![0u8; 64 * 1024];
    let mut extractor = SummaryExtractor::new(context, RecQuirks::default(), &mut scratch).unwrap();
    for (start, end) in chunks(text.len(), sizes) {
        extractor.write(&text.as_bytes()[start..end]).unwrap();
    }
    extractor.finish().unwrap()
}

fn summarize(text: &str, context: SummaryContext) -> SummaryReport<RecQuirks> {
    summarize_sized(text, context, &[usize::MAX])
}

/// The scoreboard extract of a fixture and its summary, whole-buffer.
fn pair(name: &str) -> (Extract, Option<Summary>) {
    let event = fixture(name);
    let extract = found(&wrap(&[&event]), &id_of(&event));
    let summary =
        summary_text(name).map(|text| summarize(&text, extract.summary_context()).summary);
    (extract, summary)
}

/// The summary report of a fixture, quirks included.
fn summary_report(name: &str) -> (Extract, SummaryReport<RecQuirks>) {
    let event = fixture(name);
    let extract = found(&wrap(&[&event]), &id_of(&event));
    let report = summarize(
        &summary_text(name).expect("pair"),
        extract.summary_context(),
    );
    (extract, report)
}

fn encode_at(game: &Game<'_>, level: u8) -> Vec<u8> {
    let mut buf = [0u8; 4096];
    let mut sink = SliceSink::new(&mut buf);
    wire::encode_at(game, level, &mut sink).expect("encode fits");
    sink.written().to_vec()
}

/// Every value the lane emits must survive the wire at `ext=2`: encode,
/// decode, equal.
fn round_trip(game: &Game<'_>) -> Vec<u8> {
    let bytes = encode_at(game, 2);
    let decoded = wire::decode(&bytes).expect("decodes");
    assert_eq!(&decoded, game, "round trip");
    bytes
}

fn live<'a>(game: &'a Game<'a>) -> &'a wire::Live<'a> {
    match game {
        Game::Live(live) => live,
        other => panic!("expected live, got {other:?}"),
    }
}

fn final_<'a>(game: &'a Game<'a>) -> &'a wire::Final<'a> {
    match game {
        Game::Final(game) => game,
        other => panic!("expected final, got {other:?}"),
    }
}

fn pregame<'a>(game: &'a Game<'a>) -> &'a wire::Pregame<'a> {
    match game {
        Game::Pregame(game) => game,
        other => panic!("expected pregame, got {other:?}"),
    }
}

fn column(fouls: u8, in_bonus: bool, timeouts: u8) -> TeamLedger {
    TeamLedger {
        fouls,
        in_bonus,
        timeouts,
    }
}

fn ledger(away: TeamLedger, home: TeamLedger) -> Option<Ledger> {
    Some(Ledger { away, home })
}

fn last_play<'a>(game: &'a wire::Live<'a>) -> Option<(&'a str, &'a str)> {
    game.last_play.map(|play| (play.id, play.text))
}

/// `(side, seq, scorer, points, marked)` of the wire's newest score.
fn newest<'a>(game: &'a wire::Live<'a>) -> Option<(Side, u32, &'a str, u8, bool)> {
    game.newest_score.map(|score| {
        (
            score.side,
            score.seq,
            score.scorer,
            score.scorer_points,
            score.marked,
        )
    })
}

/// `(side, value, name)` per leader row.
fn leader_rows<'a>(game: &'a wire::Final<'a>) -> Option<[(Side, u8, &'a str); 3]> {
    game.leaders.map(|leaders| {
        [leaders.points, leaders.rebounds, leaders.assists]
            .map(|leader| (leader.side, leader.value, leader.name))
    })
}

fn record(wins: u16, losses: u16) -> Record {
    Record { wins, losses }
}

// ----------------------------------------------------------------- corpus

/// What a live pair's summary half says (the reference walk of §3 over the
/// corpus, checked against PROVENANCE where it speaks).
struct LiveExpect {
    ledger: Option<Ledger>,
    run: Option<Run>,
    makes: usize,
    newest: Option<(Side, u32, &'static str, u8, bool)>,
    /// The flash the combined view carries, and whether the scoreboard's
    /// own `lastPlay` is what carries it.
    flash: Option<(&'static str, &'static str, bool)>,
}

fn live_expect(name: &str) -> LiveExpect {
    let (away, home) = (Side::Away, Side::Home);
    match name {
        "tip" => LiveExpect {
            ledger: ledger(column(0, false, 7), column(0, false, 7)),
            run: None,
            makes: 0,
            newest: None,
            flash: None,
        },
        "in_progress" => LiveExpect {
            ledger: ledger(column(0, false, 6), column(0, false, 6)),
            run: None, // home 5, under RUN_SHOW
            makes: 19,
            newest: Some((home, 180, "MINIX", 3, true)),
            flash: Some(("40190994086", "Knicks Full timeout", false)),
        },
        "run" => LiveExpect {
            ledger: ledger(column(1, false, 6), column(0, false, 6)),
            run: Some(Run {
                side: home,
                points: 8,
            }),
            makes: 19,
            newest: Some((home, 193, "MCCAIN", 4, true)),
            flash: Some(("401909941136", "Thunder Full timeout", false)),
        },
        "bonus" => LiveExpect {
            // DEN (away) DOUBLE ⇔ IND has no fouls to give; IND NONE.
            ledger: ledger(column(3, true, 2), column(4, false, 2)),
            run: None,
            makes: 83,
            newest: Some((home, 625, "SIAKAM", 23, true)),
            flash: Some(("401909938579", "Pacers Full timeout", false)),
        },
        "timeout" => LiveExpect {
            ledger: ledger(column(1, false, 7), column(3, false, 6)),
            run: None,
            makes: 7,
            newest: Some((away, 62, "TOWNS", 7, true)),
            flash: Some(("40190994067", "Cavaliers Full timeout", true)),
        },
        "end_of_period" => LiveExpect {
            ledger: ledger(column(4, true, 2), column(5, true, 3)),
            run: None,
            makes: 68,
            newest: Some((away, 484, "DEROZAN", 9, true)),
            flash: Some((
                "401909938499",
                "(00:09.7) [Nuggets] COACH'S CHALLENGE (CALL OVERTURNED) [Nuggets] retain their timeout",
                false,
            )),
        },
        "halftime" => LiveExpect {
            ledger: ledger(column(4, true, 5), column(6, true, 4)),
            run: None,
            makes: 50,
            newest: Some((away, 340, "TRAORE", 15, true)),
            flash: Some(("401909939291", "Nets Full timeout", false)),
        },
        "end_of_fourth" => LiveExpect {
            // Both out of timeouts and both in the penalty: real, not the glitch.
            ledger: ledger(column(6, true, 0), column(8, true, 0)),
            run: None,
            makes: 79,
            newest: Some((away, 782, "CAIN", 18, true)),
            flash: Some(("401909942773", "Magic Full timeout", false)),
        },
        "overtime" => LiveExpect {
            // The overtime allotment: 2 timeouts a side, the fouls reset.
            ledger: ledger(column(0, false, 2), column(0, false, 2)),
            run: None,
            makes: 79,
            newest: Some((away, 782, "CAIN", 18, true)),
            flash: Some(("401909942773", "Magic Full timeout", false)),
        },
        "frozen_clock" => LiveExpect {
            ledger: ledger(column(0, false, 3), column(1, false, 3)),
            run: None,
            makes: 72,
            newest: Some((away, 540, "JOHNSON", 18, true)),
            flash: Some((
                "401909938499",
                "(00:09.7) [Nuggets] COACH'S CHALLENGE (CALL OVERTURNED) [Nuggets] retain their timeout",
                false,
            )),
        },
        "ledger_glitch" => LiveExpect {
            ledger: None,
            run: None,
            makes: 75,
            newest: Some((home, 550, "SIAKAM", 18, true)),
            flash: Some(("401909938555", "Nuggets Full timeout", true)),
        },
        "placeholder_play" => LiveExpect {
            ledger: ledger(column(2, false, 6), column(3, false, 6)),
            run: None,
            makes: 11,
            newest: Some((away, 111, "MCBRIDE", 3, true)),
            flash: Some(("40190994086", "Knicks Full timeout", false)),
        },
        "in_progress_no_last_play" => LiveExpect {
            ledger: ledger(column(0, false, 7), column(0, false, 7)),
            run: None,
            makes: 0,
            newest: None,
            flash: None,
        },
        "in_progress_subminute" => LiveExpect {
            ledger: ledger(column(3, true, 2), column(6, false, 1)),
            run: None,
            makes: 85,
            newest: Some((away, 654, "MURRAY", 16, false)),
            flash: Some(("401909938646", "Pacers Full timeout", false)),
        },
        "bonus_both" => LiveExpect {
            ledger: ledger(column(5, true, 6), column(5, true, 6)),
            run: None,
            makes: 16,
            // A free throw: the newest score, but not a make to blink.
            newest: Some((away, 156, "TOWNS", 14, false)),
            flash: Some(("40190994086", "Knicks Full timeout", false)),
        },
        other => panic!("no live expectations for {other}"),
    }
}

/// Everything the corpus says about a pair, on the decoded wire values: the
/// scoreboard alone (`without`) and combined with its summary (`with`).
fn check_fixture(name: &str, without: &Game<'_>, with: Option<&Game<'_>>) {
    match name {
        "pregame" => {
            let game = pregame(without);
            assert_eq!(game.game_id, "401909944");
            assert_eq!(game.info_secondary, "", "no notes on this slate");
            assert_eq!(game.away.record, Some(record(7, 2)));
            assert_eq!(game.home.record, Some(record(6, 2)));
            assert_eq!(game.away.line, Some("-5.5  -205"));
            assert!(with.is_none());
        }
        "final" | "final_ot" | "final_2ot" | "final_playoff" => {
            // Nothing on the final screen comes from the summary (D3).
            assert_eq!(Some(without), with);
            let game = final_(without);
            let (rows, records, series) = match name {
                "final" => (
                    [
                        (Side::Home, 23, "P. Siakam"),
                        (Side::Away, 13, "N. Jokic"),
                        // 8 apiece: the tie keeps the away leader.
                        (Side::Away, 8, "N. Jokic"),
                    ],
                    (record(5, 3), record(3, 7)),
                    None,
                ),
                "final_ot" => (
                    [
                        (Side::Home, 41, "S. Curry"),
                        (Side::Away, 12, "J. Cain"),
                        (Side::Home, 13, "D. Green"),
                    ],
                    (record(2, 6), record(6, 3)),
                    None,
                ),
                "final_2ot" => (
                    [
                        (Side::Away, 41, "V. Wembanyama"),
                        (Side::Away, 24, "V. Wembanyama"),
                        (Side::Home, 12, "S. Gilgeous-Alexander"),
                    ],
                    (record(62, 20), record(64, 18)),
                    Some("SA leads series 1-0"),
                ),
                _ => (
                    [
                        (Side::Home, 36, "J. Brunson"),
                        (Side::Away, 13, "V. Wembanyama"),
                        // 7 apiece: away.
                        (Side::Away, 7, "D. Fox"),
                    ],
                    (record(62, 20), record(53, 29)),
                    Some("NY leads series 3-1"),
                ),
            };
            assert_eq!(leader_rows(game), Some(rows), "{name}: leaders");
            assert_eq!(
                game.records.map(|r| (r.away, r.home)),
                Some(records),
                "{name}: records"
            );
            assert_eq!(game.series, series, "{name}: series");
        }
        live_name => {
            let expect = live_expect(live_name);
            // The scoreboard alone: the groups absent, the flash only when
            // its own lastPlay is eligible.
            let alone = live(without);
            assert_eq!(alone.ledger, None, "{name}: no ledger without a summary");
            assert_eq!(alone.run, None);
            assert_eq!(alone.newest_score, None);
            assert!(alone.makes.is_empty());
            let from_scoreboard = expect
                .flash
                .filter(|&(_, _, scoreboard)| scoreboard)
                .map(|(id, text, _)| (id, text));
            assert_eq!(
                last_play(alone),
                from_scoreboard,
                "{name}: scoreboard flash"
            );

            let game = live(with.expect("every live fixture has its summary"));
            assert_eq!(game.ledger, expect.ledger, "{name}: ledger");
            assert_eq!(game.run, expect.run, "{name}: run");
            assert_eq!(game.makes.len(), expect.makes, "{name}: makes");
            assert_eq!(newest(game), expect.newest, "{name}: newest score");
            assert_eq!(
                last_play(game),
                expect.flash.map(|(id, text, _)| (id, text)),
                "{name}: flash"
            );
            // Every mark is inside the lines the wire accepts, before the
            // encoder has to clamp anything.
            for make in game.makes.iter() {
                assert!((-25..=25).contains(&make.x), "{name}: x {}", make.x);
                assert!((-8..=47).contains(&make.y), "{name}: y {}", make.y);
            }
        }
    }
}

#[test]
fn every_pair_extracts_and_round_trips_with_and_without_its_summary() {
    for name in FIXTURES {
        let (extract, summary) = pair(name);
        let without = extract.as_game(None);
        round_trip(&without);
        let with = summary
            .as_ref()
            .map(|summary| extract.as_game(Some(summary)));
        if let Some(with) = &with {
            round_trip(with);
        }
        check_fixture(name, &without, with.as_ref());
    }
}

/// The scoreboard alone is today's view: the ext=2 groups absent and, below
/// `ext=2`, exactly the bytes of the `ext=1` encoding.
#[test]
fn without_a_summary_the_view_is_the_scoreboard_alone_at_every_level() {
    for name in FIXTURES {
        let (extract, _) = pair(name);
        let game = extract.as_game(None);
        let plain = encode_at(&game, 1);
        let extended = encode_at(&game, 2);
        assert!(
            extended.starts_with(&plain),
            "{name}: ext=2 appends to ext=1"
        );
        let decoded = wire::decode(&plain).expect("ext=1 decodes");
        match (&decoded, &game) {
            (Game::Live(short), Game::Live(full)) => {
                assert_eq!(short.last_play, full.last_play);
                assert_eq!(
                    (short.ledger, short.run, short.newest_score),
                    (None, None, None)
                );
                assert!(full.makes.is_empty());
            }
            (Game::Pregame(short), Game::Pregame(full)) => {
                assert_eq!(short.info_secondary, "");
                assert_eq!(
                    (short.away.line, short.home.line),
                    (full.away.line, full.home.line)
                );
            }
            (Game::Final(short), Game::Final(_)) => {
                assert_eq!(
                    (short.leaders, short.series, short.records),
                    (None, None, None)
                );
            }
            other => panic!("{name}: states differ: {other:?}"),
        }
    }
}

#[test]
fn the_corpus_pairs_raise_only_the_quirks_the_data_predicts() {
    for name in FIXTURES {
        let event = fixture(name);
        let (extract, quirks) = detail(&wrap(&[&event]), &id_of(&event));
        assert_eq!(quirks.0, vec![], "{name}: scoreboard quirks");
        if let Some(text) = summary_text(name) {
            let report = summarize(&text, extract.summary_context());
            let expected = match *name {
                // PROVENANCE's one glitch body.
                "ledger_glitch" => vec![Quirk::LedgerGlitch],
                // ORL@GS: two rows after Cain's tying putback carry the
                // stale 108-110 before the buzzer reads 110-110 again — a
                // running score that fell, which §3.5 treats as a
                // correction. (Not in PROVENANCE; a §2 contradiction.)
                "end_of_fourth" | "overtime" => vec![Quirk::RunCorrection],
                _ => vec![],
            };
            assert_eq!(report.quirks.0, expected, "{name}: summary quirks");
        }
    }
}

#[test]
fn chunk_split_invariance_over_every_pair() {
    let seeded: Vec<usize> = (0..64)
        .map(|i| 1 + (17usize.wrapping_mul(31).wrapping_add(i * 7)) % 97)
        .collect();
    for name in FIXTURES {
        let event = fixture(name);
        let target = id_of(&event);
        let body = wrap(&[&event]);
        let reference = found(&body, &target);
        for sizes in [&[1][..], &[7], &seeded] {
            let (outcome, _) = extract_sized(&body, &target, sizes);
            let DetailOutcome::Found(split) = outcome else {
                panic!("{name} at {sizes:?}");
            };
            assert_eq!(split, reference, "{name}: scoreboard diverged at {sizes:?}");
        }

        let Some(text) = summary_text(name) else {
            continue;
        };
        let context = reference.summary_context();
        let whole = summarize(&text, context.clone());
        // The summaries run to 460 KB; the byte-at-a-time feed covers the
        // largest live one and the tip, the rest the two others.
        let one_byte = matches!(*name, "end_of_fourth" | "tip");
        for sizes in [&[7][..], &seeded]
            .into_iter()
            .chain(one_byte.then_some(&[1][..]))
        {
            let split = summarize_sized(&text, context.clone(), sizes);
            assert_eq!(
                split.summary, whole.summary,
                "{name}: summary diverged at {sizes:?}"
            );
            assert_eq!(
                split.quirks.0, whole.quirks.0,
                "{name}: quirks diverged at {sizes:?}"
            );
        }
    }
}

/// Re-serialize with every object's keys in sorted or reverse-sorted order —
/// which moves `header` behind `plays`, a play's `team` behind its
/// `participants`, a block's `names` behind its `athletes`, and so on.
fn reordered(value: &serde_json::Value, reverse: bool, out: &mut String) {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            if reverse {
                keys.reverse();
            }
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key).unwrap());
                out.push(':');
                reordered(&map[*key], reverse, out);
            }
            out.push('}');
        }
        serde_json::Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                reordered(item, reverse, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&serde_json::to_string(scalar).unwrap()),
    }
}

#[test]
fn field_order_independence_over_every_pair() {
    for name in FIXTURES {
        let event = fixture(name);
        let target = id_of(&event);
        let reference = found(&wrap(&[&event]), &target);
        let summary = summary_text(name);
        let reference_summary = summary
            .as_ref()
            .map(|text| summarize(text, reference.summary_context()));
        for reverse in [false, true] {
            let mut shuffled = String::new();
            reordered(&parse(&event), reverse, &mut shuffled);
            assert_ne!(
                shuffled, event,
                "{name}: the reordering must change the text"
            );
            let extract = found(&wrap(&[&shuffled]), &target);
            assert_eq!(
                extract, reference,
                "{name}: scoreboard order-dependent (reverse={reverse})"
            );

            let Some(text) = &summary else {
                continue;
            };
            let mut shuffled = String::new();
            reordered(&parse(text), reverse, &mut shuffled);
            let report = summarize(&shuffled, extract.summary_context());
            let reference_summary = reference_summary.as_ref().unwrap();
            assert_eq!(
                report.summary, reference_summary.summary,
                "{name}: summary order-dependent (reverse={reverse})"
            );
            assert_eq!(
                report.quirks.0, reference_summary.quirks.0,
                "{name}: quirks order-dependent"
            );
        }
    }
}

// ------------------------------------------------------- §3.1 scoreboard

#[test]
fn unknown_status_name_is_a_quirk_and_delayed_is_not() {
    let event = fixture("in_progress");
    let target = id_of(&event);
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["status"]["type"]["name"] = serde_json::json!("STATUS_RAIN_DELAY");
    })]);
    let (extract, quirks) = detail(&body, &target);
    assert!(matches!(extract.kind, Kind::Live(_)), "follows its state");
    assert_eq!(quirks.0, vec![Quirk::UnknownStatusName]);

    // §2 says to assume STATUS_DELAYED exists; it is not unknown.
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["status"]["type"]["name"] = serde_json::json!("STATUS_DELAYED");
    })]);
    assert_eq!(detail(&body, &target).1 .0, vec![]);
}

#[test]
fn final_alt_detail_disagreement_is_a_quirk_not_a_rejection() {
    // Regulation with an altDetail, overtime without one: both quirks.
    let event = fixture("final");
    let target = id_of(&event);
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["status"]["type"]["altDetail"] = serde_json::json!("OT");
    })]);
    let (extract, quirks) = detail(&body, &target);
    assert_eq!(final_(&extract.as_game(None)).periods_played, 4);
    assert_eq!(quirks.0, vec![Quirk::FinalDetailMismatch]);

    let event = fixture("final_ot");
    let target = id_of(&event);
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["status"]["type"]
            .as_object_mut()
            .unwrap()
            .remove("altDetail");
    })]);
    assert_eq!(
        detail(&body, &target).1 .0,
        vec![Quirk::FinalDetailMismatch]
    );

    // "2OT" at period 6 and "OT" at 5 agree (the corpus sweep is quiet).
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["status"]["period"] = serde_json::json!(6);
        v["competitions"][0]["status"]["type"]["altDetail"] = serde_json::json!("2OT");
    })]);
    assert_eq!(detail(&body, &target).1 .0, vec![]);
}

// --------------------------------------------------------- §3.3 ledger

#[test]
fn the_ledger_reads_fouls_bonus_and_timeouts_by_home_away() {
    // DEN (away) is in the bonus — IND has no fouls to give — IND is not.
    let (extract, summary) = pair("bonus");
    let game = extract.as_game(summary.as_ref());
    assert_eq!(
        live(&game).ledger,
        ledger(column(3, true, 2), column(4, false, 2))
    );
    // Both sides in the penalty at once: both arrows.
    let (extract, summary) = pair("bonus_both");
    assert_eq!(
        live(&extract.as_game(summary.as_ref())).ledger,
        ledger(column(5, true, 6), column(5, true, 6))
    );
}

#[test]
fn bonus_state_is_the_equivalence_the_data_proved() {
    // `X.bonusState == "DOUBLE"` ⇔ the opponent's `foulsToGive == 0`, on
    // every side of every live pair (494/494 at design time).
    for name in FIXTURES {
        let Some(text) = summary_text(name) else {
            continue;
        };
        let summary = parse(&text);
        let competitors = summary["header"]["competitions"][0]["competitors"]
            .as_array()
            .unwrap();
        if competitors.iter().any(|c| c.get("fouls").is_none()) {
            continue; // a final: the keys are gone
        }
        for (k, competitor) in competitors.iter().enumerate() {
            let opponent = &competitors[1 - k];
            assert_eq!(
                competitor["fouls"]["bonusState"] == "DOUBLE",
                opponent["fouls"]["foulsToGive"] == 0,
                "{name}: side {k}"
            );
        }
    }
}

#[test]
fn the_glitch_body_has_no_ledger_and_the_look_alike_keeps_its_own() {
    // 0/0 timeouts, 0/0 to give, DOUBLE/DOUBLE over 1/1 team fouls: the
    // one-poll reset, a quirk and no ledger.
    let (extract, report) = summary_report("ledger_glitch");
    assert_eq!(report.summary.ledger, None);
    assert_eq!(report.quirks.0, vec![Quirk::LedgerGlitch]);
    let game = extract.as_game(Some(&report.summary));
    assert_eq!(live(&game).ledger, None);
    round_trip(&game);

    // The end of ORL@GS's 4th: both out of timeouts, both in the penalty,
    // over 6/8 team fouls — a real state the detector must not touch.
    let (extract, report) = summary_report("end_of_fourth");
    assert_eq!(
        report.summary.ledger,
        ledger(column(6, true, 0), column(8, true, 0))
    );
    assert!(!report.quirks.0.contains(&Quirk::LedgerGlitch));
    round_trip(&extract.as_game(Some(&report.summary)));
}

#[test]
fn overtime_resets_the_ledger_to_two_timeouts_and_no_fouls() {
    let (_, summary) = pair("overtime");
    assert_eq!(
        summary.unwrap().ledger,
        ledger(column(0, false, 2), column(0, false, 2))
    );
}

#[test]
fn a_missing_ledger_field_on_either_side_drops_the_group_quietly() {
    let (extract, _) = pair("bonus");
    for strip in [
        |c: &mut serde_json::Value| {
            c.as_object_mut().unwrap().remove("timeoutsRemaining");
        },
        |c: &mut serde_json::Value| {
            c["fouls"].as_object_mut().unwrap().remove("bonusState");
        },
        |c: &mut serde_json::Value| {
            c["fouls"]["teamFoulsCurrent"] = serde_json::json!("four");
        },
        |c: &mut serde_json::Value| {
            c["fouls"].as_object_mut().unwrap().remove("foulsToGive");
        },
    ] {
        let text = mutate(&summary_text("bonus").unwrap(), |v| {
            strip(&mut v["header"]["competitions"][0]["competitors"][1]);
        });
        let report = summarize(&text, extract.summary_context());
        assert_eq!(report.summary.ledger, None);
        assert_eq!(report.quirks.0, vec![], "absence is not a glitch");
        // Everything else stands.
        assert_eq!(report.summary.makes.len(), 83);
    }
}

// ---------------------------------------------------------- §3.4 makes

#[test]
fn a_make_is_a_field_goal_with_a_team_and_a_coordinate() {
    // bonus_both's newest scoring play is a free throw: the newest score,
    // not a make — the last mark is Towns' dunk at (26, 1) → (1, 1).
    let (_, summary) = pair("bonus_both");
    let summary = summary.unwrap();
    assert_eq!(summary.makes.len(), 16);
    assert_eq!(
        summary.makes.last(),
        Some(&Make {
            side: Side::Away,
            three: false,
            x: 1,
            y: 1
        })
    );
    assert!(!summary.newest_score.as_ref().unwrap().marked);

    // A placeholder-typed make (type 91 "No Shot") is still a make: the
    // filter is scoringPlay and pointsAttempted, never the type. Minix's
    // corner three for CLE (home) at (48, −1) → (−23, −1).
    let (_, summary) = pair("in_progress");
    let summary = summary.unwrap();
    assert_eq!(
        summary.makes.last(),
        Some(&Make {
            side: Side::Home,
            three: true,
            x: -23,
            y: -1
        })
    );
    assert!(summary.newest_score.as_ref().unwrap().marked);
}

/// The newest make of a summary, by document order.
fn newest_make(v: &mut serde_json::Value) -> &mut serde_json::Value {
    v["plays"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .rev()
        .find(|p| p["scoringPlay"] == true && p["pointsAttempted"].as_u64().unwrap_or(0) >= 2)
        .unwrap()
}

#[test]
fn a_field_goal_without_a_team_or_with_the_sentinel_gets_no_mark_with_a_quirk() {
    let (extract, _) = pair("in_progress");
    let sentinel = |v: &mut serde_json::Value| {
        newest_make(v)["coordinate"] = serde_json::json!({"x": -214748340, "y": -214748365});
    };
    let text = mutate(&summary_text("in_progress").unwrap(), sentinel);
    let report = summarize(&text, extract.summary_context());
    assert_eq!(report.summary.makes.len(), 18, "one mark fewer");
    assert_eq!(report.quirks.0, vec![Quirk::UnplacedMake]);
    // The score still lands: the newest scoring play stands, unmarked.
    let score = report.summary.newest_score.unwrap();
    assert_eq!((score.seq, score.marked), (180, false));

    let text = mutate(&summary_text("in_progress").unwrap(), |v| {
        newest_make(v).as_object_mut().unwrap().remove("team");
    });
    let report = summarize(&text, extract.summary_context());
    assert_eq!(report.summary.makes.len(), 18);
    assert_eq!(report.quirks.0, vec![Quirk::UnplacedMake]);
    // Team-less, so not the newest score either: the scoring play with a
    // team before it is (read from the fixture, not guessed).
    let before: u32 = parse(&summary_text("in_progress").unwrap())["plays"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .skip(1)
        .find(|p| p["scoringPlay"] == true && p.get("team").is_some())
        .unwrap()["sequenceNumber"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(report.summary.newest_score.unwrap().seq, before);
    assert_ne!(before, 180);

    // A team matching neither side is the same quirk at finish.
    let text = mutate(&summary_text("in_progress").unwrap(), |v| {
        newest_make(v)["team"]["id"] = serde_json::json!("999");
    });
    let report = summarize(&text, extract.summary_context());
    assert_eq!(report.summary.makes.len(), 18);
    assert_eq!(report.quirks.0, vec![Quirk::UnplacedMake]);
    assert_eq!(report.summary.newest_score, None, "no side for the caption");
}

/// A made field goal for `team` at ESPN's `(x, y)`, appended to the plays.
fn make_play(seq: u32, team: &str, points: u8, x: i64, y: i64, text: &str) -> serde_json::Value {
    serde_json::json!({
        "id": format!("9{seq:06}"),
        "sequenceNumber": seq.to_string(),
        "type": {"id": "92", "text": "Jump Shot"},
        "text": text,
        "awayScore": 0, "homeScore": 0,
        "scoringPlay": true,
        "scoreValue": points,
        "pointsAttempted": points,
        "shootingPlay": true,
        "coordinate": {"x": x, "y": y},
        "team": {"id": team},
        "participants": [{"athlete": {"id": "1"}}]
    })
}

#[test]
fn home_x_is_negated_and_y_is_clamped_to_the_half_court() {
    // NY is away (18), CLE home (5). Appended plays keep the running score
    // at 0-0 so the walk sees a correction once and nothing else changes.
    let (extract, _) = pair("in_progress");
    let text = mutate(&summary_text("in_progress").unwrap(), |v| {
        let plays = v["plays"].as_array_mut().unwrap();
        plays.push(make_play(901, "18", 3, 48, -1, "A makes shot"));
        plays.push(make_play(902, "5", 3, 48, -1, "B makes shot"));
        plays.push(make_play(903, "18", 2, 25, 70, "C makes shot"));
        plays.push(make_play(904, "5", 2, 60, -20, "D makes shot"));
        plays.push(make_play(905, "18", 2, 0, 47, "E makes shot"));
    });
    let report = summarize(&text, extract.summary_context());
    let makes = &report.summary.makes;
    let tail = &makes[makes.len() - 5..];
    let make = |side, three, x, y| Make { side, three, x, y };
    assert_eq!(tail[0], make(Side::Away, true, 23, -1), "away: x − 25");
    assert_eq!(tail[1], make(Side::Home, true, -23, -1), "home: negated");
    assert_eq!(
        tail[2],
        make(Side::Away, false, 0, 47),
        "a heave sits on the line"
    );
    assert_eq!(
        tail[3],
        make(Side::Home, false, -25, -8),
        "pulled back to the lines"
    );
    assert_eq!(tail[4], make(Side::Away, false, -25, 47), "the sideline");
    let game = extract.as_game(Some(&report.summary));
    assert_eq!(live(&game).makes.len(), 24);
    round_trip(&game);
}

#[test]
fn the_ring_keeps_the_newest_128_makes() {
    // bonus has 83 real makes; 60 more make 143, of which the newest 128
    // are kept: the oldest fifteen real ones drop, the newest stays.
    let (extract, reference) = pair("bonus");
    let reference = reference.unwrap();
    let text = mutate(&summary_text("bonus").unwrap(), |v| {
        let plays = v["plays"].as_array_mut().unwrap();
        for n in 0..60u32 {
            let team = if n % 2 == 0 { "7" } else { "11" };
            plays.push(make_play(
                1000 + n,
                team,
                2,
                10 + (n % 30) as i64,
                5,
                "P makes shot",
            ));
        }
    });
    let report = summarize(&text, extract.summary_context());
    let makes = &report.summary.makes;
    assert_eq!(makes.len(), 128);
    assert_eq!(
        makes[0], reference.makes[15],
        "the sixteenth real make is the oldest kept"
    );
    assert_eq!(makes[127].side, Side::Home, "n = 59 is odd: IND, home");
    assert_eq!(
        makes[127].x,
        -(10 + 29 - 25),
        "the newest synthesized: x 39, home negates"
    );
    let score = report.summary.newest_score.as_ref().unwrap();
    assert!(score.marked, "the newest make is always kept");
    let game = extract.as_game(Some(&report.summary));
    assert_eq!(live(&game).makes.len(), 128);
    round_trip(&game);
}

// ------------------------------------------------------------ §3.5 runs

#[test]
fn the_run_is_the_trailing_unanswered_points_and_survives_the_break() {
    // WSH@OKC: OKC 8-0 from 19-21 late in Q1, through the break, to 19-29.
    let (extract, summary) = pair("run");
    let summary = summary.unwrap();
    assert_eq!(
        summary.run,
        Some(Run {
            side: Side::Home,
            points: 8
        })
    );
    let game = extract.as_game(Some(&summary));
    assert_eq!(live(&game).run, summary.run);
    round_trip(&game);

    // in_progress: home 5, under RUN_SHOW — no run on the wire.
    let (_, summary) = pair("in_progress");
    assert_eq!(summary.unwrap().run, None);
}

/// A row with running scores and nothing else the walk reads.
fn score_row(seq: u32, type_id: &str, away: u32, home: u32) -> serde_json::Value {
    serde_json::json!({
        "id": format!("8{seq:06}"),
        "sequenceNumber": seq.to_string(),
        "type": {"id": type_id, "text": "row"},
        "text": "row",
        "awayScore": away, "homeScore": home,
        "scoringPlay": false, "scoreValue": 0, "pointsAttempted": 0,
        "coordinate": {"x": -214748340, "y": -214748365}
    })
}

/// Only the appended rows: `tip`'s one jumpball at 0-0 before them.
fn walk(rows: Vec<serde_json::Value>) -> SummaryReport<RecQuirks> {
    let (extract, _) = pair("tip");
    let text = mutate(&summary_text("tip").unwrap(), |v| {
        v["plays"].as_array_mut().unwrap().extend(rows);
    });
    summarize(&text, extract.summary_context())
}

#[test]
fn a_step_on_which_both_scores_rose_is_a_correction_that_ends_the_run() {
    // Home 3, 3 (6-0) … then a row where both sides rose: no run, a quirk.
    let report = walk(vec![
        score_row(10, "92", 0, 3),
        score_row(11, "92", 0, 6),
        score_row(12, "155", 2, 8),
    ]);
    assert_eq!(report.summary.run, None);
    assert_eq!(report.quirks.0, vec![Quirk::RunCorrection]);

    // …and the next step starts a fresh run from the corrected totals.
    let report = walk(vec![
        score_row(10, "92", 0, 3),
        score_row(11, "92", 0, 6),
        score_row(12, "155", 2, 8),
        score_row(13, "92", 2, 11),
        score_row(14, "92", 2, 14),
    ]);
    assert_eq!(
        report.summary.run,
        Some(Run {
            side: Side::Home,
            points: 6
        })
    );

    // A side that fell is a correction too (ESPN's stale rows, ORL@GS).
    let report = walk(vec![
        score_row(10, "92", 0, 3),
        score_row(11, "92", 0, 6),
        score_row(12, "155", 0, 4),
    ]);
    assert_eq!(report.summary.run, None);
    assert_eq!(report.quirks.0, vec![Quirk::RunCorrection]);
}

#[test]
fn a_run_continues_across_a_period_break_and_ends_when_the_other_side_scores() {
    // Away 2 + 3 + 2 across an End Period row (412): 7-0 away.
    let report = walk(vec![
        score_row(10, "92", 2, 0),
        score_row(11, "92", 5, 0),
        score_row(12, "412", 5, 0),
        score_row(13, "92", 7, 0),
    ]);
    assert_eq!(
        report.summary.run,
        Some(Run {
            side: Side::Away,
            points: 7
        })
    );
    // Home answers: a new run of 3 for home, under the threshold.
    let report = walk(vec![
        score_row(10, "92", 2, 0),
        score_row(11, "92", 5, 0),
        score_row(12, "92", 7, 0),
        score_row(13, "92", 7, 3),
    ]);
    assert_eq!(report.summary.run, None);
    assert_eq!(report.quirks.0, vec![]);
}

// ------------------------------------------------- §3.6 the newest score

/// The scorer the caption would show for one scoring play with `text`.
fn scorer_of(text: &str) -> String {
    let report = walk(vec![serde_json::json!({
        "id": "8000001",
        "sequenceNumber": "500",
        "type": {"id": "92", "text": "Jump Shot"},
        "text": text,
        "awayScore": 2, "homeScore": 0,
        "scoringPlay": true, "scoreValue": 2, "pointsAttempted": 2,
        "coordinate": {"x": 25, "y": 5},
        "team": {"id": "18"},
        "participants": [{"athlete": {"id": "1"}}]
    })]);
    let score = report
        .summary
        .newest_score
        .expect("a scoring play with a team");
    assert_eq!(score.seq, 500);
    score.scorer.as_str().to_string()
}

#[test]
fn the_family_name_is_the_last_token_minus_a_generational_suffix() {
    assert_eq!(scorer_of("Kelly Oubre Jr. makes 2-foot shot"), "OUBRE");
    assert_eq!(
        scorer_of("Michael Porter Jr. makes 26-foot three pointer"),
        "PORTER"
    );
    assert_eq!(scorer_of("Robert Williams III makes dunk"), "WILLIAMS");
    assert_eq!(scorer_of("Dereck Lively II makes layup"), "LIVELY");
    assert_eq!(
        scorer_of("Shai Gilgeous-Alexander makes jumper"),
        "GILGEOUS-ALEXANDER"
    );
    assert_eq!(
        scorer_of("Karl-Anthony Towns makes free throw 1 of 1"),
        "TOWNS"
    );
    assert_eq!(scorer_of("Luc Mbah a Moute makes layup"), "MOUTE");
    assert_eq!(scorer_of("Nene makes layup"), "NENE");
    // No name before " makes " (a placeholder), or no " makes " at all (a
    // goaltending award): no scorer, the score still lands.
    assert_eq!(scorer_of("makes 2-foot shot"), "");
    assert_eq!(scorer_of("Defensive goaltending violation"), "");
    assert_eq!(scorer_of("Jr. makes shot"), "JR.");
}

#[test]
fn the_scorers_points_come_from_the_boxscore_by_the_pts_column() {
    // McCain's PTS is 4 in the run fixture's boxscore.
    let (extract, summary) = pair("run");
    let summary = summary.unwrap();
    assert_eq!(
        newest(live(&extract.as_game(Some(&summary)))),
        Some((Side::Home, 193, "MCCAIN", 4, true))
    );

    // Not listed yet: 0, the caption shows the name alone.
    let text = mutate(&summary_text("run").unwrap(), |v| {
        for block in v["boxscore"]["players"].as_array_mut().unwrap() {
            block["statistics"][0]["athletes"]
                .as_array_mut()
                .unwrap()
                .retain(|a| a["athlete"]["id"] != "4683778");
        }
    });
    let summary = summarize(&text, extract.summary_context()).summary;
    assert_eq!(
        newest(live(&extract.as_game(Some(&summary)))),
        Some((Side::Home, 193, "MCCAIN", 0, true))
    );

    // The column is found by name, not position: PTS moved to the end.
    let text = mutate(&summary_text("run").unwrap(), |v| {
        for block in v["boxscore"]["players"].as_array_mut().unwrap() {
            let stats = &mut block["statistics"][0];
            let names = stats["names"].as_array_mut().unwrap();
            let pts = names.remove(1);
            names.push(pts);
            stats.as_object_mut().unwrap().remove("keys");
            for athlete in stats["athletes"].as_array_mut().unwrap() {
                if let Some(row) = athlete["stats"].as_array_mut() {
                    if row.len() > 1 {
                        let pts = row.remove(1);
                        row.push(pts);
                    }
                }
            }
        }
    });
    let summary = summarize(&text, extract.summary_context()).summary;
    assert_eq!(summary.newest_score.as_ref().unwrap().scorer_points, 4);

    // `keys` stands in when `names` is gone; neither means unknown.
    let text = mutate(&summary_text("run").unwrap(), |v| {
        for block in v["boxscore"]["players"].as_array_mut().unwrap() {
            block["statistics"][0]
                .as_object_mut()
                .unwrap()
                .remove("names");
        }
    });
    let summary = summarize(&text, extract.summary_context()).summary;
    assert_eq!(summary.newest_score.as_ref().unwrap().scorer_points, 4);
    let text = mutate(&summary_text("run").unwrap(), |v| {
        for block in v["boxscore"]["players"].as_array_mut().unwrap() {
            let stats = block["statistics"][0].as_object_mut().unwrap();
            stats.remove("names");
            stats.remove("keys");
        }
    });
    let summary = summarize(&text, extract.summary_context()).summary;
    assert_eq!(summary.newest_score.as_ref().unwrap().scorer_points, 0);
}

#[test]
fn a_nineteenth_boxscore_row_is_a_bounded_overflow_and_unlisted() {
    // Pad OKC's block to eighteen rows ahead of McCain, then a nineteenth:
    // he is the overflow and his points are unknown.
    let (extract, _) = pair("run");
    let text = mutate(&summary_text("run").unwrap(), |v| {
        for block in v["boxscore"]["players"].as_array_mut().unwrap() {
            if block["team"]["id"] != "25" {
                continue;
            }
            let athletes = block["statistics"][0]["athletes"].as_array_mut().unwrap();
            let mccain = athletes
                .iter()
                .position(|a| a["athlete"]["id"] == "4683778")
                .unwrap();
            let row = athletes.remove(mccain);
            let filler = athletes[0].clone();
            while athletes.len() < 18 {
                athletes.push(filler.clone());
            }
            athletes.push(row);
        }
    });
    let report = summarize(&text, extract.summary_context());
    assert_eq!(report.quirks.0, vec![Quirk::BoundedOverflow]);
    assert_eq!(
        report.summary.newest_score.as_ref().unwrap().scorer_points,
        0
    );
}

#[test]
fn marked_follows_the_newest_scoring_play_being_a_make() {
    let (_, summary) = pair("run");
    assert!(summary.unwrap().newest_score.unwrap().marked, "a layup");
    let (_, summary) = pair("in_progress_subminute");
    assert!(
        !summary.unwrap().newest_score.unwrap().marked,
        "a free throw"
    );
}

// ----------------------------------------------------------- §3.7 flash

#[test]
fn the_flash_prefers_the_scoreboards_eligible_play_then_the_summarys() {
    // The scoreboard's lastPlay is a full timeout: it flashes with or
    // without the summary, and the summary names the same play.
    let (extract, summary) = pair("timeout");
    let cavs = Some(("40190994067", "Cavaliers Full timeout"));
    assert_eq!(last_play(live(&extract.as_game(None))), cavs);
    assert_eq!(last_play(live(&extract.as_game(summary.as_ref()))), cavs);
    assert_eq!(
        summary
            .unwrap()
            .last_play
            .map(|p| p.id.as_str().to_string()),
        Some("40190994067".to_string())
    );

    // The scoreboard's lastPlay is a rebound: nothing without a summary,
    // the summary's newest eligible play (an earlier timeout) with one.
    let (extract, summary) = pair("run");
    assert_eq!(last_play(live(&extract.as_game(None))), None);
    assert_eq!(
        last_play(live(&extract.as_game(summary.as_ref()))),
        Some(("401909941136", "Thunder Full timeout"))
    );

    // Strip the timeouts and the Wizards' 216 challenge outcome before them
    // is the newest eligible play; strip every eligible type and a summary
    // with none flashes nothing.
    let text = mutate(&summary_text("run").unwrap(), |v| {
        v["plays"]
            .as_array_mut()
            .unwrap()
            .retain(|p| p["type"]["id"] != "16");
    });
    let summary = summarize(&text, extract.summary_context()).summary;
    assert_eq!(
        last_play(live(&extract.as_game(Some(&summary)))),
        Some((
            "40190994121",
            "(10:59) [Wizards] COACH'S CHALLENGE (CALL STANDS) [Wizards] charged with a timeout"
        ))
    );
    let text = mutate(&summary_text("run").unwrap(), |v| {
        v["plays"].as_array_mut().unwrap().retain(|p| {
            let type_id: u16 = p["type"]["id"].as_str().unwrap().parse().unwrap();
            !basketball::FLASH_TYPES.contains(&type_id)
        });
    });
    let summary = summarize(&text, extract.summary_context()).summary;
    assert_eq!(live(&extract.as_game(Some(&summary))).last_play, None);
}

#[test]
fn made_baskets_placeholders_and_transient_challenges_never_flash() {
    // The scoreboard's lastPlay is a make (type 113), a placeholder (91), a
    // free throw (97), a jumpball (615): none flash.
    for name in ["in_progress", "placeholder_play", "bonus_both", "tip"] {
        let (extract, _) = pair(name);
        assert_eq!(live(&extract.as_game(None)).last_play, None, "{name}");
    }

    // In the summary: a 213 "Challenge" first posting after the timeout
    // does not replace it; re-typed to 215 in place, it does — same id.
    let (extract, _) = pair("run");
    let challenge = |type_id: &str| {
        serde_json::json!({
            "id": "401909941999",
            "sequenceNumber": "999",
            "type": {"id": type_id, "text": "Challenge"},
            "text": "(05:00) [Thunder] COACH'S CHALLENGE (CALL OVERTURNED)",
            "awayScore": 19, "homeScore": 29,
            "scoringPlay": false, "scoreValue": 0, "pointsAttempted": 0,
            "coordinate": {"x": -214748340, "y": -214748365}
        })
    };
    for (type_id, expected) in [
        ("213", ("401909941136", "Thunder Full timeout")),
        ("277", ("401909941136", "Thunder Full timeout")),
        ("91", ("401909941136", "Thunder Full timeout")),
        ("61", ("401909941136", "Thunder Full timeout")),
        (
            "215",
            (
                "401909941999",
                "(05:00) [Thunder] COACH'S CHALLENGE (CALL OVERTURNED)",
            ),
        ),
        (
            "279",
            (
                "401909941999",
                "(05:00) [Thunder] COACH'S CHALLENGE (CALL OVERTURNED)",
            ),
        ),
        (
            "35",
            (
                "401909941999",
                "(05:00) [Thunder] COACH'S CHALLENGE (CALL OVERTURNED)",
            ),
        ),
    ] {
        let text = mutate(&summary_text("run").unwrap(), |v| {
            v["plays"].as_array_mut().unwrap().push(challenge(type_id));
        });
        let summary = summarize(&text, extract.summary_context()).summary;
        assert_eq!(
            last_play(live(&extract.as_game(Some(&summary)))),
            Some(expected),
            "type {type_id}"
        );
    }

    // The scoreboard's own lastPlay re-typed to an outcome flashes from
    // there; the defensive three-seconds technical's trailing newline is
    // trimmed.
    let event = fixture("run");
    let body = wrap(&[&mutate(&event, |v| {
        let play = &mut v["competitions"][0]["situation"]["lastPlay"];
        play["type"]["id"] = serde_json::json!("29");
        play["text"] = serde_json::json!("Danny Wolf defensive 3-seconds (technical foul)\n");
    })]);
    let extract = found(&body, &id_of(&event));
    assert_eq!(
        last_play(live(&extract.as_game(None))),
        Some((
            "401909941196",
            "Danny Wolf defensive 3-seconds (technical foul)"
        ))
    );
    assert!(
        matches!(&extract.kind, Kind::Live(game) if game.last_play.as_ref().unwrap().text.ends_with('\n')),
        "the unfiltered play keeps ESPN's text"
    );
}

// ------------------------------------------------------- §3.8 pregame line

#[test]
fn the_pregame_second_line_is_the_headline_plus_the_series_in_the_playoffs() {
    let event = fixture("pregame");
    let target = id_of(&event);
    // A Cup night: the note alone, upper-cased.
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["notes"] =
            serde_json::json!([{"type": "event", "headline": "NBA Cup - Group Play"}]);
    })]);
    let extract = found(&body, &target);
    let game = extract.as_game(None);
    assert_eq!(pregame(&game).info_secondary, "NBA CUP - GROUP PLAY");
    round_trip(&game);

    // The Finals: the game number, then the series.
    let playoff = |v: &mut serde_json::Value| {
        v["season"]["type"] = serde_json::json!(3);
        v["competitions"][0]["notes"] =
            serde_json::json!([{"type": "event", "headline": "NBA Finals - Game 4"}]);
        v["competitions"][0]["series"] = serde_json::json!({"summary": "NY leads series 2-1"});
    };
    let extract = found(&wrap(&[&mutate(&event, playoff)]), &target);
    let game = extract.as_game(None);
    assert!(extract.playoffs);
    assert_eq!(
        pregame(&game).info_secondary,
        "NBA FINALS - GAME 4 - NY LEADS SERIES 2-1"
    );
    round_trip(&game);

    // No headline: nothing, series or not. The play-in (5) is not the
    // playoffs, so its series (if ESPN ever sent one) stays off the line.
    let body = wrap(&[&mutate(&event, |v| {
        playoff(v);
        v["competitions"][0]["notes"] = serde_json::json!([]);
    })]);
    assert_eq!(
        pregame(&found(&body, &target).as_game(None)).info_secondary,
        ""
    );
    let body = wrap(&[&mutate(&event, |v| {
        playoff(v);
        v["season"]["type"] = serde_json::json!(5);
        v["competitions"][0]["notes"][0]["headline"] =
            serde_json::json!("NBA Play-In - East - 8th Seed Game");
    })]);
    let extract = found(&body, &target);
    assert!(!extract.playoffs);
    assert_eq!(
        pregame(&extract.as_game(None)).info_secondary,
        "NBA PLAY-IN - EAST - 8TH SEED GAME"
    );
}

// ------------------------------------------------------------ §3.9 final

/// One leader row of a final, owned, so a mutated body can be judged inline.
fn leader_row(extract: &Extract, row: usize) -> Option<(Side, u8, String)> {
    let game = extract.as_game(None);
    leader_rows(final_(&game)).map(|rows| {
        let (side, value, name) = rows[row];
        (side, value, name.to_string())
    })
}

#[test]
fn leaders_take_the_higher_value_of_both_competitors_and_ties_go_away() {
    // Pinned per fixture by the corpus sweep; here the edges. final's
    // assists are 8 apiece (Jokic / Nembhard): away.
    let (extract, _) = pair("final");
    assert_eq!(
        leader_row(&extract, 2),
        Some((Side::Away, 8, "N. Jokic".to_string()))
    );

    let event = fixture("final");
    let target = id_of(&event);
    // Fixture order is [home, away]. Home's assists leader one better: home.
    let body = wrap(&[&mutate(&event, |v| {
        let leaders = v["competitions"][0]["competitors"][0]["leaders"]
            .as_array_mut()
            .unwrap();
        let assists = leaders.iter_mut().find(|l| l["name"] == "assists").unwrap();
        assists["leaders"][0]["value"] = serde_json::json!(9.0);
        assists["leaders"][0]["displayValue"] = serde_json::json!("9");
    })]);
    assert_eq!(
        leader_row(&found(&body, &target), 2),
        Some((Side::Home, 9, "A. Nembhard".to_string()))
    );

    // A category on one side only: that side's. On neither: no leaders at
    // all, whatever the other two rows have.
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["competitors"][0]["leaders"]
            .as_array_mut()
            .unwrap()
            .retain(|l| l["name"] != "rebounds");
    })]);
    assert_eq!(
        leader_row(&found(&body, &target), 1),
        Some((Side::Away, 13, "N. Jokic".to_string()))
    );
    let body = wrap(&[&mutate(&event, |v| {
        for competitor in v["competitions"][0]["competitors"].as_array_mut().unwrap() {
            competitor["leaders"]
                .as_array_mut()
                .unwrap()
                .retain(|l| l["name"] != "rebounds");
        }
    })]);
    let extract = found(&body, &target);
    let game = extract.as_game(None);
    assert_eq!(final_(&game).leaders, None);
    round_trip(&game);

    // `displayValue` stands in for a missing `value`; the `rating` category
    // and the pregame's per-game averages are never rows.
    let body = wrap(&[&mutate(&event, |v| {
        let leaders = v["competitions"][0]["competitors"][0]["leaders"]
            .as_array_mut()
            .unwrap();
        let points = leaders.iter_mut().find(|l| l["name"] == "points").unwrap();
        points["leaders"][0]
            .as_object_mut()
            .unwrap()
            .remove("value");
        points["leaders"][0]["displayValue"] = serde_json::json!("30");
    })]);
    assert_eq!(
        leader_row(&found(&body, &target), 0),
        Some((Side::Home, 30, "P. Siakam".to_string()))
    );
}

#[test]
fn final_records_are_both_totals_or_none_and_the_series_is_playoffs_only() {
    let (extract, _) = pair("final");
    let game = extract.as_game(None);
    assert_eq!(
        final_(&game).records.map(|r| (r.away, r.home)),
        Some((record(5, 3), record(3, 7)))
    );
    assert_eq!(final_(&game).series, None);

    // A malformed total on one side: the quirk and no records.
    let event = fixture("final");
    let target = id_of(&event);
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["competitors"][1]["records"][0]["summary"] = serde_json::json!("TBD");
    })]);
    let (extract, quirks) = detail(&body, &target);
    assert_eq!(final_(&extract.as_game(None)).records, None);
    assert_eq!(quirks.0, vec![Quirk::MalformedRecord]);

    // A regular-season event carrying a series is not served one.
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["series"] = serde_json::json!({"summary": "Series tied 1-1"});
    })]);
    assert_eq!(final_(&found(&body, &target).as_game(None)).series, None);
    let body = wrap(&[&mutate(&event, |v| {
        v["season"]["type"] = serde_json::json!(3);
        v["competitions"][0]["series"] = serde_json::json!({"summary": "Series tied 1-1"});
    })]);
    let extract = found(&body, &target);
    let game = extract.as_game(None);
    assert_eq!(final_(&game).series, Some("Series tied 1-1"));
    round_trip(&game);
}

// ---------------------------------------------------------- the context

#[test]
fn a_final_or_pregame_context_reads_nothing_from_a_summary() {
    // The final's summary carries 463 plays and 85 makes; the context says
    // final, so none of it is derived (D3: finals come from the scoreboard).
    let (_, report) = summary_report("final");
    assert_eq!(report.summary.makes.len(), 0);
    assert_eq!(report.summary.newest_score, None);
    assert_eq!(report.summary.last_play, None);
    assert_eq!(report.quirks.0, vec![]);

    // The same body under a live context derives everything.
    let (live_extract, _) = pair("bonus");
    let summary = summarize(
        &summary_text("final").unwrap(),
        live_extract.summary_context(),
    )
    .summary;
    assert_eq!(summary.makes.len(), 85);
    assert_eq!(summary.ledger, None, "the final header has no ledger keys");
}
