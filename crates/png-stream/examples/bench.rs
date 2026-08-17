//! Host-scale timing: `cargo run -p png-stream --example bench --release`.
//! Not a device number — the orchestrator's bench firmware measures that —
//! just a sanity magnitude for the decode pipeline.

use png_stream::{Rgb8, Scratch, SpriteDecoder};
use std::time::Instant;

fn main() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let mut scratch = Scratch::new();
    for name in [
        "nfl-500-kc.png",
        "mlb-500-bos.png",
        "mlb-500-nyy.png",
        "ncaa-500-2294.png",
        "bos-100.png",
        "nyy-100.png",
    ] {
        let data = std::fs::read(dir.join(name)).expect("read logo");
        const ITERS: u32 = 50;
        let start = Instant::now();
        let mut last = 0u16;
        for _ in 0..ITERS {
            let mut d = SpriteDecoder::new(&mut scratch);
            d.write(&data).expect("write");
            let sprite = d.finish(Rgb8::new(0, 0, 0)).expect("finish");
            last ^= sprite[300];
        }
        let per = start.elapsed() / ITERS;
        println!("{name}: {} bytes -> 24x24 in {per:?} (checksum {last:04x})", data.len());
    }
}
