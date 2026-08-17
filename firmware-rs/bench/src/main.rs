//! On-silicon timing bench for Phase S local processing (owner-requested,
//! 2026-08-16 night): what does streaming ESPN extraction actually cost on
//! the RP2350 at 150 MHz? PHASE-S.md's "~20 ms per 300 KB" is a paper
//! number; the drill-day rule says measure before believing.
//!
//! Three REAL bodies are baked into flash: the largest stored MLB slate
//! (489 KB), tonight's live 99-event college-football slate (1.21 MB — a
//! body that can never fit in RAM, the case the streaming design exists
//! for), and the largest stored MLS slate (207 KB). Each is fed through the
//! real extractors in network-sized chunks (4096 B receive-buffer sized and
//! 1379 B TLS-record sized), copied chunkwise into RAM inside the timed
//! region like a socket read would be.
//!
//! Exists to produce numbers, not to be a product.

#![no_std]
#![no_main]

use defmt::info;
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use panic_probe as _;
use scoreboard_espn::common::IgnoreQuirks;
use scoreboard_espn::{football, mlb, soccer};
use scoreboard_wire::GameState;
use static_cell::ConstStaticCell;

static MLB_BODY: &[u8] = include_bytes!("../assets/body-mlb-max.json");
static CFB_BODY: &[u8] = include_bytes!("../assets/body-cfb-live.json");
static MLS_BODY: &[u8] = include_bytes!("../assets/body-mls-max.json");

/// Receive-buffer stand-in: chunks are copied here before parsing, inside
/// the timed region, the way a socket read lands bytes.
static CHUNK: ConstStaticCell<[u8; 4096]> = ConstStaticCell::new([0; 4096]);
/// picojson token scratch. 16 KB clears every token the 149k-body host
/// sweep encountered (longest are summary commentary lines, low-KB).
static SCRATCH: ConstStaticCell<[u8; 16 * 1024]> = ConstStaticCell::new([0; 16 * 1024]);

struct NoopSink;
impl mlb::ListSink for NoopSink {
    fn entry(&mut self, _id: &str, _state: GameState) {}
}
struct NoopEntries;
impl football::ListEntries for NoopEntries {
    fn entry(&mut self, _id: &str, _state: GameState) {}
}

fn feed<F: FnMut(&[u8])>(body: &[u8], chunk_buf: &mut [u8], chunk: usize, mut write: F) {
    let mut pos = 0;
    while pos < body.len() {
        let end = (pos + chunk).min(body.len());
        let n = end - pos;
        chunk_buf[..n].copy_from_slice(&body[pos..end]);
        write(&chunk_buf[..n]);
        pos = end;
    }
}

fn report(label: &str, chunk: usize, rep: u32, body_len: usize, us: u64, ok: u32, failed: u32) {
    // KB/s with integer math: bytes * 1000 / µs = KB/s (since 1 KB·µs ≈ byte·ms).
    let kb_s = (body_len as u64).saturating_mul(1000) / us.max(1);
    info!(
        "LIST {=str} chunk={=usize} rep={=u32}: {=u64} us for {=usize} B -> {=u64} KB/s (ok={=u32} failed={=u32})",
        label, chunk, rep, us, body_len, kb_s, ok, failed
    );
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let _p = embassy_rp::init(Default::default());
    let sys_hz = embassy_rp::clocks::clk_sys_freq();
    info!("=== espn-bench up: sys {=u32} Hz ===", sys_hz);
    info!(
        "bodies: mlb {=usize} B, cfb {=usize} B, mls {=usize} B",
        MLB_BODY.len(),
        CFB_BODY.len(),
        MLS_BODY.len()
    );

    let chunk_buf = CHUNK.take();
    let scratch = SCRATCH.take();

    for &chunk in &[4096usize, 1379] {
        for rep in 0..3u32 {
            // MLB list.
            let mut sink = NoopSink;
            let mut quirks = IgnoreQuirks;
            let t0 = Instant::now();
            let mut ex = mlb::ListExtractor::new(&mut sink, &mut quirks, scratch).unwrap();
            feed(MLB_BODY, chunk_buf, chunk, |c| ex.write(c).unwrap());
            let counts = ex.finish().unwrap();
            let us = t0.elapsed().as_micros();
            report("mlb-489K", chunk, rep, MLB_BODY.len(), us, counts.ok, counts.failed);

            // College football list — the 1.21 MB body.
            let t0 = Instant::now();
            let mut ex = football::ListExtractor::new(NoopEntries, IgnoreQuirks, scratch).unwrap();
            feed(CFB_BODY, chunk_buf, chunk, |c| ex.write(c).unwrap());
            let rep_out = ex.finish().unwrap();
            let us = t0.elapsed().as_micros();
            report(
                "cfb-1.21M",
                chunk,
                rep,
                CFB_BODY.len(),
                us,
                rep_out.counts.ok as u32,
                rep_out.counts.failed as u32,
            );

            // MLS list.
            let t0 = Instant::now();
            let mut ex = soccer::ListExtractor::new(scratch).unwrap();
            feed(MLS_BODY, chunk_buf, chunk, |c| ex.write(c).unwrap());
            let list = ex.finish().unwrap();
            let us = t0.elapsed().as_micros();
            report(
                "mls-207K",
                chunk,
                rep,
                MLS_BODY.len(),
                us,
                u32::from(list.ok),
                u32::from(list.failed),
            );
        }
    }

    // Detail extraction: the poll loop's real per-game cost, 4 KB chunks.
    for rep in 0..3u32 {
        // MLB: first event of the slate.
        let mut quirks = IgnoreQuirks;
        let t0 = Instant::now();
        let mut ex = mlb::DetailExtractor::new("401816219", &mut quirks, scratch).unwrap();
        feed(MLB_BODY, chunk_buf, 4096, |c| ex.write(c).unwrap());
        let outcome = ex.finish();
        let us = t0.elapsed().as_micros();
        info!(
            "DETAIL mlb first-id rep={=u32}: {=u64} us, found={=bool}",
            rep,
            us,
            outcome.is_ok()
        );

        // CFB: FIRST event (early target -> post-found skip covers ~1.2 MB)…
        let t0 = Instant::now();
        let mut ex =
            football::DetailExtractor::new("401856766", true, IgnoreQuirks, scratch).unwrap();
        feed(CFB_BODY, chunk_buf, 4096, |c| ex.write(c).unwrap());
        let rep_out = ex.finish().unwrap();
        let us = t0.elapsed().as_micros();
        info!(
            "DETAIL cfb FIRST-id rep={=u32}: {=u64} us (post-found skip active), found={=bool}",
            rep,
            us,
            matches!(rep_out.outcome, football::DetailOutcome::Found(_))
        );

        // …vs LAST event (validate-until-found walks all 99 events).
        let t0 = Instant::now();
        let mut ex =
            football::DetailExtractor::new("401858212", true, IgnoreQuirks, scratch).unwrap();
        feed(CFB_BODY, chunk_buf, 4096, |c| ex.write(c).unwrap());
        let rep_out = ex.finish().unwrap();
        let us = t0.elapsed().as_micros();
        info!(
            "DETAIL cfb LAST-id rep={=u32}: {=u64} us (full validate-until-found), found={=bool}",
            rep,
            us,
            matches!(rep_out.outcome, football::DetailOutcome::Found(_))
        );

        // MLS: last event.
        let t0 = Instant::now();
        let mut ex = soccer::GameExtractor::new("761676", IgnoreQuirks, scratch).unwrap();
        feed(MLS_BODY, chunk_buf, 4096, |c| ex.write(c).unwrap());
        let rep_out = ex.finish().unwrap();
        let us = t0.elapsed().as_micros();
        info!(
            "DETAIL mls last-id rep={=u32}: {=u64} us, found={=bool}",
            rep,
            us,
            matches!(rep_out.outcome, soccer::GameOutcome::Found(_))
        );
    }

    info!("=== BENCH COMPLETE ===");
    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}
