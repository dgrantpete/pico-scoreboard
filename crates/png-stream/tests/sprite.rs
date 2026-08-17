//! Sprite-level sanity on the six real logos, plus PPM dumps for eyeball
//! review (tests/out/*.ppm — the `-x10.ppm` files are nearest-neighbor
//! blowups of the same pixels).

mod common;

use common::*;
use png_stream::{Rgb8, Scratch, Sprite, SPRITE_DIM};
use std::io::Write as _;
use std::path::PathBuf;

const BLACK: Rgb8 = Rgb8::new(0, 0, 0);

fn out_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/out");
    std::fs::create_dir_all(&d).expect("create tests/out");
    d
}

fn sprite_to_rgb(sprite: &Sprite) -> Vec<u8> {
    let mut v = Vec::with_capacity(sprite.len() * 3);
    for &px in sprite {
        let r5 = (px >> 11) & 0x1F;
        let g6 = (px >> 5) & 0x3F;
        let b5 = px & 0x1F;
        v.push(((r5 * 255 + 15) / 31) as u8);
        v.push(((g6 * 255 + 31) / 63) as u8);
        v.push(((b5 * 255 + 15) / 31) as u8);
    }
    v
}

fn write_ppm(path: &PathBuf, w: usize, h: usize, rgb: &[u8]) {
    let mut f = std::fs::File::create(path).expect("create ppm");
    write!(f, "P6\n{w} {h}\n255\n").unwrap();
    f.write_all(rgb).unwrap();
}

fn dump(name: &str, sprite: &Sprite) {
    let rgb = sprite_to_rgb(sprite);
    let stem = name.trim_end_matches(".png");
    write_ppm(
        &out_dir().join(format!("{stem}.ppm")),
        SPRITE_DIM,
        SPRITE_DIM,
        &rgb,
    );
    // ×10 nearest-neighbor blowup, same pixels.
    const SCALE: usize = 10;
    let big_dim = SPRITE_DIM * SCALE;
    let mut big = vec![0u8; big_dim * big_dim * 3];
    for y in 0..big_dim {
        for x in 0..big_dim {
            let src = ((y / SCALE) * SPRITE_DIM + (x / SCALE)) * 3;
            let dst = (y * big_dim + x) * 3;
            big[dst..dst + 3].copy_from_slice(&rgb[src..src + 3]);
        }
    }
    write_ppm(
        &out_dir().join(format!("{stem}-x10.ppm")),
        big_dim,
        big_dim,
        &big,
    );
}

/// True iff every source pixel mapping to sprite cell (cx, cy) is fully
/// transparent — computed from the oracle decode, so the corner assertion
/// tests our blend, not an assumption about the artwork.
fn cell_fully_transparent(data: &[u8], cx: usize, cy: usize) -> bool {
    let (w, h, ch, px) = oracle_decode(data);
    if ch != 4 {
        return false;
    }
    let (w, h) = (w as usize, h as usize);
    for y in (0..h).filter(|y| y * SPRITE_DIM / h == cy) {
        for x in (0..w).filter(|x| x * SPRITE_DIM / w == cx) {
            if px[(y * w + x) * 4 + 3] != 0 {
                return false;
            }
        }
    }
    true
}

#[test]
fn corners_are_background_and_centers_are_ink() {
    let mut scratch = Scratch::new();
    let teal = Rgb8::from_rgb888(0x12_34_56);
    for name in LOGOS {
        let data = logo_bytes(name);
        for bg in [BLACK, teal] {
            let sprite = sprite_whole(&mut scratch, &data, bg).expect(name);
            let expect = bg565(bg);

            // Corners: the artwork is verified transparent there first.
            for (cx, cy) in [(0, 0), (23, 0), (0, 23), (23, 23)] {
                assert!(
                    cell_fully_transparent(&data, cx, cy),
                    "{name}: source corner ({cx},{cy}) unexpectedly has ink"
                );
                let got = sprite[cy * SPRITE_DIM + cx];
                assert_eq!(got, expect, "{name}: corner ({cx},{cy}) with bg {bg:?}");
            }

        }

        // Ink: judged against a background no sports logo is painted in
        // (ncaa-500-2294 is solid *black* artwork — over a black bg its
        // cells legitimately equal the background, so black can't be the
        // contrast reference).
        let magenta = Rgb8::new(255, 0, 255);
        let sprite = sprite_whole(&mut scratch, &data, magenta).expect(name);
        let expect = bg565(magenta);
        let non_bg = sprite.iter().filter(|&&p| p != expect).count();
        assert!(non_bg >= 150, "{name}: only {non_bg} non-background cells");
        let central = (8..16)
            .flat_map(|cy| (8..16).map(move |cx| sprite[cy * SPRITE_DIM + cx]))
            .filter(|&p| p != expect)
            .count();
        assert!(
            central >= 16,
            "{name}: central 8×8 has only {central} inked cells"
        );
    }
}

#[test]
fn dump_ppms_for_eyeball_review() {
    let mut scratch = Scratch::new();
    for name in LOGOS {
        let data = logo_bytes(name);
        let sprite = sprite_whole(&mut scratch, &data, BLACK).expect(name);
        dump(name, &sprite);
    }
}

#[test]
fn opaque_rgb_source_ignores_background() {
    // Fully opaque input: the background must not leak into any cell.
    let (w, h) = (96, 96);
    let px = gradient_rgb(w, h);
    let data = encode_png(w, h, png::ColorType::Rgb, png::Filter::Adaptive, &px);
    let mut scratch = Scratch::new();
    let a = sprite_whole(&mut scratch, &data, BLACK).expect("rgb black");
    let b = sprite_whole(&mut scratch, &data, Rgb8::new(255, 0, 255)).expect("rgb magenta");
    assert!(a == b, "background leaked into an opaque image");
}

#[test]
fn box_average_is_exact_on_a_uniform_image() {
    // A constant-color, constant-alpha image must come out as the exact
    // blend everywhere, independent of cell pixel counts.
    let (w, h) = (50, 50); // 50 → cells of 2 and 3 source pixels
    let (r, g, b, a) = (200u8, 40u8, 90u8, 128u8);
    let px: Vec<u8> = (0..w * h).flat_map(|_| [r, g, b, a]).collect();
    let data = encode_png(w, h, png::ColorType::Rgba, png::Filter::Adaptive, &px);
    let bg = Rgb8::new(10, 20, 30);
    let mut scratch = Scratch::new();
    let sprite = sprite_whole(&mut scratch, &data, bg).expect("uniform");
    let blend = |c: u8, bgc: u8| -> u8 {
        ((a as u32 * c as u32 + (255 - a as u32) * bgc as u32 + 127) / 255) as u8
    };
    let expect = pack565(blend(r, bg.r), blend(g, bg.g), blend(b, bg.b));
    for (i, &cell) in sprite.iter().enumerate() {
        assert_eq!(cell, expect, "cell {i}");
    }
}

#[test]
fn images_smaller_than_the_grid_leave_empty_cells_as_background() {
    let (w, h) = (3, 3);
    let px = gradient_rgba(w, h);
    let data = encode_png(w, h, png::ColorType::Rgba, png::Filter::NoFilter, &px);
    let mut scratch = Scratch::new();
    let bg = Rgb8::new(7, 77, 177);
    let sprite = sprite_whole(&mut scratch, &data, bg).expect("3x3");
    let expect = bg565(bg);
    // 3 source columns land in sprite columns 0, 8, 16 — everything else
    // is pure background.
    let mut bg_cells = 0;
    for cy in 0..SPRITE_DIM {
        for cx in 0..SPRITE_DIM {
            let hit = [0, 8, 16].contains(&cx) && [0, 8, 16].contains(&cy);
            if !hit {
                assert_eq!(sprite[cy * SPRITE_DIM + cx], expect, "cell ({cx},{cy})");
                bg_cells += 1;
            }
        }
    }
    assert_eq!(bg_cells, SPRITE_DIM * SPRITE_DIM - 9);
}
