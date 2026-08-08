//! The two time rails, as types you cannot mix up by accident.
//!
//! # The rule
//!
//! **A stall stretches motion but consumes waiting.**
//!
//! Every animated value on the panel rides one of two clocks, and picking the
//! wrong one is not a rounding error — it is the difference between a scroll
//! that jumps a handful of pixels after a stalled frame and a match clock that
//! silently runs slow.
//!
//! * **Wall rail** ([`WallMs`]) — the monotonic clock, as `time.ticks_ms()`
//!   was. It carries *event windows and durations*: how long a toast lives, how
//!   long a play flash stays up, the soccer match clock, the menu marquee's
//!   anchor. A frame that took 90 ms instead of 17 still spent 90 ms of a
//!   toast's life, and pretending otherwise makes feedback timing dishonest.
//! * **Frame rail** ([`FrameElapsed`]) — advances exactly one frame period per
//!   rendered frame, no matter how long that frame actually took. It carries
//!   *continuous motion*: scroll offsets, the count-dot pulse, the pregame
//!   info cycle. A stalled frame holds position for one frame instead of
//!   teleporting, and pixel steps stay even.
//!
//! Both rails are milliseconds, so nothing on either side had to change when
//! the frame rate did — a `pause_ms` is a `pause_ms` at any rate. What changed
//! is the size of the frame rail's step, and it is derived from [`FPS`] alone.
//!
//! Under perfect pacing the two are identical, which is exactly why a mix-up
//! survives every test that does not stall a frame.
//!
//! # Why newtypes
//!
//! In MicroPython both rails were bare `int` milliseconds, three of them in
//! `render_frame`'s signature (`now_ms`, `view_elapsed_ms`, `play_elapsed_ms`),
//! and the only thing keeping them apart was the parameter name. Here they are
//! distinct types, so passing a wall stamp where a frame rail belongs does not
//! compile. Feeding either into an animation goes through [`Motion`], which
//! makes the rail choice a visible call — grep for `.motion()` to audit every
//! animation's rail in one pass.

use scoreboard_model::{Millis, ScoreboardSnapshot};

/// The rate the render loop paces at, and **the one constant the frame quantum
/// comes from**. Everything frame-coupled in this crate is derived from it.
///
/// Scroll speeds must divide it evenly in one direction or the other — see
/// [`crate::geometry::SCROLL_SPEEDS`].
///
/// The parity release ran at 20 (`display.FRAME_MS = 50`). 60 is what makes
/// 30 px/s expressible: at 20 FPS that is 1.5 px per frame and every third
/// pixel column of a scroll is never displayed.
pub const FPS: u32 = 60;

/// The frame rail's position after `frames` frames, in milliseconds.
///
/// # Why the period is a function and not a constant
///
/// At 60 FPS the period is 16⅔ ms, which is not a whole number of anything the
/// rail is measured in. Adding a rounded per-frame constant would accumulate
/// error without bound — 16 ms per frame runs 4 % slow, which is a scroll that
/// falls a second behind every 25 seconds. So the rail keeps a **frame count**
/// and derives its position from it, which is exact by construction: the whole
/// rounding is the one applied to the final answer.
///
/// # Why it rounds up
///
/// The choice is load-bearing and not a matter of taste. A scroll offset is
/// `floor(elapsed_ms × speed / 1000)`, so the quantiser's error decides which
/// frame a pixel step lands on. Rounding **up** keeps the rail a hair ahead of
/// true time (by under 1 ms, so under 0.06 px at the fastest legal speed),
/// which lands every step on the frame the exact arithmetic would put it on.
/// Rounding down puts the rail a hair behind, and a step whose exact time falls
/// on a frame boundary — which is every step, for a speed that divides the
/// frame rate — arrives one frame late, *sometimes*: the result is 1 px steps
/// of alternating 1- and 2-frame dwell. That is precisely the stutter the legal
/// scroll set exists to prevent, reintroduced by a rounding mode.
/// `tests/time.rs` walks every legal speed and checks the spacing rather than
/// trusting this paragraph.
///
/// At `FPS = 20` this is `frames * 50`, exactly the rail the parity release ran.
pub const fn frame_ms(frames: u64) -> Millis {
    (frames * 1_000).div_ceil(FPS as u64)
}

/// The same instant in microseconds — what the render loop's deadline is
/// measured in, since `embassy_time` resolves to 1 µs and 16⅔ ms is not a whole
/// number of those either.
///
/// Pacing against `frame_us(k)` from a fixed anchor rather than adding a
/// per-frame duration is the same argument as [`frame_ms`]: the error stays
/// under a microsecond forever instead of compounding.
pub const fn frame_us(frames: u64) -> u64 {
    (frames * 1_000_000).div_ceil(FPS as u64)
}

/// An absolute stamp on the wall rail: monotonic milliseconds since boot, the
/// same domain as [`Millis`] and as every `*_ms` field on the snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct WallMs(pub Millis);

impl WallMs {
    /// Time since `stamp`, saturating at zero.
    ///
    /// MicroPython used `time.ticks_diff`, which wrapped a 30-bit counter and
    /// could legitimately return a negative value — hence the `if elapsed < 0:
    /// elapsed = 0` guards scattered through `display.py`. `embassy_time`'s
    /// clock is a 64-bit monotonic that does not wrap in any reachable
    /// lifetime, so a stamp in the future can only mean a stamp that was never
    /// set, and saturating is both the right answer and the whole story.
    pub const fn since(self, stamp: Millis) -> WallElapsed {
        WallElapsed(self.0.saturating_sub(stamp))
    }
}

/// A duration measured on the wall rail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct WallElapsed(pub Millis);

impl WallElapsed {
    /// Drive an animation from the wall rail — correct when the animation *is*
    /// a duration (a toast's fade, the menu marquee anchored to the last
    /// highlight change), wrong for anything that must not jump after a stall.
    pub const fn motion(self) -> Motion {
        Motion(self.0)
    }
}

/// A duration measured on the frame rail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FrameElapsed(pub Millis);

impl FrameElapsed {
    /// Drive an animation from the frame rail — the default for continuous
    /// motion.
    pub const fn motion(self) -> Motion {
        Motion(self.0)
    }
}

/// Elapsed milliseconds feeding one animation, with the rail already chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Motion(pub Millis);

/// The frame rail and its epoch latches.
///
/// The port of `LoopState`'s time fields and `advance_and_latch`. The rail
/// advances one frame quantum per rendered frame; core 0's epoch stamps
/// (`animation_start_ms`, `play.updated_ms`) live in the wall domain and are
/// never subtracted against it — a *change* in an epoch stamp is translated
/// into "frame-rail time zero" instead.
///
/// # It counts frames, and both epochs are frame counts too
///
/// The rail's state is a frame *count*, converted to milliseconds by
/// [`frame_ms`] only when a renderer asks. Storing milliseconds and adding a
/// period would need a period, and at 60 FPS there is no whole-millisecond one.
///
/// The epochs are counts for the same reason and a sharper one: elapsed time is
/// `frame_ms(frames - epoch)`, not `frame_ms(frames) - frame_ms(epoch)`. Those
/// two differ by a millisecond depending on where the epoch fell in the
/// rounding cycle, which would make a scroll's step pattern depend on *when its
/// screen appeared*. Differencing the counts first makes elapsed time depend on
/// nothing but how many frames have been drawn since the latch.
///
/// # Where this lives
///
/// One instance, owned by the app's render-loop-local `LoopState` (Phase 3).
/// Never a `static`, never shared with core 0, and never passed into a
/// renderer: renderers receive [`FrameElapsed`] *values*. That is the same
/// confinement `display.py` enforced by grepping for the name `ls` below
/// `render_frame`, except here the reference genuinely does not exist in a
/// renderer's scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameRail {
    /// Frames advanced since the loop started.
    frames: u64,
    view_stamp: Option<Millis>,
    view_epoch: u64,
    play_stamp: Option<Millis>,
    play_epoch: u64,
    view_elapsed: FrameElapsed,
    play_elapsed: FrameElapsed,
}

impl FrameRail {
    pub const fn new() -> Self {
        FrameRail {
            frames: 0,
            view_stamp: None,
            view_epoch: 0,
            play_stamp: None,
            play_epoch: 0,
            view_elapsed: FrameElapsed(0),
            play_elapsed: FrameElapsed(0),
        }
    }

    /// Advance one frame and re-latch any epoch that changed.
    ///
    /// Call once per loop tick, before rendering, with the snapshot the frame
    /// will render from.
    pub fn advance_and_latch(&mut self, snapshot: &ScoreboardSnapshot) {
        self.frames += 1;
        if self.view_stamp != Some(snapshot.animation_start_ms) {
            self.view_stamp = Some(snapshot.animation_start_ms);
            self.view_epoch = self.frames;
        }
        if self.play_stamp != Some(snapshot.play.updated_ms) {
            self.play_stamp = Some(snapshot.play.updated_ms);
            self.play_epoch = self.frames;
        }
        self.view_elapsed = FrameElapsed(frame_ms(self.frames - self.view_epoch));
        self.play_elapsed = FrameElapsed(frame_ms(self.frames - self.play_epoch));
    }

    /// Frame-rail time since the displayed view last changed identity — the
    /// rail every screen's continuous motion rides.
    pub const fn view_elapsed(&self) -> FrameElapsed {
        self.view_elapsed
    }

    /// Frame-rail time since the play/commentary line last changed.
    pub const fn play_elapsed(&self) -> FrameElapsed {
        self.play_elapsed
    }
}

impl Default for FrameRail {
    fn default() -> Self {
        Self::new()
    }
}
