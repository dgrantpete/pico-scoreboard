//! The ambient-light curve: lux in, panel brightness out.
//!
//! Port of `scoreboard/brightness.py`, which is already four pure functions and
//! no state — so this is a transcription, and the tests below pin it against
//! values computed by running the Python.
//!
//! The chain, once per [`TICK_MS`]:
//!
//! 1. [`smooth_lux`] — an exponential moving average over the raw reading. A
//!    VEML7700 pointed at a room with a television in it is noisy, and the panel
//!    following that noise is the thing people notice.
//! 2. [`lux_to_ambient`] — a **log-scale** map from lux to brightness, because
//!    perceived brightness is logarithmic in luminance. A linear map spends
//!    almost its whole range on the difference between "bright" and "brighter".
//! 3. [`ramp`] — rate-limits the change, so a hand passing over the sensor is a
//!    slow drift rather than a flash.
//! 4. [`apply_preference`] — folds in the user's `display.brightness` setting.
//!
//! # Why `f32`
//!
//! MicroPython's rp2 port is built with single-precision floats, so `f32` is
//! the *closer* parity rather than the cheaper approximation — and it is also
//! what the RP2350's FPU does in hardware. The reference values in the tests
//! come from CPython (double) and are compared within 1e-5, which is two orders
//! of magnitude finer than the 1/255 the result is quantised to before it
//! crosses to core 1.

/// Lux at or below which the panel sits at [`BRI_MIN`].
pub const LUX_MIN: f32 = 2.0;
/// Lux at or above which the panel sits at [`BRI_MAX`].
pub const LUX_MAX: f32 = 300.0;
/// Never fully black: a scoreboard that reads as "off" in a dark room is a
/// scoreboard people think has crashed.
pub const BRI_MIN: f32 = 0.05;
pub const BRI_MAX: f32 = 1.0;
/// Lux smoothing. Lower is slower and less flickery.
pub const EMA_ALPHA: f32 = 0.08;

/// The auto-brightness tick. 5 Hz.
pub const TICK_MS: u64 = 200;
/// The most the brightness may move per second.
pub const RAMP_PER_SECOND: f32 = 0.2;
/// Per-tick step, **derived** from [`TICK_MS`] rather than written out, so the
/// ramp stays expressed in real units if the tick rate ever changes. 0.04 at
/// 5 Hz.
pub const RAMP_STEP: f32 = RAMP_PER_SECOND * TICK_MS as f32 / 1000.0;

/// `ln(LUX_MAX / LUX_MIN)` — the denominator that normalises the log map.
/// Not a `const` because `logf` is not `const`; the compiler folds it anyway.
fn log_range() -> f32 {
    libm::logf(LUX_MAX / LUX_MIN)
}

/// One EMA step.
pub fn smooth_lux(current: f32, reading: f32) -> f32 {
    current + EMA_ALPHA * (reading - current)
}

/// Log-scale map from lux to ambient brightness, clamped to
/// `[BRI_MIN, BRI_MAX]`.
pub fn lux_to_ambient(lux: f32) -> f32 {
    let t = libm::logf(lux.max(LUX_MIN) / LUX_MIN) / log_range();
    BRI_MIN + t.clamp(0.0, 1.0) * (BRI_MAX - BRI_MIN)
}

/// Rate-limit a brightness change to [`RAMP_STEP`] per call.
///
/// Returns the target exactly once it is within a step, rather than
/// asymptotically approaching it — which is what keeps a settled panel from
/// rewriting the driver's timing stream forever over a difference nobody can
/// see.
pub fn ramp(current: f32, target: f32) -> f32 {
    let delta = target - current;
    if delta > RAMP_STEP {
        return current + RAMP_STEP;
    }
    if delta < -RAMP_STEP {
        return current - RAMP_STEP;
    }
    target
}

/// Fold the user's preference into the ambient reading — a dual lerp.
///
/// `0` is [`BRI_MIN`], `50` is pure auto, `100` is [`BRI_MAX`]. The two halves
/// are separate lerps rather than one curve because the midpoint has to be
/// *exactly* the ambient value: 50 means "the sensor decides", and a single
/// smooth curve through three points would leave the default setting quietly
/// biased one way.
pub fn apply_preference(ambient: f32, user_preference: u8) -> f32 {
    let preference = user_preference as f32;
    if user_preference <= 50 {
        let blend = preference / 50.0;
        return BRI_MIN + blend * (ambient - BRI_MIN);
    }
    let blend = (preference - 50.0) / 50.0;
    ambient + blend * (BRI_MAX - ambient)
}

/// The whole chain's state: the smoothed lux and the ramped ambient level.
///
/// `main.py` kept these as three locals of `auto_brightness_loop`
/// (`smoothed_lux`, `ambient_bri`, `initialized`). They are a struct here so
/// the loop is a call and the arithmetic is testable end to end — including the
/// one rule that is not in `brightness.py` at all and matters most in practice:
/// **with no reading ever taken, assume a bright room.** A device whose sensor
/// is absent or broken must not dim itself to 5 %, which is what falling back to
/// the bottom of the curve would do.
#[derive(Debug, Clone, Copy)]
pub struct AutoBrightness {
    smoothed_lux: f32,
    ambient: f32,
    /// Whether any reading has ever landed. Not "is the sensor healthy" — a
    /// sensor that worked and then failed keeps its last smoothed value and
    /// rides it, which is the same thing `main.py` did by simply not updating.
    seeded: bool,
}

impl AutoBrightness {
    /// Start from the configured preference, as `main.py` did: the first tick
    /// then ramps from there rather than from black.
    pub fn new(user_preference: u8) -> AutoBrightness {
        AutoBrightness {
            smoothed_lux: 0.0,
            ambient: user_preference as f32 / 100.0,
            seeded: false,
        }
    }

    /// One tick. `lux` is `None` while the sensor is unavailable.
    pub fn tick(&mut self, lux: Option<f32>, user_preference: u8) -> f32 {
        if let Some(lux) = lux {
            self.smoothed_lux = if self.seeded {
                smooth_lux(self.smoothed_lux, lux)
            } else {
                self.seeded = true;
                lux
            };
        }
        let target = if self.seeded {
            lux_to_ambient(self.smoothed_lux)
        } else {
            BRI_MAX
        };
        self.ambient = ramp(self.ambient, target);
        apply_preference(self.ambient, user_preference)
    }

    /// The value as the panel takes it: 0..=255, which is what
    /// `display_core1::BRIGHTNESS` carries.
    pub fn quantize(level: f32) -> u8 {
        (level.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two orders of magnitude finer than the 1/255 quantisation step.
    const EPSILON: f32 = 1e-5;

    #[track_caller]
    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() <= EPSILON,
            "{actual} is not {expected}"
        );
    }

    #[track_caller]
    fn assert_close_at(actual: f32, expected: f32, context: core::fmt::Arguments) {
        assert!(
            (actual - expected).abs() <= EPSILON,
            "{actual} is not {expected} ({context})"
        );
    }

    #[test]
    fn the_ramp_step_is_derived_and_lands_on_the_documented_value() {
        assert_close(RAMP_STEP, 0.04);
        assert_close(log_range(), 5.010_635_3);
    }

    /// Reference values from `firmware/src/scoreboard/brightness.py` under
    /// CPython. These are the parity check: a curve that disagrees here is a
    /// panel that is visibly a different brightness from the MicroPython unit
    /// standing next to it.
    ///
    /// CPython computes in double and prints, at nine places,
    /// `0.126874853 / 0.355144144 / 0.660288288 / 0.868581570`; the literals
    /// below are those rounded to the nearest `f32`, which is as close as this
    /// type can hold them and still 300× finer than [`EPSILON`].
    #[test]
    fn lux_to_ambient_matches_the_python_curve() {
        for (lux, expected) in [
            (0.0, 0.05),
            (0.5, 0.05),
            (2.0, 0.05),
            (3.0, 0.126_874_85),
            (10.0, 0.355_144_14),
            (50.0, 0.660_288_3),
            (150.0, 0.868_581_6),
            (300.0, 1.0),
            (1000.0, 1.0),
            (12000.0, 1.0),
        ] {
            assert_close_at(lux_to_ambient(lux), expected, format_args!("at {lux} lux"));
        }
    }

    #[test]
    fn smooth_lux_matches_the_python_ema() {
        for (current, reading, expected) in [
            (0.0, 100.0, 8.0),
            (100.0, 0.0, 92.0),
            (50.0, 50.0, 50.0),
            (10.0, 12.5, 10.2),
        ] {
            assert_close(smooth_lux(current, reading), expected);
        }
    }

    #[test]
    fn ramp_matches_the_python_rate_limit() {
        for (current, target, expected) in [
            (0.0, 1.0, 0.04),
            (1.0, 0.0, 0.96),
            (0.5, 0.52, 0.52),
            (0.5, 0.5, 0.5),
            (0.5, 0.53, 0.53),
            (0.5, 0.47, 0.47),
        ] {
            assert_close_at(
                ramp(current, target),
                expected,
                format_args!("{current} -> {target}"),
            );
        }
    }

    #[test]
    fn apply_preference_matches_the_python_dual_lerp() {
        for (ambient, preference, expected) in [
            (0.5, 0, 0.05),
            (0.5, 25, 0.275),
            (0.5, 50, 0.5),
            (0.5, 75, 0.75),
            (0.5, 100, 1.0),
            (0.05, 0, 0.05),
            (1.0, 100, 1.0),
            (0.3, 10, 0.1),
            (0.8, 90, 0.96),
        ] {
            assert_close_at(
                apply_preference(ambient, preference),
                expected,
                format_args!("ambient {ambient}, preference {preference}"),
            );
        }
    }

    #[test]
    fn fifty_is_exactly_the_ambient_value_at_every_level() {
        for ambient in [0.05, 0.2, 0.5, 0.87, 1.0] {
            assert_close(apply_preference(ambient, 50), ambient);
        }
    }

    #[test]
    fn a_sensor_that_never_answers_assumes_a_bright_room() {
        // main.py:970-975. The alternative — treating "no reading" as the
        // bottom of the curve — gives a device with no sensor a panel at 5 %,
        // which reads as broken.
        let mut auto = AutoBrightness::new(50);
        let mut level = 0.0;
        for _ in 0..200 {
            level = auto.tick(None, 50);
        }
        assert_close(level, BRI_MAX);
    }

    #[test]
    fn the_first_reading_seeds_the_average_rather_than_being_averaged_into_zero() {
        // Without the seed, a device booting in a bright room would take
        // dozens of ticks to climb out of a smoothed lux of 0.
        let mut seeded = AutoBrightness::new(50);
        assert_close(seeded.tick(Some(300.0), 50), ramp(0.5, 1.0));
        let mut unseeded = AutoBrightness::new(50);
        unseeded.smoothed_lux = 0.0;
        unseeded.seeded = true;
        assert!(
            unseeded.tick(Some(300.0), 50) < seeded.tick(Some(300.0), 50),
            "an unseeded average lags a seeded one"
        );
    }

    #[test]
    fn brightness_ramps_rather_than_jumping() {
        let mut auto = AutoBrightness::new(50);
        let first = auto.tick(Some(2.0), 50);
        // Started at preference/100 = 0.5 and the target is BRI_MIN, so the
        // first tick may only move one step.
        assert_close(first, 0.5 - RAMP_STEP);
        let second = auto.tick(Some(2.0), 50);
        assert_close(second, 0.5 - 2.0 * RAMP_STEP);
    }

    #[test]
    fn a_settled_level_stops_moving_exactly_rather_than_approaching_forever() {
        let mut auto = AutoBrightness::new(50);
        let mut previous = f32::NAN;
        for _ in 0..200 {
            previous = auto.tick(Some(2.0), 50);
        }
        assert_close(previous, BRI_MIN);
        assert_close(auto.tick(Some(2.0), 50), BRI_MIN);
    }

    #[test]
    fn quantisation_covers_the_whole_byte_range_and_clamps() {
        assert_eq!(AutoBrightness::quantize(0.0), 0);
        assert_eq!(AutoBrightness::quantize(1.0), 255);
        assert_eq!(AutoBrightness::quantize(0.5), 128);
        assert_eq!(AutoBrightness::quantize(-1.0), 0);
        assert_eq!(AutoBrightness::quantize(2.0), 255);
    }
}
