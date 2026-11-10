//! College basketball lane: the B0 corpus
//! (`backend/testdata/basketball/mens-college-basketball/`, PROVENANCE.md)
//! under [`Rules::MENS_COLLEGE`], and one named test per `firmware-rs/
//! NCAAB.md` §3 rule the extractor owns.
//!
//! Fixtures are single EVENT objects, wrapped here as `{"events":[…]}`
//! scoreboard bodies, each live or final one paired with the raw summary
//! body of the same moment. The expected values are PROVENANCE's moments
//! plus an independent reference walk of §3 over the pairs, which agreed
//! with the lane on every live pair; every view is encoded at every
//! extension level and decoded back through `scoreboard_wire::basketball`.

use std::fs;
use std::path::PathBuf;

use scoreboard_espn::basketball::{
    self, DetailOutcome, Extract, Extractor, Rules, Summary, SummaryContext, SummaryExtractor,
    SummaryReport,
};
use scoreboard_espn::common::{Quirk, Quirks};
use scoreboard_espn::StreamMatcher;
use scoreboard_wire::basketball::{self as wire, Game, Make, Run};
use scoreboard_wire::{GameState, LivePhase, Record, Side, SliceSink, EXT_LEVEL_MAX};

/// Every committed fixture, in `PROVENANCE.md` order.
const FIXTURES: &[&str] = &[
    "tip",
    "in_progress",
    "run",
    "halftime",
    "end_of_regulation",
    "team_timeout",
    "tv_timeout",
    "placeholder_play",
    "final",
    "final_ot",
    "final_tournament",
    "pregame_ranked",
    "pregame_colourless",
];

const COLLEGE: Rules = Rules::MENS_COLLEGE;

// ---------------------------------------------------------------- helpers

fn testdata(rel: &str) -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../backend/testdata/basketball/mens-college-basketball"
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

fn wrap(event: &str) -> String {
    format!("{{\"events\":[{event}]}}")
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

fn extract_sized(
    body: &str,
    target: &str,
    rules: Rules,
    sizes: &[usize],
) -> (DetailOutcome, RecQuirks) {
    let mut quirks = RecQuirks::default();
    let outcome = {
        let extractor = Extractor::game_detail(target, rules, &mut quirks);
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

fn detail(body: &str, target: &str, rules: Rules) -> (Extract, RecQuirks) {
    match extract_sized(body, target, rules, &[usize::MAX]) {
        (DetailOutcome::Found(extract), quirks) => (extract, quirks),
        (other, _) => panic!("expected Found for {target}, got {other:?}"),
    }
}

/// An event's scoreboard extract under `rules`.
fn extract_event(event: &str, rules: Rules) -> Extract {
    detail(&wrap(event), &id_of(event), rules).0
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

/// A fixture's scoreboard extract and summary under the college rules.
fn pair(name: &str) -> (Extract, Option<Summary>) {
    let extract = extract_event(&fixture(name), COLLEGE);
    let summary =
        summary_text(name).map(|text| summarize(&text, extract.summary_context()).summary);
    (extract, summary)
}

fn encode_at(game: &Game<'_>, level: u8) -> Vec<u8> {
    let mut buf = [0u8; 4096];
    let mut sink = SliceSink::new(&mut buf);
    wire::encode_at(game, level, &mut sink).expect("encode fits");
    sink.written().to_vec()
}

/// Every level decodes and re-encodes to its own bytes, and the top level
/// decodes to the view itself.
fn round_trip(name: &str, game: &Game<'_>) {
    for level in 0..=EXT_LEVEL_MAX {
        let bytes = encode_at(game, level);
        let decoded = wire::decode(&bytes).unwrap_or_else(|e| panic!("{name}@{level}: {e}"));
        assert_eq!(encode_at(&decoded, level), bytes, "{name}@{level}");
        if level == EXT_LEVEL_MAX {
            assert_eq!(&decoded, game, "{name}: the view survives the wire");
        }
    }
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

fn last_play<'a>(game: &'a wire::Live<'a>) -> Option<(&'a str, &'a str)> {
    game.last_play.map(|play| (play.id, play.text))
}

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

fn record(wins: u16, losses: u16) -> Record {
    Record { wins, losses }
}

// ------------------------------------------------------------- the corpus

/// One live pair's moment: the scoreboard's period, phase, clock and scores,
/// then what the summary adds — the flash, the run, the newest score
/// `(side, seq, scorer, points, marked)` and the number of marks.
struct LiveExpect {
    period: u8,
    phase: LivePhase,
    clock: &'static str,
    scores: (u16, u16),
    flash: Option<(&'static str, &'static str)>,
    run: Option<Run>,
    newest: Option<(Side, u32, &'static str, u8, bool)>,
    makes: usize,
}

fn live_expect(name: &str) -> LiveExpect {
    use LivePhase::{EndOfPeriod, Halftime, InProgress};
    let home_run = |points| {
        Some(Run {
            side: Side::Home,
            points,
        })
    };
    match name {
        "tip" => LiveExpect {
            period: 1,
            phase: InProgress,
            clock: "19:58",
            scores: (0, 0),
            flash: None,
            run: None,
            newest: None,
            makes: 0,
        },
        "in_progress" => LiveExpect {
            period: 2,
            phase: InProgress,
            clock: "11:18",
            scores: (39, 77),
            flash: Some(("401906865121019166", "Grambling Timeout")),
            run: None,
            newest: Some((Side::Home, 121023345, "SPEARS", 11, true)),
            makes: 41,
        },
        "run" => LiveExpect {
            period: 2,
            phase: InProgress,
            clock: "6:32",
            scores: (43, 78),
            flash: Some(("401921720121026042", "North Florida Timeout")),
            run: home_run(8),
            newest: Some((Side::Home, 121025974, "ALLEN", 8, true)),
            makes: 50,
        },
        "halftime" => LiveExpect {
            period: 1,
            phase: Halftime,
            clock: "0:00",
            scores: (24, 33),
            flash: Some(("401906849121013444", "Tennessee Timeout")),
            run: None,
            newest: Some((Side::Home, 121012983, "HARALSON", 4, true)),
            makes: 20,
        },
        "end_of_regulation" => LiveExpect {
            period: 2,
            phase: EndOfPeriod,
            clock: "0:00",
            scores: (62, 93),
            flash: Some(("401906865121029037", "Grambling Timeout")),
            run: None,
            newest: Some((Side::Home, 121032181, "CLARK", 3, false)),
            makes: 53,
        },
        "team_timeout" => LiveExpect {
            period: 1,
            phase: InProgress,
            clock: "7:58",
            scores: (12, 16),
            flash: Some(("401906849121008056", "Radford Timeout")),
            run: home_run(6),
            newest: Some((Side::Home, 121008030, "HENDERSON", 2, true)),
            makes: 10,
        },
        "tv_timeout" => LiveExpect {
            period: 2,
            phase: InProgress,
            clock: "15:29",
            scores: (27, 38),
            flash: Some(("401906849121013444", "Tennessee Timeout")),
            run: None,
            newest: Some((Side::Away, 121018845, "KEEL", 9, true)),
            makes: 23,
        },
        "placeholder_play" => LiveExpect {
            period: 2,
            phase: InProgress,
            clock: "19:14",
            scores: (29, 55),
            flash: Some(("401906865121005705", "Grambling Timeout")),
            run: None,
            newest: Some((Side::Away, 121014350, "SHANNON", 6, true)),
            makes: 27,
        },
        other => panic!("{other} is not a live fixture"),
    }
}

/// A final's periods, scores `(away, home)`, line scores and records.
struct FinalExpect {
    periods: u8,
    scores: (u16, u16),
    lines: [&'static [u8]; 2],
    records: [Record; 2],
}

fn final_expect(name: &str) -> FinalExpect {
    match name {
        "final" => FinalExpect {
            periods: 2,
            scores: (47, 70),
            lines: [&[24, 23], &[33, 37]],
            records: [record(0, 3), record(2, 0)],
        },
        "final_ot" => FinalExpect {
            periods: 3,
            scores: (90, 93),
            lines: [&[36, 43, 11], &[37, 42, 14]],
            records: [record(15, 20), record(25, 8)],
        },
        "final_tournament" => FinalExpect {
            periods: 2,
            scores: (63, 69),
            lines: [&[29, 34], &[33, 36]],
            records: [record(34, 6), record(37, 3)],
        },
        other => panic!("{other} is not a final fixture"),
    }
}

fn check_fixture(name: &str, extract: &Extract, summary: Option<&Summary>) {
    let without = extract.as_game(None);
    let with = extract.as_game(summary);
    match extract.state() {
        GameState::Live => {
            let expect = live_expect(name);
            let game = live(&with);
            assert_eq!(game.period, expect.period, "{name}: period");
            assert_eq!(game.phase, expect.phase, "{name}: phase");
            assert_eq!(game.clock, expect.clock, "{name}: clock");
            assert_eq!((game.away.score, game.home.score), expect.scores, "{name}");
            assert_eq!(last_play(game), expect.flash, "{name}: flash");
            assert_eq!(game.run, expect.run, "{name}: run");
            assert_eq!(newest(game), expect.newest, "{name}: newest score");
            assert_eq!(game.makes.len(), expect.makes, "{name}: makes");
            // §3.3: never a ledger for college, summary or not.
            assert_eq!(game.ledger, None, "{name}: ledger");
            assert!(summary.is_some(), "{name}: a live fixture is a pair");
        }
        GameState::Final => {
            let expect = final_expect(name);
            let game = final_(&with);
            assert_eq!(with, without, "{name}: a final reads no summary");
            assert_eq!(game.periods_played, expect.periods, "{name}");
            assert_eq!((game.away.score, game.home.score), expect.scores, "{name}");
            assert_eq!(
                [game.away.line_score, game.home.line_score],
                expect.lines,
                "{name}"
            );
            let both = game.records.expect("both totals");
            assert_eq!([both.away, both.home], expect.records, "{name}");
            assert!(game.leaders.is_some(), "{name}: the leaders ride the final");
            assert_eq!(game.series, None, "{name}: college has no series");
        }
        GameState::Pregame => {
            assert!(summary.is_none(), "{name}: a pregame has no summary");
            assert_eq!(pregame(&with).info_secondary, "", "{name}: no note");
        }
    }
}

#[test]
fn every_college_pair_extracts_and_round_trips_at_every_level() {
    for name in FIXTURES {
        let (extract, summary) = pair(name);
        round_trip(name, &extract.as_game(None));
        if let Some(summary) = &summary {
            round_trip(name, &extract.as_game(Some(summary)));
        }
        check_fixture(name, &extract, summary.as_ref());
    }
}

/// The scoreboard extracts quietly; the summaries do too but for one shape:
/// ESPN posts the rows between a foul and its free throws (here a TV
/// timeout, then two substitutions) with the score after both throws, so
/// the first free throw reads as the running score falling by one — §3.5's
/// correction, twice in Texas–Grambling's second half.
#[test]
fn the_college_pairs_raise_only_the_quirks_the_data_predicts() {
    for name in FIXTURES {
        let event = fixture(name);
        let (extract, quirks) = detail(&wrap(&event), &id_of(&event), COLLEGE);
        assert_eq!(quirks.0, vec![], "{name}: scoreboard quirks");
        if let Some(text) = summary_text(name) {
            let report = summarize(&text, extract.summary_context());
            let expected = match *name {
                "end_of_regulation" => vec![Quirk::RunCorrection, Quirk::RunCorrection],
                _ => vec![],
            };
            assert_eq!(report.quirks.0, expected, "{name}: summary quirks");
        }
    }
}

#[test]
fn chunk_split_invariance_over_every_college_pair() {
    let seeded: Vec<usize> = (0..64)
        .map(|i| 1 + (17usize.wrapping_mul(31).wrapping_add(i * 7)) % 97)
        .collect();
    for name in FIXTURES {
        let event = fixture(name);
        let target = id_of(&event);
        let body = wrap(&event);
        let reference = detail(&body, &target, COLLEGE).0;
        for sizes in [&[1][..], &[7], &seeded] {
            let (outcome, _) = extract_sized(&body, &target, COLLEGE, sizes);
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
        // Byte at a time over the tip and a 385 KB live body; the other
        // sizes over every pair.
        let one_byte = matches!(*name, "tip" | "run");
        for sizes in [&[7][..], &seeded]
            .into_iter()
            .chain(one_byte.then_some(&[1][..]))
        {
            let split = summarize_sized(&text, context.clone(), sizes);
            assert_eq!(split.summary, whole.summary, "{name}: summary at {sizes:?}");
            assert_eq!(
                split.quirks.0, whole.quirks.0,
                "{name}: quirks at {sizes:?}"
            );
        }
    }
}

/// Re-serialize with every object's keys in sorted or reverse-sorted order —
/// which moves `header` behind `plays`, a competitor's `curatedRank` ahead
/// of its `team`, a block's `names` behind its `athletes`, and so on.
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
fn field_order_independence_over_every_college_pair() {
    for name in FIXTURES {
        let event = fixture(name);
        let reference = extract_event(&event, COLLEGE);
        let summary = summary_text(name);
        let reference_summary = summary
            .as_ref()
            .map(|text| summarize(text, reference.summary_context()));
        for reverse in [false, true] {
            let mut shuffled = String::new();
            reordered(&parse(&event), reverse, &mut shuffled);
            let extract = extract_event(&shuffled, COLLEGE);
            assert_eq!(extract, reference, "{name}: scoreboard (reverse={reverse})");

            let (Some(text), Some(reference_summary)) = (&summary, &reference_summary) else {
                continue;
            };
            let mut shuffled = String::new();
            reordered(&parse(text), reverse, &mut shuffled);
            let report = summarize(&shuffled, extract.summary_context());
            assert_eq!(
                report.summary, reference_summary.summary,
                "{name}: summary (reverse={reverse})"
            );
            assert_eq!(
                report.quirks.0, reference_summary.quirks.0,
                "{name}: quirks"
            );
        }
    }
}

// --------------------------------------------------------- the §3 rules

/// §3.3: ESPN's college header carries the ledger's keys with every value
/// zero, all game long. The college rules never read them, so there is no
/// ledger and — zeros being the league's normal — no glitch either; the
/// NBA's rules would read the same body as the one-poll glitch.
#[test]
fn college_reads_no_ledger_and_raises_no_quirk_on_zero_headers() {
    for name in FIXTURES {
        let Some(text) = summary_text(name) else {
            continue;
        };
        let extract = extract_event(&fixture(name), COLLEGE);
        if extract.state() != GameState::Live {
            continue;
        }
        let header = parse(&text);
        for competitor in header["header"]["competitions"][0]["competitors"]
            .as_array()
            .unwrap()
        {
            assert_eq!(competitor["fouls"]["teamFoulsCurrent"], 0, "{name}");
            assert_eq!(competitor["fouls"]["foulsToGive"], 0, "{name}");
            assert_eq!(competitor["fouls"]["bonusState"], "NONE", "{name}");
            assert_eq!(competitor["timeoutsRemaining"], 0, "{name}");
        }

        let report = summarize(&text, extract.summary_context());
        assert_eq!(report.summary.ledger, None, "{name}");
        assert!(!report.quirks.0.contains(&Quirk::LedgerGlitch), "{name}");

        let as_nba = extract_event(&fixture(name), Rules::NBA);
        let report = summarize(&text, as_nba.summary_context());
        assert_eq!(report.summary.ledger, None, "{name} as the NBA");
        assert!(
            report.quirks.0.contains(&Quirk::LedgerGlitch),
            "{name} as the NBA"
        );
    }
}

/// A one-make summary: the play's team, its coordinate as ESPN writes it.
fn one_make(x: f64, y: f64) -> String {
    serde_json::json!({
        "header": {"competitions": [{"competitors": [
            {"homeAway": "home", "team": {"id": "1"}},
            {"homeAway": "away", "team": {"id": "2"}},
        ]}]},
        "plays": [{
            "id": "1", "sequenceNumber": "1", "type": {"id": "572"},
            "text": "Ada Lovelace makes layup", "scoringPlay": true,
            "scoreValue": 2, "pointsAttempted": 2,
            "coordinate": {"x": x, "y": y}, "team": {"id": "2"},
            "awayScore": 2, "homeScore": 0,
        }],
    })
    .to_string()
}

/// §3.4 / D4: the wire's y is feet from the backboard plane in every league.
/// ESPN's college rim sits at y ≈ 0.25 where the NBA's sits at 1, so the
/// college rules add one foot after rounding and before the wire's clamp —
/// on a made layup at the rim and on every make of a real game.
#[test]
fn the_college_frame_puts_the_rim_one_foot_out() {
    let college = extract_of_live(COLLEGE);
    let nba = extract_of_live(Rules::NBA);
    let make_at = |extract: &Extract, x: f64, y: f64| -> Make {
        let summary = summarize(&one_make(x, y), extract.summary_context()).summary;
        assert_eq!(summary.makes.len(), 1);
        summary.makes[0]
    };
    let rim = Make {
        side: Side::Away,
        three: false,
        x: 0,
        y: 1,
    };
    assert_eq!(make_at(&college, 25.0, 0.25), rim);
    assert_eq!(make_at(&nba, 25.0, 0.25), Make { y: 0, ..rim });
    // A heave past half court: shifted, then clamped to the wire's 47.
    assert_eq!(make_at(&college, 25.0, 46.6).y, 47);
    assert_eq!(make_at(&college, 25.0, 45.6).y, 47);
    assert_eq!(make_at(&nba, 25.0, 45.6).y, 46);

    let text = summary_text("run").unwrap();
    let marks = |rules: Rules| {
        let extract = extract_event(&fixture("run"), rules);
        summarize(&text, extract.summary_context()).summary.makes
    };
    let (college, nba) = (marks(COLLEGE), marks(Rules::NBA));
    assert_eq!(college.len(), nba.len());
    for (shifted, plain) in college.iter().zip(&nba) {
        assert_eq!(
            (shifted.side, shifted.three, shifted.x),
            (plain.side, plain.three, plain.x)
        );
        assert_eq!(shifted.y, (plain.y + 1).min(47));
    }
}

/// A live college extract (Tennessee–Radford at the team timeout).
fn extract_of_live(rules: Rules) -> Extract {
    extract_event(&fixture("team_timeout"), rules)
}

/// §3.7 / D8 on the corpus: a team timeout flashes from the scoreboard
/// alone; the TV timeout and the 91 placeholder do not — the flash then
/// falls back to the summary's newest eligible play, an earlier team
/// timeout, never the scheduled stoppage itself.
#[test]
fn a_team_timeout_flashes_and_a_tv_timeout_or_placeholder_does_not() {
    let (extract, summary) = pair("team_timeout");
    let radford = Some(("401906849121008056", "Radford Timeout"));
    assert_eq!(last_play(live(&extract.as_game(None))), radford);
    assert_eq!(last_play(live(&extract.as_game(summary.as_ref()))), radford);
    // The NBA's table has no 579: the same body flashes nothing there.
    let as_nba = extract_of_live(Rules::NBA);
    assert_eq!(last_play(live(&as_nba.as_game(None))), None);

    for (name, last_play_id) in [
        ("tv_timeout", "401906849121019113"),
        ("placeholder_play", "401906865121018404"),
    ] {
        let event = parse(&fixture(name));
        assert_eq!(
            event["competitions"][0]["situation"]["lastPlay"]["id"], last_play_id,
            "{name}"
        );
        let (extract, summary) = pair(name);
        assert_eq!(last_play(live(&extract.as_game(None))), None, "{name}");
        let with = extract.as_game(summary.as_ref());
        let (id, text) = last_play(live(&with)).expect("an earlier team timeout");
        assert_ne!(id, last_play_id, "{name}");
        assert!(text.ends_with(" Timeout"), "{name}: {text}");
    }
}

/// D8's whole table from both documents: the corpus has no live technical
/// (521) or regular timeout (578), so each eligible type is posted by hand
/// as the scoreboard's `lastPlay` and as a summary's only play; the TV
/// timeout, the placeholder and the NBA's full timeout (16) are not
/// college's.
#[test]
fn every_college_flash_type_flashes_from_either_document() {
    let event = fixture("team_timeout");
    let context = extract_of_live(COLLEGE).summary_context();
    for (type_id, flashes) in [
        ("578", true),
        ("579", true),
        ("521", true),
        ("215", true),
        ("216", true),
        ("580", false),
        ("91", false),
        ("16", false),
        ("519", false),
    ] {
        let posted = mutate(&event, |v| {
            let play = &mut v["competitions"][0]["situation"]["lastPlay"];
            play["type"]["id"] = serde_json::json!(type_id);
            play["text"] = serde_json::json!("Technical foul on Radford");
        });
        let extract = extract_event(&posted, COLLEGE);
        let game = extract.as_game(None);
        assert_eq!(
            last_play(live(&game)).is_some(),
            flashes,
            "scoreboard type {type_id}"
        );

        let summary = serde_json::json!({"plays": [
            {"id": "7", "type": {"id": type_id}, "text": "Tennessee Timeout"},
        ]})
        .to_string();
        let summary = summarize(&summary, context.clone()).summary;
        let flash = summary.last_play.map(|play| play.id.as_str().to_string());
        assert_eq!(flash.is_some(), flashes, "summary type {type_id}");
    }
}

/// §3.1: two halves, so a college overtime final is period 3 and its
/// `altDetail` "OT" agrees — no quirk. The NBA's regulation would call the
/// same body a mismatch.
#[test]
fn a_college_overtime_final_is_period_three_with_no_quirk() {
    let event = fixture("final_ot");
    assert_eq!(
        parse(&event)["competitions"][0]["status"]["type"]["altDetail"],
        "OT"
    );
    let (extract, quirks) = detail(&wrap(&event), &id_of(&event), COLLEGE);
    assert_eq!(final_(&extract.as_game(None)).periods_played, 3);
    assert_eq!(quirks.0, vec![]);

    let (_, quirks) = detail(&wrap(&event), &id_of(&event), Rules::NBA);
    assert_eq!(quirks.0, vec![Quirk::FinalDetailMismatch]);

    for name in ["final", "final_tournament"] {
        let event = fixture(name);
        let (extract, quirks) = detail(&wrap(&event), &id_of(&event), COLLEGE);
        assert_eq!(final_(&extract.as_game(None)).periods_played, 2, "{name}");
        assert_eq!(quirks.0, vec![], "{name}");
    }
}

/// §3.8: the ranked side reads "#5 ARIZONA" and the unranked side is named,
/// even with betting lines on the board (ranks or lines, never one of each),
/// at `ext=1` like any team line. Without the rank line the same game gets
/// its betting lines.
#[test]
fn a_ranked_pregame_names_the_unranked_side() {
    let (extract, _) = pair("pregame_ranked");
    let game = extract.as_game(None);
    let lines = |game: &Game<'_>| {
        let game = pregame(game);
        (
            game.away.line.map(str::to_owned),
            game.home.line.map(str::to_owned),
        )
    };
    let ranked = (Some("CAL POLY".to_string()), Some("#5 ARIZONA".to_string()));
    assert_eq!(lines(&game), ranked);
    assert_eq!(lines(&wire::decode(&encode_at(&game, 1)).unwrap()), ranked);
    assert_eq!(
        (pregame(&game).away.record, pregame(&game).home.record),
        (Some(record(1, 0)), Some(record(0, 1)))
    );

    let as_nba = extract_event(&fixture("pregame_ranked"), Rules::NBA);
    let (away, home) = lines(&as_nba.as_game(None));
    assert_eq!(away.as_deref(), Some("+30.5  +5000"));
    assert!(home.unwrap().starts_with('-'), "the favourite's spread");

    // Both ranked: both ranks, no names.
    let both = mutate(&fixture("pregame_ranked"), |v| {
        for competitor in v["competitions"][0]["competitors"].as_array_mut().unwrap() {
            if competitor["homeAway"] == "away" {
                competitor["curatedRank"]["current"] = serde_json::json!(16);
            }
        }
    });
    let extract = extract_event(&both, COLLEGE);
    assert_eq!(
        lines(&extract.as_game(None)),
        (
            Some("#16 CAL POLY".to_string()),
            Some("#5 ARIZONA".to_string())
        )
    );
}

/// §3.8: the info cycle's second line is ESPN's note upper-cased; college
/// has no series, in the postseason or out of it.
#[test]
fn the_pregame_second_line_is_the_note_upper_cased() {
    for season_type in [2, 3] {
        let event = mutate(&fixture("pregame_ranked"), |v| {
            v["season"]["type"] = serde_json::json!(season_type);
            v["competitions"][0]["notes"] =
                serde_json::json!([{"type": "event", "headline": "Lake Tahoe Classic"}]);
        });
        let extract = extract_event(&event, COLLEGE);
        assert_eq!(
            pregame(&extract.as_game(None)).info_secondary,
            "LAKE TAHOE CLASSIC",
            "season type {season_type}"
        );
    }
}

/// D13 (DESIGN.md ruling 18): West Coast Baptist comes with no `color` and
/// no `alternateColor`. The game lists and details, the team reading black
/// in both slots; a null reads the same.
#[test]
fn a_colourless_team_keeps_its_game() {
    let event = fixture("pregame_colourless");
    let id = id_of(&event);
    let nulled = mutate(&event, |v| {
        for competitor in v["competitions"][0]["competitors"].as_array_mut().unwrap() {
            if competitor["team"]["abbreviation"] == "WCBC" {
                competitor["team"]["color"] = serde_json::Value::Null;
                competitor["team"]["alternateColor"] = serde_json::Value::Null;
            }
        }
    });
    for body in [wrap(&event), wrap(&nulled)] {
        let mut listed = Vec::new();
        let mut on_game = |id: &str, state: GameState| listed.push((id.to_string(), state));
        let mut quirks = RecQuirks::default();
        let stats = {
            let extractor = Extractor::games_list(&mut on_game, &mut quirks);
            let mut scratch = vec![0u8; 64 * 1024];
            let mut matcher =
                StreamMatcher::new(basketball::PATHS, extractor, &mut scratch).unwrap();
            matcher.write(body.as_bytes()).unwrap();
            matcher.finish().unwrap().stats()
        };
        assert_eq!((stats.ok, stats.failed), (1, 0));
        assert_eq!(listed, [(id.clone(), GameState::Pregame)]);

        let extract = detail(&body, &id, COLLEGE).0;
        let game = extract.as_game(None);
        let game = pregame(&game);
        assert_eq!(game.away.abbreviation, "WCBC");
        assert_eq!(
            (game.away.colors.primary, game.away.colors.alternate),
            (0, 0)
        );
        assert_eq!(
            game.home.colors.primary, 0x0033a0,
            "Boise State keeps its own"
        );
        round_trip("pregame_colourless", &extract.as_game(None));
    }
}

/// §3.6: the scorer's points are found among every row the box score lists.
/// College lists 14–18 a team, a non-Division-I side 37; the cap holds that
/// listing with the scorer last, and only a row past the cap is unlisted.
#[test]
fn the_athlete_cap_holds_the_longest_listing_seen() {
    let (extract, summary) = pair("run");
    let points = newest(live(&extract.as_game(summary.as_ref()))).unwrap().3;
    let text = summary_text("run").unwrap();
    let shooter = parse(&text)["plays"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|play| play["scoringPlay"] == true)
        .map(|play| play["participants"][0]["athlete"]["id"].clone())
        .unwrap();
    let padded = |rows: usize| {
        mutate(&text, |v| {
            for block in v["boxscore"]["players"].as_array_mut().unwrap() {
                let athletes = block["statistics"][0]["athletes"].as_array_mut().unwrap();
                let Some(at) = athletes.iter().position(|a| a["athlete"]["id"] == shooter) else {
                    continue;
                };
                let row = athletes.remove(at);
                let filler = athletes[0].clone();
                while athletes.len() < rows - 1 {
                    athletes.push(filler.clone());
                }
                athletes.push(row);
            }
        })
    };

    let report = summarize(&padded(37), extract.summary_context());
    assert_eq!(report.quirks.0, vec![]);
    assert_eq!(report.summary.newest_score.unwrap().scorer_points, points);

    let report = summarize(
        &padded(basketball::ATHLETES_PER_TEAM + 1),
        extract.summary_context(),
    );
    assert_eq!(report.quirks.0, vec![Quirk::BoundedOverflow]);
    assert_eq!(report.summary.newest_score.unwrap().scorer_points, 0);
}
