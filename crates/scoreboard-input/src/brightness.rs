//! The ambient-light curve: lux in, panel duty out.
//!
//! **Not a port.** `brightness.py` fed a log-lux number straight into the
//! panel's duty cycle and folded the user's preference in with a dual lerp;
//! this is a different pipeline on purpose, and the argument for it is in
//! PARITY.md's post-parity divergences. What carries over unchanged is the EMA,
//! the 2–300 lux anchors that feed it, the 200 ms tick, and the rule that a
//! device with no sensor assumes a bright room.
//!
//! The chain, once per [`TICK_MS`]:
//!
//! 1. [`smooth_lux`] — an exponential moving average over the raw reading. A
//!    VEML7700 pointed at a room with a television in it is noisy, and the panel
//!    following that noise is the thing people notice.
//! 2. [`lux_to_perceptual`] — a **log-scale** map from lux to a *perceptual*
//!    brightness `B` in `[0, 1]`: where this room sits between "dark" and "as
//!    bright as the panel needs to care about".
//! 3. [`apply_bias`] — the user's `display.brightness` setting as an **additive
//!    offset on `B`**, clamped.
//! 4. [`ramp`] — rate-limits `B`, **asymmetrically**: fast up, slow down.
//! 5. [`perceptual_to_duty`] — cubes `B` onto the duty cycle the panel takes.
//!
//! # Perception and duty are different quantities, and the old chain conflated them
//!
//! The panel's brightness control is the OE duty cycle, which is very nearly
//! linear in emitted light. The old chain handed it a log-lux number directly,
//! so one curve was doing two jobs: describing how the *room* changes and
//! describing how an *eye* responds. Splitting them is the whole redesign.
//! [`lux_to_perceptual`] answers the first question, [`perceptual_to_duty`] the
//! second, and everything in between — the preference, the ramp — happens in
//! the perceptual middle, where "half as bright" means what a person means by
//! it.
//!
//! # Why `f32`
//!
//! The RP2350's FPU is single precision, so `f32` is what the arithmetic costs
//! nothing in. The reference values in the tests come from evaluating this
//! module's formulas in CPython (double) and are compared within 1e-5, which is
//! two orders of magnitude finer than the 1/255 the result is quantised to
//! before it crosses to core 1.

/// Lux at or below which the room reads as perceptually black.
pub const LUX_MIN: f32 = 2.0;
/// Lux at or above which the room reads as perceptually full-scale. The
/// VEML7700 saturates around 1,900 lux in this driver's fixed gain, which does
/// not matter precisely because the curve stopped moving at 300.
pub const LUX_MAX: f32 = 300.0;

/// The panel's duty floor. Never fully black: a scoreboard that reads as "off"
/// in a dark room is a scoreboard people think has crashed.
///
/// This is the *only* place the floor is applied. `brightness.py` carried it in
/// the lux curve as well, where it was really a duty concern wearing the
/// curve's clothes.
pub const DUTY_MIN: f32 = 0.05;
/// The panel's duty ceiling.
pub const DUTY_MAX: f32 = 1.0;

/// Lux smoothing. Lower is slower and less flickery. Unchanged from
/// `brightness.py` — it was never the problem.
pub const EMA_ALPHA: f32 = 0.08;

/// The auto-brightness tick. 5 Hz.
pub const TICK_MS: u64 = 200;

/// Seconds a full-span brighten takes: the room's lights just came on and the
/// panel is unreadably dim, so this is a readability deadline, not an
/// aesthetic. Fast enough to feel immediate, slow enough not to be a flash.
///
/// The realised time is 1.6 s — eight ticks — because the ramp moves once per
/// [`TICK_MS`] and 1.5 s is seven and a half of them.
pub const RAMP_UP_SECONDS: f32 = 1.5;

/// Seconds a full-span dim takes. Nobody should ever see this happen; past
/// roughly five seconds a brightness change stops reading as an event and
/// starts reading as the room.
///
/// This is a **bound, not the usual pace**. The EMA in front of it cannot push
/// the perceptual target down faster than `-ln(1 - EMA_ALPHA) / ln(150)` ≈
/// 0.0166 per tick — a full span in 12 s — so for a room that dims, the EMA is
/// already the slower of the two and this limit never binds. What it does bind
/// is every step change: the preference moving down, a clamp releasing, and a
/// missing sensor coming back to report a dark room.
pub const RAMP_DOWN_SECONDS: f32 = 8.0;

/// Per-tick ramp step, **derived** from [`TICK_MS`] so the rates stay expressed
/// in seconds if the tick rate ever changes.
pub const RAMP_UP_STEP: f32 = TICK_MS as f32 / 1000.0 / RAMP_UP_SECONDS;
/// Per-tick ramp step downward. 0.025 at 5 Hz.
pub const RAMP_DOWN_STEP: f32 = TICK_MS as f32 / 1000.0 / RAMP_DOWN_SECONDS;

/// What the room is assumed to be until a reading lands.
///
/// A device whose sensor is absent or broken must not dim itself to the floor,
/// which is what falling back to the bottom of the curve would do — and the
/// bench unit has no VEML7700 at all. The assumption is about the *room*, so it
/// enters the chain as `B_auto` and the preference still biases it: a sensorless
/// device at preference 25 is dimmer than one at 50, exactly as a user turning
/// the slider down expects.
pub const BRIGHT_ROOM: f32 = 1.0;

/// `ln(LUX_MAX / LUX_MIN)` — the denominator that normalises the log map.
/// Not a `const` because `logf` is not `const`; the compiler folds it anyway.
fn log_range() -> f32 {
    libm::logf(LUX_MAX / LUX_MIN)
}

/// One EMA step.
pub fn smooth_lux(current: f32, reading: f32) -> f32 {
    current + EMA_ALPHA * (reading - current)
}

/// Log-scale map from lux to perceptual brightness in `[0, 1]`.
///
/// Log because perceived brightness is logarithmic in luminance: a linear map
/// spends almost its whole range on the difference between "bright" and
/// "brighter". The output is a *position between the two lux anchors* and
/// nothing else — no floor, no ceiling but the clamp, no panel units.
pub fn lux_to_perceptual(lux: f32) -> f32 {
    let t = libm::logf(lux.max(LUX_MIN) / LUX_MIN) / log_range();
    t.clamp(0.0, 1.0)
}

/// Fold the user's preference into the room's reading: an **additive** offset
/// in perceptual space, spanning the full range.
///
/// `50` is pure auto, `0` subtracts 1.0 and `100` adds it. Two properties follow
/// and both are the point:
///
/// * **The knob is as strong in a dark room as in a bright one.** This is the
///   dual lerp's defect, and it was not subtle: the same slider at 25 sat 0.475
///   below auto at 300 lux and 0.143 below it at 9 lux, so a user who found
///   their setting in a lit room came back after dark to a knob that had
///   quietly lost two thirds of its authority. Here a step of *n* is a step of
///   *n* wherever it does not clamp.
/// * **The endpoints saturate; they do not switch modes.** At `0` the clamp
///   holds `B` at 0 for every room, and at `100` at 1 — true floor and true
///   maximum, reached by arithmetic rather than by a branch that turns the
///   sensor off. In between the sensor is always live: at preference 20 the
///   bias is −0.6, and a room that climbs past `B_auto = 0.6` un-clamps and
///   moves the panel again.
///
/// Worth knowing before reading a bench result: the **shipped default is 100**,
/// not 50 — `config.py` chose it and `scoreboard-config` kept it — so a device
/// nobody has touched the slider on sits saturated at full duty and ignores the
/// sensor entirely. That was equally true of the dual lerp, which is why the
/// default needed no migration; it also means the sensor does nothing at all
/// until somebody moves the slider, and a sweep run at the default measures
/// nothing.
pub fn apply_bias(auto: f32, user_preference: u8) -> f32 {
    let bias = (user_preference as f32 - 50.0) / 50.0;
    (auto + bias).clamp(0.0, 1.0)
}

/// Map perceptual brightness onto the duty cycle that produces it.
///
/// The cube is the inverse of the eye. CIE lightness inverts as
/// `Y ≈ ((L* + 16) / 116)³` across most of its range, and Stevens' power law
/// puts the exponent for brightness near 1/3 in luminance; both say perceived
/// goes as the cube root of emitted, so emitted goes as the cube of perceived.
///
/// What it buys, concretely: `B = 0.5` costs 17 % duty, `0.75` costs 45 %, and
/// the top quarter of the scale is more than half the panel's range. A map
/// linear in duty spends half the panel on the difference between "bright" and
/// "slightly brighter", which nobody can see, and crowds every distinction
/// people *can* see into the bottom fifth.
pub fn perceptual_to_duty(brightness: f32) -> f32 {
    let b = brightness.clamp(0.0, 1.0);
    DUTY_MIN + (DUTY_MAX - DUTY_MIN) * b * b * b
}

/// Rate-limit a change in perceptual brightness: [`RAMP_UP_STEP`] per call
/// upward, [`RAMP_DOWN_STEP`] downward.
///
/// Two things distinguish this from the symmetric limit it replaces. It is
/// **asymmetric**, because brightening and dimming are not the same event —
/// one is a person who needs to read the panel now and the other is one who
/// should never notice. And it acts on `B` rather than on duty, so the rate is
/// perceptually uniform: a fixed *duty* rate crawls through the bottom of the
/// range, where the eye is sharpest, and then races through the top, where it
/// is not.
///
/// Returns the target exactly once it is within a step, rather than
/// asymptotically approaching it — which is what keeps a settled panel from
/// rewriting the driver's timing stream forever over a difference nobody can
/// see.
pub fn ramp(current: f32, target: f32) -> f32 {
    let delta = target - current;
    if delta > RAMP_UP_STEP {
        return current + RAMP_UP_STEP;
    }
    if delta < -RAMP_DOWN_STEP {
        return current - RAMP_DOWN_STEP;
    }
    target
}

/// The whole chain's state: the smoothed lux and the ramped perceptual level.
///
/// # The ramp holds the *biased* level, which is what keeps it from winding up
///
/// [`ramp`] is applied after [`apply_bias`], so the stored value is always
/// inside `[0, 1]` and always a level the panel is actually showing. Ramping
/// the pre-bias reading instead would let the state drift somewhere the clamp
/// hides — a dark room under a low preference would park `B_auto` well above
/// the visible floor — and the panel would then sit still for the first second
/// after the lights came on, working off a charge nobody could see. It also
/// means a preference change ramps rather than snapping, which is the same
/// smoothness applied to the same quantity for the same reason.
#[derive(Debug, Clone, Copy)]
pub struct AutoBrightness {
    smoothed_lux: f32,
    /// Perceptual brightness, post-bias, post-ramp. The value [`perceptual_to_duty`]
    /// is called on.
    brightness: f32,
    /// Whether any reading has ever landed. Not "is the sensor healthy" — a
    /// sensor that worked and then failed keeps its last smoothed value and
    /// rides it, which is the same thing `main.py` did by simply not updating.
    seeded: bool,
}

impl AutoBrightness {
    /// Start where the sensorless rule already says to start: [`BRIGHT_ROOM`],
    /// biased by the preference.
    ///
    /// This is the one value that needs no justification beyond the rules
    /// already written down — it is exactly what [`tick`](Self::tick) returns
    /// before the first reading, so a device with no sensor is at its final
    /// brightness from the first tick with no transient at all, and a device
    /// with one starts bright and corrects. `main.py` seeded from
    /// `preference / 100`, which was a *level*; the preference is a bias now
    /// and reusing it as a level would be a leftover, not a choice.
    pub fn new(user_preference: u8) -> AutoBrightness {
        AutoBrightness {
            smoothed_lux: 0.0,
            brightness: apply_bias(BRIGHT_ROOM, user_preference),
            seeded: false,
        }
    }

    /// One tick. `lux` is `None` while the sensor is unavailable. Returns the
    /// panel duty cycle.
    pub fn tick(&mut self, lux: Option<f32>, user_preference: u8) -> f32 {
        if let Some(lux) = lux {
            self.smoothed_lux = if self.seeded {
                smooth_lux(self.smoothed_lux, lux)
            } else {
                self.seeded = true;
                lux
            };
        }
        let auto = if self.seeded {
            lux_to_perceptual(self.smoothed_lux)
        } else {
            BRIGHT_ROOM
        };
        self.brightness = ramp(self.brightness, apply_bias(auto, user_preference));
        perceptual_to_duty(self.brightness)
    }

    /// The duty as the panel takes it: 0..=255, which is what
    /// `display_core1::BRIGHTNESS` carries.
    pub fn quantize(duty: f32) -> u8 {
        (duty.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
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

    /// Run the chain to a standstill and return the duty. Long enough for the
    /// slowest leg — a full-span dim is 40 ticks — several times over.
    fn settle(lux: Option<f32>, user_preference: u8) -> f32 {
        let mut auto = AutoBrightness::new(user_preference);
        let mut duty = f32::NAN;
        for _ in 0..200 {
            duty = auto.tick(lux, user_preference);
        }
        duty
    }

    /// [`perceptual_to_duty`] read backwards, so a test can talk about `B`
    /// where only duty crosses the API.
    fn perceptual_of(duty: f32) -> f32 {
        libm::cbrtf((duty - DUTY_MIN) / (DUTY_MAX - DUTY_MIN))
    }

    #[test]
    fn the_ramp_steps_are_derived_and_land_on_the_documented_values() {
        assert_close(RAMP_UP_STEP, 0.133_333_33);
        assert_close(RAMP_DOWN_STEP, 0.025);
        assert_close(log_range(), 5.010_635_3);
        // The claim on RAMP_DOWN_SECONDS: the EMA cannot push the perceptual
        // target down faster than this, so the down ramp never binds on a room
        // that dims. `-ln(1 - alpha) / ln(150)` is the worst case, approached
        // when the room is far darker than the smoothed value.
        let fastest_ema_fall = -libm::logf(1.0 - EMA_ALPHA) / log_range();
        assert_close(fastest_ema_fall, 0.016_640_9);
        assert!(fastest_ema_fall < RAMP_DOWN_STEP);
    }

    /// Reference values computed from this module's formulas in CPython
    /// (double) and rounded to the nearest `f32`. These pin the *spec*, not
    /// `brightness.py` — the curve deliberately no longer matches it, and the
    /// numbers here are 300× finer than [`EPSILON`].
    #[test]
    fn lux_to_perceptual_is_the_normalised_log_position() {
        for (lux, expected) in [
            (0.0, 0.0),
            (0.5, 0.0),
            (2.0, 0.0),
            (3.0, 0.080_920_9),
            (9.0, 0.300_177),
            (10.0, 0.321_204_36),
            (50.0, 0.642_408_7),
            (150.0, 0.861_664_8),
            (300.0, 1.0),
            (1000.0, 1.0),
            (12000.0, 1.0),
        ] {
            assert_close_at(
                lux_to_perceptual(lux),
                expected,
                format_args!("at {lux} lux"),
            );
        }
    }

    #[test]
    fn perceptual_to_duty_is_the_cube_between_the_floor_and_full_scale() {
        for (brightness, expected) in [
            (0.0, 0.05),
            (0.25, 0.064_843_75),
            (0.5, 0.168_75),
            (0.75, 0.450_781_25),
            (0.8, 0.536_4),
            (1.0, 1.0),
        ] {
            assert_close_at(
                perceptual_to_duty(brightness),
                expected,
                format_args!("at B {brightness}"),
            );
        }
        // The floor is applied here and only here.
        assert_close(perceptual_to_duty(-1.0), DUTY_MIN);
        assert_close(perceptual_to_duty(2.0), DUTY_MAX);
    }

    /// The whole pipeline, settled, as a grid — the table to read when asking
    /// "what does the panel actually do". Reference values from CPython.
    #[test]
    fn the_settled_chain_matches_the_reference_grid() {
        for (lux, preference, expected) in [
            (2.0, 0, 0.05),
            (2.0, 25, 0.05),
            (2.0, 50, 0.05),
            (2.0, 75, 0.168_75),
            (2.0, 100, 1.0),
            (9.0, 0, 0.05),
            (9.0, 25, 0.05),
            (9.0, 50, 0.075_695_42),
            (9.0, 75, 0.536_722_9),
            (9.0, 100, 1.0),
            (50.0, 0, 0.05),
            (50.0, 25, 0.052_743_68),
            (50.0, 50, 0.301_859_24),
            (50.0, 75, 1.0),
            (50.0, 100, 1.0),
            (300.0, 0, 0.05),
            (300.0, 25, 0.168_75),
            (300.0, 50, 1.0),
            (300.0, 75, 1.0),
            (300.0, 100, 1.0),
        ] {
            assert_close_at(
                settle(Some(lux), preference),
                expected,
                format_args!("at {lux} lux, preference {preference}"),
            );
        }
    }

    #[test]
    fn smooth_lux_matches_the_python_ema() {
        // This stage is unchanged, so its reference values still come from
        // `brightness.py`.
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
    fn ramp_is_asymmetric_and_lands_exactly() {
        for (current, target, expected) in [
            (0.0, 1.0, RAMP_UP_STEP),
            (1.0, 0.0, 1.0 - RAMP_DOWN_STEP),
            (0.5, 0.6, 0.6),
            (0.5, 0.5, 0.5),
            (0.5, 0.51, 0.51),
            (0.5, 0.49, 0.49),
            // Just outside each step, in each direction.
            (0.5, 0.5 + RAMP_UP_STEP + 0.01, 0.5 + RAMP_UP_STEP),
            (0.5, 0.5 - RAMP_DOWN_STEP - 0.01, 0.5 - RAMP_DOWN_STEP),
            // A fall of 0.1 is a single step up but five steps down.
            (0.5, 0.4, 0.5 - RAMP_DOWN_STEP),
        ] {
            assert_close_at(
                ramp(current, target),
                expected,
                format_args!("{current} -> {target}"),
            );
        }
    }

    #[test]
    fn a_full_span_brightens_in_eight_ticks_and_dims_in_forty() {
        // Driven by the preference rather than by lux, because the EMA would
        // otherwise be the thing under test. Preference 100 and 0 saturate the
        // clamp, so the target is exactly 1 and exactly 0 whatever the room is.
        // Counted in `B`, not in duty: the cube compresses the bottom of the
        // range so hard that duty quantises to the floor five ticks before the
        // ramp is actually finished, and it is the ramp under test here.
        let mut auto = AutoBrightness::new(0);
        assert_close(auto.tick(Some(2.0), 0), DUTY_MIN);

        let mut up = 1;
        while perceptual_of(auto.tick(Some(2.0), 100)) < 1.0 - EPSILON {
            up += 1;
            assert!(up < 100, "the up ramp never arrived");
        }
        assert_eq!(up, 8, "1.6 s — the 1.5 s target rounded up to a whole tick");

        let mut down = 1;
        while perceptual_of(auto.tick(Some(2.0), 0)) > EPSILON {
            down += 1;
            assert!(down < 200, "the down ramp never arrived");
        }
        assert_eq!(down, 40, "8.0 s exactly");

        // The asymmetry itself, stated as the ratio the constants claim.
        assert_close(RAMP_DOWN_SECONDS / RAMP_UP_SECONDS, 5.333_333_5);
    }

    /// The defect the redesign exists to fix. The dual lerp's answer to this
    /// question was 0.475 at 300 lux and 0.143 at 9 lux for the same slider.
    #[test]
    fn an_unclamped_bias_step_moves_every_room_by_the_same_amount() {
        for (from, to) in [(50u8, 60u8), (50, 40), (40, 55), (45, 50), (44, 56)] {
            let expected = (to as f32 - from as f32) / 50.0;
            // Ambients and preferences chosen so nothing clamps: B stays inside
            // [0.1, 0.9] for every pair below.
            for ambient in [0.3, 0.4, 0.5, 0.6, 0.7] {
                assert_close_at(
                    apply_bias(ambient, to) - apply_bias(ambient, from),
                    expected,
                    format_args!("preference {from} -> {to} at B_auto {ambient}"),
                );
            }
        }
    }

    /// The same property through the whole state machine, at settled values,
    /// where it is what a person actually experiences.
    #[test]
    fn the_settled_chain_is_ambient_independent_in_the_preference() {
        // 6, 12 and 24 lux put B_auto at roughly 0.22, 0.36 and 0.50 — three
        // genuinely different rooms, all far enough from the clamps that a
        // +-0.2 bias stays inside them.
        for (from, to) in [(50u8, 60u8), (50, 40), (60, 40)] {
            let expected = (to as f32 - from as f32) / 50.0;
            for lux in [6.0, 12.0, 24.0] {
                let before = perceptual_of(settle(Some(lux), from));
                let after = perceptual_of(settle(Some(lux), to));
                assert_close_at(
                    after - before,
                    expected,
                    format_args!("preference {from} -> {to} at {lux} lux"),
                );
            }
        }
    }

    #[test]
    fn fifty_is_exactly_the_room_at_every_level() {
        for lux in [2.0, 6.0, 9.0, 50.0, 150.0, 300.0, 5000.0] {
            assert_close_at(
                perceptual_of(settle(Some(lux), 50)),
                lux_to_perceptual(lux),
                format_args!("at {lux} lux"),
            );
        }
    }

    #[test]
    fn zero_and_one_hundred_are_the_true_floor_and_ceiling_in_every_room() {
        // Endpoints by saturation, not by a branch: the same two numbers come
        // out of a pitch-dark room, an evening room, a lit room and a room
        // past the top of the curve.
        for lux in [2.0, 9.0, 50.0, 300.0, 12000.0] {
            assert_close_at(
                settle(Some(lux), 0),
                DUTY_MIN,
                format_args!("preference 0 at {lux} lux"),
            );
            assert_close_at(
                settle(Some(lux), 100),
                DUTY_MAX,
                format_args!("preference 100 at {lux} lux"),
            );
        }
        // And with no sensor at all, which is the same rule applied to
        // BRIGHT_ROOM rather than to a reading.
        assert_close(settle(None, 0), DUTY_MIN);
        assert_close(settle(None, 100), DUTY_MAX);
    }

    #[test]
    fn duty_never_falls_as_the_preference_rises() {
        for lux in [2.0, 9.0, 50.0, 300.0] {
            let mut previous = 0.0;
            for preference in 0..=100u8 {
                let duty = settle(Some(lux), preference);
                assert!(
                    duty >= previous - EPSILON,
                    "{duty} < {previous} going up through preference {preference} at {lux} lux"
                );
                previous = duty;
            }
        }
    }

    #[test]
    fn duty_never_falls_as_the_room_brightens() {
        for preference in [0u8, 25, 50, 75, 100] {
            let mut previous = 0.0;
            for lux in [0.0, 1.0, 2.0, 3.0, 6.0, 9.0, 20.0, 50.0, 150.0, 300.0, 5000.0] {
                let duty = settle(Some(lux), preference);
                assert!(
                    duty >= previous - EPSILON,
                    "{duty} < {previous} at {lux} lux, preference {preference}"
                );
                previous = duty;
            }
        }
    }

    #[test]
    fn a_sensor_that_never_answers_assumes_a_bright_room_from_the_first_tick() {
        // The alternative — treating "no reading" as the bottom of the curve —
        // gives a device with no sensor a panel at 5 %, which reads as broken.
        // There is no transient either: `new` starts at the value this rule
        // produces, so the bench unit is at full duty on tick one.
        let mut auto = AutoBrightness::new(50);
        assert_close(auto.tick(None, 50), DUTY_MAX);
        for _ in 0..200 {
            assert_close(auto.tick(None, 50), DUTY_MAX);
        }
    }

    #[test]
    fn a_missing_sensor_still_obeys_the_preference() {
        // "Assume bright" is a statement about the room, so the bias applies to
        // it like any other. A sensorless device is not stuck at full.
        assert_close(settle(None, 50), DUTY_MAX);
        assert_close(settle(None, 75), DUTY_MAX);
        assert_close(settle(None, 25), perceptual_to_duty(0.5));
        assert_close(settle(None, 10), perceptual_to_duty(0.2));
        assert_close(settle(None, 0), DUTY_MIN);
    }

    #[test]
    fn the_first_reading_seeds_the_average_rather_than_being_averaged_into_zero() {
        // Without the seed, a device booting in a bright room would take dozens
        // of ticks to climb out of a smoothed lux of 0.
        let mut seeded = AutoBrightness::new(50);
        seeded.tick(Some(300.0), 50);
        assert_close(seeded.smoothed_lux, 300.0);

        let mut unseeded = AutoBrightness::new(50);
        unseeded.smoothed_lux = 0.0;
        unseeded.seeded = true;
        assert!(
            unseeded.tick(Some(300.0), 50) < seeded.tick(Some(300.0), 50),
            "an unseeded average lags a seeded one"
        );
    }

    #[test]
    fn a_dark_boot_dims_at_the_slow_rate_rather_than_dropping() {
        // Boot is a brightness *fall* — BRIGHT_ROOM down to whatever the sensor
        // reports — and it takes the graceful path like any other fall.
        let mut auto = AutoBrightness::new(50);
        let first = auto.tick(Some(2.0), 50);
        assert_close(first, perceptual_to_duty(1.0 - RAMP_DOWN_STEP));
        let second = auto.tick(Some(2.0), 50);
        assert_close(second, perceptual_to_duty(1.0 - 2.0 * RAMP_DOWN_STEP));
    }

    #[test]
    fn a_settled_level_stops_moving_exactly_rather_than_approaching_forever() {
        let mut auto = AutoBrightness::new(50);
        let mut previous = f32::NAN;
        for _ in 0..200 {
            previous = auto.tick(Some(2.0), 50);
        }
        assert_close(previous, DUTY_MIN);
        assert_close(auto.tick(Some(2.0), 50), DUTY_MIN);
    }

    #[test]
    fn a_clamped_state_has_no_hidden_charge_to_work_off() {
        // The reason the ramp holds the biased level. At preference 25 a dark
        // room is clamped hard against the floor; the lights coming on must
        // move the panel on the very next tick rather than after a second of
        // an invisible value catching up.
        let mut auto = AutoBrightness::new(25);
        for _ in 0..200 {
            auto.tick(Some(2.0), 25);
        }
        assert_close(perceptual_of(auto.tick(Some(2.0), 25)), 0.0);
        let after_lights_on = perceptual_of(auto.tick(Some(300.0), 25));
        assert!(
            after_lights_on > 0.0,
            "the panel sat still for a tick after the lights came on"
        );
    }

    #[test]
    fn quantisation_covers_the_whole_byte_range_and_clamps() {
        assert_eq!(AutoBrightness::quantize(0.0), 0);
        assert_eq!(AutoBrightness::quantize(1.0), 255);
        assert_eq!(AutoBrightness::quantize(0.5), 128);
        assert_eq!(AutoBrightness::quantize(-1.0), 0);
        assert_eq!(AutoBrightness::quantize(2.0), 255);
        // The floor is 13/255, which is visible in a dark room and unmistakably
        // not "off".
        assert_eq!(AutoBrightness::quantize(DUTY_MIN), 13);
    }
}
