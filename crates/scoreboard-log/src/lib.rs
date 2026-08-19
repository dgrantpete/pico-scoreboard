//! The log a deployed scoreboard can still tell you about itself.
//!
//! Port of `scoreboard/logger.py`'s first layer. MicroPython had three, each
//! surviving the failure of the one above: a RAM ring, a bounded flash mirror,
//! and reading the files over USB. SPEC §9 keeps the ring and drops the other
//! two — defmt over RTT is the development channel, and a gift unit in someone
//! else's living room is reached over HTTP, not over a serial cable. So this
//! ring *is* the deployed diagnostic surface, and `/api/logs` is how it is read.
//!
//! Everything here is a decision about bytes — which entries are newer than a
//! sequence number, how a message with a quote in it is escaped — so it lives
//! in a crate and is tested on the desktop (SPEC §2's crate-boundary rule). The
//! firmware supplies the lock and the clock.
//!
//! # The shape on the wire is fixed by a client that already exists
//!
//! `/api/logs` streams NDJSON, one `[seq, ts, level, msg]` array per line, and
//! the settings SPA tail-follows it by sending the last line's `seq` back as
//! `?since=`. The array form, the level integers and the timestamp convention
//! are all read by `frontend/src/routes/logs/+page.svelte` as it ships today —
//! this crate matches them rather than improving on them.
//!
//! **Timestamps are seconds, and they are boot-relative until a clock exists.**
//! MicroPython called `time.time()`, which before an RTC sync returns seconds
//! on the 2000 epoch. The SPA already copes: it renders anything below 1e8 as
//! `+Ns`, which is exactly what an unsynced Rust device produces. Task #11's
//! time sync sets the wall-clock offset and the numbers become Unix seconds
//! with no client change — [`Entry::ts`] is just what the caller passed.

//! # Two surfaces, one crate
//!
//! The ring is what a *running* device tells you. [`breadcrumb`] is what a
//! device tells you about the boot it did not survive — one record, written to
//! flash when something dies, served by `/api/logs/previous`. They are in one
//! crate because they answer the same question at two timescales, and because
//! the truncating formatter is the same in both.

#![no_std]
#![forbid(unsafe_code)]

use core::fmt::Write as _;

use heapless::String;

pub mod breadcrumb;

/// The longest message a slot holds.
///
/// The MicroPython ring allowed 200 characters. Measured against the 87 log
/// call sites in `firmware/src`, the *format strings* run to a median of 43
/// bytes and a maximum of 116, and interpolated values push the long ones
/// further — so 200 was generous and 64 would truncate real messages. 128 keeps
/// every observed line whole while costing 200 × 140 B of RAM rather than
/// 200 × 212 B (BUDGET.md carries the measured figure).
pub const MAX_MESSAGE: usize = 128;

/// Slots in the ring, and therefore the most entries a client can be behind by
/// before it misses some. `logger.py`'s `_SLOTS`, unchanged in the default
/// build: at the SPA's 3 s poll interval this is minutes of history, and it
/// is the number the `?since=` contract is sized against.
///
/// `small-ring` cuts it to 48, trading retained history for ~22 KB of RAM.
/// It exists for the direct-feed firmware, whose poll task needs the stack
/// more than the ring needs the depth — the S3 bring-up put SP probes on the
/// poll path and measured its true need within kilobytes of the whole
/// remainder. 48 slots still holds minutes of steady-state history (the
/// chatty part of a boot scrolls past faster); a client further behind than
/// that re-syncs through the same `?since=` gap contract it always had.
pub const SLOTS: usize = if cfg!(feature = "small-ring") { 48 } else { 200 };

/// A message, already bounded.
pub type Message = String<MAX_MESSAGE>;

/// Severity, as the integers the SPA renders.
///
/// `logger.py`'s `NONE = 0, ERROR = 1, DEBUG = 2`. The numbers are wire format
/// — `levelName()` in the logs page maps 1 to `ERR` and 2 to `DBG` — so they
/// are fixed even though a Rust enum would happily have been ordered the other
/// way round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Level {
    /// Log nothing. Only ever a *filter* value; no entry is recorded at it.
    None = 0,
    Error = 1,
    Debug = 2,
}

impl Level {
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// Parse `config.json`'s `log.level`, falling back to `Debug`.
    ///
    /// `config.py`'s `_LOG_LEVEL_MAP.get(name, DEBUG)` — an unrecognised level
    /// means "log everything", not "log nothing", so a typo in a hand-edited
    /// config cannot silently blind the device.
    pub fn from_name(name: &str) -> Level {
        match name {
            "none" => Level::None,
            "error" => Level::Error,
            _ => Level::Debug,
        }
    }

    /// The spelling `config.json` uses, for the round trip back out of
    /// `GET /api/config`.
    pub const fn name(self) -> &'static str {
        match self {
            Level::None => "none",
            Level::Error => "error",
            Level::Debug => "debug",
        }
    }
}

/// One recorded line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Strictly increasing from 1. The `?since=` cursor.
    pub seq: u32,
    /// Seconds — Unix once a clock exists, boot-relative before that. See the
    /// crate docs.
    pub ts: u32,
    pub level: Level,
    pub message: Message,
}

impl Entry {
    const fn empty() -> Entry {
        Entry {
            seq: 0,
            ts: 0,
            level: Level::None,
            message: Message::new(),
        }
    }
}

/// What one [`Ring::render_ndjson_since`] pass produced.
pub struct Rendered {
    /// Bytes written to the front of the caller's buffer.
    pub len: usize,
    /// The `since` value that continues from here — the last rendered entry's
    /// `seq`, or the caller's own `after` if nothing was rendered.
    pub next_since: u32,
    /// Whether entries remain that did not fit. The caller loops on this.
    pub more: bool,
}

/// A fixed ring of log entries.
///
/// Not internally synchronised: the firmware wraps it in a
/// `blocking_mutex::Mutex` because both cores may record, which is also what
/// makes reads atomic. `logger.py` had to re-check `slot[0] == seq` while
/// snapshotting to catch a wrap that tore underneath it (`logger.py:114`);
/// holding the lock across a whole read means that race does not exist here,
/// and the guard is not ported.
pub struct Ring {
    slots: [Entry; SLOTS],
    /// The highest seq recorded; 0 before anything is. Entries run
    /// `1..=last_seq`.
    ///
    /// Deliberately not "the *next* seq", which is how `logger.py` tracked it:
    /// a next-seq of 1 is a non-zero field, and one non-zero field puts this
    /// whole 28 KB struct in `.data` — where it costs its own size again in
    /// flash for an initializer image that is almost all zeros. Counting from
    /// the other end keeps the zeroed state correct and the struct in `.bss`.
    last_seq: u32,
    level: StoredLevel,
}

/// The filter level, stored so that the zero value is the default.
///
/// `Level::Debug` is the default filter and its wire value is 2, which would
/// again force `Ring` out of `.bss`. Storing `2 - level` costs one subtraction
/// on a path that runs once per log line and buys 28 KB of flash. The wire
/// values themselves are untouched — this representation never leaves the
/// struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct StoredLevel(u8);

impl StoredLevel {
    const fn get(self) -> Level {
        match 2 - self.0 {
            0 => Level::None,
            1 => Level::Error,
            _ => Level::Debug,
        }
    }

    const fn set(level: Level) -> StoredLevel {
        StoredLevel(2 - level as u8)
    }
}

impl Ring {
    pub const fn new() -> Ring {
        Ring {
            slots: [const { Entry::empty() }; SLOTS],
            last_seq: 0,
            level: StoredLevel(0),
        }
    }

    /// The active filter. `config.json`'s `log.level` owns this; the firmware
    /// pushes changes in from `PUT /api/config`, as `Config` did.
    pub const fn level(&self) -> Level {
        self.level.get()
    }

    pub const fn set_level(&mut self, level: Level) {
        self.level = StoredLevel::set(level);
    }

    /// Whether a message at `level` would be recorded.
    ///
    /// Checked before building the message, so a caller can skip the
    /// formatting entirely — `logger.py`'s reason for exposing the level as a
    /// bare module int.
    pub const fn enabled(&self, level: Level) -> bool {
        (self.level.get() as u8) >= (level as u8)
    }

    /// The highest seq recorded, or 0 if nothing has been.
    pub const fn latest_seq(&self) -> u32 {
        self.last_seq
    }

    /// Record a message, truncating it to [`MAX_MESSAGE`].
    ///
    /// Returns the seq it was given, or `None` if the level filter dropped it.
    /// Truncation lands on a character boundary, so a multi-byte character
    /// straddling the limit is dropped whole rather than cut into invalid
    /// UTF-8 — `logger.py`'s `msg[:200]` got that for free from Python slicing
    /// characters rather than bytes.
    pub fn record(&mut self, level: Level, ts: u32, message: &str) -> Option<u32> {
        if level == Level::None || !self.enabled(level) {
            return None;
        }
        // Saturating rather than wrapping: at one entry per millisecond a u32
        // takes seven weeks to wrap, and a cursor that goes backwards would
        // make the SPA replay its whole history. Pinning at the ceiling stops
        // new entries appearing rather than corrupting the sequence, and a
        // device that has logged four billion lines has a different problem.
        let seq = self.last_seq.saturating_add(1);
        let slot = &mut self.slots[(seq % SLOTS as u32) as usize];
        slot.seq = seq;
        slot.ts = ts;
        slot.level = level;
        slot.message.clear();
        let _ = slot.message.push_str(truncate_on_boundary(message, MAX_MESSAGE));
        self.last_seq = seq;
        Some(seq)
    }

    /// Record a `core::fmt` message without a formatting buffer of the
    /// caller's own — the arguments are rendered straight into the slot.
    ///
    /// This is the shape that keeps the ring off the caller's stack: a
    /// `format_args!` that would need 128 B of scratch somewhere writes into
    /// the slot it is destined for. Overlong output is truncated at the
    /// boundary, as [`record`](Ring::record) does.
    pub fn record_fmt(&mut self, level: Level, ts: u32, args: core::fmt::Arguments) -> Option<u32> {
        if level == Level::None || !self.enabled(level) {
            return None;
        }
        let seq = self.last_seq.saturating_add(1);
        let slot = &mut self.slots[(seq % SLOTS as u32) as usize];
        slot.seq = seq;
        slot.ts = ts;
        slot.level = level;
        slot.message.clear();
        // Through a truncating adaptor, not straight into the `String`.
        // `heapless`'s own `write_str` rejects a fragment that does not fit
        // *whole*, and `write_fmt` stops at the first error — so formatting one
        // long `{}` into a full slot lands zero bytes, not 128. A message that
        // is too long would have been logged as an empty line, which is worse
        // than a truncated one and looks like a different bug.
        let _ = Truncating(&mut slot.message).write_fmt(args);
        self.last_seq = seq;
        Some(seq)
    }

    /// The oldest seq still in the ring.
    const fn oldest_seq(&self) -> u32 {
        let latest = self.latest_seq();
        if latest > SLOTS as u32 {
            latest - SLOTS as u32 + 1
        } else {
            1
        }
    }

    /// Entries newer than `after`, oldest first.
    ///
    /// `logger.py`'s `entries_since`. A client that has fallen further behind
    /// than the ring is deep silently resumes at the oldest entry present
    /// rather than erroring — it has missed lines either way, and the
    /// alternative is a logs page that shows nothing.
    pub fn since(&self, after: u32) -> impl Iterator<Item = &Entry> {
        let start = self.oldest_seq().max(after.saturating_add(1));
        let latest = self.latest_seq();
        (start..=latest).filter_map(move |seq| {
            let slot = &self.slots[(seq % SLOTS as u32) as usize];
            (slot.seq == seq).then_some(slot)
        })
    }

    /// Render entries newer than `after` as NDJSON into `out`, stopping before
    /// the first line that would not fit.
    ///
    /// One line per entry, `[seq,ts,level,"message"]\n`, which is what
    /// `api_routes.py`'s generator produced via `json.dumps` — a stream rather
    /// than one large body, because a full ring is tens of kilobytes and the
    /// device has no buffer to hold it. Here the chunking is explicit: the
    /// caller loops while [`Rendered::more`], writing each pass to the socket,
    /// so the lock is released between passes and the buffer stays small.
    pub fn render_ndjson_since(&self, after: u32, out: &mut [u8]) -> Rendered {
        let mut len = 0;
        let mut next_since = after;
        for entry in self.since(after) {
            match write_line(entry, &mut out[len..]) {
                Some(written) => {
                    len += written;
                    next_since = entry.seq;
                }
                None => {
                    return Rendered {
                        len,
                        next_since,
                        // A single entry too long for the whole buffer would
                        // otherwise loop forever. It cannot happen — a line is
                        // at most MAX_MESSAGE plus escaping and framing, and
                        // the caller's buffer is sized for that (see
                        // MAX_LINE) — but saying so in code beats saying so in
                        // a comment.
                        more: len > 0,
                    };
                }
            }
        }
        Rendered {
            len,
            next_since,
            more: false,
        }
    }
}

impl Default for Ring {
    fn default() -> Ring {
        Ring::new()
    }
}

/// The longest NDJSON line a single entry can produce.
///
/// `[` + seq + `,` + ts + `,` + level + `,"` + message + `"]\n`. Ten digits
/// each for two `u32`s, one for the level, and every message byte escaping to
/// at most six (`\u00XX`). A caller whose buffer is at least this large can
/// never stall on a line that does not fit.
pub const MAX_LINE: usize = 1 + 10 + 1 + 10 + 1 + 1 + 2 + (MAX_MESSAGE * 6) + 3;

/// Write one `[seq,ts,level,"msg"]\n`, or `None` if it does not fit.
fn write_line(entry: &Entry, out: &mut [u8]) -> Option<usize> {
    let mut cursor = Cursor { out, len: 0 };
    cursor.byte(b'[')?;
    cursor.integer(entry.seq)?;
    cursor.byte(b',')?;
    cursor.integer(entry.ts)?;
    cursor.byte(b',')?;
    cursor.integer(entry.level.as_u8() as u32)?;
    cursor.byte(b',')?;
    cursor.json_string(&entry.message)?;
    cursor.byte(b']')?;
    cursor.byte(b'\n')?;
    Some(cursor.len)
}

struct Cursor<'a> {
    out: &'a mut [u8],
    len: usize,
}

impl Cursor<'_> {
    fn byte(&mut self, value: u8) -> Option<()> {
        *self.out.get_mut(self.len)? = value;
        self.len += 1;
        Some(())
    }

    fn slice(&mut self, values: &[u8]) -> Option<()> {
        let end = self.len.checked_add(values.len())?;
        self.out.get_mut(self.len..end)?.copy_from_slice(values);
        self.len = end;
        Some(())
    }

    fn integer(&mut self, mut value: u32) -> Option<()> {
        let mut digits = [0u8; 10];
        let mut index = digits.len();
        loop {
            index -= 1;
            digits[index] = b'0' + (value % 10) as u8;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        self.slice(&digits[index..])
    }

    /// A JSON string literal, escaped per RFC 8259 §7.
    ///
    /// Not decoration: a play-by-play line reaching the log carries quotes and
    /// backslashes, and one unescaped quote turns the whole NDJSON stream into
    /// a parse error in the SPA — which is a logs page that goes blank exactly
    /// when something interesting was logged. `json.dumps` did this for the
    /// MicroPython route.
    fn json_string(&mut self, text: &str) -> Option<()> {
        self.byte(b'"')?;
        for byte in text.bytes() {
            match byte {
                b'"' => self.slice(br#"\""#)?,
                b'\\' => self.slice(br"\\")?,
                b'\n' => self.slice(br"\n")?,
                b'\r' => self.slice(br"\r")?,
                b'\t' => self.slice(br"\t")?,
                // Everything else below 0x20 must be escaped, and there is no
                // shorthand for it. Bytes at or above 0x20 pass through, which
                // includes every continuation byte of a multi-byte character —
                // UTF-8 is valid JSON string content unescaped.
                0x00..=0x1F => {
                    self.slice(b"\\u00")?;
                    self.byte(hex_digit(byte >> 4))?;
                    self.byte(hex_digit(byte & 0x0F))?;
                }
                _ => self.byte(byte)?,
            }
        }
        self.byte(b'"')
    }
}

const fn hex_digit(nibble: u8) -> u8 {
    if nibble < 10 {
        b'0' + nibble
    } else {
        b'a' + (nibble - 10)
    }
}

/// A [`core::fmt::Write`] that fills a bounded string and then quietly stops.
///
/// Every `write_str` succeeds from `core::fmt`'s point of view, so a
/// `write_fmt` runs to the end of its arguments instead of aborting at the
/// first fragment that does not fit — the difference between a message
/// truncated at 128 bytes and no message at all.
struct Truncating<'a>(&'a mut Message);

impl core::fmt::Write for Truncating<'_> {
    fn write_str(&mut self, fragment: &str) -> core::fmt::Result {
        let room = self.0.capacity() - self.0.len();
        let _ = self.0.push_str(truncate_on_boundary(fragment, room));
        Ok(())
    }
}

/// The longest prefix of `text` that fits in `limit` bytes without splitting a
/// character.
pub(crate) fn truncate_on_boundary(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        return text;
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    extern crate std;
    use std::string::String as StdString;
    use std::vec::Vec;

    fn render_all(ring: &Ring, after: u32) -> StdString {
        let mut out = [0u8; 64 * 1024];
        let rendered = ring.render_ndjson_since(after, &mut out);
        assert!(!rendered.more, "the test buffer should fit everything");
        StdString::from_utf8(out[..rendered.len].to_vec()).unwrap()
    }

    fn seqs(ring: &Ring, after: u32) -> Vec<u32> {
        ring.since(after).map(|entry| entry.seq).collect()
    }

    #[test]
    fn sequence_numbers_start_at_one() {
        let mut ring = Ring::new();
        assert_eq!(ring.latest_seq(), 0);
        assert_eq!(ring.record(Level::Debug, 10, "first"), Some(1));
        assert_eq!(ring.record(Level::Debug, 11, "second"), Some(2));
        assert_eq!(ring.latest_seq(), 2);
    }

    #[test]
    fn since_returns_only_newer_entries_oldest_first() {
        let mut ring = Ring::new();
        for index in 0..5 {
            ring.record(Level::Debug, index, "line");
        }
        assert_eq!(seqs(&ring, 0), std::vec![1, 2, 3, 4, 5]);
        assert_eq!(seqs(&ring, 3), std::vec![4, 5]);
        assert_eq!(seqs(&ring, 5), std::vec![]);
        // A cursor from the future — a client that saw a device reboot — gets
        // nothing rather than a panic or a replay.
        assert_eq!(seqs(&ring, 99), std::vec![]);
    }

    #[test]
    fn the_ring_wraps_and_keeps_the_newest_slots() {
        let mut ring = Ring::new();
        for index in 0..(SLOTS as u32 + 50) {
            ring.record(Level::Debug, index, "line");
        }
        let latest = ring.latest_seq();
        assert_eq!(latest, SLOTS as u32 + 50);
        let present = seqs(&ring, 0);
        assert_eq!(present.len(), SLOTS);
        assert_eq!(*present.first().unwrap(), latest - SLOTS as u32 + 1);
        assert_eq!(*present.last().unwrap(), latest);
    }

    #[test]
    fn a_client_further_behind_than_the_ring_resumes_at_the_oldest() {
        let mut ring = Ring::new();
        for index in 0..(SLOTS as u32 * 2) {
            ring.record(Level::Debug, index, "line");
        }
        // Asked for everything after seq 5, which fell out of the ring long
        // ago. It gets the oldest entry still held, not an empty answer.
        let present = seqs(&ring, 5);
        assert_eq!(present.len(), SLOTS);
        assert_eq!(*present.first().unwrap(), ring.latest_seq() - SLOTS as u32 + 1);
    }

    #[test]
    fn the_level_filter_drops_entries_without_consuming_a_sequence_number() {
        let mut ring = Ring::new();
        ring.set_level(Level::Error);
        assert_eq!(ring.record(Level::Debug, 0, "chatty"), None);
        assert_eq!(ring.record(Level::Error, 1, "broken"), Some(1));
        // The dropped debug line did not burn seq 1, so a client's cursor
        // never skips over a gap that has no entry behind it.
        assert_eq!(seqs(&ring, 0), std::vec![1]);
    }

    #[test]
    fn level_none_records_nothing_at_all() {
        let mut ring = Ring::new();
        ring.set_level(Level::None);
        assert_eq!(ring.record(Level::Error, 0, "broken"), None);
        assert_eq!(ring.record(Level::Debug, 0, "chatty"), None);
        // And Level::None is never a thing you can record *at*, even wide open.
        ring.set_level(Level::Debug);
        assert_eq!(ring.record(Level::None, 0, "nothing"), None);
    }

    #[test]
    fn ndjson_is_one_array_per_line() {
        let mut ring = Ring::new();
        ring.record(Level::Error, 1_700_000_000, "wifi down");
        ring.record(Level::Debug, 1_700_000_001, "wifi up");
        assert_eq!(
            render_all(&ring, 0),
            "[1,1700000000,1,\"wifi down\"]\n[2,1700000001,2,\"wifi up\"]\n"
        );
    }

    #[test]
    fn json_strings_are_escaped() {
        let mut ring = Ring::new();
        ring.record(Level::Debug, 0, "he said \"hi\"\\then\nleft\ttab");
        assert_eq!(
            render_all(&ring, 0),
            "[1,0,2,\"he said \\\"hi\\\"\\\\then\\nleft\\ttab\"]\n"
        );
    }

    #[test]
    fn control_bytes_become_unicode_escapes() {
        let mut ring = Ring::new();
        ring.record(Level::Debug, 0, "bell\x07end");
        assert_eq!(render_all(&ring, 0), "[1,0,2,\"bell\\u0007end\"]\n");
    }

    #[test]
    fn multibyte_characters_pass_through_unescaped() {
        let mut ring = Ring::new();
        ring.record(Level::Debug, 0, "café ✓");
        assert_eq!(render_all(&ring, 0), "[1,0,2,\"café ✓\"]\n");
    }

    #[test]
    fn a_long_message_is_truncated_on_a_character_boundary() {
        let mut ring = Ring::new();
        // A 3-byte character straddling the limit must be dropped whole, not
        // cut into a byte sequence that is no longer UTF-8.
        let mut message: StdString = "a".repeat(MAX_MESSAGE - 1);
        message.push('✓');
        ring.record(Level::Debug, 0, &message);
        let entry = ring.since(0).next().unwrap().clone();
        assert_eq!(entry.message.len(), MAX_MESSAGE - 1);
        assert!(entry.message.chars().all(|character| character == 'a'));
    }

    #[test]
    fn record_fmt_writes_arguments_straight_into_the_slot() {
        let mut ring = Ring::new();
        ring.record_fmt(Level::Error, 5, format_args!("join failed: attempt {}", 3));
        assert_eq!(render_all(&ring, 0), "[1,5,1,\"join failed: attempt 3\"]\n");
    }

    #[test]
    fn record_fmt_truncates_rather_than_failing() {
        let mut ring = Ring::new();
        ring.record_fmt(Level::Debug, 0, format_args!("{}", "x".repeat(MAX_MESSAGE * 2)));
        let entry = ring.since(0).next().unwrap().clone();
        assert_eq!(entry.message.len(), MAX_MESSAGE);
    }

    #[test]
    fn rendering_stops_cleanly_when_the_buffer_fills_and_resumes_where_it_left_off() {
        let mut ring = Ring::new();
        for index in 0..10 {
            ring.record(Level::Debug, index, "0123456789");
        }
        // Each line is `[N,T,2,"0123456789"]\n` — 20 bytes for single-digit
        // seq and ts. Room for two and a bit.
        let mut out = [0u8; 50];
        let first = ring.render_ndjson_since(0, &mut out);
        assert!(first.more);
        assert_eq!(first.next_since, 2);
        assert_eq!(&out[..first.len], b"[1,0,2,\"0123456789\"]\n[2,1,2,\"0123456789\"]\n");

        let second = ring.render_ndjson_since(first.next_since, &mut out);
        assert!(second.more);
        assert_eq!(second.next_since, 4);
        assert_eq!(&out[..second.len], b"[3,2,2,\"0123456789\"]\n[4,3,2,\"0123456789\"]\n");
    }

    #[test]
    fn an_empty_ring_renders_nothing_and_asks_for_no_more_passes() {
        let ring = Ring::new();
        let mut out = [0u8; 128];
        let rendered = ring.render_ndjson_since(0, &mut out);
        assert_eq!(rendered.len, 0);
        assert_eq!(rendered.next_since, 0);
        assert!(!rendered.more);
    }

    #[test]
    fn max_line_is_large_enough_for_the_worst_message() {
        // Every byte escaping to six, at the longest seq and ts. If MAX_LINE
        // were ever cut below this, a caller sized by it would stall forever
        // on a line that could not fit.
        let mut ring = Ring::new();
        ring.record(Level::Debug, u32::MAX, &"\x01".repeat(MAX_MESSAGE));
        let mut out = [0u8; MAX_LINE];
        let rendered = ring.render_ndjson_since(0, &mut out);
        assert!(!rendered.more);
        assert!(rendered.len <= MAX_LINE);
    }

    #[test]
    fn level_names_round_trip_through_config() {
        for level in [Level::None, Level::Error, Level::Debug] {
            assert_eq!(Level::from_name(level.name()), level);
        }
        // The documented fallback: anything unrecognised logs everything.
        assert_eq!(Level::from_name("verbose"), Level::Debug);
        assert_eq!(Level::from_name(""), Level::Debug);
    }
}
