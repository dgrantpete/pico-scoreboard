//! What the list pass actually needs from picojson's scratch — measured, not
//! assumed (S3-DESIGN wave-2 item 12).
//!
//! The scratch must hold the longest token the *tokenizer* handles, and the
//! question that decides the poller's memory plan is whether tokens on paths
//! the list tables skip still transit it. The probe below answers by binary
//! search over the real full-slate captures in `firmware-rs/bench/assets/` —
//! the biggest bodies in the repo, including a 1.2 MB college-football
//! Saturday. The printed minimum is diagnostic; the assertion pins the
//! decision taken on it.

use std::path::{Path, PathBuf};

use scoreboard_direct::{Feed, ListStream};
use scoreboard_espn::common::IgnoreQuirks;
use scoreboard_espn::{ListRow, ListSink};

fn assets() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../firmware-rs/bench/assets")
}

struct CountRows(usize);

impl ListSink for CountRows {
    fn row(&mut self, _row: ListRow<'_>) {
        self.0 += 1;
    }
}

fn runs_with(feed: Feed, body: &[u8], scratch_len: usize) -> Option<usize> {
    let mut quirks = IgnoreQuirks;
    let mut scratch = vec![0u8; scratch_len];
    let mut stream = ListStream::new(feed, CountRows(0), &mut quirks, &mut scratch).ok()?;
    for piece in body.chunks(4096) {
        stream.write(piece).ok()?;
    }
    stream.finish().ok().map(|report| report.sink.0)
}

fn minimum_scratch(feed: Feed, body: &[u8]) -> (usize, usize) {
    let rows = runs_with(feed, body, 64 * 1024).expect("64 KiB streams every asset");
    let (mut lo, mut hi) = (1usize, 64 * 1024);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        match runs_with(feed, body, mid) {
            Some(got) if got == rows => hi = mid,
            _ => lo = mid + 1,
        }
    }
    (lo, rows)
}

/// The poller ships one 2 KiB scratch for its list pass. Measured minima on
/// the real captures: MLB 210 B, college football 464 B, MLS 245 B — whatever
/// the longest transiting token's path is, no scoreboard body in the corpus
/// carries one near this bound. 2 KiB is the worst measurement with 4× slack,
/// and this fails the day ESPN ships a token that threatens it.
const LIST_SCRATCH: usize = 2 * 1024;

#[test]
fn every_full_slate_lists_within_the_list_scratch() {
    let cases = [
        ("body-mlb-max.json", Feed::Mlb),
        ("body-cfb-live.json", Feed::Football { college: true }),
        ("body-mls-max.json", Feed::Soccer),
    ];
    for (name, feed) in cases {
        let path = assets().join(name);
        let body = std::fs::read(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
        let (minimum, rows) = minimum_scratch(feed, &body);
        println!("{name}: {rows} rows, minimum scratch {minimum} B");
        assert!(rows > 0, "{name}: no rows listed");
        assert!(
            minimum <= LIST_SCRATCH,
            "{name}: needs {minimum} B, more than the poller's {LIST_SCRATCH}"
        );
    }
}
