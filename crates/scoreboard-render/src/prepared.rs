//! Everything the renderer derives from a snapshot, built once per commit.
//!
//! # The rule
//!
//! `scoreboard-model`'s snapshot carries semantics and no pixels: strings,
//! numbers, colors, and the *inputs* to geometry. Anything that has to be
//! computed from those — a QR bitmap, a measured scroll window, a projected
//! field coordinate — is derived here, keyed on
//! [`ScoreboardSnapshot::commit_seq`], and read unchanged by every frame until
//! the next commit.
//!
//! That split is the whole reason the state machine can be tested without a
//! panel and the render path can stay a pure reader. It is *not* an
//! optimisation the port inherited blindly: `state.py` carried derived pixels
//! because glyph-looping three line-score rows on core 1 measured ~41 ms of a
//! 50 ms frame in MicroPython. The constraint was real, but it was a constraint
//! on *when* the work happens, not on which crate holds it.
//!
//! # What that constraint costs in Rust
//!
//! Much less. The MicroPython answer to the 41 ms measurement was to
//! pre-render every scrolling line into a 1-bit strip on core 0 so core 1 could
//! draw it with one blit — a pool per line, a fallback path for text too wide
//! for the pool, and a registry of it all. None of that is here: a glyph blit
//! is a few dozen instructions, [`crate::font::draw`] skips glyphs scrolled
//! outside the region without touching a pixel, so a 255-glyph play line costs
//! the 76 px window it shows through and nothing more. The strips, the pools,
//! their capacity invariants and their fallback disappear; what stays is this
//! type, for derivations that are genuinely expensive or genuinely stateful.
//!
//! # Ownership
//!
//! One instance, owned by the render loop (Phase 3) alongside its
//! [`FrameRail`](crate::time::FrameRail). Renderers take `&PreparedView` — a
//! shared borrow, so nothing below [`sync`](PreparedView::sync) can write to
//! it. That is the compile-checked form of MicroPython's rule that cross-frame
//! state lives in exactly one loop-local object.

use crate::font::{self, Scroll};
use crate::generated::{SPLEEN_5X8, UNSCII_16};
use crate::geometry::{
    PLAY_SCROLL_PAUSE_MS, PLAY_TEXT, PREGAME, PREGAME_INFO_DWELL_MS, PREGAME_SCROLL, RenderSettings,
};
use crate::qr::{self, QrBitmap};
use scoreboard_model::snapshot::SSID;
use scoreboard_model::{Millis, ScoreboardSnapshot, Text};

#[derive(Debug)]
pub struct PreparedView {
    /// The commit this view was built from; `None` before the first sync.
    commit_seq: Option<u32>,
    qr: QrBitmap,
    /// The SSID [`PreparedView::qr`] encodes. The QR is the one derivation
    /// expensive enough to be worth a finer key than the commit sequence:
    /// re-encoding it on an unrelated commit would be milliseconds of
    /// Reed-Solomon for an identical bitmap.
    qr_ssid: Text<SSID>,
    play_window_ms: Millis,
    pregame: PregameCycle,
}

impl PreparedView {
    pub const fn new() -> Self {
        PreparedView {
            commit_seq: None,
            qr: QrBitmap::empty(),
            qr_ssid: Text::new(),
            play_window_ms: 0,
            pregame: PregameCycle::EMPTY,
        }
    }

    /// Bring the view up to date with `snapshot`, rebuilding if the commit
    /// changed.
    ///
    /// Call once per frame, before rendering. Returns whether anything was
    /// rebuilt — which is exactly "this frame's content differs from the last
    /// one's", and therefore what the render loop's static-screen skip wants.
    pub fn sync(&mut self, snapshot: &ScoreboardSnapshot, settings: &RenderSettings) -> bool {
        if self.commit_seq == Some(snapshot.commit_seq) {
            return false;
        }
        self.commit_seq = Some(snapshot.commit_seq);
        self.rebuild(snapshot, settings);
        true
    }

    fn rebuild(&mut self, snapshot: &ScoreboardSnapshot, settings: &RenderSettings) {
        let ssid = &snapshot.setup.ap_ssid;
        // An empty SSID leaves the previous code alone rather than clearing it,
        // matching `set_setup_mode`'s `if ap_ssid:` guard: a setup screen
        // published without an SSID is a re-publish of context, not a new
        // network to join.
        if !ssid.is_empty() && self.qr_ssid != *ssid {
            self.qr_ssid = ssid.clone();
            // A failure leaves the bitmap empty and the setup screen draws
            // without a QR — the Python caught, logged and did the same.
            self.qr.encode(&qr::wifi_payload(ssid));
        }

        self.play_window_ms = play_window_ms(&snapshot.play.text, settings);
        self.pregame = PregameCycle::build(
            &snapshot.pregame.info_primary,
            &snapshot.pregame.info_secondary,
        );
    }

    /// The setup screen's Wi-Fi QR. Empty when there is none.
    pub fn qr(&self) -> &QrBitmap {
        &self.qr
    }

    /// How long the current play flash stays up.
    pub fn play_window_ms(&self) -> Millis {
        self.play_window_ms
    }

    /// The pregame info line's phase schedule.
    pub fn pregame(&self) -> &PregameCycle {
        &self.pregame
    }

    /// The commit this view was built from.
    pub fn commit_seq(&self) -> Option<u32> {
        self.commit_seq
    }
}

/// How long a play flash stays on screen: exactly one scroll cycle — the
/// opening dwell, the scroll to the end, and the closing dwell — so a long
/// line gets the time it needs and a short one does not linger.
///
/// `display.play_text_display_ms`, which core 0 called at commit time so core 1
/// never measured text. Core 1 does not measure text here either; the prepared
/// view does, once per commit.
///
/// Depends on the configured scroll speed, so a speed change mid-flash leaves
/// the current window alone and self-corrects on the next play — the behavior
/// `set_scroll_speed`'s docstring describes.
pub fn play_window_ms(text: &str, settings: &RenderSettings) -> Millis {
    scroll_cycle_ms(
        font::measure(text, &UNSCII_16),
        PLAY_TEXT.width,
        settings.game_scroll(PLAY_SCROLL_PAUSE_MS),
    )
}

/// One full `calculate_scroll_offset` cycle for text of `text_width` in a
/// `window` px slot: pause, scroll, pause. Text that fits shows for just the
/// two pauses.
fn scroll_cycle_ms(text_width: i32, window: i32, scroll: Scroll) -> Millis {
    let max_scroll = text_width - window;
    let scroll_ms = if max_scroll > 0 {
        max_scroll as u64 * 1000 / scroll.pixels_per_second as u64
    } else {
        0
    };
    scroll.pause_ms + scroll_ms + scroll.pause_ms
}

/// The pregame info line's two-phase cycle, as cumulative dwell ends.
///
/// The renderer locates the active phase with `elapsed % total`, so the whole
/// cycle is a pure function of the frame rail. Each phase stays up for at least
/// [`PREGAME_INFO_DWELL_MS`] and never less than one full scroll cycle of its
/// own text — the coupling that keeps a line from being swapped out mid-scroll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PregameCycle {
    ends: [Millis; 2],
    /// Which snapshot line each phase draws. A skipped empty line shifts the
    /// ones after it, so the phase index is not the line index.
    lines: [PregameLine; 2],
    count: u8,
}

/// Which of the pregame view's two info lines a cycle phase shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PregameLine {
    /// Stadium, or the league name on the sports that lead with it.
    Primary,
    /// Weather, or the stadium on the sports that lead with the league.
    Secondary,
}

impl PregameCycle {
    const EMPTY: Self = PregameCycle {
        ends: [0; 2],
        lines: [PregameLine::Primary; 2],
        count: 0,
    };

    fn build(primary: &str, secondary: &str) -> Self {
        let mut cycle = Self::EMPTY;
        let mut running = 0;
        // Empty lines are skipped, not drawn blank: NBA fills only the first.
        for (line, text) in [
            (PregameLine::Primary, primary),
            (PregameLine::Secondary, secondary),
        ] {
            if text.is_empty() {
                continue;
            }
            running += Self::dwell(text);
            cycle.ends[cycle.count as usize] = running;
            cycle.lines[cycle.count as usize] = line;
            cycle.count += 1;
        }
        cycle
    }

    fn dwell(text: &str) -> Millis {
        scroll_cycle_ms(
            font::measure(text, &SPLEEN_5X8),
            PREGAME.info_cycle.width,
            PREGAME_SCROLL,
        )
        .max(PREGAME_INFO_DWELL_MS)
    }

    /// Which line is showing at `elapsed`, and how long it has been showing.
    ///
    /// `None` when there is nothing to cycle. The second value is the phase's
    /// own elapsed time, which is what its scroll clock counts from — the
    /// `_cycle_phase` early-return bug the Core-1 scratch contract was written
    /// after came from getting exactly this wrong.
    pub fn phase_at(&self, elapsed: Millis) -> Option<(PregameLine, Millis)> {
        let total = *self.ends.get(self.count.checked_sub(1)? as usize)?;
        if total == 0 {
            return None;
        }
        let position = elapsed % total;
        let mut start = 0;
        for index in 0..self.count as usize {
            if position < self.ends[index] {
                return Some((self.lines[index], position - start));
            }
            start = self.ends[index];
        }
        // Unreachable: `position` is always below the last end. Degrading to
        // the last phase beats returning something stale.
        let last = self.count as usize - 1;
        Some((self.lines[last], position - start))
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

impl Default for PreparedView {
    fn default() -> Self {
        Self::new()
    }
}
