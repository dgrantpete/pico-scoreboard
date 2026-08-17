//! Host A/B timing for the extraction paths the on-silicon bench exercises:
//! list and detail (first/last target) over one real body, socket-shaped
//! 4 KB chunks. Exists to attribute device-bench deltas to code, not
//! silicon: run it against two picojson checkouts and compare.
//!
//! Usage: detail_bench <body.json> <first_id> <last_id> [--college] [iters]

use scoreboard_espn::common::IgnoreQuirks;
use scoreboard_espn::football;
use scoreboard_wire::GameState;
use std::time::Instant;

struct NoopEntries;
impl football::ListEntries for NoopEntries {
    fn entry(&mut self, _id: &str, _state: GameState) {}
}

fn feed<F: FnMut(&[u8])>(body: &[u8], mut write: F) {
    let mut chunk_buf = [0u8; 4096];
    let mut pos = 0;
    while pos < body.len() {
        let end = (pos + 4096).min(body.len());
        let n = end - pos;
        chunk_buf[..n].copy_from_slice(&body[pos..end]);
        write(&chunk_buf[..n]);
        pos = end;
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: detail_bench <body> <first_id> <last_id> [--college] [iters]");
    let first_id = args.next().expect("first_id");
    let last_id = args.next().expect("last_id");
    let rest: Vec<String> = args.collect();
    let college = rest.iter().any(|a| a == "--college");
    let iters: u32 = rest
        .iter()
        .find_map(|a| a.parse().ok())
        .unwrap_or(20);

    let body = std::fs::read(&path).expect("read body");
    let mut scratch = vec![0u8; 16 * 1024];

    let t0 = Instant::now();
    for _ in 0..iters {
        let mut ex = football::ListExtractor::new(NoopEntries, IgnoreQuirks, &mut scratch).unwrap();
        feed(&body, |c| ex.write(c).unwrap());
        ex.finish().unwrap();
    }
    let list = t0.elapsed() / iters;

    let t0 = Instant::now();
    for _ in 0..iters {
        let mut ex =
            football::DetailExtractor::new(&first_id, college, IgnoreQuirks, &mut scratch).unwrap();
        feed(&body, |c| ex.write(c).unwrap());
        ex.finish().unwrap();
    }
    let first = t0.elapsed() / iters;

    let t0 = Instant::now();
    for _ in 0..iters {
        let mut ex =
            football::DetailExtractor::new(&last_id, college, IgnoreQuirks, &mut scratch).unwrap();
        feed(&body, |c| ex.write(c).unwrap());
        ex.finish().unwrap();
    }
    let last = t0.elapsed() / iters;

    println!(
        "{} B x {iters}: list {:?}, detail-first {:?}, detail-last {:?}",
        body.len(),
        list,
        first,
        last
    );
}
