//! Shared test plumbing: the six-real-logo corpus, feed helpers, the
//! `png`-crate oracle, synthetic-fixture encoding, and a seeded RNG so
//! every "random" test is deterministic.
#![allow(dead_code)]

use png_stream::{Error, Rgb8, RowSink, Scratch, Sprite, SpriteDecoder};
use std::path::PathBuf;

/// The six real ESPN CDN logos the format facts were measured from.
pub const LOGOS: [&str; 6] = [
    "bos-100.png",
    "mlb-500-bos.png",
    "mlb-500-nyy.png",
    "ncaa-500-2294.png",
    "nfl-500-kc.png",
    "nyy-100.png",
];

pub fn logo_bytes(name: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// xorshift64* — deterministic, seedable, no external dep.
pub struct XorShift(pub u64);

impl XorShift {
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform-ish in `1..=max`.
    pub fn next_len(&mut self, max: usize) -> usize {
        (self.next_u64() as usize % max) + 1
    }
}

/// Decode in one whole-buffer write.
pub fn sprite_whole(scratch: &mut Scratch, data: &[u8], bg: Rgb8) -> Result<Sprite, Error> {
    let mut d = SpriteDecoder::new(scratch);
    d.write(data)?;
    d.finish(bg)
}

/// Decode feeding `chunk`-sized slices (1 = the byte-at-a-time worst case).
pub fn sprite_fixed_chunks(
    scratch: &mut Scratch,
    data: &[u8],
    chunk: usize,
    bg: Rgb8,
) -> Result<Sprite, Error> {
    let mut d = SpriteDecoder::new(scratch);
    for c in data.chunks(chunk) {
        d.write(c)?;
    }
    d.finish(bg)
}

/// Decode feeding seeded-random-sized slices (1..=97 bytes each).
pub fn sprite_random_chunks(
    scratch: &mut Scratch,
    data: &[u8],
    seed: u64,
    bg: Rgb8,
) -> Result<Sprite, Error> {
    let mut d = SpriteDecoder::new(scratch);
    let mut rng = XorShift(seed);
    let mut pos = 0;
    while pos < data.len() {
        let n = rng.next_len(97).min(data.len() - pos);
        d.write(&data[pos..pos + n])?;
        pos += n;
    }
    d.finish(bg)
}

/// Collects the full-resolution decode for oracle comparison.
#[derive(Default)]
pub struct CollectSink {
    pub width: u32,
    pub height: u32,
    pub channels: u8,
    pub rows: u32,
    pub data: Vec<u8>,
}

impl RowSink for CollectSink {
    fn start(&mut self, width: u32, height: u32, channels: u8) {
        self.width = width;
        self.height = height;
        self.channels = channels;
        self.data
            .reserve(width as usize * height as usize * channels as usize);
    }

    fn row(&mut self, y: u32, px: &[u8]) {
        assert_eq!(y, self.rows, "rows must arrive in order");
        assert_eq!(px.len(), self.width as usize * self.channels as usize);
        self.data.extend_from_slice(px);
        self.rows += 1;
    }
}

/// The `png` crate's decode of the same bytes: (width, height, channels,
/// raw pixel bytes).
pub fn oracle_decode(data: &[u8]) -> (u32, u32, u8, Vec<u8>) {
    let decoder = png::Decoder::new(std::io::Cursor::new(data));
    let mut reader = decoder.read_info().expect("oracle read_info");
    let mut buf = vec![0u8; reader.output_buffer_size().expect("oracle size")];
    let info = reader.next_frame(&mut buf).expect("oracle frame");
    assert_eq!(info.bit_depth, png::BitDepth::Eight);
    let channels = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        other => panic!("oracle: unexpected color type {other:?}"),
    };
    buf.truncate(info.buffer_size());
    (info.width, info.height, channels, buf)
}

/// Encode a synthetic PNG through the `png` crate with a forced filter.
pub fn encode_png(
    width: u32,
    height: u32,
    color: png::ColorType,
    filter: png::Filter,
    pixels: &[u8],
) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(std::io::Cursor::new(&mut out), width, height);
        enc.set_color(color);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_filter(filter);
        let mut w = enc.write_header().expect("encode header");
        w.write_image_data(pixels).expect("encode data");
    }
    out
}

/// A deterministic RGBA gradient with a transparency ramp — exercises
/// every channel and partial alpha.
pub fn gradient_rgba(width: u32, height: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            v.push((x * 7 + y) as u8);
            v.push((y * 5 + x * 3) as u8);
            v.push((x ^ y) as u8);
            v.push((x * 255 / width.max(1)) as u8);
        }
    }
    v
}

/// A deterministic RGB gradient.
pub fn gradient_rgb(width: u32, height: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity((width * height * 3) as usize);
    for y in 0..height {
        for x in 0..width {
            v.push((x * 11 + y * 2) as u8);
            v.push((x + y * 13) as u8);
            v.push(((x * 3) ^ (y * 5)) as u8);
        }
    }
    v
}

/// Same math as the crate's pack: expected RGB565 of an opaque color, and
/// therefore of the background anywhere coverage is zero.
pub fn pack565(r: u8, g: u8, b: u8) -> u16 {
    let r5 = (r as u16 * 31 + 127) / 255;
    let g6 = (g as u16 * 63 + 127) / 255;
    let b5 = (b as u16 * 31 + 127) / 255;
    (r5 << 11) | (g6 << 5) | b5
}

pub fn bg565(bg: Rgb8) -> u16 {
    pack565(bg.r, bg.g, bg.b)
}

/// Byte offset and length of the first IDAT chunk's payload.
pub fn first_idat_payload(data: &[u8]) -> (usize, usize) {
    let mut pos = 8;
    loop {
        let len = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        let kind = &data[pos + 4..pos + 8];
        if kind == b"IDAT" {
            return (pos + 8, len);
        }
        pos += 12 + len; // header + payload + CRC
        assert!(pos < data.len(), "no IDAT found");
    }
}
