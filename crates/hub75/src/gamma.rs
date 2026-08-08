//! Gamma correction modes and LUT construction (replaces `gamma.py` +
//! driver.py's `_create_gamma_lut`).

include!(concat!(env!("OUT_DIR"), "/srgb_lut.rs"));

const IDENTITY_LUT: [u8; 256] = {
    let mut lut = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        lut[i] = i as u8;
        i += 1;
    }
    lut
};

/// Gamma correction mode. `Identity` is the Python driver's `gamma=None`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gamma {
    /// sRGB EOTF (IEC 61966-2-1): linear segment below 0.04045, 2.4-power
    /// curve above. The production default.
    Srgb,
    /// `output = input ** value`. Negative exponents clamp to 0.0 and 1.0
    /// short-circuits to identity, matching `gamma.Power` + driver.py.
    Power(f64),
    /// No correction.
    Identity,
}

impl Gamma {
    /// Build the 256-entry channel LUT for this mode.
    ///
    /// `Srgb` and `Identity` come from const tables; `Power` is computed with
    /// `libm::pow`. Halfway rounding ties cannot occur for any real exponent
    /// (tests/gen_goldens.py asserts the margin), so half-even here vs
    /// MicroPython's half-away rounding on device never shows.
    pub fn build_lut(self) -> [u8; 256] {
        match self {
            Gamma::Srgb => SRGB_LUT,
            Gamma::Identity => IDENTITY_LUT,
            Gamma::Power(value) => {
                let value = value.max(0.0);
                if value == 1.0 {
                    return IDENTITY_LUT;
                }
                let inv_max = 1.0f64 / 255.0;
                let mut lut = [0u8; 256];
                for (i, slot) in lut.iter_mut().enumerate() {
                    let x = i as f64 * inv_max;
                    *slot = round_ties_even(255.0 * libm::pow(x, value)) as u8;
                }
                lut
            }
        }
    }
}

/// CPython `round()` semantics (`f64::round_ties_even` is unavailable in
/// `core` without unstable features going through libm anyway).
fn round_ties_even(x: f64) -> f64 {
    let floor = libm::floor(x);
    let frac = x - floor;
    let round_up = frac > 0.5 || (frac == 0.5 && libm::fmod(floor, 2.0) != 0.0);
    if round_up { floor + 1.0 } else { floor }
}
