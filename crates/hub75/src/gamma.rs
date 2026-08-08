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

/// A gamma mode and the finished 256-entry LUT it builds.
///
/// # Why the table is a value and not something the driver derives
///
/// [`Gamma::build_lut`] is cheap for `Srgb` and `Identity` — a copy of a const
/// table — and expensive for `Power`, which is 256 `libm::pow` calls. On the
/// RP2350 that measured **27,562 µs** (BACKLOG 68), which fit inside the 50 ms
/// frame the parity release paced at and does not fit inside a 16.7 ms one.
///
/// The work is not on the render path by nature: a gamma change arrives with a
/// `PUT /api/config`, which core 0 handles. So the table is built where the
/// request lands and the finished 256 bytes cross the core seam, leaving core 1
/// with a `copy_from_slice` inside its frame. [`Hub75Driver::set_gamma`] takes
/// one of these for exactly that reason, and there is no entry point that lets a
/// caller hand the driver a bare [`Gamma`] to expand.
///
/// [`Hub75Driver::set_gamma`]: crate::driver::Hub75Driver::set_gamma
#[derive(Clone, Copy)]
pub struct GammaTable {
    gamma: Gamma,
    lut: [u8; 256],
}

impl GammaTable {
    /// Build the table for `gamma`. **Call this off the render path** — see the
    /// type's docs for the measurement that makes it a rule rather than advice.
    pub fn new(gamma: Gamma) -> GammaTable {
        GammaTable {
            gamma,
            lut: gamma.build_lut(),
        }
    }

    /// The mode this table was built from, so the driver can still report it.
    pub const fn gamma(&self) -> Gamma {
        self.gamma
    }

    pub const fn lut(&self) -> &[u8; 256] {
        &self.lut
    }
}

impl core::fmt::Debug for GammaTable {
    /// The 256 entries are noise in a log line and are fully determined by the
    /// mode, so only the mode is printed.
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("GammaTable")
            .field("gamma", &self.gamma)
            .finish_non_exhaustive()
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
