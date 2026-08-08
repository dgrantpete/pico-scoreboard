//! One frame: which screen draws it, and whether it needs drawing at all.
//!
//! # The order a render loop calls these in
//!
//! ```text
//! rail.advance_and_latch(&snapshot);          // crate::time
//! prepared.sync(&snapshot, &settings);        // crate::prepared
//! let scene = Scene { .. };                   // borrows the prepared view
//! if memo.should_render(&snapshot, now) {
//!     frame::render(&mut canvas, &scene);
//!     display.show();
//! }
//! ```
//!
//! The rebuild comes before the skip check because a rebuilt prepared view is
//! exactly what a new commit means, and the scene borrows the prepared view, so
//! the rebuild has to be finished before the scene exists. The borrow checker
//! enforces that ordering rather than a comment doing it.

use crate::blit::Canvas;
use crate::game::{self, Scene};
use crate::time::WallMs;
use crate::{menu, screens, toast};
use scoreboard_model::{Mode, ScoreboardSnapshot};

/// Draw the frame the scene describes.
///
/// # The menu preempts everything
///
/// While the league menu is up it *is* the frame. Rotation, poll commits and
/// toasts carry on underneath, invisible — toast drawing lives inside the mode
/// renderers this bypasses, so the suppression is structural rather than a
/// special case at each call site. That is also why toast feedback is
/// unavailable under the menu by design.
pub fn render(canvas: &mut Canvas<'_>, scene: &Scene<'_>) {
    if scene.snapshot.menu.active {
        menu::render(canvas, scene.snapshot, scene.now);
        return;
    }
    match scene.snapshot.mode {
        Mode::Startup => screens::startup(canvas, scene.snapshot),
        Mode::Idle => screens::idle(canvas, scene.snapshot),
        Mode::NoGames => screens::no_games(canvas, scene.snapshot, scene.now),
        Mode::Setup => screens::setup(canvas, scene.snapshot, scene.prepared, scene.now),
        Mode::Error => screens::error(canvas, scene.snapshot),
        Mode::Updating => screens::updating(canvas, scene.snapshot),
        Mode::MlbLive => game::mlb::render(canvas, scene),
        Mode::Pregame => game::pregame::render(canvas, scene),
        Mode::Final => game::score::render(canvas, scene),
        Mode::SoccerLive => game::soccer::render_live(canvas, scene),
        Mode::SoccerFinal => game::soccer::render_final(canvas, scene),
        Mode::NbaLive => game::nba::render(canvas, scene),
        Mode::FootballLive => game::football::render(canvas, scene),
    }
}

/// The render loop's static-screen skip.
///
/// A screen with no time-driven animation only needs redrawing when a new
/// commit lands, so an idle scoreboard is not repainting twenty times a second.
/// Deciding that takes two values that must survive between frames, which makes
/// this the render loop's cross-frame state — one instance, owned by the app's
/// loop-local struct beside its [`FrameRail`](crate::time::FrameRail), never
/// reachable from a renderer.
#[derive(Debug, Clone, Copy, Default)]
pub struct SkipMemo {
    last_drawn: Option<u32>,
    last_had_toast: bool,
}

impl SkipMemo {
    pub const fn new() -> Self {
        SkipMemo {
            last_drawn: None,
            last_had_toast: false,
        }
    }

    /// Whether this frame has to be drawn, recording the decision.
    ///
    /// Call once per tick, before rendering, and render only when it says so.
    ///
    /// Four things force a redraw of an otherwise static screen: a new commit;
    /// a toast being up; a toast having been up on the *previous* frame, so the
    /// frame that removes it is drawn; and the menu, which marquees regardless
    /// of what mode is underneath it. "A toast being up" includes the icon
    /// overlay's fade-out tail, so a static screen keeps redrawing until the
    /// dim has fully eased back out — there is no commit coming to finish it.
    pub fn should_render(&mut self, snapshot: &ScoreboardSnapshot, now: WallMs) -> bool {
        let toast_active =
            toast::is_active(&snapshot.toast, now) || toast::overlay_fading(&snapshot.toast, now);
        let skip = self.last_drawn == Some(snapshot.commit_seq)
            && snapshot.mode.is_static()
            && !toast_active
            && !self.last_had_toast
            && !snapshot.menu.active;
        if skip {
            return false;
        }
        self.last_drawn = Some(snapshot.commit_seq);
        self.last_had_toast = toast_active;
        true
    }

    /// Force the next [`should_render`](Self::should_render) to say yes.
    ///
    /// The static-screen skip assumes the only thing that can change a frame is
    /// a new commit. A settings change breaks that assumption — turn the
    /// dividers off while an idle screen is up and there is no commit coming to
    /// redraw it, so the panel would keep showing them until the next game
    /// update. Core 1 calls this when it takes a settings update.
    pub fn invalidate(&mut self) {
        self.last_drawn = None;
    }
}
