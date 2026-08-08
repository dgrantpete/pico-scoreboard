//! The game-facing screens, and what they share.
//!
//! Every live screen is built from the same three pieces: an identity column or
//! corner stack carrying logos and scores, a data column carrying the clock or
//! the count, and a bottom strip whose owner is decided by [`bottom_strip`].
//! The pregame and final screens drop the third and keep the frame.
//!
//! # Bottom-strip priority
//!
//! **toast > play flash > sport content**, on every live screen. The toast is
//! button feedback and outranks everything; the play flash is the most recent
//! play-by-play line and outranks the persistent content underneath (MLB's
//! pitcher/batter, soccer's last goal, football's field strip; NBA has none, so
//! its strip goes empty between flashes).
//!
//! # Rails
//!
//! The play flash's *visibility window* rides the wall rail — a stall consumes
//! it, because it is a duration. Its *scroll offset* rides the frame rail — a
//! stall stretches it, because it is motion. Both appear in [`bottom_strip`],
//! two lines apart, which is the clearest place in the codebase to see why the
//! two rails exist.

pub mod football;
pub mod mlb;
pub mod nba;
pub mod pregame;
pub mod score;
pub mod soccer;

use crate::blit::{Canvas, PixelFormat, Slice, Source};
use crate::font::{self, Align, Scroll, Style};
use crate::geometry::{PLAY_SCROLL_PAUSE_MS, PLAY_TEXT, RenderSettings};
use crate::prepared::PreparedView;
use crate::time::{FrameElapsed, WallMs};
use crate::{DIM_GRAY, WHITE, generated, geometry, toast};
use scoreboard_model::ScoreboardSnapshot;
use scoreboard_model::snapshot::LogoRef;

/// A team crest: 24×24, RGB565, as `LogoPool` holds them.
pub const LOGO_EDGE: i32 = 24;
pub const LOGO_BYTES: usize = (LOGO_EDGE * LOGO_EDGE * 2) as usize;
pub type LogoSlot = [u8; LOGO_BYTES];

/// The app's crest pool, as the renderer sees it.
///
/// The snapshot carries a [`LogoRef`] handle rather than 1,152 bytes of pixels,
/// because the pool outlives any one commit and copying two crests per publish
/// would double the handoff cost for data that rarely changes. This is the
/// other half of that arrangement: a borrow of the pool, valid for the frame.
#[derive(Debug, Clone, Copy)]
pub struct Logos<'pool> {
    slots: &'pool [LogoSlot],
}

impl<'pool> Logos<'pool> {
    pub const fn new(slots: &'pool [LogoSlot]) -> Self {
        Logos { slots }
    }

    /// An empty pool — every crest resolves to nothing, and the screens that
    /// would draw one simply do not.
    pub const fn none() -> Self {
        Logos { slots: &[] }
    }

    /// The crest behind a handle, as a blit source. `None` for no handle, and
    /// for a handle the pool has no slot for.
    pub fn source(&self, handle: Option<LogoRef>) -> Option<Source<'pool>> {
        let slot = self.slots.get(handle?.0 as usize)?;
        Some(Source::new(
            slot,
            LOGO_EDGE,
            LOGO_EDGE,
            PixelFormat::Rgb565,
            None,
            None,
        ))
    }
}

/// Everything one frame reads. Assembled by the render loop, borrowed by every
/// screen.
///
/// The MicroPython renderers took eight positional arguments, three of them
/// bare millisecond integers distinguished only by parameter name. Here the
/// rails are distinct types and the rest travels as one borrow.
#[derive(Debug, Clone, Copy)]
pub struct Scene<'a> {
    pub snapshot: &'a ScoreboardSnapshot,
    pub prepared: &'a PreparedView,
    pub settings: &'a RenderSettings,
    pub logos: Logos<'a>,
    /// Wall rail: event windows and durations.
    pub now: WallMs,
    /// Frame rail, since the displayed view last changed identity.
    pub view: FrameElapsed,
    /// Frame rail, since the play line last changed.
    pub play: FrameElapsed,
}

impl Scene<'_> {
    /// The scroll feel for this screen's game-description lines.
    pub fn game_scroll(&self, pause_ms: u64) -> Scroll {
        self.settings.game_scroll(pause_ms)
    }
}

/// Who owns the bottom strip this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strip {
    /// A toast drew there; the screen skips its own content.
    Toast,
    /// The play flash drew there.
    Play,
    /// Nothing claimed it — the screen draws its own content.
    Free,
}

/// Resolve the bottom strip's owner and draw whichever of the two shared
/// claimants wins.
pub fn bottom_strip(canvas: &mut Canvas<'_>, scene: &Scene<'_>) -> Strip {
    if toast::strip(canvas, scene.snapshot, scene.now) {
        return Strip::Toast;
    }
    let play = &scene.snapshot.play;
    // The visibility window is a duration, so it rides the wall rail: a stalled
    // frame really did spend that time and the flash must not outstay it.
    let visible = !play.text.is_empty()
        && play.updated_ms != 0
        && scene.now.since(play.updated_ms).0 < scene.prepared.play_window_ms();
    if !visible {
        return Strip::Free;
    }
    let mut region = canvas.slice(PLAY_TEXT);
    font::draw(
        &mut region,
        &play.text,
        Align::Left,
        // The scroll offset is motion, so it rides the frame rail: a stalled
        // frame holds position instead of jumping a handful of pixels.
        scene.play.motion(),
        Style::new(&generated::UNSCII_16, WHITE),
        scene.game_scroll(PLAY_SCROLL_PAUSE_MS),
    );
    Strip::Play
}

/// Blit a crest, if there is one to blit.
pub fn logo(canvas: &mut Canvas<'_>, scene: &Scene<'_>, handle: Option<LogoRef>, slot: Slice) {
    if let Some(source) = scene.logos.source(handle) {
        canvas.blit(&source, slot.x, slot.y);
    }
}

/// Both crests, from a screen's own two slots.
pub fn both_logos(canvas: &mut Canvas<'_>, scene: &Scene<'_>, away: Slice, home: Slice) {
    logo(canvas, scene, scene.snapshot.away_logo, away);
    logo(canvas, scene, scene.snapshot.home_logo, home);
}

/// The column frame the live screens and the pregame screen share: a full-height
/// rule down the identity/data split, and a rule under the data column only, so
/// the bottom strip runs the full width beneath both.
///
/// A style-wide switch rather than a per-variant one: the screens have to read
/// consistently, so it is all or nothing.
pub fn column_dividers(
    canvas: &mut Canvas<'_>,
    settings: &RenderSettings,
    divider_x: Option<i32>,
    separator_y: Option<i32>,
) {
    if !settings.show_dividers {
        return;
    }
    if let Some(x) = divider_x {
        canvas.vline(x, 0, geometry::HEIGHT, DIM_GRAY);
    }
    if let Some(y) = separator_y {
        let start = divider_x.map_or(0, |x| x + 1);
        canvas.hline(start, y, geometry::WIDTH - start, DIM_GRAY);
    }
}

/// Winner emphasis: the winning side keeps its team color, the losing side goes
/// gray. No abbreviations — color is the whole signal.
pub fn winner_colors(home_won: bool, away: u16, home: u16) -> (u16, u16) {
    if home_won {
        (DIM_GRAY, home)
    } else {
        (away, DIM_GRAY)
    }
}
