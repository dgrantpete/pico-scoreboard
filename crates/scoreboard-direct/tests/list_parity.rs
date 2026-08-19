//! [`ListStream`] against the four sport dispatches it replaces.
//!
//! The stream's whole job is to be indistinguishable from calling the sport's
//! own list extractor, so that is the test: for every fixture in
//! `backend/testdata/`, the same body is driven through `ListStream` and
//! through the sport's extractor directly, and the captured rows and tallies
//! must match exactly — across chunk sizes 1, 4096 and whole-body, the same
//! split discipline `parity.rs` applies to details.
//!
//! The direct side deliberately duplicates the four-way dispatch rather than
//! calling any shared helper: it is the independent oracle, and sharing the
//! plumbing under test would let a mis-wired variant agree with itself.

use std::path::{Path, PathBuf};

use scoreboard_direct::{Feed, ListStream};
use scoreboard_espn::common::IgnoreQuirks;
use scoreboard_espn::path::StreamMatcher;
use scoreboard_espn::{ListRow, ListSink, football, mlb, nba, soccer};
use scoreboard_wire::GameState;

const SCRATCH: usize = 64 * 1024;
const CHUNKS: [usize; 3] = [1, 4096, usize::MAX];

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/testdata")
}

fn fixture_bodies(sport: &str) -> Vec<(String, String)> {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {dir:?}: {e}")) {
            let entry = entry.expect("readable dir entry");
            let name = entry.file_name().into_string().expect("utf-8 filename");
            if entry.file_type().expect("file type").is_dir() {
                walk(&entry.path(), &format!("{prefix}{name}/"), out);
            } else if let Some(stem) = name.strip_suffix(".json") {
                let raw = std::fs::read_to_string(entry.path()).expect("readable fixture");
                out.push((format!("{prefix}{stem}"), format!("{{\"events\":[{raw}]}}")));
            }
        }
    }
    let mut bodies = Vec::new();
    walk(&testdata().join(sport), "", &mut bodies);
    bodies.sort();
    bodies
}

/// A multi-event body from every fixture of one sport, so ordering and
/// per-event scratch reuse are exercised — single-event bodies cannot catch a
/// row that leaks state into its successor.
fn slate_body(sport: &str) -> String {
    let events: Vec<String> = fixture_bodies(sport)
        .into_iter()
        .map(|(_, body)| {
            body.strip_prefix("{\"events\":[")
                .and_then(|rest| rest.strip_suffix("]}"))
                .expect("fixture body shape")
                .to_string()
        })
        .collect();
    format!("{{\"events\":[{}]}}", events.join(","))
}

/// An owned copy of one row, so the borrow ends inside the callback and the
/// comparison cannot alias the scratch it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    id: String,
    state: GameState,
    away: (Option<String>, Option<String>),
    home: (Option<String>, Option<String>),
}

#[derive(Debug, Default)]
struct Rows(Vec<Row>);

impl ListSink for Rows {
    fn row(&mut self, row: ListRow<'_>) {
        let side = |team: &scoreboard_espn::ListTeam<'_>| {
            (
                team.abbreviation.map(str::to_owned),
                team.crest.map(str::to_owned),
            )
        };
        self.0.push(Row {
            id: row.id.to_owned(),
            state: row.state,
            away: side(&row.away),
            home: side(&row.home),
        });
    }
}

fn feed_for(sport: &str) -> Feed {
    match sport {
        "mlb" => Feed::Mlb,
        "nba" => Feed::Nba,
        "football" => Feed::Football { college: false },
        "soccer" => Feed::Soccer,
        other => panic!("unknown sport {other}"),
    }
}

fn stream_rows(feed: Feed, body: &[u8], chunk: usize) -> (Vec<Row>, u32, u32) {
    let mut quirks = IgnoreQuirks;
    let mut scratch = vec![0u8; SCRATCH];
    let mut stream = ListStream::new(feed, Rows::default(), &mut quirks, &mut scratch)
        .expect("stream constructs");
    for piece in body.chunks(chunk.min(body.len().max(1))) {
        stream.write(piece).expect("clean fixture bodies stream");
    }
    let report = stream.finish().expect("clean fixture bodies finish");
    (report.sink.0, report.counts.ok, report.counts.failed)
}

/// The oracle: the sport's own extractor, driven by hand.
fn direct_rows(sport: &str, body: &[u8]) -> (Vec<Row>, u32, u32) {
    let mut scratch = vec![0u8; SCRATCH];
    match sport {
        "mlb" => {
            let mut quirks = IgnoreQuirks;
            let mut extractor = mlb::ListExtractor::new(Rows::default(), &mut quirks, &mut scratch)
                .expect("constructs");
            extractor.write(body).expect("streams");
            let (rows, counts) = extractor.finish().expect("finishes");
            (rows.0, counts.ok, counts.failed)
        }
        "nba" => {
            let mut quirks = IgnoreQuirks;
            let extractor = nba::Extractor::games_list(Rows::default(), &mut quirks);
            let mut matcher =
                StreamMatcher::new(nba::PATHS, extractor, &mut scratch).expect("constructs");
            matcher.write(body).expect("streams");
            let sink = matcher.finish().expect("finishes");
            let stats = sink.stats();
            assert!(!stats.events_malformed);
            let (ok, failed) = (stats.ok, stats.failed);
            (sink.into_list().expect("list mode").0, ok, failed)
        }
        "football" => {
            let mut extractor =
                football::ListExtractor::new(Rows::default(), IgnoreQuirks, &mut scratch)
                    .expect("constructs");
            extractor.write(body).expect("streams");
            let report = extractor.finish().expect("finishes");
            (
                report.entries.0,
                u32::try_from(report.counts.ok).unwrap(),
                u32::try_from(report.counts.failed).unwrap(),
            )
        }
        "soccer" => {
            let mut extractor =
                soccer::ListExtractor::new(Rows::default(), IgnoreQuirks, &mut scratch)
                    .expect("constructs");
            extractor.write(body).expect("streams");
            let report = extractor.finish().expect("finishes");
            (
                report.entries.0,
                u32::from(report.ok),
                u32::from(report.failed),
            )
        }
        other => panic!("unknown sport {other}"),
    }
}

#[test]
fn list_stream_matches_every_direct_dispatch() {
    let mut total_rows = 0usize;
    for sport in ["mlb", "nba", "football", "soccer"] {
        let feed = feed_for(sport);
        let mut bodies = fixture_bodies(sport);
        bodies.push((format!("{sport}: synthetic slate"), slate_body(sport)));
        for (name, body) in bodies {
            let oracle = direct_rows(sport, body.as_bytes());
            for chunk in CHUNKS {
                let streamed = stream_rows(feed, body.as_bytes(), chunk);
                assert_eq!(
                    streamed, oracle,
                    "{name}: ListStream diverged from the {sport} extractor at chunk {chunk}"
                );
            }
            total_rows += oracle.0.len();
        }
    }
    // The harness's own liveness: if fixture discovery broke, every equality
    // above would pass over empty vectors.
    assert!(total_rows > 40, "only {total_rows} rows compared");
}

/// Both football feeds are one list extraction — the college flag gates a
/// detail-mode rank line no row carries. Pinned so a future college-specific
/// list path cannot land without this test noticing.
#[test]
fn college_and_pro_football_lists_agree() {
    let body = slate_body("football");
    let pro = stream_rows(Feed::Football { college: false }, body.as_bytes(), 4096);
    let college = stream_rows(Feed::Football { college: true }, body.as_bytes(), 4096);
    assert_eq!(pro, college);
}
