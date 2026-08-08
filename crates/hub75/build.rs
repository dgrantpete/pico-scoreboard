use std::env;
use std::fmt::Write;
use std::fs;
use std::path::PathBuf;

/// sRGB EOTF LUT (IEC 61966-2-1), same formula as driver.py's
/// `_create_gamma_lut`. Computed here so the production gamma table is const
/// data in flash rather than 256 float pows at init. `x` must be built as
/// `i * (1/255)`, not `i / 255` — driver.py multiplies by a precomputed
/// reciprocal and the two differ by an ULP for some inputs. No entry lands
/// near a rounding tie (asserted by tests/gen_goldens.py), so the rounding
/// mode and pow implementation cannot change the table.
fn srgb_lut() -> [u8; 256] {
    let inv_max = 1.0f64 / 255.0;
    let mut lut = [0u8; 256];
    for (i, slot) in lut.iter_mut().enumerate() {
        let x = i as f64 * inv_max;
        let linear = if x <= 0.04045 {
            x / 12.92
        } else {
            ((x + 0.055) / 1.055).powf(2.4)
        };
        *slot = (255.0 * linear).round_ties_even() as u8;
    }
    lut
}

fn main() {
    let mut out = String::from("pub(crate) const SRGB_LUT: [u8; 256] = [\n");
    for chunk in srgb_lut().chunks(16) {
        out.push_str("    ");
        for value in chunk {
            write!(out, "{value}, ").unwrap();
        }
        out.push('\n');
    }
    out.push_str("];\n");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::write(out_dir.join("srgb_lut.rs"), out).unwrap();
    println!("cargo::rerun-if-changed=build.rs");
}
