//! Malformed-input behavior: clean errors, never panics, poisoned-decoder
//! semantics, and the documented scope rejections.

mod common;

use common::*;
use png_stream::{Error, Rgb8, Scratch, SpriteDecoder};

const BG: Rgb8 = Rgb8::new(0, 0, 0);

/// Signature + one IHDR chunk with the given fields (dummy CRC — the
/// decoder skips CRCs by design, so none of these need real ones).
fn ihdr_prefix(w: u32, h: u32, depth: u8, color: u8, interlace: u8) -> Vec<u8> {
    let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    v.extend_from_slice(&13u32.to_be_bytes());
    v.extend_from_slice(b"IHDR");
    v.extend_from_slice(&w.to_be_bytes());
    v.extend_from_slice(&h.to_be_bytes());
    v.extend_from_slice(&[depth, color, 0, 0, interlace]);
    v.extend_from_slice(&[0; 4]); // CRC, skipped
    v
}

fn decode_err(data: &[u8]) -> Error {
    let mut scratch = Scratch::new();
    let mut d = SpriteDecoder::new(&mut scratch);
    match d.write(data) {
        Err(e) => e,
        Ok(()) => d.finish(BG).expect_err("expected a decode error"),
    }
}

#[test]
fn truncation_at_every_256_byte_boundary_is_a_clean_error() {
    let mut scratch = Scratch::new();
    for name in LOGOS {
        let data = logo_bytes(name);
        for cut in (0..data.len()).step_by(256) {
            let mut d = SpriteDecoder::new(&mut scratch);
            // A prefix of a valid file can never itself be invalid…
            d.write(&data[..cut])
                .unwrap_or_else(|e| panic!("{name} cut at {cut}: write errored {e:?}"));
            // …but finishing early must fail cleanly (IEND is required).
            let err = d.finish(BG).expect_err("finish on truncated input");
            assert_eq!(err, Error::Truncated, "{name} cut at {cut}");
        }
    }
}

#[test]
fn corrupt_idat_byte_is_detected() {
    // CRCs are skipped by design; corruption still surfaces through the
    // deflate framing or the zlib adler32 over the pixel bytes.
    for name in LOGOS {
        let mut data = logo_bytes(name);
        let (off, len) = first_idat_payload(&data);
        data[off + len / 2] ^= 0xA5;
        let err = decode_err(&data);
        assert!(
            matches!(err, Error::Deflate | Error::Malformed | Error::Truncated),
            "{name}: corrupt IDAT gave {err:?}"
        );
    }
}

#[test]
fn oversized_dimensions_rejected() {
    assert_eq!(decode_err(&ihdr_prefix(2048, 16, 8, 6, 0)), Error::TooLarge);
    assert_eq!(decode_err(&ihdr_prefix(16, 2048, 8, 6, 0)), Error::TooLarge);
    assert_eq!(decode_err(&ihdr_prefix(1025, 1, 8, 6, 0)), Error::TooLarge);
    assert_eq!(
        decode_err(&ihdr_prefix(u32::MAX, u32::MAX, 8, 6, 0)),
        Error::TooLarge
    );
    assert_eq!(decode_err(&ihdr_prefix(0, 16, 8, 6, 0)), Error::Malformed);
    assert_eq!(decode_err(&ihdr_prefix(16, 0, 8, 6, 0)), Error::Malformed);
}

#[test]
fn out_of_scope_formats_are_unsupported() {
    // 16-bit.
    assert_eq!(decode_err(&ihdr_prefix(16, 16, 16, 6, 0)), Error::Unsupported);
    // Palette, grayscale, grayscale+alpha.
    assert_eq!(decode_err(&ihdr_prefix(16, 16, 8, 3, 0)), Error::Unsupported);
    assert_eq!(decode_err(&ihdr_prefix(16, 16, 8, 0, 0)), Error::Unsupported);
    assert_eq!(decode_err(&ihdr_prefix(16, 16, 8, 4, 0)), Error::Unsupported);
    // Adam7.
    assert_eq!(decode_err(&ihdr_prefix(16, 16, 8, 6, 1)), Error::Unsupported);
    // Nonsense color type / interlace are malformed, not merely unsupported.
    assert_eq!(decode_err(&ihdr_prefix(16, 16, 8, 7, 0)), Error::Malformed);
    assert_eq!(decode_err(&ihdr_prefix(16, 16, 8, 6, 2)), Error::Malformed);
}

#[test]
fn out_of_scope_real_encodings_are_unsupported() {
    // Through the real encoder, not a handcrafted header.
    let gray: Vec<u8> = (0..64u32 * 64).map(|i| i as u8).collect();
    let data = encode_png(64, 64, png::ColorType::Grayscale, png::Filter::Adaptive, &gray);
    assert_eq!(decode_err(&data), Error::Unsupported);

    let px = gradient_rgba(1025, 1);
    let data = encode_png(1025, 1, png::ColorType::Rgba, png::Filter::Adaptive, &px);
    assert_eq!(decode_err(&data), Error::TooLarge);
}

#[test]
fn framing_violations_are_malformed() {
    // First chunk not IHDR.
    let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    v.extend_from_slice(&0u32.to_be_bytes());
    v.extend_from_slice(b"gAMA");
    assert_eq!(decode_err(&v), Error::Malformed);

    // IHDR with the wrong length.
    let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    v.extend_from_slice(&12u32.to_be_bytes());
    v.extend_from_slice(b"IHDR");
    assert_eq!(decode_err(&v), Error::Malformed);

    // Chunk length ≥ 2³¹.
    let mut v = ihdr_prefix(16, 16, 8, 6, 0);
    v.extend_from_slice(&0x8000_0000u32.to_be_bytes());
    v.extend_from_slice(b"IDAT");
    assert_eq!(decode_err(&v), Error::Malformed);

    // A second IHDR.
    let mut v = ihdr_prefix(16, 16, 8, 6, 0);
    v.extend_from_slice(&13u32.to_be_bytes());
    v.extend_from_slice(b"IHDR");
    assert_eq!(decode_err(&v), Error::Malformed);
}

#[test]
fn bad_signature_poisons_and_repeats() {
    let mut scratch = Scratch::new();
    let mut d = SpriteDecoder::new(&mut scratch);
    assert_eq!(d.write(b"\x89PNX"), Err(Error::Signature));
    // Poisoned: same error again, and finish reports it too.
    assert_eq!(d.write(b"more"), Err(Error::Signature));
    assert_eq!(d.finish(BG), Err(Error::Signature));
}

#[test]
fn bytes_after_iend_are_ignored() {
    let mut scratch = Scratch::new();
    let data = logo_bytes("bos-100.png");
    let clean = sprite_whole(&mut scratch, &data, BG).expect("clean");

    let mut noisy = data.clone();
    noisy.extend_from_slice(b"HTTP trailers or whatever the socket had left");
    let with_tail = sprite_whole(&mut scratch, &noisy, BG).expect("tail ignored");
    assert!(clean == with_tail);
}

#[test]
fn random_garbage_never_panics() {
    let mut scratch = Scratch::new();
    let mut rng = XorShift(0x0BAD_F00D);
    for _ in 0..200 {
        let len = (rng.next_u64() % 4096) as usize;
        let buf: Vec<u8> = (0..len).map(|_| rng.next_u64() as u8).collect();
        let mut d = SpriteDecoder::new(&mut scratch);
        let mut r = d.write(&buf);
        if r.is_ok() {
            r = d.finish(BG).map(|_| ());
        }
        // Garbage essentially always errors; what matters is that it
        // returns rather than panics.
        let _ = r;
    }
}

#[test]
fn mutated_real_logos_never_panic() {
    let mut scratch = Scratch::new();
    let base = logo_bytes("mlb-500-bos.png");
    let mut rng = XorShift(0xFEED_FACE);
    for _ in 0..100 {
        let mut data = base.clone();
        for _ in 0..1 + (rng.next_u64() % 8) {
            let i = (rng.next_u64() as usize) % data.len();
            data[i] ^= rng.next_u64() as u8;
        }
        let mut d = SpriteDecoder::new(&mut scratch);
        let mut r = d.write(&data);
        if r.is_ok() {
            r = d.finish(BG).map(|_| ());
        }
        let _ = r; // any Result, no panic
    }
}

#[test]
fn truncated_mid_write_split_across_chunks_never_panics() {
    // Truncation with 1-byte feeds — the state machine's worst case.
    let data = logo_bytes("nyy-100.png");
    let mut scratch = Scratch::new();
    for cut in (0..data.len()).step_by(509) {
        let mut d = SpriteDecoder::new(&mut scratch);
        for b in &data[..cut] {
            d.write(std::slice::from_ref(b)).expect("prefix write");
        }
        assert_eq!(d.finish(BG), Err(Error::Truncated));
    }
}
