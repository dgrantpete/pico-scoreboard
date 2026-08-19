//! One streaming games-list extraction, whichever sport it is.
//!
//! The mirror of [`crate::detail`], and the file the crate's charter said
//! could not be written: until the list extractors converged on owning their
//! sinks (2026-08-19), MLB borrowed a caller-owned trait object and NBA
//! borrowed a caller-owned closure, and no single type could be both. They
//! own their sinks now, so the four-way dispatch the poller would otherwise
//! carry lives here instead, exactly as it does for details.
//!
//! What is absorbed: MLB and NBA take their quirk receiver by `&mut`,
//! football and soccer by value ([`ByRef`] bridges); NBA is a bare sink the
//! caller drives through a [`StreamMatcher`]; and the three spellings of the
//! per-event tally (`u32`, `usize`, `u16`) fold into one [`Counts`]. The rows
//! themselves need no absorbing — [`ListSink`] and [`ListRow`] are already
//! the shared vocabulary, defined one crate down.

use scoreboard_espn::common::ListSink;
use scoreboard_espn::path::StreamMatcher;
use scoreboard_espn::{football, mlb, nba, soccer};

use crate::{ByRef, Counts, Error, Feed, Quirks, count};

/// The result of a list extraction: the caller's sink handed back, and the
/// tallies behind the rows it received.
///
/// Unlike [`DetailReport`](crate::DetailReport) there is no verdict here — a
/// list has no target to be absent — and `counts` is not optional: every lane
/// surfaces its list tallies. `failed > 0` with a quiet sink is how a fully
/// glitched board is told apart from a day with no games, which is the same
/// evidence the poller's keep-cached-slate-on-failure rule already weighs.
#[derive(Debug)]
pub struct ListReport<L> {
    pub sink: L,
    pub counts: Counts,
}

/// A streaming games-list extraction over one ESPN scoreboard body.
///
/// Feed the body with [`write`](Self::write) as it arrives — chunk boundaries
/// are irrelevant — then [`finish`](Self::finish) to get the sink back.
/// `scratch` is picojson's token buffer, same contract as
/// [`DetailStream`](crate::DetailStream)'s.
///
/// The in-flight state is the memory cost here too: hold one in a
/// `StaticCell`, never on a task stack, and size it at integration — the
/// device numbers belong in BUDGET.md once measured there, not estimated
/// here.
pub struct ListStream<'c, 's, L: ListSink, Q: Quirks> {
    inner: Inner<'c, 's, L, Q>,
}

#[allow(
    clippy::large_enum_variant,
    reason = "every variant is a bounded streaming scratch and only one is \
              live at a time; boxing needs alloc, which SPEC §10 forbids"
)]
enum Inner<'c, 's, L: ListSink, Q: Quirks> {
    Mlb(mlb::ListExtractor<'c, 's, L, Q>),
    // NBA's extractor is the sink itself, not a driver over one.
    Nba(StreamMatcher<'static, 's, nba::Extractor<'c, L, Q>>),
    Football(football::ListExtractor<'s, L, ByRef<'c, Q>>),
    Soccer(soccer::ListExtractor<'s, L, ByRef<'c, Q>>),
}

impl<'c, 's, L: ListSink, Q: Quirks> ListStream<'c, 's, L, Q> {
    pub fn new(
        feed: Feed,
        sink: L,
        quirks: &'c mut Q,
        scratch: &'s mut [u8],
    ) -> Result<Self, Error> {
        let inner = match feed {
            Feed::Mlb => Inner::Mlb(mlb::ListExtractor::new(sink, quirks, scratch)?),
            Feed::Nba => Inner::Nba(StreamMatcher::new(
                nba::PATHS,
                nba::Extractor::games_list(sink, quirks),
                scratch,
            )?),
            // The college flag gates the pregame rank line, which no list row
            // carries, so the two football feeds are one list extraction.
            Feed::Football { .. } => {
                Inner::Football(football::ListExtractor::new(sink, ByRef(quirks), scratch)?)
            }
            Feed::Soccer => {
                Inner::Soccer(soccer::ListExtractor::new(sink, ByRef(quirks), scratch)?)
            }
        };
        Ok(Self { inner })
    }

    pub fn write(&mut self, chunk: &[u8]) -> Result<(), Error> {
        match &mut self.inner {
            Inner::Mlb(extractor) => extractor.write(chunk)?,
            Inner::Nba(matcher) => matcher.write(chunk)?,
            Inner::Football(extractor) => extractor.write(chunk)?,
            Inner::Soccer(extractor) => extractor.write(chunk)?,
        }
        Ok(())
    }

    /// `inline(always)` for the stack, not for speed — the reasoning and the
    /// on-silicon measurement live on [`DetailStream::finish`], and the same
    /// by-value-enum arithmetic applies here.
    ///
    /// [`DetailStream::finish`]: crate::DetailStream::finish
    #[inline(always)]
    pub fn finish(self) -> Result<ListReport<L>, Error> {
        match self.inner {
            Inner::Mlb(extractor) => {
                let (sink, counts) = extractor.finish().map_err(|error| match error {
                    mlb::ListError::Stream(error) => Error::Stream(error),
                    mlb::ListError::Events => Error::MalformedBody,
                })?;
                Ok(ListReport {
                    sink,
                    counts: Counts {
                        ok: counts.ok,
                        failed: counts.failed,
                    },
                })
            }
            Inner::Nba(matcher) => {
                let extractor = matcher.finish()?;
                let stats = extractor.stats();
                if stats.events_malformed {
                    return Err(Error::MalformedBody);
                }
                let counts = Counts {
                    ok: stats.ok,
                    failed: stats.failed,
                };
                let sink = extractor
                    .into_list()
                    .expect("constructed in list mode by ListStream::new");
                Ok(ListReport { sink, counts })
            }
            Inner::Football(extractor) => {
                let report = extractor.finish().map_err(fold_football)?;
                Ok(ListReport {
                    sink: report.entries,
                    counts: Counts {
                        ok: count(report.counts.ok),
                        failed: count(report.counts.failed),
                    },
                })
            }
            Inner::Soccer(extractor) => {
                let report = extractor.finish().map_err(fold_soccer)?;
                Ok(ListReport {
                    sink: report.entries,
                    counts: Counts {
                        ok: u32::from(report.ok),
                        failed: u32::from(report.failed),
                    },
                })
            }
        }
    }
}

/// List mode never reaches the transform tier — there is no target event to
/// transform — but the lanes' error enums are shared with detail mode, so the
/// folds must still be total. Routing the unreachable arms through the same
/// mapping [`crate::detail`] uses keeps them honest if that ever changes.
fn fold_football(error: football::FootballError) -> Error {
    match error {
        football::FootballError::Stream(error) => Error::Stream(error),
        football::FootballError::MalformedEvents => Error::MalformedBody,
        football::FootballError::Extract(kind) => Error::Transform(crate::fold_football_kind(kind)),
    }
}

fn fold_soccer(error: soccer::ExtractError) -> Error {
    match error {
        soccer::ExtractError::Stream(error) => Error::Stream(error),
        soccer::ExtractError::MalformedBody => Error::MalformedBody,
        soccer::ExtractError::Transform(kind) => Error::Transform(crate::fold_soccer_kind(kind)),
    }
}
