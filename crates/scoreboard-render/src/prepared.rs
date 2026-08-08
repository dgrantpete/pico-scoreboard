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

use crate::qr::{self, QrBitmap};
use scoreboard_model::snapshot::SSID;
use scoreboard_model::{ScoreboardSnapshot, Text};

pub struct PreparedView {
    /// The commit this view was built from; `None` before the first sync.
    commit_seq: Option<u32>,
    qr: QrBitmap,
    /// The SSID [`PreparedView::qr`] encodes. The QR is the one derivation
    /// expensive enough to be worth a finer key than the commit sequence:
    /// re-encoding it on an unrelated commit would be milliseconds of
    /// Reed-Solomon for an identical bitmap.
    qr_ssid: Text<SSID>,
}

impl PreparedView {
    pub const fn new() -> Self {
        PreparedView {
            commit_seq: None,
            qr: QrBitmap::empty(),
            qr_ssid: Text::new(),
        }
    }

    /// Bring the view up to date with `snapshot`, rebuilding if the commit
    /// changed.
    ///
    /// Call once per frame, before rendering. Returns whether anything was
    /// rebuilt — which is exactly "this frame's content differs from the last
    /// one's", and therefore what the render loop's static-screen skip wants.
    pub fn sync(&mut self, snapshot: &ScoreboardSnapshot) -> bool {
        if self.commit_seq == Some(snapshot.commit_seq) {
            return false;
        }
        self.commit_seq = Some(snapshot.commit_seq);
        self.rebuild(snapshot);
        true
    }

    fn rebuild(&mut self, snapshot: &ScoreboardSnapshot) {
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
    }

    /// The setup screen's Wi-Fi QR. Empty when there is none.
    pub fn qr(&self) -> &QrBitmap {
        &self.qr
    }

    /// The commit this view was built from.
    pub fn commit_seq(&self) -> Option<u32> {
        self.commit_seq
    }
}

impl Default for PreparedView {
    fn default() -> Self {
        Self::new()
    }
}
