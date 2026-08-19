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
use png_stream::{Rgb8, Scratch, SpriteDecoder};
use scoreboard_espn::common::{IgnoreQuirks, NoRows};
use scoreboard_espn::{football, mlb, soccer};
use static_cell::{ConstStaticCell, StaticCell};

static MLB_BODY: &[u8] = include_bytes!("../assets/body-mlb-max.json");
static CFB_BODY: &[u8] = include_bytes!("../assets/body-cfb-live.json");
static MLS_BODY: &[u8] = include_bytes!("../assets/body-mls-max.json");

/// Receive-buffer stand-in: chunks are copied here before parsing, inside
/// the timed region, the way a socket read lands bytes.
static CHUNK: ConstStaticCell<[u8; 4096]> = ConstStaticCell::new([0; 4096]);
/// picojson token scratch. 16 KB clears every token the 149k-body host
/// sweep encountered (longest are summary commentary lines, low-KB).
static SCRATCH: ConstStaticCell<[u8; 16 * 1024]> = ConstStaticCell::new([0; 16 * 1024]);
/// png-stream working memory (61.7 KB): initialized in place, per the
/// lane's device-placement note (avoids a transient stack copy).
static PNG_SCRATCH: StaticCell<Scratch> = StaticCell::new();

/// RAM copy of the MLS body (the one slate that fits in SRAM). The real
/// firmware parses socket data that already sits in RAM; this bench's
/// flash-resident bodies stream through the XIP cache and evict hot code as
/// a side effect. Feeding the same extraction from RAM isolates that
/// artifact from real parse cost.
const MLS_LEN: usize = 207_433;
static MLS_RAM: ConstStaticCell<[u8; MLS_LEN]> = ConstStaticCell::new([0; MLS_LEN]);

/// The six real CDN logos: 500 px originals and 100 px combiner variants.
static LOGOS: [(&str, &[u8]); 6] = [
    ("nyy-100", include_bytes!("../assets/nyy-100.png")),
    ("bos-100", include_bytes!("../assets/bos-100.png")),
    ("mlb-500-nyy", include_bytes!("../assets/mlb-500-nyy.png")),
    ("mlb-500-bos", include_bytes!("../assets/mlb-500-bos.png")),
    ("nfl-500-kc", include_bytes!("../assets/nfl-500-kc.png")),
    ("ncaa-500-2294", include_bytes!("../assets/ncaa-500-2294.png")),
];

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
            let mut quirks = IgnoreQuirks;
            let t0 = Instant::now();
            let mut ex = mlb::ListExtractor::new(NoRows, &mut quirks, scratch).unwrap();
            feed(MLB_BODY, chunk_buf, chunk, |c| ex.write(c).unwrap());
            let (_, counts) = ex.finish().unwrap();
            let us = t0.elapsed().as_micros();
            report("mlb-489K", chunk, rep, MLB_BODY.len(), us, counts.ok, counts.failed);

            // College football list — the 1.21 MB body.
            let t0 = Instant::now();
            let mut ex = football::ListExtractor::new(NoRows, IgnoreQuirks, scratch).unwrap();
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
            let mut ex = soccer::ListExtractor::new(NoRows, IgnoreQuirks, scratch).unwrap();
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

    // Data-source attribution: the same MLS list extraction fed from RAM,
    // plus a memcpy+sum control that measures the flash->RAM streaming cost
    // (and its XIP-cache pollution) by itself.
    let mls_ram = MLS_RAM.take();
    mls_ram.copy_from_slice(MLS_BODY);
    for rep in 0..3u32 {
        let t0 = Instant::now();
        let mut acc: u32 = 0;
        feed(MLS_BODY, chunk_buf, 4096, |c| {
            acc = acc.wrapping_add(c.iter().map(|&b| b as u32).sum::<u32>());
        });
        let us = t0.elapsed().as_micros();
        info!(
            "CTRL mls flash-memcpy+sum rep={=u32}: {=u64} us (acc {=u32})",
            rep, us, acc
        );

        let t0 = Instant::now();
        let mut ex = soccer::ListExtractor::new(NoRows, IgnoreQuirks, scratch).unwrap();
        feed(mls_ram, chunk_buf, 4096, |c| ex.write(c).unwrap());
        let list = ex.finish().unwrap();
        let us = t0.elapsed().as_micros();
        report(
            "mls-207K-RAMSRC",
            4096,
            rep,
            MLS_LEN,
            us,
            u32::from(list.ok),
            u32::from(list.failed),
        );
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

    // PNG decode: real CDN logos, streamed in 4 KB chunks (fetch-shaped),
    // decoded + box-downsampled to a 24x24 RGB565 sprite over black.
    let png_scratch = PNG_SCRATCH.init_with(Scratch::new);
    for rep in 0..3u32 {
        for (name, png) in LOGOS.iter() {
            let t0 = Instant::now();
            let mut dec = SpriteDecoder::new(png_scratch);
            let mut pos = 0;
            while pos < png.len() {
                let end = (pos + 4096).min(png.len());
                let n = end - pos;
                chunk_buf[..n].copy_from_slice(&png[pos..end]);
                dec.write(&chunk_buf[..n]).unwrap();
                pos = end;
            }
            let sprite = dec.finish(Rgb8::new(0, 0, 0)).unwrap();
            let us = t0.elapsed().as_micros();
            // Touch the sprite so the whole decode can't be optimized out.
            let checksum: u32 = sprite.iter().map(|&px| px as u32).sum();
            info!(
                "PNG {=str} rep={=u32}: {=u64} us for {=usize} B (sprite sum {=u32})",
                name,
                rep,
                us,
                png.len(),
                checksum
            );
        }
    }

    info!("=== BENCH COMPLETE ===");
    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}
