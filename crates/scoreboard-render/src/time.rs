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
//!   anchor. A frame that took 90 ms instead of 50 still spent 90 ms of a
//!   toast's life, and pretending otherwise makes feedback timing dishonest.
//! * **Frame rail** ([`FrameElapsed`]) — advances exactly one frame period per
//!   rendered frame, no matter how long that frame actually took. It carries
//!   *continuous motion*: scroll offsets, the count-dot pulse, the pregame
//!   info cycle. A stalled frame holds position for one frame instead of
//!   teleporting, and pixel steps stay even.
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

/// The render loop's frame period: 20 FPS (`display.FRAME_MS`).
///
/// Scroll speeds must evenly divide this rate — see
/// [`crate::geometry::SCROLL_SPEEDS`].
pub const FRAME_MS: Millis = 50;

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
/// advances [`FRAME_MS`] per rendered frame; core 0's epoch stamps
/// (`animation_start_ms`, `play.updated_ms`) live in the wall domain and are
/// never subtracted against it — a *change* in an epoch stamp is translated
/// into "frame-rail time zero" instead.
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
    /// Frames rendered, in milliseconds.
    now: Millis,
    view_stamp: Option<Millis>,
    view_epoch: Millis,
    play_stamp: Option<Millis>,
    play_epoch: Millis,
    view_elapsed: FrameElapsed,
    play_elapsed: FrameElapsed,
}

impl FrameRail {
    pub const fn new() -> Self {
        FrameRail {
            now: 0,
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
        self.now += FRAME_MS;
        if self.view_stamp != Some(snapshot.animation_start_ms) {
            self.view_stamp = Some(snapshot.animation_start_ms);
            self.view_epoch = self.now;
        }
        if self.play_stamp != Some(snapshot.play.updated_ms) {
            self.play_stamp = Some(snapshot.play.updated_ms);
            self.play_epoch = self.now;
        }
        self.view_elapsed = FrameElapsed(self.now - self.view_epoch);
        self.play_elapsed = FrameElapsed(self.now - self.play_epoch);
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
