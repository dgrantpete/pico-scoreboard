//! A minimal PNG writer for the parity harness's diff artifacts.
//!
//! Truecolor, 8 bits per channel, no filtering, and a zlib stream built from
//! *stored* (uncompressed) deflate blocks — a valid PNG that any viewer opens,
//! in about sixty lines and with no dependency. The files are only written when
//! a frame mismatches, so their size does not matter; being able to look at one
//! without adding an image crate to a `no_std` renderer's dev-dependencies does.

#![allow(dead_code)]

use std::io::Write;
use std::path::Path;

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
/// The largest payload a single stored deflate block can carry.
const STORED_MAX: usize = 0xFFFF;

/// Write `rgb` (row-major, 3 bytes per pixel) as a PNG at `path`.
pub fn write_rgb(path: &Path, width: usize, height: usize, rgb: &[u8]) -> std::io::Result<()> {
    assert_eq!(
        rgb.len(),
        width * height * 3,
        "pixel buffer is not width*height*3"
    );

    let mut out = Vec::new();
    out.extend_from_slice(&SIGNATURE);

    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&(width as u32).to_be_bytes());
    header.extend_from_slice(&(height as u32).to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]); // depth 8, truecolor, no interlace
    chunk(&mut out, b"IHDR", &header);

    // Each scanline is prefixed with its filter type; 0 is "none".
    let mut raw = Vec::with_capacity(height * (1 + width * 3));
    for row in rgb.chunks_exact(width * 3) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    chunk(&mut out, b"IDAT", &zlib(&raw));
    chunk(&mut out, b"IEND", &[]);

    std::fs::File::create(path)?.write_all(&out)
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = Crc::new();
    crc.update(kind);
    crc.update(data);
    out.extend_from_slice(&crc.finish().to_be_bytes());
}

/// A zlib stream whose deflate payload is nothing but stored blocks.
fn zlib(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01]; // deflate, 32 KiB window, fastest
    let mut blocks = data.chunks(STORED_MAX).peekable();
    if data.is_empty() {
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0xFF, 0xFF]);
    }
    while let Some(block) = blocks.next() {
        out.push(u8::from(blocks.peek().is_none())); // BFINAL, BTYPE = stored
        let len = block.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(block);
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn adler32(data: &[u8]) -> u32 {
    let (mut low, mut high) = (1u32, 0u32);
    for &byte in data {
        low = (low + byte as u32) % 65521;
        high = (high + low) % 65521;
    }
    (high << 16) | low
}

struct Crc(u32);

impl Crc {
    fn new() -> Self {
        Crc(0xFFFF_FFFF)
    }

    fn update(&mut self, data: &[u8]) {
        for &byte in data {
            self.0 ^= byte as u32;
            for _ in 0..8 {
                let mask = (self.0 & 1).wrapping_neg();
                self.0 = (self.0 >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
    }

    fn finish(self) -> u32 {
        !self.0
    }
}
