//! Cross-sport semantics shared by the sport tables.
//!
//! Everything here replicates a specific backend behavior the inventories
//! documented; the DESIGN.md rulings say which. Divergence from the backend
//! is a parity bug even when the backend's behavior looks accidental.

use scoreboard_wire::{MAX_STRING_BYTES, truncate_utf8};

/// Extract-struct string storage: bound at the wire cap, never tighter
/// (ruling 2). Tighter bounds live downstream in `scoreboard-model`.
pub type EText = heapless::String<MAX_STRING_BYTES>;

/// Copy into bounded storage with the wire's own truncation — the shared
/// `truncate_utf8` is what makes truncate-at-copy == truncate-at-encode.
pub fn set_text<const N: usize>(dst: &mut heapless::String<N>, src: &str) {
    dst.clear();
    // truncate_utf8 guarantees the result fits N, so push_str cannot fail.
    let _ = dst.push_str(truncate_utf8(src, N));
}

/// Structured quirk diagnostics (ruling 6): the crate never formats or logs;
/// the backend routes these to `tracing`, the device to its ring log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quirk {
    /// `status.type.description` unknown to the NBA/football phase mapping.
    UnknownLivePhase,
    /// Soccer live `description` unknown to `is_break` — degraded to play.
    UnknownBreakDescription,
    /// Soccer post `description` unknown — degraded to full time.
    UnknownFinalFlavor,
    /// MLB `shortDetail` prefix not Top/Mid/Bot/End — the game is vetoed.
    UnknownInningHalf,
    /// A `records[].summary` that did not parse as `W-L`.
    MalformedRecord,
    /// A line score longer than the wire's 255-entry cap was clipped.
    ClippedLineScore,
    /// MLB weather block dropped (no resolvable condition or no temperature).
    WeatherDropped,
}

/// Receives quirk events. Implemented by the callers, not the tables.
pub trait Quirks {
    fn quirk(&mut self, quirk: Quirk);
}

/// For tests and callers that don't care.
pub struct IgnoreQuirks;

impl Quirks for IgnoreQuirks {
    fn quirk(&mut self, _quirk: Quirk) {}
}

/// The backend's `saturate_score`: JSON carries `u32`, the wire `u16`.
pub fn saturate_score(score: u32) -> u16 {
    score.min(u16::MAX as u32) as u16
}

/// The backend's `parse_hex_rgb`, byte for byte: strip one optional leading
/// `#`, require exactly six remaining chars, then `u32::from_str_radix` —
/// `core`'s own function, so quirks like the accepted leading `+` carry over
/// by construction (ruling 11). `None` is the transform-tier hard failure.
pub fn parse_hex_rgb(text: &str) -> Option<u32> {
    let hex = text.strip_prefix('#').unwrap_or(text);
    if hex.len() != 6 {
        return None;
    }
    u32::from_str_radix(hex, 16).ok()
}

/// The backend's record parse: first hyphen splits wins from losses, both
/// halves `u16`. `"12-1-1"` fails on the `"1-1"` half and drops the record —
/// bug-compatible by ruling 7.
pub fn parse_record(summary: &str) -> Option<(u16, u16)> {
    let (w, l) = summary.split_once('-')?;
    Some((w.parse().ok()?, l.parse().ok()?))
}

/// Line-score entry: ESPN sends `f64`; the backend does `clamp(0,255) as u8`
/// (NaN becomes 0 through the same cast).
pub fn linescore_byte(value: f64) -> u8 {
    value.clamp(0.0, u8::MAX as f64) as u8
}

/// `homeAway` marker on a competitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeAway {
    Home,
    Away,
}

impl HomeAway {
    /// Strict: anything but the two exact strings is a deserialize-tier
    /// failure upstream, so unknown markers are `None` here.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "home" => Some(HomeAway::Home),
            "away" => Some(HomeAway::Away),
            _ => None,
        }
    }
}

/// The backend's `order_home_away`: `(home, away)` by marker, never by array
/// index (all corpora happen to send home first — a trap, not a rule). Two
/// homes or two aways is `None`, the transform-tier hard failure.
pub fn order_home_away<T>(a: (HomeAway, T), b: (HomeAway, T)) -> Option<(T, T)> {
    match (a.0, b.0) {
        (HomeAway::Home, HomeAway::Away) => Some((a.1, b.1)),
        (HomeAway::Away, HomeAway::Home) => Some((b.1, a.1)),
        _ => None,
    }
}

/// NBA/football live sub-state, from `status.type.description`
/// (`parse_live_phase` in the backend). Wire codes 0/1/2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LivePhase {
    InProgress,
    Halftime,
    EndOfPeriod,
}

impl LivePhase {
    pub fn code(self) -> u8 {
        match self {
            LivePhase::InProgress => 0,
            LivePhase::Halftime => 1,
            LivePhase::EndOfPeriod => 2,
        }
    }
}

/// Unknown labels degrade to in-play with a quirk, never an error — an OT
/// break renders as live play today and ruling 7 keeps it that way. `None`
/// degrades silently, exactly like the backend.
pub fn parse_live_phase(description: Option<&str>, quirks: &mut impl Quirks) -> LivePhase {
    match description {
        Some("Halftime") => LivePhase::Halftime,
        Some("End of Period" | "End of Quarter") => LivePhase::EndOfPeriod,
        Some("In Progress") | None => LivePhase::InProgress,
        Some(_) => {
            quirks.quirk(Quirk::UnknownLivePhase);
            LivePhase::InProgress
        }
    }
}

/// The backend's `parse_start_time`, `no_std`: `%Y-%m-%dT%H:%MZ` with a
/// seconds-bearing fallback, chrono's 1-or-2-digit flexibility for the
/// non-year fields (ruling 12), real-calendar validation, then
/// `epoch.max(0) as u32` — the same clamp-and-truncate expression.
pub fn parse_start_time(text: &str) -> Option<u32> {
    let text = text.strip_suffix('Z')?;
    let (date, time) = text.split_once('T')?;

    let mut date_parts = date.split('-');
    let year = field(date_parts.next()?, 4)? as i32;
    let month = field(date_parts.next()?, 2)?;
    let day = field(date_parts.next()?, 2)?;
    if date_parts.next().is_some() {
        return None;
    }

    let mut time_parts = time.split(':');
    let hour = field(time_parts.next()?, 2)?;
    let minute = field(time_parts.next()?, 2)?;
    let second = match time_parts.next() {
        Some(part) => field(part, 2)?,
        None => 0,
    };
    if time_parts.next().is_some() {
        return None;
    }

    if !(1..=12).contains(&month) || !(1..=days_in_month(year, month)).contains(&day) {
        return None;
    }
    if hour >= 24 || minute >= 60 || second >= 60 {
        return None;
    }

    let days = days_from_civil(year, month, day);
    let epoch = days * 86_400 + (hour as i64) * 3_600 + (minute as i64) * 60 + second as i64;
    Some(epoch.max(0) as u32)
}

/// 1..=max_len ASCII digits, no sign — chrono's numeric-field behavior for
/// the widths the two formats use.
fn field(part: &str, max_len: usize) -> Option<u32> {
    if part.is_empty() || part.len() > max_len || !part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    part.parse().ok()
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// Howard Hinnant's `days_from_civil`: days since 1970-01-01 for a proleptic
/// Gregorian date. Pure integer arithmetic, exact over the full i32 year
/// range — property-tested against chrono in `tests/common_semantics.rs`.
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let y = year as i64 - if month <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
