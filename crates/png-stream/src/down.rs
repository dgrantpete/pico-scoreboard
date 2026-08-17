//! Box-filter downsampler: full-resolution rows → 24×24 premultiplied
//! accumulators → RGB565 blend. Integer-only; every bound is cited in the
//! crate doc's downsample section.

use crate::decode::RowSink;
use crate::{Rgb8, Sprite, MAX_DIM, SPRITE_DIM, SPRITE_PIXELS};

pub(crate) struct Down<'a> {
    /// `[Σa·r, Σa·g, Σa·b, Σa]` per cell, row-major.
    acc: &'a mut [u32; 4 * SPRITE_PIXELS],
    /// Source column → sprite column (floor mapping), filled at `start`.
    col_map: &'a mut [u8; MAX_DIM],
    /// Source columns/rows landing in each sprite column/row. Max
    /// ⌈1024/24⌉ = 43, so `u16` is generous.
    col_cnt: [u16; SPRITE_DIM],
    row_cnt: [u16; SPRITE_DIM],
    height: u32,
    channels: u8,
}

impl<'a> Down<'a> {
    pub(crate) fn new(
        acc: &'a mut [u32; 4 * SPRITE_PIXELS],
        col_map: &'a mut [u8; MAX_DIM],
    ) -> Self {
        acc.fill(0);
        col_map.fill(0);
        Down {
            acc,
            col_map,
            col_cnt: [0; SPRITE_DIM],
            row_cnt: [0; SPRITE_DIM],
            height: 0,
            channels: 0,
        }
    }

    /// Divide, blend over `bg`, pack RGB565. For each cell holding `n`
    /// source pixels: `out = (Σa·c + bg·(255n − Σa) + 255n/2) / (255n)` —
    /// premultiplied average blended over the background in one integer
    /// division, round-to-nearest. `Σa ≤ 255n` by construction, and the
    /// numerator peaks ≈ 2.4×10⁸ (crate doc), inside u32.
    pub(crate) fn finish(self, bg: Rgb8) -> Sprite {
        let mut out: Sprite = [0; SPRITE_PIXELS];
        for cy in 0..SPRITE_DIM {
            for cx in 0..SPRITE_DIM {
                let cell = cy * SPRITE_DIM + cx;
                let n = self.col_cnt[cx] as u32 * self.row_cnt[cy] as u32;
                let (r, g, b) = if n == 0 {
                    // No source pixels map here (image narrower/shorter
                    // than 24) — pure background.
                    (bg.r, bg.g, bg.b)
                } else {
                    let a = &self.acc[cell * 4..cell * 4 + 4];
                    let denom = 255 * n;
                    let inv_a = denom - a[3];
                    let blend = |sum_ac: u32, bg_c: u8| -> u8 {
                        ((sum_ac + bg_c as u32 * inv_a + denom / 2) / denom) as u8
                    };
                    (blend(a[0], bg.r), blend(a[1], bg.g), blend(a[2], bg.b))
                };
                out[cell] = pack565(r, g, b);
            }
        }
        out
    }
}

impl RowSink for Down<'_> {
    fn start(&mut self, width: u32, height: u32, channels: u8) {
        self.height = height;
        self.channels = channels;
        // Floor mapping (crate doc): cell = coord·24/dim. coord·24 ≤
        // 1023·24, far inside u32.
        for x in 0..width as usize {
            let c = (x as u32 * SPRITE_DIM as u32 / width) as u8;
            self.col_map[x] = c;
            self.col_cnt[c as usize] += 1;
        }
        for y in 0..height {
            self.row_cnt[(y * SPRITE_DIM as u32 / height) as usize] += 1;
        }
    }

    fn row(&mut self, y: u32, px: &[u8]) {
        let base = (y * SPRITE_DIM as u32 / self.height) as usize * SPRITE_DIM;
        if self.channels == 4 {
            for (x, p) in px.chunks_exact(4).enumerate() {
                let a = p[3] as u32;
                if a == 0 {
                    continue; // fully transparent adds nothing
                }
                let cell = (base + self.col_map[x] as usize) * 4;
                self.acc[cell] += a * p[0] as u32;
                self.acc[cell + 1] += a * p[1] as u32;
                self.acc[cell + 2] += a * p[2] as u32;
                self.acc[cell + 3] += a;
            }
        } else {
            // RGB: opaque, a = 255.
            for (x, p) in px.chunks_exact(3).enumerate() {
                let cell = (base + self.col_map[x] as usize) * 4;
                self.acc[cell] += 255 * p[0] as u32;
                self.acc[cell + 1] += 255 * p[1] as u32;
                self.acc[cell + 2] += 255 * p[2] as u32;
                self.acc[cell + 3] += 255;
            }
        }
    }
}

/// 8-bit channels → RGB565, round-to-nearest rescale.
fn pack565(r: u8, g: u8, b: u8) -> u16 {
    let r5 = (r as u16 * 31 + 127) / 255;
    let g6 = (g as u16 * 63 + 127) / 255;
    let b5 = (b as u16 * 31 + 127) / 255;
    (r5 << 11) | (g6 << 5) | b5
}
