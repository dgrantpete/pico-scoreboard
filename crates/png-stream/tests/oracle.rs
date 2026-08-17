//! Exact-decode oracle: the crate's inflate + defilter stage must
//! byte-match the `png` crate's decode — over all six real logos and over
//! synthetic images that force each of the five filter types in both
//! supported color types.

mod common;

use common::*;
use png_stream::{RowDecoder, Scratch};

fn assert_row_decode_matches(data: &[u8]) {
    let (ow, oh, och, oracle) = oracle_decode(data);

    let mut scratch = Scratch::new();
    let mut dec = RowDecoder::new(&mut scratch);
    let mut sink = CollectSink::default();
    dec.write(data, &mut sink).expect("write");
    dec.finish().expect("finish");

    assert_eq!((sink.width, sink.height, sink.channels), (ow, oh, och));
    assert_eq!(sink.rows, oh);
    assert_eq!(sink.data.len(), oracle.len());
    assert!(sink.data == oracle, "pixel bytes diverge from oracle");
}

#[test]
fn six_logos_byte_match_the_png_crate() {
    for name in LOGOS {
        let data = logo_bytes(name);
        assert_row_decode_matches(&data);
    }
}

#[test]
fn oracle_match_survives_one_byte_feeds() {
    // The exact-decode stage itself must be split-invariant, not just the
    // sprite: same oracle comparison, worst-case chunking, smallest logo.
    let data = logo_bytes("nyy-100.png");
    let (_, _, _, oracle) = oracle_decode(&data);

    let mut scratch = Scratch::new();
    let mut dec = RowDecoder::new(&mut scratch);
    let mut sink = CollectSink::default();
    for b in &data {
        dec.write(std::slice::from_ref(b), &mut sink).expect("write");
    }
    dec.finish().expect("finish");
    assert!(sink.data == oracle);
}

#[test]
fn every_filter_type_rgba_matches_oracle() {
    // Dimensions deliberately not multiples of 24, nor of each other.
    let (w, h) = (61, 47);
    let px = gradient_rgba(w, h);
    for filter in [
        png::Filter::NoFilter,
        png::Filter::Sub,
        png::Filter::Up,
        png::Filter::Avg,
        png::Filter::Paeth,
        png::Filter::Adaptive,
    ] {
        let data = encode_png(w, h, png::ColorType::Rgba, filter, &px);
        assert_row_decode_matches(&data);
    }
}

#[test]
fn every_filter_type_rgb_matches_oracle() {
    let (w, h) = (53, 38);
    let px = gradient_rgb(w, h);
    for filter in [
        png::Filter::NoFilter,
        png::Filter::Sub,
        png::Filter::Up,
        png::Filter::Avg,
        png::Filter::Paeth,
        png::Filter::Adaptive,
    ] {
        let data = encode_png(w, h, png::ColorType::Rgb, filter, &px);
        assert_row_decode_matches(&data);
    }
}

#[test]
fn dimension_edges_match_oracle() {
    // 1-pixel edges and the MAX_DIM boundary row shape.
    for (w, h) in [(1, 1), (1, 100), (100, 1), (1024, 3), (3, 1024)] {
        let px = gradient_rgba(w, h);
        let data = encode_png(w, h, png::ColorType::Rgba, png::Filter::Adaptive, &px);
        assert_row_decode_matches(&data);
    }
}
