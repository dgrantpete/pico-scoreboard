//! The S3 exit gate at crate scope: direct mode and wire mode must hand the
//! display stack the *same* [`GameDetail`].
//!
//! For every fixture in `backend/testdata/` that has a committed wire golden,
//! the raw ESPN body is streamed through [`DetailStream`] in network-sized
//! chunks and the resulting view is compared, field for field, against the
//! view `WireFeed` decodes from the golden bytes.
//!
//! `scoreboard-espn` already pins `as_game()` + encode to these same goldens
//! byte for byte, so this is not a second check of the transform — over
//! wire-bounded strings that encoding is injective and the two gates would
//! agree. What it *does* check is everything between that crate and `Store`,
//! which nothing else covers: the sport dispatch, the four-way stream
//! plumbing, the verdict fold, and the `GameDetail` construction. Comparing
//! views rather than bytes is what makes a mis-wired dispatch fail here
//! instead of shipping. `the_comparator_is_loud` keeps the comparison honest.
//!
//! The goldens are never re-blessed here. They are the format's identity
//! contract, captured from the encoder that shipped to the deployed firmware;
//! a diff is a wire break, not a refactor artifact.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use scoreboard_direct::{
    CommentaryExtract, DetailStream, DirectExtract, Feed, GameDetail, Outcome, Sport,
};
use scoreboard_espn::common::IgnoreQuirks;
use scoreboard_model::feed::{GameFeed, WireFeed};

/// The poller's receive-buffer size (S3-DESIGN decision 6 grows the TLS
/// window to 16 KB, but the app's chunk hand-off stays 4096) — the split the
/// real device will actually produce.
const CHUNK: usize = 4096;

/// Test-side picojson scratch. The device uses 16 KiB and the backend 64 KiB;
/// the single-event fixture bodies need far less, and a tight-but-sufficient
/// buffer here would hide a token-length regression rather than expose it.
const SCRATCH: usize = 64 * 1024;

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/testdata")
}

/// Fixture stems under `testdata/{sport}`, recursing into the per-league
/// subdirectories football and soccer use — the same walk `wire_corpus.rs`
/// does, so the two harnesses cannot disagree about what the corpus is.
fn fixture_names(sport: &str) -> Vec<String> {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {dir:?}: {e}")) {
            let entry = entry.expect("readable dir entry");
            let name = entry.file_name().into_string().expect("utf-8 filename");
            if entry.file_type().expect("file type").is_dir() {
                walk(&entry.path(), &format!("{prefix}{name}/"), out);
            } else if let Some(stem) = name.strip_suffix(".json") {
                out.push(format!("{prefix}{stem}"));
            }
        }
    }
    let mut names = Vec::new();
    walk(&testdata().join(sport), "", &mut names);
    names.sort();
    names
}

/// One fixture as the extractors consume it: the event id, and a synthetic
/// one-event scoreboard body built from the RAW fixture text. Not a
/// `serde_json` round trip — that would launder key order, which is exactly
/// the property the streaming tables must not depend on (DESIGN.md ruling 4).
fn fixture_body(sport: &str, name: &str) -> (String, String) {
    let path = testdata().join(sport).join(format!("{name}.json"));
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    let value: serde_json::Value =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {path:?}: {e}"));
    let id = value["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{path:?}: event has a string id"))
        .to_string();
    (id, format!("{{\"events\":[{raw}]}}"))
}

fn golden_path(sport: &str, name: &str) -> PathBuf {
    testdata()
        .join("wire")
        .join(sport)
        .join(format!("{name}.bin"))
}

fn feed_for(sport: &str, name: &str) -> Feed {
    match sport {
        "mlb" => Feed::Mlb,
        "nba" => Feed::Nba,
        // The fixture's league directory is the ESPN slug, so this is the same
        // test `Feed::from_league` applies to a polled `LeagueId.key`.
        "football" => Feed::Football {
            college: name.starts_with("college-football/"),
        },
        "soccer" => Feed::Soccer,
        other => panic!("unknown sport {other}"),
    }
}

fn sport_for(sport: &str) -> Sport {
    match sport {
        "mlb" => Sport::Mlb,
        "nba" => Sport::Nba,
        "football" => Sport::Football,
        "soccer" => Sport::Soccer,
        other => panic!("unknown sport {other}"),
    }
}

/// Stream one body through the direct path in `chunk`-byte pieces.
fn extract(feed: Feed, game_id: &str, body: &[u8], chunk: usize) -> Outcome {
    let mut quirks = IgnoreQuirks;
    let mut scratch = vec![0u8; SCRATCH];
    let mut stream = DetailStream::new(feed, game_id, &mut quirks, &mut scratch)
        .expect("the pattern tables and scratch are well-formed");
    for piece in body.chunks(chunk) {
        stream.write(piece).expect("fixture bodies are valid JSON");
    }
    stream
        .finish()
        .expect("fixture bodies extract cleanly")
        .outcome
}

fn found(feed: Feed, game_id: &str, body: &[u8], chunk: usize) -> DirectExtract {
    match extract(feed, game_id, body, chunk) {
        Outcome::Found(extract) => extract,
        other => panic!("expected the target to be found, got {other:?}"),
    }
}

/// Every `{sport}/{fixture}` that has a committed golden.
fn corpus() -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for sport in ["mlb", "nba", "football", "soccer"] {
        for name in fixture_names(sport) {
            if golden_path(sport, &name).exists() {
                out.push((sport, name));
            }
        }
    }
    out
}

// ------------------------------------------------------------- the gate

/// The exit gate: direct-mode view == wire-mode view, for every golden.
#[test]
fn every_golden_decodes_to_the_view_the_direct_path_extracts() {
    let corpus = corpus();
    assert!(!corpus.is_empty(), "corpus must not be empty");

    for (sport, name) in &corpus {
        let (id, body) = fixture_body(sport, name);
        let extract = found(feed_for(sport, name), &id, body.as_bytes(), CHUNK);
        let direct = extract.detail();

        let golden = std::fs::read(golden_path(sport, name))
            .unwrap_or_else(|e| panic!("read golden for {sport}/{name}: {e}"));
        let wire = WireFeed
            .detail(sport_for(sport), &golden)
            .unwrap_or_else(|e| panic!("decode golden for {sport}/{name}: {e:?}"));

        assert_eq!(
            direct, wire,
            "{sport}/{name}: the direct extract's view differs from the wire golden's"
        );
    }
}

/// A fixture that quietly loses its golden, or a golden with no fixture behind
/// it, would make the gate above pass by testing less. Pin the set.
#[test]
fn the_gate_covers_every_committed_golden() {
    fn walk(dir: &Path, prefix: &str, out: &mut BTreeSet<String>) {
        for entry in std::fs::read_dir(dir).expect("golden dir readable") {
            let entry = entry.expect("readable dir entry");
            let name = entry.file_name().into_string().expect("utf-8 filename");
            if entry.file_type().expect("file type").is_dir() {
                walk(&entry.path(), &format!("{prefix}{name}/"), out);
            } else if let Some(stem) = name.strip_suffix(".bin") {
                out.insert(format!("{prefix}{stem}"));
            }
        }
    }

    let mut on_disk = BTreeSet::new();
    walk(&testdata().join("wire"), "", &mut on_disk);

    let exercised: BTreeSet<String> = corpus()
        .into_iter()
        .map(|(sport, name)| format!("{sport}/{name}"))
        .collect();

    assert_eq!(
        on_disk, exercised,
        "the parity gate and the committed goldens disagree about the corpus"
    );
    // Guards the arithmetic in the per-sport report, not the sets above.
    assert_eq!(exercised.len(), 33, "corpus size changed");
}

/// Per-sport counts, so a sport silently dropping out of the walk is loud.
#[test]
fn every_sport_is_represented() {
    let mut counts = [("mlb", 0), ("nba", 0), ("football", 0), ("soccer", 0)];
    for (sport, _) in corpus() {
        let slot = counts
            .iter_mut()
            .find(|(name, _)| *name == sport)
            .expect("known sport");
        slot.1 += 1;
    }
    assert_eq!(
        counts,
        [("mlb", 4), ("nba", 7), ("football", 9), ("soccer", 13)],
        "per-sport golden counts changed"
    );
}

/// A gate that cannot fail proves nothing. Every fixture's view is compared
/// against every *other* golden of the same sport: all of those must differ.
/// This is the S1 differential's negative control — if the comparator were
/// shallow (matching on the variant, say, or on `game_id` alone) this test
/// goes red while the gate above stays green.
#[test]
fn the_comparator_is_loud() {
    let corpus = corpus();
    for (sport, name) in &corpus {
        let (id, body) = fixture_body(sport, name);
        let extract = found(feed_for(sport, name), &id, body.as_bytes(), CHUNK);
        let direct = extract.detail();

        for (other_sport, other_name) in &corpus {
            if other_sport != sport || other_name == name {
                continue;
            }
            let golden = std::fs::read(golden_path(other_sport, other_name)).unwrap();
            let wire = WireFeed.detail(sport_for(other_sport), &golden).unwrap();
            assert_ne!(
                direct, wire,
                "{sport}/{name} compares equal to the unrelated golden {other_sport}/{other_name}"
            );
        }
    }
}

// ------------------------------------------------- chunk-split invariance

/// A view that depends on where the receive buffer happened to split is a view
/// that breaks in the field and never in a test. One byte at a time versus the
/// real 4096 must be indistinguishable — over the whole corpus, not a sample,
/// because the corpus is small enough that sampling only loses coverage.
#[test]
fn chunk_boundaries_do_not_change_the_view() {
    for (sport, name) in corpus() {
        let (id, body) = fixture_body(sport, &name);
        let feed = feed_for(sport, &name);
        let whole = found(feed, &id, body.as_bytes(), body.len());
        let split = found(feed, &id, body.as_bytes(), 1);
        let chunked = found(feed, &id, body.as_bytes(), CHUNK);
        assert_eq!(
            whole.detail(),
            split.detail(),
            "{sport}/{name}: one-byte chunking changes the extracted view"
        );
        assert_eq!(
            whole.detail(),
            chunked.detail(),
            "{sport}/{name}: {CHUNK}-byte chunking changes the extracted view"
        );
    }
}

/// The corpus is ASCII-clean, so escapes and multi-byte text — the cases where
/// a chunk boundary is most likely to fall mid-token — need a fixture built
/// for them. A boundary inside a `\uXXXX` escape and inside a UTF-8 sequence
/// is exactly what the string cases below place, at every offset.
#[test]
fn escapes_and_multibyte_text_survive_every_split() {
    // A goal scorer with a combining accent, an emoji (4-byte UTF-8, a
    // surrogate pair as `\u`), a quote and a backslash: the token shapes the
    // tokenizer has to reassemble across a chunk boundary.
    const NASTY: &str = r#"José Martínez \"El Niño\" 🏆 \\ done"#;
    let body = format!(
        r#"{{"events":[{{"id":"1","date":"2026-07-08T01:40Z","competitions":[{{"status":{{"period":3,"type":{{"state":"in","shortDetail":"Top 3rd","description":"In Progress"}}}},"situation":{{"balls":1,"strikes":2,"outs":1,"onFirst":true,"onSecond":false,"onThird":false,"lastPlay":{{"id":"p1","text":"{NASTY}"}}}},"competitors":[{{"homeAway":"home","score":"4","team":{{"abbreviation":"SEA","color":"0C2C56","alternateColor":"005C5C"}}}},{{"homeAway":"away","score":"2","team":{{"abbreviation":"TEX","color":"003278","alternateColor":"C0111F"}}}}]}}]}}]}}"#
    );

    let reference = found(Feed::Mlb, "1", body.as_bytes(), body.len());
    let expected = match reference.detail() {
        GameDetail::Mlb(scoreboard_wire::mlb::Game::Live(live)) => live.last_play.text,
        other => panic!("expected a live MLB game, got {other:?}"),
    };
    assert!(
        expected.contains('é') && expected.contains('🏆') && expected.contains('\\'),
        "the fixture must actually exercise escapes and multi-byte text, got {expected:?}"
    );

    for chunk in 1..=24 {
        let split = found(Feed::Mlb, "1", body.as_bytes(), chunk);
        assert_eq!(
            reference.detail(),
            split.detail(),
            "a {chunk}-byte split changed the extracted view"
        );
    }
}

// ------------------------------------------------------------- verdicts

/// The MLB rain-delay veto: the one corpus fixture with no golden, because a
/// live game in a non-inning state has no displayable payload. Direct mode
/// must reach the same "game ended" verdict the backend serves as 404 —
/// getting this wrong would strand the display on a game it can never fetch.
#[test]
fn the_rain_delay_veto_is_not_found() {
    let (id, body) = fixture_body("mlb", "rain_delay");
    assert!(
        matches!(
            extract(Feed::Mlb, &id, body.as_bytes(), CHUNK),
            Outcome::NotFound
        ),
        "a vetoed live game must read as gone, not as a glitch"
    );
}

/// The 404-vs-502 split, on every sport: an absent id on a clean board is
/// `NotFound`, and the same id on a board carrying one unparseable event is
/// `Glitched`. Folding these the wrong way would either strand a live game or
/// make a transient upstream glitch look like the season ending.
#[test]
fn an_absent_target_separates_a_clean_board_from_a_glitched_one() {
    const CLEAN: &str = r#"{"id":"401","date":"2026-07-08T01:40Z","competitions":[]}"#;

    for feed in [
        Feed::Mlb,
        Feed::Nba,
        Feed::Football { college: false },
        Feed::Football { college: true },
        Feed::Soccer,
    ] {
        let clean = format!(r#"{{"events":[{CLEAN}]}}"#);
        assert!(
            matches!(
                extract(feed, "999", clean.as_bytes(), CHUNK),
                Outcome::NotFound
            ),
            "{feed:?}: an absent id on a clean board is a finished game"
        );

        let glitched = format!(r#"{{"events":[{CLEAN},{{}}]}}"#);
        assert!(
            matches!(
                extract(feed, "999", glitched.as_bytes(), CHUNK),
                Outcome::Glitched
            ),
            "{feed:?}: an absent id beside an unparseable event must not read as gone"
        );

        // The id *was* on the board, so its missing competition is real
        // absence — 404 even with a sibling failure.
        assert!(
            matches!(
                extract(feed, "401", glitched.as_bytes(), CHUNK),
                Outcome::NotFound
            ),
            "{feed:?}: a competition-less target is gone regardless of siblings"
        );
    }
}

/// `$.events` present but not an array fails the whole body before any event,
/// so it can never look like an empty slate.
#[test]
fn a_malformed_events_shell_is_an_error_not_an_empty_board() {
    for feed in [
        Feed::Mlb,
        Feed::Nba,
        Feed::Football { college: false },
        Feed::Soccer,
    ] {
        let mut quirks = IgnoreQuirks;
        let mut scratch = vec![0u8; SCRATCH];
        let mut stream = DetailStream::new(feed, "401", &mut quirks, &mut scratch).unwrap();
        stream.write(br#"{"events":"glitch"}"#).unwrap();
        assert_eq!(
            stream.finish().err(),
            Some(scoreboard_direct::Error::MalformedBody),
            "{feed:?}: a scalar events shell must be an upstream error"
        );
    }
}

// -------------------------------------------------------- the two-body seam

/// Soccer commentary arrives on a second body. Attaching it must show up in
/// the view — and clearing it must return the view to the golden's shape,
/// which is what the committed goldens encode.
#[test]
fn soccer_commentary_merges_into_the_live_view() {
    let (id, body) = fixture_body("soccer", "fifa.world/first_half");
    let mut extract = found(Feed::Soccer, &id, body.as_bytes(), CHUNK);
    assert!(
        extract.wants_commentary(),
        "a live soccer game is what earns the summary fetch"
    );

    let golden = std::fs::read(golden_path("soccer", "fifa.world/first_half")).unwrap();
    let wire = WireFeed.detail(Sport::Soccer, &golden).unwrap();
    assert_eq!(
        extract.detail(),
        wire,
        "the scoreboard-only view is the golden's"
    );

    let mut commentary = CommentaryExtract {
        id: heapless::String::new(),
        text: heapless::String::new(),
    };
    commentary.id.push_str("512").unwrap();
    commentary.text.push_str("Corner, Argentina.").unwrap();
    extract.set_commentary(Some(commentary));

    match extract.detail() {
        GameDetail::Soccer(scoreboard_wire::soccer::Game::Live(live)) => {
            let attached = live.commentary.expect("commentary is attached");
            assert_eq!((attached.id, attached.text), ("512", "Corner, Argentina."));
        }
        other => panic!("expected a live soccer game, got {other:?}"),
    }
    assert_ne!(
        extract.detail(),
        wire,
        "attached commentary must be visible in the view"
    );

    extract.set_commentary(None);
    assert_eq!(
        extract.detail(),
        wire,
        "clearing commentary returns the view to the golden's shape"
    );
}

/// Only live soccer has a commentary slot; every other extract must ignore the
/// call rather than mutate into a shape the wire cannot represent.
#[test]
fn commentary_is_ignored_where_there_is_no_slot() {
    for (sport, name) in [
        ("soccer", "fifa.world/pregame"),
        ("soccer", "fifa.world/full_time"),
        ("mlb", "live_inning"),
        ("nba", "in_progress"),
        ("football", "nfl/in_progress"),
    ] {
        let (id, body) = fixture_body(sport, name);
        let mut extract = found(feed_for(sport, name), &id, body.as_bytes(), CHUNK);
        assert!(!extract.wants_commentary(), "{sport}/{name}");

        let golden = std::fs::read(golden_path(sport, name)).unwrap();
        let wire = WireFeed.detail(sport_for(sport), &golden).unwrap();

        let mut commentary = CommentaryExtract {
            id: heapless::String::new(),
            text: heapless::String::new(),
        };
        commentary.id.push_str("1").unwrap();
        commentary.text.push_str("ignored").unwrap();
        extract.set_commentary(Some(commentary));

        assert_eq!(
            extract.detail(),
            wire,
            "{sport}/{name}: commentary must not perturb a view with no slot"
        );
    }
}

// ------------------------------------------------------------- the seam

/// The extract answers with the id and state ESPN served, which is what the
/// poller keys the slate and the crest pool on.
#[test]
fn the_extract_reports_the_identity_the_view_carries() {
    for (sport, name) in corpus() {
        let (id, body) = fixture_body(sport, &name);
        let extract = found(feed_for(sport, &name), &id, body.as_bytes(), CHUNK);
        let detail = extract.detail();
        assert_eq!(extract.game_id(), detail.game_id(), "{sport}/{name}");
        assert_eq!(extract.game_id(), id, "{sport}/{name}");
        assert_eq!(extract.state(), detail.state(), "{sport}/{name}");
        assert_eq!(extract.sport(), detail.sport(), "{sport}/{name}");
        assert_eq!(extract.sport(), sport_for(sport), "{sport}/{name}");
    }
}

/// `LeagueId.key` is ESPN's path segment, so the college test is a string
/// compare on the slug the backend's registry uses — no second registry.
#[test]
fn the_feed_a_league_selects_matches_its_espn_slug() {
    use scoreboard_direct::LeagueId;

    for (sport, slug, expected) in [
        (Sport::Mlb, "mlb", Feed::Mlb),
        (Sport::Nba, "nba", Feed::Nba),
        (Sport::Football, "nfl", Feed::Football { college: false }),
        (
            Sport::Football,
            "college-football",
            Feed::Football { college: true },
        ),
        (Sport::Soccer, "usa.1", Feed::Soccer),
        (Sport::Soccer, "eng.1", Feed::Soccer),
    ] {
        let league = LeagueId::from_slug(sport, slug);
        let feed = Feed::from_league(&league);
        assert_eq!(feed, expected, "{slug}");
        assert_eq!(feed.sport(), sport, "{slug}");
    }
}

/// The owned extract is the poller's biggest single allocation on a device
/// whose static budget is counted in kilobytes, so its size is pinned rather
/// than assumed. Sized by the largest variant; football's is the largest.
#[test]
fn the_extract_size_is_budgeted() {
    let size = std::mem::size_of::<DirectExtract>();
    // Printed, not just bounded: BUDGET.md takes measured numbers, and the
    // device's 32-bit `usize` makes the host figure an upper bound, not the
    // number itself. Run with `--nocapture` to read it off.
    println!("size_of::<DirectExtract>() = {size} bytes (host)");
    assert!(
        size <= 4096,
        "DirectExtract grew to {size} bytes; the poller holds one and the \
         static budget is measured, not estimated"
    );
    // Loud on shrinkage too: a drop means a bound moved, which needs a look.
    assert!(size >= 2048, "DirectExtract shrank to {size} bytes");
}
