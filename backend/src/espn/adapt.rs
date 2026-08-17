//! Cross-sport plumbing for the `scoreboard-espn` extractors: the quirk →
//! `tracing` bridge (DESIGN.md ruling 6), the shared error mapping that
//! reproduces `find_event`'s 404-vs-502 semantics (ruling 14: the counts are
//! exact precisely when the verdict consumes them), and the small
//! wire-vocabulary → domain-DTO conversions every sport adapter shares.

use scoreboard_espn::common::{Quirk, Quirks};

use crate::error::AppError;
use crate::shared::game::{GameState, Record};
use crate::shared::team::TeamColors;

/// picojson token scratch for one streaming pass: must hold the longest
/// contiguous string/number token in the body. ESPN link URLs run a few
/// hundred bytes and commentary lines a few KB; 64 KiB is deep margin.
pub(crate) const SCRATCH_LEN: usize = 64 * 1024;

/// Routes the shared crate's structured quirk events to `tracing` warns,
/// roughly matching the messages the deleted per-sport transforms emitted.
/// The crate never formats or logs (ruling 6); this is the backend's half.
pub(crate) struct TracingQuirks {
    sport: &'static str,
}

impl TracingQuirks {
    pub(crate) fn new(sport: &'static str) -> Self {
        Self { sport }
    }
}

impl Quirks for TracingQuirks {
    fn quirk(&mut self, quirk: Quirk) {
        let sport = self.sport;
        match quirk {
            Quirk::UnknownLivePhase => tracing::warn!(
                sport,
                "unknown live status description — treating as in-play"
            ),
            Quirk::UnknownBreakDescription => tracing::warn!(
                sport,
                "unknown live soccer status description — treating as active play"
            ),
            Quirk::UnknownFinalFlavor => tracing::warn!(
                sport,
                "unknown post-state soccer description — defaulting to full-time flavor"
            ),
            Quirk::UnknownInningHalf => tracing::warn!(
                sport,
                "ESPN shortDetail has non-inning prefix (delay/suspension?) — treating game as not displayable"
            ),
            Quirk::MalformedRecord => tracing::warn!(
                sport,
                "ESPN total record not in 'W-L' form — dropping record"
            ),
            Quirk::ClippedLineScore => tracing::warn!(
                sport,
                "line score longer than the wire's 255-entry cap — clipped"
            ),
            Quirk::WeatherDropped => tracing::warn!(
                sport,
                "ESPN weather block unusable (no non-numeric condition or no temperature) — dropping weather"
            ),
            Quirk::SituationDropped => tracing::warn!(
                sport,
                "football situation failed validation (down/yardLine/possession) — dropping situation"
            ),
            Quirk::DisplayClockFallback => tracing::warn!(
                sport,
                "unparseable soccer displayClock; falling back to numeric status.clock"
            ),
            Quirk::PeriodOutOfRange => tracing::warn!(
                sport,
                "period outside the wire's decodable range — passing through"
            ),
            Quirk::BoundedOverflow => tracing::warn!(
                sport,
                "bounded extract buffer overflowed — excess dropped (wire bytes unaffected)"
            ),
        }
    }
}

/// One aggregated warn for events the lenient per-event parse dropped —
/// the successor of `parse_events`' per-event warn. The streaming extractor
/// cannot capture the offending payload (nothing is buffered), so the count
/// and URL are what remains; capture the body upstream when debugging.
pub(crate) fn warn_failed_events(url: &str, failed: u64) {
    if failed > 0 {
        tracing::warn!(
            url = %url,
            failed,
            "skipped unparseable ESPN event(s) during lenient parse"
        );
    }
}

/// The tokenizer/engine rejected the body — today's whole-body deserialize
/// failure, surfaced exactly like `EspnClient::deserialize_logged` (502).
pub(crate) fn stream_error(url: &str, error: impl core::fmt::Debug) -> AppError {
    tracing::error!(url = %url, error = ?error, "ESPN JSON body failed streaming extraction");
    AppError::EspnDeserialize {
        url: url.to_string(),
        json_path: "$".to_string(),
        message: format!("body failed streaming extraction: {error:?}"),
    }
}

/// `$.events` present but not an array — `RawScoreboard` would fail the
/// whole response (502) before any per-event parsing.
pub(crate) fn events_malformed(url: &str) -> AppError {
    AppError::EspnDeserialize {
        url: url.to_string(),
        json_path: "events".to_string(),
        message: "`events` is not an array".to_string(),
    }
}

/// `find_event`'s verdict for an absent target id, verbatim: 404 is the
/// firmware's "game ended, drop it" signal, so it is only served when the
/// scoreboard parsed clean — with failures the game may be inside the
/// glitched subset and the caller gets an upstream 502 instead.
pub(crate) fn absent_verdict(game_id: &str, failed: u64, url: &str) -> AppError {
    if failed > 0 {
        AppError::EspnDeserialize {
            url: url.to_string(),
            json_path: "events".to_string(),
            message: format!(
                "{failed} event(s) unparseable; cannot distinguish 'ended' from 'glitched'"
            ),
        }
    } else {
        AppError::GameNotFound(game_id.to_string())
    }
}

/// The four lanes' transform-tier failure enums, folded to one vocabulary.
/// Each sport adapter maps its lane's nominal enum here (they carry the
/// same four cases under different names) and shares [`transform_error`].
pub(crate) enum TransformKind {
    Color,
    Score,
    StartTime,
    HomeAway,
}

/// A transform-tier failure on the requested game: today's hard 5xx, never
/// a skip (the two-tier error model, ruling 1). The status and error code
/// match the deleted transforms exactly; the streaming extractor cannot
/// carry the offending team/raw text, so the message payload is thinner.
pub(crate) fn transform_error(kind: TransformKind, url: &str) -> AppError {
    match kind {
        TransformKind::Color => AppError::InvalidTeamColor {
            team: "(not captured by streaming parse)".to_string(),
            raw: "(not captured by streaming parse)".to_string(),
        },
        TransformKind::Score => AppError::EspnDeserialize {
            url: url.to_string(),
            json_path: "events[?].competitions[0].competitors[?].score".to_string(),
            message: "competitor score failed to parse as u32".to_string(),
        },
        TransformKind::StartTime => AppError::EspnDeserialize {
            url: url.to_string(),
            json_path: "events[?].date".to_string(),
            message: "event date failed to parse".to_string(),
        },
        TransformKind::HomeAway => AppError::EspnDeserialize {
            url: url.to_string(),
            json_path: "events[?].competitions[0].competitors".to_string(),
            message: "expected one home and one away competitor".to_string(),
        },
    }
}

/// The extractors speak the wire crate's vocabulary; the JSON DTOs keep
/// their own (serde-derived) types. These conversions are the whole bridge.
pub(crate) fn domain_state(state: scoreboard_wire::GameState) -> GameState {
    match state {
        scoreboard_wire::GameState::Pregame => GameState::Pregame,
        scoreboard_wire::GameState::Live => GameState::Live,
        scoreboard_wire::GameState::Final => GameState::Final,
    }
}

pub(crate) fn domain_colors(colors: scoreboard_wire::TeamColors) -> TeamColors {
    TeamColors {
        primary: colors.primary,
        alternate: colors.alternate,
    }
}

pub(crate) fn domain_record(record: scoreboard_wire::Record) -> Record {
    Record {
        wins: record.wins,
        losses: record.losses,
    }
}
