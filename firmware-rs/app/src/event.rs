//! Events: which ones exist, and the two atomics that carry them across cores.
//!
//! The logic — the clip format, the date window, when a clip plays and what
//! each tick shows — is [`scoreboard_render::event`], host-tested there. This
//! is the wiring, and it follows [`crate::display_core1::BRIGHTNESS`]'s shape:
//! core 0 decides a value from the world, core 1 reads it at the top of a
//! frame.
//!
//! | | written by | read by |
//! |---|---|---|
//! | [`LIVE`] — which event's window is open | core 0, the poller, each tick | core 1, each frame |
//! | [`CAPTURING`] — a press right now dismisses | core 1, each frame | core 0, the button task |
//! | [`DISMISS`] — a press was captured | core 0, the button task | core 1, which clears it |
//!
//! Plain atomics, not snapshot fields: "is it Colin's birthday" is not
//! something the screen *shows*, and a commit per change would restamp the
//! view for no visible reason.
//!
//! # The clips are embedded, for now
//!
//! Each event's clip is `include_bytes!`d into the image and read in place from
//! flash, costing flash and no RAM. The alternative — downloading a clip into
//! a flash region at runtime so events ship without a firmware release — needs
//! a region to put it in, and the only free space large enough is inside the
//! storage partition, whose map erases itself if its range changes under it.
//! That migration is its own piece of work; the clip format and the player do
//! not change when it lands, only where the bytes come from.
//!
//! **An event whose window has closed should be deleted from [`EVENTS`] in the
//! next release**, clip and all: nothing plays it again, and its flash is the
//! headroom the next one needs.

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use scoreboard_model::LocalClock;
use scoreboard_render::event::{Window, local_day};

pub struct Event {
    /// For logs. Matches the clip's file name.
    pub name: &'static str,
    pub window: Window,
    /// A `.sbev` clip; see `assets/events/README.md`.
    pub clip: &'static [u8],
}

/// Every event this image knows. Where two windows overlap, the first wins.
///
/// Adding one: render and encode the clip (`tools/events/`), add it here and to
/// `SHIPPED` in `crates/scoreboard-render/tests/event.rs`, and check the image
/// still fits — `publish-fw` refuses one that does not.
pub static EVENTS: [Event; 1] = [Event {
    name: "colin-birthday-2026",
    window: Window::new((2026, 9, 30), (2026, 10, 2)),
    clip: include_bytes!("../assets/events/colin-birthday-2026.sbev"),
}];

/// No event is live.
const NONE: u8 = u8::MAX;

/// The index into [`EVENTS`] whose window contains today, or [`NONE`].
static LIVE: AtomicU8 = AtomicU8::new(NONE);
static CAPTURING: AtomicBool = AtomicBool::new(false);
static DISMISS: AtomicBool = AtomicBool::new(false);

/// Core 0: re-evaluate which event is live. The poller calls this once a tick,
/// straight after its clock sync, so "today" follows the clock within one poll
/// interval — far finer than a window measured in days.
///
/// An unsynced clock means no event: there is no date to test, and a clip
/// playing on the wrong day is the one failure worse than not playing.
pub fn refresh(clock: LocalClock) {
    let live = if clock.now_epoch_s == 0 {
        None
    } else {
        let today = local_day(clock.now_epoch_s, clock.utc_offset_s);
        EVENTS.iter().position(|event| event.window.contains(today))
    };
    let live = live.map_or(NONE, |index| index as u8);
    let was = LIVE.swap(live, Ordering::Relaxed);
    if was != live {
        match EVENTS.get(live as usize) {
            Some(event) => crate::debug!("event: {} is live", event.name),
            None => crate::debug!("event: none live"),
        }
    }
}

/// Core 1: the live event, if any.
pub fn live() -> Option<(u8, &'static Event)> {
    let index = LIVE.load(Ordering::Relaxed);
    EVENTS.get(index as usize).map(|event| (index, event))
}

/// Core 1: whether presses should dismiss the event, this frame.
pub fn set_capturing(capturing: bool) {
    CAPTURING.store(capturing, Ordering::Relaxed);
}

/// Core 1: whether a press was captured since the last call.
///
/// Called every frame, so a press that lands just as a play ends is consumed
/// by the next frame and cannot dismiss a play minutes later.
pub fn take_dismiss() -> bool {
    DISMISS.swap(false, Ordering::Relaxed)
}

/// Core 0, the button task: if an event is on screen, this press dismisses it
/// and returns `true` — the press is spent, and must not also skip a game or
/// open the menu underneath.
pub fn capture_press() -> bool {
    if CAPTURING.load(Ordering::Relaxed) {
        DISMISS.store(true, Ordering::Relaxed);
        true
    } else {
        false
    }
}
