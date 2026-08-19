//! The second body a live soccer poll needs.
//!
//! ESPN's scoreboard carries everything `scoreboard_wire::soccer` encodes
//! except the commentary line, which lives on the per-event summary endpoint
//! (390–456 KB live, which streaming makes a non-issue — S1 measured ~30 ms of
//! CPU and put 39,649 real summaries through this extractor clean). This is a
//! thin pass-through over `scoreboard_espn::soccer::SummaryExtractor`; it
//! exists so the poller reaches for one crate and one error type, not two.

use scoreboard_espn::soccer::{SummaryExtractor, SummaryOutcome};

use crate::Error;

/// Streams one per-event summary body down to its latest commentary line.
///
/// Best-effort by contract: the caller attaches the result with
/// [`DirectExtract::set_commentary`](crate::DirectExtract::set_commentary) and
/// serves the game either way. A malformed summary comes back as
/// `SummaryOutcome { commentary: None, malformed: true }` rather than an
/// error, matching the backend's degrade-with-a-warn; only an unreadable body
/// reaches [`Error`], and the poller drops that too.
pub struct CommentaryStream<'s> {
    inner: SummaryExtractor<'s>,
}

impl<'s> CommentaryStream<'s> {
    pub fn new(scratch: &'s mut [u8]) -> Result<Self, Error> {
        Ok(Self {
            inner: SummaryExtractor::new(scratch)?,
        })
    }

    pub fn write(&mut self, chunk: &[u8]) -> Result<(), Error> {
        self.inner.write(chunk)?;
        Ok(())
    }

    pub fn finish(self) -> Result<SummaryOutcome, Error> {
        Ok(self.inner.finish()?)
    }
}
