//! One streaming detail extraction, whichever sport it is.
//!
//! The four S1 sport lanes landed with four different extractor surfaces —
//! quirk receivers borrowed (MLB, NBA) or owned (football, soccer), tallies
//! spelled `u32`/`usize`/`u16`, four transform-error enums, and an NBA
//! extractor that is a bare `Sink` the caller wraps in a `StreamMatcher`
//! itself. All four differences are absorbed in this file so the poller sees
//! one `new`/`write`/`finish` and one verdict vocabulary.

use scoreboard_espn::common::NoRows;
use scoreboard_espn::path::StreamMatcher;
use scoreboard_espn::{football, mlb, nba, soccer};

use crate::{ByRef, Counts, DirectExtract, Error, Feed, Quirks, TransformError, count};

/// What a detail extraction concluded about the requested game.
///
/// The 404-vs-502 split is the backend's `find_event` rule and it is
/// load-bearing on the device: [`Outcome::NotFound`] is the firmware's "this
/// game is gone, drop it" signal, so a scoreboard that merely *failed to
/// parse* must never produce it. Every lane validates every event until the
/// target is found (`scoreboard-espn` DESIGN.md ruling 14) precisely so the
/// failure count is exact at the moment this verdict is taken.
#[derive(Debug)]
#[allow(
    clippy::large_enum_variant,
    reason = "Found carries the whole bounded extract; boxing needs alloc"
)]
pub enum Outcome {
    Found(DirectExtract),
    /// The target is not on a cleanly parsed board, or it is there with no
    /// competition, or it is a live MLB game vetoed on its `shortDetail`
    /// (rain delay, suspension). All three are today's 404.
    ///
    /// The no-competition case stays `NotFound` even when sibling events
    /// failed: the id *was* on the board, so its absence is real.
    NotFound,
    /// The target is absent *and* at least one event failed to parse, so the
    /// game may be inside the glitched subset. Today's 502 — retry, never
    /// "game ended".
    Glitched,
}

/// The result of a detail extraction.
#[derive(Debug)]
pub struct DetailReport {
    pub outcome: Outcome,
    /// The per-event tallies, where the lane surfaces them.
    ///
    /// `None` on MLB's not-found paths only: that lane folds the
    /// glitched-vs-ended verdict inside the crate and drops the tallies on the
    /// way out. The verdict itself is never affected — only the diagnostic is
    /// — and closing the gap belongs to the pending API-unification pass
    /// rather than to a second fold here.
    pub counts: Option<Counts>,
}

/// A streaming detail extraction over one ESPN scoreboard body.
///
/// Feed the body with [`write`](Self::write) as it arrives — chunk boundaries
/// are irrelevant, which the crate's tests pin at sizes 1 and 4096 — then
/// [`finish`](Self::finish). `scratch` is picojson's token buffer and must
/// hold the longest contiguous string or number token in the body.
///
/// # Size
///
/// Measured on `thumbv8m.main-none-eabihf`: **30,856 bytes**, which is
/// soccer's variant exactly (MLB 7,424; NBA 6,360; football 10,536). Sizing to
/// the largest variant costs nothing a hand-written `match` on sport would not
/// also pay — one stream is live at a time either way.
///
/// The number matters because it is an order of magnitude more than the
/// 2,916-byte [`DirectExtract`] it produces: on this path the *in-flight*
/// state is the memory cost, not the result. Put one in a `StaticCell`, never
/// on a task stack. And S3-DESIGN decision 3 runs a list stream alongside this
/// one over the same body, so a soccer poll's concurrent extractor state is
/// 30,856 + 30,992 = 61,848 bytes before either picojson scratch — priced for
/// the poller lane rather than assumed by it.
pub struct DetailStream<'c, 's, Q: Quirks> {
    inner: Inner<'c, 's, Q>,
}

#[allow(
    clippy::large_enum_variant,
    reason = "every variant is a bounded streaming scratch and only one is \
              live at a time; boxing needs alloc, which SPEC §10 forbids"
)]
enum Inner<'c, 's, Q: Quirks> {
    Mlb(mlb::DetailExtractor<'c, 's, Q>),
    // NBA's extractor is the sink itself, not a driver over one.
    Nba(StreamMatcher<'static, 's, nba::Extractor<'c, NoRows, Q>>),
    Football(football::DetailExtractor<'s, ByRef<'c, Q>>),
    Soccer(soccer::GameExtractor<'s, ByRef<'c, Q>>),
}

impl<'c, 's, Q: Quirks> DetailStream<'c, 's, Q> {
    pub fn new(
        feed: Feed,
        game_id: &'c str,
        quirks: &'c mut Q,
        scratch: &'s mut [u8],
    ) -> Result<Self, Error> {
        let inner = match feed {
            Feed::Mlb => Inner::Mlb(mlb::DetailExtractor::new(game_id, quirks, scratch)?),
            Feed::Nba => Inner::Nba(StreamMatcher::new(
                nba::PATHS,
                nba::Extractor::game_detail(game_id, quirks),
                scratch,
            )?),
            Feed::Football { college } => Inner::Football(football::DetailExtractor::new(
                game_id,
                college,
                ByRef(quirks),
                scratch,
            )?),
            Feed::Soccer => {
                Inner::Soccer(soccer::GameExtractor::new(game_id, ByRef(quirks), scratch)?)
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

    /// `inline(always)` is a stack decision, not a speed one: `self` is the
    /// enum, sized by its largest variant, and consuming it by value in a
    /// non-inlined callee materializes a whole extra copy in a *nested*
    /// frame — measured as a 32 KB `sub sp` under the poller's own ~38 KB
    /// poll frame on the RP2350, which together overran the stack guard.
    /// Fused into the caller, the copy shares the frame the stream already
    /// occupies.
    #[inline(always)]
    pub fn finish(self) -> Result<DetailReport, Error> {
        match self.inner {
            Inner::Mlb(extractor) => finish_mlb(extractor),
            Inner::Nba(matcher) => finish_nba(matcher),
            Inner::Football(extractor) => finish_football(extractor),
            Inner::Soccer(extractor) => finish_soccer(extractor),
        }
    }
}

/// Absent-target verdict, the one rule all four lanes share: a clean board
/// means the game really is gone, any parse failure means it might not be.
fn absent(counts: Counts) -> Outcome {
    if counts.failed > 0 {
        Outcome::Glitched
    } else {
        Outcome::NotFound
    }
}

fn finish_mlb<Q: Quirks>(
    extractor: mlb::DetailExtractor<'_, '_, Q>,
) -> Result<DetailReport, Error> {
    match extractor.finish() {
        Ok((extract, counts)) => Ok(DetailReport {
            outcome: Outcome::Found(DirectExtract::Mlb(extract)),
            counts: Some(Counts {
                ok: counts.ok,
                failed: counts.failed,
            }),
        }),
        Err(mlb::DetailError::Stream(error)) => Err(Error::Stream(error)),
        Err(mlb::DetailError::Events) => Err(Error::MalformedBody),
        // This lane already folded the verdict; the tallies behind it are not
        // exposed, hence the `None` documented on `DetailReport::counts`.
        Err(mlb::DetailError::NotFound) => Ok(DetailReport {
            outcome: Outcome::NotFound,
            counts: None,
        }),
        Err(mlb::DetailError::Glitched) => Ok(DetailReport {
            outcome: Outcome::Glitched,
            counts: None,
        }),
        Err(mlb::DetailError::Transform(kind)) => Err(Error::Transform(match kind {
            mlb::TransformError::Date => TransformError::StartTime,
            mlb::TransformError::HomeAway => TransformError::HomeAway,
            mlb::TransformError::Score => TransformError::Score,
            mlb::TransformError::Color => TransformError::Color,
        })),
    }
}

fn finish_nba<Q: Quirks>(
    matcher: StreamMatcher<'static, '_, nba::Extractor<'_, NoRows, Q>>,
) -> Result<DetailReport, Error> {
    let extractor = matcher.finish()?;
    let stats = extractor.stats();
    if stats.events_malformed {
        return Err(Error::MalformedBody);
    }
    let counts = Counts {
        ok: stats.ok,
        failed: stats.failed,
    };
    let outcome = extractor
        .into_detail()
        .expect("constructed in detail mode by DetailStream::new");
    Ok(DetailReport {
        outcome: match outcome {
            nba::DetailOutcome::Found(extract) => Outcome::Found(DirectExtract::Nba(extract)),
            nba::DetailOutcome::NoCompetition => Outcome::NotFound,
            nba::DetailOutcome::NotFound => absent(counts),
            nba::DetailOutcome::Rejected(kind) => {
                return Err(Error::Transform(match kind {
                    nba::TransformError::StartTime => TransformError::StartTime,
                    nba::TransformError::HomeAway => TransformError::HomeAway,
                    nba::TransformError::Score => TransformError::Score,
                    nba::TransformError::Color => TransformError::Color,
                }));
            }
        },
        counts: Some(counts),
    })
}

fn finish_football<Q: Quirks>(
    extractor: football::DetailExtractor<'_, ByRef<'_, Q>>,
) -> Result<DetailReport, Error> {
    let report = extractor.finish().map_err(|error| match error {
        football::FootballError::Stream(error) => Error::Stream(error),
        football::FootballError::MalformedEvents => Error::MalformedBody,
        football::FootballError::Extract(kind) => {
            Error::Transform(crate::fold_football_kind(kind))
        }
    })?;
    let counts = Counts {
        ok: count(report.counts.ok),
        failed: count(report.counts.failed),
    };
    Ok(DetailReport {
        outcome: match report.outcome {
            football::DetailOutcome::Found(extract) => {
                Outcome::Found(DirectExtract::Football(extract))
            }
            football::DetailOutcome::NoCompetitions => Outcome::NotFound,
            football::DetailOutcome::Absent => absent(counts),
        },
        counts: Some(counts),
    })
}

fn finish_soccer<Q: Quirks>(
    extractor: soccer::GameExtractor<'_, ByRef<'_, Q>>,
) -> Result<DetailReport, Error> {
    let report = extractor.finish().map_err(|error| match error {
        soccer::ExtractError::Stream(error) => Error::Stream(error),
        soccer::ExtractError::MalformedBody => Error::MalformedBody,
        soccer::ExtractError::Transform(kind) => Error::Transform(crate::fold_soccer_kind(kind)),
    })?;
    let counts = Counts {
        ok: u32::from(report.ok),
        failed: u32::from(report.failed),
    };
    Ok(DetailReport {
        outcome: match report.outcome {
            soccer::GameOutcome::Found(extract) => Outcome::Found(DirectExtract::Soccer(extract)),
            soccer::GameOutcome::NoCompetition => Outcome::NotFound,
            soccer::GameOutcome::Absent => absent(counts),
        },
        counts: Some(counts),
    })
}
