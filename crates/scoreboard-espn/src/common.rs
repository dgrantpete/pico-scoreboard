//! Cross-sport semantics shared by the sport tables.
//!
//! Everything here replicates a specific backend behavior the inventories
//! documented; the DESIGN.md rulings say which. Divergence from the backend
//! is a parity bug even when the backend's behavior looks accidental.

use scoreboard_wire::{GameState, MAX_STRING_BYTES, truncate_utf8};

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
    /// A live football situation dropped by validation (backend warns).
    /// Football-only: soccer has no situation path (its lane checked).
    SituationDropped,
    /// Soccer clock fell back to the numeric field (display string unparseable).
    DisplayClockFallback,
    /// A period outside the wire's decodable range passed through (encode
    /// warns, decode rejects — the asymmetry is BACKLOG material).
    PeriodOutOfRange,
    /// A bounded extract buffer overflowed (scorer records, list entries…);
    /// diagnostics only — the wire bytes are unaffected by where we clip.
    BoundedOverflow,
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

// ---------------------------------------------------------------------------
// Promoted from the sport lanes (ruling 15): one consolidation pass after all
// four landed. Semantics are the lanes' tested ones, lifted verbatim.

/// serde's `u8` acceptance for a raw JSON number: an unsigned integer
/// literal that fits. Floats, exponents and signs are type errors, exactly
/// as `serde_json` rejects them for integer fields.
pub fn num_u8(text: &str) -> Option<u8> {
    if !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// serde's `u16` acceptance for a raw JSON number. See [`num_u8`].
pub fn num_u16(text: &str) -> Option<u16> {
    if !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// serde's `u32` acceptance for a raw JSON number. See [`num_u8`].
pub fn num_u32(text: &str) -> Option<u32> {
    if !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// serde's `i16` acceptance for a raw JSON number: one optional leading
/// minus, digits, in range — and `-0` rejected, because serde_json parses
/// `-0` as a float to preserve the sign, making it a type error for integer
/// fields (found by the MLB lane, pinned against serde_json in the tests).
pub fn num_i16(text: &str) -> Option<i16> {
    if text == "-0" {
        return None;
    }
    let digits = text.strip_prefix('-').unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// `common::LivePhase` → the wire's. Separate types because the wire crate
/// is deleted on the standalone branch at S4 while this crate survives.
pub fn wire_phase(phase: LivePhase) -> scoreboard_wire::LivePhase {
    match phase {
        LivePhase::InProgress => scoreboard_wire::LivePhase::InProgress,
        LivePhase::Halftime => scoreboard_wire::LivePhase::Halftime,
        LivePhase::EndOfPeriod => scoreboard_wire::LivePhase::EndOfPeriod,
    }
}

/// Stable insertion sort — `core` has no stable slice sort, and stability is
/// byte-load-bearing (equal keys keep arrival order; the soccer goldens and
/// NBA duplicate-period linescores both encode it). O(n²) on slates that are
/// tens of elements at most.
pub fn stable_sort_by_key<T, K: Ord>(items: &mut [T], key: impl Fn(&T) -> K) {
    for i in 1..items.len() {
        let mut j = i;
        while j > 0 && key(&items[j - 1]) > key(&items[j]) {
            items.swap(j - 1, j);
            j -= 1;
        }
    }
}

/// Append `piece`, truncating at the remaining capacity on a char boundary.
/// Returns false once truncation happened — the buffer is full.
pub fn push_trunc<const N: usize>(dst: &mut heapless::String<N>, piece: &str) -> bool {
    let take = truncate_utf8(piece, N - dst.len());
    let _ = dst.push_str(take);
    take.len() == piece.len()
}

/// `truncate_utf8(concat(pieces), N)` without materializing the
/// concatenation: earlier pieces fit whole, the cut lands inside exactly one
/// piece, and `truncate_utf8` backs up within it identically either way.
pub fn compose<const N: usize>(dst: &mut heapless::String<N>, pieces: &[&str]) {
    for piece in pieces {
        if !push_trunc(dst, piece) {
            break;
        }
    }
}

/// Bounded copy that REFUSES to truncate: parse inputs and compare keys must
/// fail loudly (matching the backend's behavior on the same input) rather
/// than silently operate on a prefix.
#[derive(Debug, Clone, Default)]
pub struct Exact<const N: usize> {
    text: heapless::String<N>,
    seen: bool,
    over: bool,
}

impl<const N: usize> Exact<N> {
    pub fn set(&mut self, src: &str) {
        self.text.clear();
        self.seen = true;
        self.over = self.text.push_str(src).is_err();
    }

    pub fn seen(&self) -> bool {
        self.seen
    }

    /// The stored text, or `None` when never set or when it overflowed.
    pub fn valid(&self) -> Option<&str> {
        if self.seen && !self.over {
            Some(self.text.as_str())
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Crest artwork: where a team's logo image lives on ESPN's CDN.
//
// Proxy mode never needed this — the backend resolved the href and handed the
// device a finished 24×24 sprite. Direct mode fetches the PNG itself, so the
// href has to survive extraction. It is taken from the payload rather than
// built from the team's abbreviation or id, because there is no single
// convention to build from: the corpus carries five different shapes across
// five leagues (`mlb/500/scoreboard/{abbrev}`, `nba/500/scoreboard/{abbrev}`,
// `ncaa/500/{id}`, `soccer/500/{id}`, `countries/500/{abbrev}`), and three of
// the eight leagues the firmware ships have no captured body to check a sixth
// guess against. `backend/src/team.rs` reached the same conclusion first.

/// The only CDN origin a crest is ever fetched from.
///
/// The backend refuses to proxy a payload href off this host so that a feed
/// cannot steer the fetch elsewhere; direct mode inherits the rule here,
/// where the href is first seen, rather than re-deriving it at the socket.
pub const CDN_ORIGIN: &str = "https://a.espncdn.com";

/// Storage for a crest's CDN path — the payload href minus [`CDN_ORIGIN`].
///
/// The origin is identical for every crest, so carrying it per team would
/// cost 21 bytes a side for nothing. 64 leaves comfortable headroom over the
/// corpus maximum of 40 bytes (an NBA scoreboard-variant path); an href that
/// somehow exceeds it is treated as no crest rather than silently truncated
/// into a URL that would 404.
pub const CREST_PATH_BYTES: usize = 64;

pub type CrestPath = heapless::String<CREST_PATH_BYTES>;

/// A team's crest path from its payload `team.logo` href.
///
/// `None` for an href on any other host, or one too long to hold — which is
/// the backend's own answer: it warns on the off-host case and treats the
/// team as having no logo at all. Requiring the `/` after the origin is what
/// makes this a host match rather than a prefix match, so a lookalike host
/// like `a.espncdn.com.evil.test` cannot pass.
pub fn crest_path(href: &str) -> Option<CrestPath> {
    let path = href.strip_prefix(CDN_ORIGIN)?;
    if !path.starts_with('/') {
        return None;
    }
    let mut out = CrestPath::new();
    out.push_str(path).ok()?;
    Some(out)
}

/// Both teams' crest paths, ordered by `homeAway` like every other pair in
/// an extract — never by array position.
///
/// `None` a side means the payload named no usable crest. That is not an
/// extraction failure: the games pipeline the backend runs never reads
/// `team.logo` at all, so a missing or malformed one must cost exactly what
/// it costs the backend, which is nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Crests {
    pub away: Option<CrestPath>,
    pub home: Option<CrestPath>,
}

const COMBINER_PREFIX: &str = "/combiner/i?img=";

/// Alpha has to survive the resize: the panel has no alpha, so the crest is
/// composited against black by the decoder (`backend/src/logo.rs` semantics),
/// and a flattened source would arrive with a box around it. ESPN's own
/// combiner URL in the corpus — the MLB dark league logo — carries the same
/// parameter.
const COMBINER_SUFFIX: &str = "&transparent=true";

/// Room for `&w=NNNNN&h=NNNNN` at `u16`'s widest.
const COMBINER_SIZE_BYTES: usize = 2 * ("&w=".len() + 5);

/// Bytes needed for the longest URL [`crest_url`] can build.
pub const CREST_URL_BYTES: usize = CDN_ORIGIN.len()
    + COMBINER_PREFIX.len()
    + CREST_PATH_BYTES
    + COMBINER_SIZE_BYTES
    + COMBINER_SUFFIX.len();

pub type CrestUrl = heapless::String<CREST_URL_BYTES>;

/// The CDN combiner URL for one crest, rendered `pixels` square.
///
/// Payload hrefs point at 500 px artwork, which is 13–40 KB of PNG and
/// 156–209 ms to decode on silicon; the combiner's 100 px variant is 3–4 KB
/// and ~8.3 ms (PARSE-PERF.md). Nothing in the payload links the small
/// variant — the corpus has no team-level combiner URL at all — so the size
/// is asked for here.
pub fn crest_url(path: &str, pixels: u16) -> Option<CrestUrl> {
    use core::fmt::Write;

    let mut url = CrestUrl::new();
    write!(
        url,
        "{CDN_ORIGIN}{COMBINER_PREFIX}{path}&w={pixels}&h={pixels}{COMBINER_SUFFIX}"
    )
    .ok()?;
    Some(url)
}

// ---------------------------------------------------------------------------
// The games list: what a list pass hands back per listed event.
//
// Proxy mode needed only `(id, state)` — the device asked the backend for a
// detail payload and the backend resolved the artwork itself. Direct mode's
// crest warmer needs two abbreviations and two artwork paths per game, and
// fetching a 300–450 KB per-event summary just to learn them is the "probe"
// this row deletes: the scoreboard body the list pass already streams carries
// all four.
//
// The extras are best-effort and never a validity gate. An event that lists
// today lists identically with every extra empty, because list membership is a
// parity contract against a backend that reads none of these fields.

/// One side's list-row extras.
///
/// `None` means the payload named nothing usable — deliberately distinct from
/// `Some("")`, which is a payload that really did send an empty abbreviation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ListTeam<'a> {
    /// `team.abbreviation`, bound at the wire cap like every other stored
    /// string (ruling 2).
    pub abbreviation: Option<&'a str>,
    /// `team.logo` reduced to a [`CrestPath`] by [`crest_path`] — the host is
    /// already checked, so this is a path, never a URL.
    pub crest: Option<&'a str>,
}

/// One games-list row. Borrowed from the extractor's per-event scratch, so it
/// is valid only for the duration of the [`ListSink::row`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListRow<'a> {
    pub id: &'a str,
    pub state: GameState,
    pub away: ListTeam<'a>,
    pub home: ListTeam<'a>,
}

/// Receives one [`ListRow`] per listed event, in body order.
///
/// All four lanes own their sink and hand it back at `finish`. That uniformity
/// is load-bearing rather than tidy: `scoreboard-direct` documented that a
/// borrowed sink in one lane and a borrowed closure in another cannot be
/// wrapped in a single streaming API without `unsafe` or a second copy of a
/// sport's transform, because the orphan rules forbid one type being both.
pub trait ListSink {
    fn row(&mut self, row: ListRow<'_>);
}

/// The stand-in for detail mode, which lists nothing.
#[derive(Debug, Default)]
pub struct NoRows;

impl ListSink for NoRows {
    fn row(&mut self, _row: ListRow<'_>) {}
}

/// `(away, home)` extras ordered by `homeAway` marker, never by array position
/// — the discipline every detail transform already follows.
///
/// Markers that are missing, or that agree (two homes, two aways), yield empty
/// extras on BOTH sides. The row still lists; it simply cannot say which crest
/// belongs to whom, and artwork attached to the wrong team is worse than no
/// artwork.
pub fn order_list_teams<'a>(
    first: (Option<HomeAway>, ListTeam<'a>),
    second: (Option<HomeAway>, ListTeam<'a>),
) -> (ListTeam<'a>, ListTeam<'a>) {
    let (Some(first_side), Some(second_side)) = (first.0, second.0) else {
        return (ListTeam::default(), ListTeam::default());
    };
    match order_home_away((first_side, first.1), (second_side, second.1)) {
        Some((home, away)) => (away, home),
        None => (ListTeam::default(), ListTeam::default()),
    }
}

/// Wire-bound string plus a presence flag: copies with the shared
/// `truncate_utf8` (ruling 2).
#[derive(Debug, Clone, Default)]
pub struct WireText {
    text: EText,
    seen: bool,
}

impl WireText {
    pub fn set(&mut self, src: &str) {
        set_text(&mut self.text, src);
        self.seen = true;
    }

    pub fn seen(&self) -> bool {
        self.seen
    }

    pub fn get(&self) -> Option<&str> {
        if self.seen { Some(self.text.as_str()) } else { None }
    }

    pub fn as_str(&self) -> &str {
        self.text.as_str()
    }
}
