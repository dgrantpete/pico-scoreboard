//! `Scratch::init_at` against `Scratch::new`: the in-place constructor must
//! be indistinguishable from the by-value one, or the device (which can only
//! afford `init_at` — a by-value `Scratch` is a 60 KB stack spike) decodes
//! differently from every host test in this crate.

mod common;

use core::mem::MaybeUninit;

use common::*;
use png_stream::{Rgb8, Scratch, SpriteDecoder};

const BLACK: Rgb8 = Rgb8::new(0, 0, 0);

#[test]
fn init_at_decodes_identically_to_new() {
    let mut slot: Box<MaybeUninit<Scratch>> = Box::new(MaybeUninit::uninit());
    let in_place = Scratch::init_at(&mut slot);
    let mut by_value = Scratch::new();

    for name in LOGOS {
        let bytes = logo_bytes(name);

        let mut decoder = SpriteDecoder::new(in_place);
        decoder.write(&bytes).expect("real logo streams");
        let a = decoder.finish(BLACK).expect("real logo finishes");

        let mut decoder = SpriteDecoder::new(&mut by_value);
        decoder.write(&bytes).expect("real logo streams");
        let b = decoder.finish(BLACK).expect("real logo finishes");

        assert_eq!(a, b, "{name}: init_at scratch diverged from new()");
    }
}

/// The reuse contract holds for an `init_at` scratch too: a second decode
/// through the same slot is not contaminated by the first.
#[test]
fn init_at_scratch_is_reusable_across_images() {
    let mut slot: Box<MaybeUninit<Scratch>> = Box::new(MaybeUninit::uninit());
    let scratch = Scratch::init_at(&mut slot);

    let first = {
        let mut decoder = SpriteDecoder::new(scratch);
        decoder.write(&logo_bytes("nfl-500-kc.png")).unwrap();
        decoder.finish(BLACK).unwrap()
    };
    {
        let mut decoder = SpriteDecoder::new(scratch);
        decoder.write(&logo_bytes("bos-100.png")).unwrap();
        decoder.finish(BLACK).unwrap();
    }
    let again = {
        let mut decoder = SpriteDecoder::new(scratch);
        decoder.write(&logo_bytes("nfl-500-kc.png")).unwrap();
        decoder.finish(BLACK).unwrap()
    };
    assert_eq!(first, again, "a decode leaked state into the next");
}
