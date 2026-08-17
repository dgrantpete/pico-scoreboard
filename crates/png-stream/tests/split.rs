//! Chunk-split invariance (house rule): whole-buffer, 1-byte, and seeded
//! random feeds must produce bit-identical sprites — and one `Scratch`
//! reused across every decode must behave like a fresh one.

mod common;

use common::*;
use png_stream::{Rgb8, Scratch};

const BG: Rgb8 = Rgb8::new(0, 0, 0);

#[test]
fn feeds_of_any_shape_are_identical() {
    // One scratch for everything: proves per-decoder reset as well.
    let mut scratch = Scratch::new();
    for name in LOGOS {
        let data = logo_bytes(name);
        let whole = sprite_whole(&mut scratch, &data, BG).expect("whole");
        let byte_at_a_time = sprite_fixed_chunks(&mut scratch, &data, 1, BG).expect("1-byte");
        assert!(whole == byte_at_a_time, "{name}: 1-byte feed diverged");
        for seed in [1u64, 0xDEAD_BEEF, 0x5C0E_B0A2] {
            let random = sprite_random_chunks(&mut scratch, &data, seed, BG).expect("random");
            assert!(whole == random, "{name}: random feed (seed {seed}) diverged");
        }
        // A typical TCP-segment-ish size for good measure.
        let mss = sprite_fixed_chunks(&mut scratch, &data, 1379, BG).expect("mss");
        assert!(whole == mss, "{name}: 1379-byte feed diverged");
    }
}

#[test]
fn scratch_reuse_is_clean_across_different_images() {
    // A after B equals A after nothing — no state bleeds through reuse.
    let mut fresh = Scratch::new();
    let a = logo_bytes("nfl-500-kc.png");
    let baseline = sprite_whole(&mut fresh, &a, BG).expect("baseline");

    let mut reused = Scratch::new();
    let b = logo_bytes("mlb-500-nyy.png");
    sprite_whole(&mut reused, &b, BG).expect("warm-up decode");
    // Even an abandoned half-decode must not contaminate the next one.
    {
        let mut d = png_stream::SpriteDecoder::new(&mut reused);
        let _ = d.write(&a[..a.len() / 2]);
    }
    let again = sprite_whole(&mut reused, &a, BG).expect("reused");
    assert!(baseline == again, "reused scratch diverged from fresh");
}
