//! How much of the image has arrived, and when that is worth redrawing.
//!
//! # The MicroPython lesson this encodes
//!
//! `main.py`'s `on_progress` commits **only on a percent change**, and its
//! comment says why: "~100 commits per image, vs one per 4 KB chunk". At 4 KB
//! chunks a 700 KB image is 175 chunks, so the saving there was modest — but
//! the Rust download is chunked to the flash erase page for reasons of its own,
//! and every commit crosses a core boundary and wakes core 1 out of its skip to
//! redraw a progress bar that has not moved. Committing per chunk would mean
//! the panel spends the download rendering frames identical to the last one,
//! during the one phase where core 1 is already being starved by flash writes
//! parking it.
//!
//! So the rule is the same rule, and it lives here where the arithmetic can be
//! tested rather than in the download loop where it cannot: [`advance`] answers
//! `Some(percent)` exactly when the number a person would read has changed.
//!
//! [`advance`]: Progress::advance

/// The download's byte and percent accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    total: u32,
    done: u32,
    /// The last percent handed out, so a redraw happens once per value.
    /// `None` before the first chunk, which is what makes 0% a real update —
    /// the screen has to be entered somehow.
    reported: Option<u8>,
}

impl Progress {
    /// `total` is the manifest's size. It is never zero — the manifest parser
    /// rejects that — but a zero here would still not divide, see [`percent`].
    ///
    /// [`percent`]: Progress::percent
    pub const fn new(total: u32) -> Progress {
        Progress {
            total,
            done: 0,
            reported: None,
        }
    }

    /// Count `bytes` more, and answer `Some(percent)` if the display should
    /// change.
    pub fn advance(&mut self, bytes: u32) -> Option<u8> {
        self.done = self.done.saturating_add(bytes).min(self.total);
        let percent = self.percent();
        (self.reported != Some(percent)).then(|| {
            self.reported = Some(percent);
            percent
        })
    }

    /// 0..=100.
    ///
    /// The multiply is in `u64` because `done * 100` overflows a `u32` at
    /// 42 MB, and while no image is that large, the arithmetic that silently
    /// stops being true above a size limit nobody wrote down is exactly the
    /// kind that gets found by a bigger image two years later.
    pub const fn percent(&self) -> u8 {
        if self.total == 0 {
            return 100;
        }
        (self.done as u64 * 100 / self.total as u64) as u8
    }

    pub const fn done(&self) -> u32 {
        self.done
    }

    pub const fn total(&self) -> u32 {
        self.total
    }

    /// Whether every byte has arrived.
    pub const fn complete(&self) -> bool {
        self.done >= self.total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_chunk_always_reports_so_the_screen_can_be_entered() {
        let mut progress = Progress::new(100_000);
        assert_eq!(
            progress.advance(1),
            Some(0),
            "0% is a real update: it is what puts the updating screen up"
        );
    }

    #[test]
    fn a_chunk_that_does_not_move_the_percent_reports_nothing() {
        let mut progress = Progress::new(100_000);
        progress.advance(1000);
        assert_eq!(progress.percent(), 1);
        assert_eq!(progress.advance(1), None, "still 1%");
        assert_eq!(progress.advance(998), None, "still 1%");
        assert_eq!(progress.advance(1), Some(2), "crossed into 2%");
    }

    #[test]
    fn a_whole_download_reports_exactly_a_hundred_and_one_times() {
        // 0 through 100 inclusive. This is the number the module docs promise
        // and the number the panel redraws.
        let total = 712_345;
        let mut progress = Progress::new(total);
        let mut reports = 0;
        let mut remaining = total;
        while remaining > 0 {
            let chunk = remaining.min(4096);
            if progress.advance(chunk).is_some() {
                reports += 1;
            }
            remaining -= chunk;
        }
        assert_eq!(reports, 101);
        assert_eq!(progress.percent(), 100);
        assert!(progress.complete());
    }

    #[test]
    fn a_chunk_larger_than_what_is_left_does_not_overshoot() {
        let mut progress = Progress::new(1000);
        assert_eq!(progress.advance(5000), Some(100));
        assert_eq!(progress.done(), 1000, "clamped to the total");
        assert_eq!(progress.percent(), 100);
        assert!(progress.complete());
    }

    #[test]
    fn an_image_larger_than_a_u32_percent_multiply_still_counts() {
        // 100 MB: `done * 100` overflows a u32 well before the end. The
        // percentages have to stay monotone all the way up.
        let total = 100 * 1024 * 1024;
        let mut progress = Progress::new(total);
        let mut previous = 0;
        let mut remaining = total;
        while remaining > 0 {
            let chunk = remaining.min(64 * 1024);
            if let Some(percent) = progress.advance(chunk) {
                assert!(percent >= previous, "{percent} went backwards from {previous}");
                previous = percent;
            }
            remaining -= chunk;
        }
        assert_eq!(previous, 100);
    }

    #[test]
    fn a_zero_total_reads_as_complete_rather_than_dividing_by_zero() {
        let progress = Progress::new(0);
        assert_eq!(progress.percent(), 100);
        assert!(progress.complete());
    }

    #[test]
    fn a_download_that_never_finishes_never_reads_complete() {
        let mut progress = Progress::new(1000);
        progress.advance(999);
        assert_eq!(progress.percent(), 99);
        assert!(!progress.complete());
    }
}
