//! Mass-validation sweep over collector-captured ESPN bodies (host tool).
//!
//! Reads NDJSON on stdin: {"sport","league","endpoint","body"} — raw bodies
//! exported from the NUC store — and runs each through the real extractors:
//! a list pass on every scoreboard body (full rejection-parity validation of
//! every event), plus detail extraction + wire encode for every event of a
//! sampled subset and of every anomalous body. Soccer summaries go through
//! the summary extractor. Panics are caught per body. Emits a JSON stats
//! report on stdout; progress on stderr.
//!
//! Usage: sweep [detail-sample-every-N] < bodies.ndjson

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};

use scoreboard_espn::common::{Quirk, Quirks};
use scoreboard_espn::{football, mlb, nba, soccer};
use scoreboard_wire::GameState;

#[derive(Default)]
struct QuirkCount(BTreeMap<String, u64>);

impl Quirks for QuirkCount {
    fn quirk(&mut self, quirk: Quirk) {
        *self.0.entry(format!("{quirk:?}")).or_insert(0) += 1;
    }
}

#[derive(Default)]
struct Stats {
    bodies: u64,
    events_ok: u64,
    events_failed: u64,
    list_errors: BTreeMap<String, u64>,
    detail_runs: u64,
    detail_found: u64,
    detail_outcomes: BTreeMap<String, u64>,
    encoded: u64,
    encode_max: usize,
    summaries: u64,
    summary_some: u64,
    summary_none: u64,
    quirks: BTreeMap<String, u64>,
    panics: Vec<String>,
}

fn merge_quirks(stats: &mut Stats, q: QuirkCount) {
    for (k, v) in q.0 {
        *stats.quirks.entry(k).or_insert(0) += v;
    }
}

const SCRATCH: usize = 64 * 1024;

fn main() {
    let sample_every: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(25);
    let stdin = std::io::stdin();
    let mut per_league: BTreeMap<String, Stats> = BTreeMap::new();
    let mut n = 0u64;

    for line in stdin.lock().lines() {
        let line = line.expect("stdin read");
        if line.trim().is_empty() {
            continue;
        }
        let rec: serde_json::Value = serde_json::from_str(&line).expect("NDJSON record");
        let sport = rec["sport"].as_str().expect("sport").to_string();
        let league = rec["league"].as_str().expect("league").to_string();
        let endpoint = rec["endpoint"].as_str().expect("endpoint").to_string();
        let body = rec["body"].as_str().expect("body").as_bytes().to_vec();
        let key = format!("{sport}/{league}/{endpoint}");
        let stats = per_league.entry(key.clone()).or_default();
        stats.bodies += 1;
        n += 1;

        let result = catch_unwind(AssertUnwindSafe(|| {
            process(&sport, &league, &endpoint, &body, stats, sample_every)
        }));
        if result.is_err() {
            stats.panics.push(format!("body #{n} in {key}"));
        }
        if n.is_multiple_of(2000) {
            eprintln!("[sweep] {n} bodies…");
            std::io::stderr().flush().ok();
        }
    }

    let mut out = serde_json::Map::new();
    for (k, s) in &per_league {
        out.insert(
            k.clone(),
            serde_json::json!({
                "bodies": s.bodies,
                "events_ok": s.events_ok,
                "events_failed": s.events_failed,
                "list_errors": s.list_errors,
                "detail_runs": s.detail_runs,
                "detail_found": s.detail_found,
                "detail_outcomes": s.detail_outcomes,
                "encoded_payloads": s.encoded,
                "encode_max_bytes": s.encode_max,
                "summaries": s.summaries,
                "summary_some": s.summary_some,
                "summary_none": s.summary_none,
                "quirks": s.quirks,
                "panics": s.panics,
            }),
        );
    }
    println!("{}", serde_json::Value::Object(out));
}

fn event_ids(body: &[u8]) -> Vec<String> {
    // Id discovery via serde_json keeps the harness independent of the four
    // list-API shapes; the extractors still do all the validation.
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else {
        return Vec::new();
    };
    v["events"]
        .as_array()
        .map(|events| {
            events
                .iter()
                .filter_map(|e| e["id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn process(
    sport: &str,
    league: &str,
    endpoint: &str,
    body: &[u8],
    stats: &mut Stats,
    sample_every: u64,
) {
    if endpoint == "summary" {
        let mut scratch = vec![0u8; SCRATCH];
        let mut ex = soccer::SummaryExtractor::new(&mut scratch).expect("table");
        stats.summaries += 1;
        if ex.write(body).is_ok() {
            match ex.finish() {
                Ok(outcome) => {
                    if outcome.commentary.is_some() {
                        stats.summary_some += 1;
                    } else {
                        stats.summary_none += 1;
                    }
                }
                Err(_) => {
                    *stats.list_errors.entry("summary_error".into()).or_insert(0) += 1;
                }
            }
        } else {
            *stats.list_errors.entry("summary_stream".into()).or_insert(0) += 1;
        }
        return;
    }

    // List pass: full per-event validation, exact ok/failed counts.
    let (ok, failed) = list_pass(sport, body, stats);
    stats.events_ok += ok;
    stats.events_failed += failed;

    // Detail + encode: sampled, plus every anomalous body.
    let anomalous = failed > 0;
    if anomalous || stats.bodies.is_multiple_of(sample_every) {
        for id in event_ids(body) {
            stats.detail_runs += 1;
            detail_pass(sport, league, body, &id, stats);
        }
    }
}

fn list_pass(sport: &str, body: &[u8], stats: &mut Stats) -> (u64, u64) {
    let mut quirks = QuirkCount::default();
    let mut scratch = vec![0u8; SCRATCH];
    let counts = match sport {
        "mlb" => {
            struct NoopSink;
            impl mlb::ListSink for NoopSink {
                fn entry(&mut self, _id: &str, _state: GameState) {}
            }
            let mut sink = NoopSink;
            let mut ex =
                mlb::ListExtractor::new(&mut sink, &mut quirks, &mut scratch).expect("table");
            match ex.write(body).and(Ok(())) {
                Ok(()) => match ex.finish() {
                    Ok(c) => Some((u64::from(c.ok), u64::from(c.failed))),
                    Err(e) => {
                        *stats.list_errors.entry(format!("mlb:{e:?}")).or_insert(0) += 1;
                        None
                    }
                },
                Err(e) => {
                    *stats.list_errors.entry(format!("mlb-stream:{e:?}")).or_insert(0) += 1;
                    None
                }
            }
        }
        "nba" => {
            let mut on_game = |_id: &str, _state: GameState| {};
            let extractor = nba::Extractor::games_list(&mut on_game, &mut quirks);
            let mut matcher =
                scoreboard_espn::StreamMatcher::new(nba::PATHS, extractor, &mut scratch)
                    .expect("table");
            match matcher.write(body) {
                Ok(()) => match matcher.finish() {
                    Ok(ex) => {
                        let s = ex.stats();
                        if s.events_malformed {
                            *stats.list_errors.entry("nba:events_malformed".into()).or_insert(0) += 1;
                        }
                        Some((u64::from(s.ok), u64::from(s.failed)))
                    }
                    Err(e) => {
                        *stats.list_errors.entry(format!("nba-stream:{e:?}")).or_insert(0) += 1;
                        None
                    }
                },
                Err(e) => {
                    *stats.list_errors.entry(format!("nba-stream:{e:?}")).or_insert(0) += 1;
                    None
                }
            }
        }
        "football" => {
            struct NoopEntries;
            impl football::ListEntries for NoopEntries {
                fn entry(&mut self, _id: &str, _state: GameState) {}
            }
            let mut ex = football::ListExtractor::new(NoopEntries, quirks, &mut scratch)
                .expect("table");
            quirks = QuirkCount::default();
            match ex.write(body) {
                Ok(()) => match ex.finish() {
                    Ok(report) => {
                        merge_quirks(stats, report.quirks.0.into_iter().fold(
                            QuirkCount::default(),
                            |mut acc, (k, v)| {
                                acc.0.insert(k, v);
                                acc
                            },
                        ));
                        Some((report.counts.ok as u64, report.counts.failed as u64))
                    }
                    Err(e) => {
                        *stats.list_errors.entry(format!("football:{e:?}")).or_insert(0) += 1;
                        None
                    }
                },
                Err(e) => {
                    *stats.list_errors.entry(format!("football-stream:{e:?}")).or_insert(0) += 1;
                    None
                }
            }
        }
        "soccer" => {
            let mut ex = soccer::ListExtractor::new(&mut scratch).expect("table");
            match ex.write(body) {
                Ok(()) => match ex.finish() {
                    Ok(list) => Some((u64::from(list.ok), u64::from(list.failed))),
                    Err(e) => {
                        *stats.list_errors.entry(format!("soccer:{e:?}")).or_insert(0) += 1;
                        None
                    }
                },
                Err(e) => {
                    *stats.list_errors.entry(format!("soccer-stream:{e:?}")).or_insert(0) += 1;
                    None
                }
            }
        }
        other => panic!("unknown sport {other}"),
    };
    merge_quirks(stats, quirks);
    counts.unwrap_or((0, 0))
}

fn detail_pass(sport: &str, league: &str, body: &[u8], id: &str, stats: &mut Stats) {
    let mut quirks = QuirkCount::default();
    let mut scratch = vec![0u8; SCRATCH];
    let mut wire_buf = [0u8; 4096];
    let mut wire = scoreboard_wire::SliceSink::new(&mut wire_buf);
    let outcome_key: String = match sport {
        "mlb" => {
            let mut ex = mlb::DetailExtractor::new(id, &mut quirks, &mut scratch).expect("table");
            match ex.write(body).map_err(|e| format!("{e:?}")).and_then(|()| ex.finish().map_err(|e| format!("{e:?}"))) {
                Ok((extract, _counts)) => {
                    scoreboard_wire::mlb::encode(&extract.as_game(), &mut wire).expect("encode");
                    "Found".into()
                }
                Err(e) => e,
            }
        }
        "nba" => {
            let extractor = nba::Extractor::game_detail(id, &mut quirks);
            let mut matcher =
                scoreboard_espn::StreamMatcher::new(nba::PATHS, extractor, &mut scratch)
                    .expect("table");
            match matcher.write(body).map_err(|e| format!("{e:?}")).and_then(|()| {
                matcher.finish().map_err(|e| format!("{e:?}"))
            }) {
                Ok(ex) => match ex.into_detail() {
                    Some(nba::DetailOutcome::Found(extract)) => {
                        scoreboard_wire::nba::encode(&extract.as_game(), &mut wire)
                            .expect("encode");
                        "Found".into()
                    }
                    Some(other) => format!("{other:?}").split('(').next().unwrap().to_string(),
                    None => "NoOutcome".into(),
                },
                Err(e) => e,
            }
        }
        "football" => {
            let is_college = league == "college-football";
            let mut ex =
                football::DetailExtractor::new(id, is_college, QuirkCount::default(), &mut scratch)
                    .expect("table");
            match ex.write(body).map_err(|e| format!("{e:?}")).and_then(|()| {
                ex.finish().map_err(|e| format!("{e:?}"))
            }) {
                Ok(report) => {
                    merge_quirks(stats, report.quirks);
                    match report.outcome {
                        football::DetailOutcome::Found(extract) => {
                            scoreboard_wire::football::encode(&extract.as_game(), &mut wire)
                                .expect("encode");
                            "Found".into()
                        }
                        other => format!("{other:?}").split('(').next().unwrap().to_string(),
                    }
                }
                Err(e) => e,
            }
        }
        "soccer" => {
            let mut ex =
                soccer::GameExtractor::new(id, QuirkCount::default(), &mut scratch).expect("table");
            match ex.write(body).map_err(|e| format!("{e:?}")).and_then(|()| {
                ex.finish().map_err(|e| format!("{e:?}"))
            }) {
                Ok(report) => {
                    merge_quirks(stats, report.quirks);
                    match report.outcome {
                        soccer::GameOutcome::Found(extract) => {
                            scoreboard_wire::soccer::encode(&extract.as_game(), &mut wire)
                                .expect("encode");
                            "Found".into()
                        }
                        other => format!("{other:?}").split('(').next().unwrap().to_string(),
                    }
                }
                Err(e) => e,
            }
        }
        other => panic!("unknown sport {other}"),
    };
    merge_quirks(stats, quirks);
    if !wire.is_empty() {
        stats.encoded += 1;
        stats.encode_max = stats.encode_max.max(wire.len());
        stats.detail_found += 1;
    }
    let _ = &wire;
    let short = outcome_key.chars().take(60).collect::<String>();
    *stats.detail_outcomes.entry(short).or_insert(0) += 1;
    stats.detail_runs += 0;
}
