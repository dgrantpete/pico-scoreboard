//! The one record that outlives a reset.
//!
//! The RAM ring above is the deployed diagnostic surface right up until the
//! device reboots, at which point it is gone — it is RAM, and that is the whole
//! design (SPEC §9: pull, not persist). So the failures a ring cannot report
//! are exactly the interesting ones: a panic, and a watchdog that starved
//! because something stopped. This is the record for those, and it is
//! deliberately *one* record — the previous boot's cause of death, not a log.
//!
//! It replaces `logger.py`'s `/logs/previous.log`, which was the whole ring
//! rotated to a second file once per boot. The trade is honest and worth
//! stating: MicroPython kept up to 200 lines of history across a reboot and
//! wrote flash on a timer to do it; this keeps one record and writes flash only
//! when something died. `GET /api/logs/previous` serves
//! [`Breadcrumb::render`]'s plain text, which is the shape the settings SPA
//! already expects (`getPreviousLog` reads `text()`, and `null` for a 404).
//!
//! The *lifetime* differs too, and the rendered text says so rather than
//! pretending otherwise. MicroPython's rotation meant the file always described
//! the immediately preceding boot; one stored record instead describes the most
//! recent abnormal shutdown, and survives however many clean boots follow. That
//! is the more useful of the two — a crash a week ago is worth reading about,
//! and a device that has never crashed answers `404` — but "previous boot"
//! would be a lie for it, so [`Breadcrumb::render`] does not say it.
//!
//! # Why the encoding is by hand
//!
//! Nothing else in the firmware serializes to flash, so there is no serde
//! shape to reuse, and the two properties that matter here are ones a derive
//! would not give: a **magic and a version** at the front, so a record written
//! by a different firmware is *rejected* rather than misread, and a decode that
//! is total — every malformed input maps to [`DecodeError`], because the input
//! is flash that may hold anything at all, including the previous project's
//! littlefs.
//!
//! The layout is little-endian and fixed-width up to the message, whose length
//! is a `u16` prefix:
//!
//! | offset | size | field |
//! |---|---|---|
//! | 0 | 4 | [`MAGIC`] |
//! | 4 | 1 | [`VERSION`] |
//! | 5 | 1 | cause |
//! | 6 | 1 | core |
//! | 7 | 1 | reserved (0) |
//! | 8 | 4 | uptime seconds |
//! | 12 | 4 | unix seconds, 0 if the clock never synced |
//! | 16 | 16 | four stack watermarks |
//! | 32 | 2 | message length |
//! | 34 | n | message |

use core::fmt::Write as _;

use heapless::String;

/// `"PSB1"` — pico-scoreboard breadcrumb. Chosen so a hex dump of the storage
/// region says what the bytes are.
pub const MAGIC: u32 = u32::from_le_bytes(*b"PSB1");

/// Bumped when the layout changes. An older record decodes to
/// [`DecodeError::Version`] and is reported as "unreadable" rather than as a
/// misparsed crash.
pub const VERSION: u8 = 1;

/// The longest message a breadcrumb carries.
///
/// Longer than the ring's [`MAX_MESSAGE`](crate::MAX_MESSAGE), because this one
/// holds a panic message *and* its `file:line`, and truncating the location off
/// the end is exactly the half you want. Measured against the panics this
/// firmware can produce, the longest is an `expect` string plus a path of about
/// 90 bytes.
pub const MAX_MESSAGE: usize = 192;

/// Bytes a full record occupies. The fixed header plus a full message.
pub const MAX_BYTES: usize = HEADER_BYTES + MAX_MESSAGE;

const HEADER_BYTES: usize = 34;

/// Why the device stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Cause {
    /// A Rust panic on either core.
    Panic = 1,
    /// The watchdog feeder stopped feeding on purpose, because the health gate
    /// failed. See the firmware's `supervise` module: a reset with no
    /// breadcrumb is indistinguishable from a power cut, and this is what
    /// tells the two apart after the fact.
    WatchdogStarved = 2,
}

impl Cause {
    const fn from_u8(value: u8) -> Option<Cause> {
        match value {
            1 => Some(Cause::Panic),
            2 => Some(Cause::WatchdogStarved),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Cause::Panic => "panic",
            Cause::WatchdogStarved => "watchdog starved",
        }
    }
}

/// Both stacks' high-water marks at the moment of death — SPEC §9's "snapshot
/// of task watermarks".
///
/// Zeroes mean "not measured yet": the marks are published by a 10 s scan, so a
/// device that died in its first ten seconds honestly has none, and reporting
/// zero is better than reporting a stale guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Watermarks {
    pub core0_used: u32,
    pub core0_total: u32,
    pub core1_used: u32,
    pub core1_total: u32,
}

/// The previous boot's cause of death.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Breadcrumb {
    pub cause: Cause,
    /// Which core wrote it. A panic on core 1 is a render bug and a panic on
    /// core 0 is everything else, and the two are diagnosed differently.
    pub core: u8,
    /// Seconds of uptime when it happened.
    pub uptime_s: u32,
    /// Unix seconds, or 0 if the clock had never synced. Kept separate from
    /// `uptime_s` rather than folded into one field for the reason the ring
    /// keeps boot-relative stamps: a device that dies before its first time
    /// sync should say so, not claim the epoch.
    pub unix_s: u32,
    pub watermarks: Watermarks,
    pub message: String<MAX_MESSAGE>,
}

impl Breadcrumb {
    pub fn new(cause: Cause, core: u8) -> Breadcrumb {
        Breadcrumb {
            cause,
            core,
            uptime_s: 0,
            unix_s: 0,
            watermarks: Watermarks::default(),
            message: String::new(),
        }
    }

    /// Set the message from format arguments, truncating on a character
    /// boundary.
    ///
    /// Infallible for the same reason [`Ring::record_fmt`](crate::Ring) is: the
    /// caller is a panic handler, and a message that did not fit is still worth
    /// more than no breadcrumb at all.
    pub fn set_message(&mut self, args: core::fmt::Arguments) {
        self.message.clear();
        let _ = Truncating(&mut self.message).write_fmt(args);
    }

    /// Encode into `out`, returning the bytes written.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, EncodeError> {
        let len = HEADER_BYTES + self.message.len();
        let out = out.get_mut(..len).ok_or(EncodeError)?;
        out[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        out[4] = VERSION;
        out[5] = self.cause as u8;
        out[6] = self.core;
        out[7] = 0;
        out[8..12].copy_from_slice(&self.uptime_s.to_le_bytes());
        out[12..16].copy_from_slice(&self.unix_s.to_le_bytes());
        out[16..20].copy_from_slice(&self.watermarks.core0_used.to_le_bytes());
        out[20..24].copy_from_slice(&self.watermarks.core0_total.to_le_bytes());
        out[24..28].copy_from_slice(&self.watermarks.core1_used.to_le_bytes());
        out[28..32].copy_from_slice(&self.watermarks.core1_total.to_le_bytes());
        out[32..34].copy_from_slice(&(self.message.len() as u16).to_le_bytes());
        out[HEADER_BYTES..].copy_from_slice(self.message.as_bytes());
        Ok(len)
    }

    /// Decode a record, or say precisely why it is not one.
    pub fn decode(bytes: &[u8]) -> Result<Breadcrumb, DecodeError> {
        if bytes.len() < HEADER_BYTES {
            return Err(DecodeError::Truncated);
        }
        if u32::from_le_bytes(read4(bytes, 0)) != MAGIC {
            return Err(DecodeError::Magic);
        }
        if bytes[4] != VERSION {
            return Err(DecodeError::Version);
        }
        let cause = Cause::from_u8(bytes[5]).ok_or(DecodeError::Cause)?;
        let length = u16::from_le_bytes([bytes[32], bytes[33]]) as usize;
        let message = bytes
            .get(HEADER_BYTES..HEADER_BYTES + length)
            .ok_or(DecodeError::Truncated)?;
        let message = core::str::from_utf8(message).map_err(|_| DecodeError::Message)?;
        Ok(Breadcrumb {
            cause,
            core: bytes[6],
            uptime_s: u32::from_le_bytes(read4(bytes, 8)),
            unix_s: u32::from_le_bytes(read4(bytes, 12)),
            watermarks: Watermarks {
                core0_used: u32::from_le_bytes(read4(bytes, 16)),
                core0_total: u32::from_le_bytes(read4(bytes, 20)),
                core1_used: u32::from_le_bytes(read4(bytes, 24)),
                core1_total: u32::from_le_bytes(read4(bytes, 28)),
            },
            // Bounded by construction: `length` came out of a `u16` and the
            // slice above proved it is in the buffer, but the *capacity* check
            // is real — a record written by a firmware with a larger
            // `MAX_MESSAGE` must not be able to overflow this one.
            message: String::try_from(message).map_err(|_| DecodeError::Message)?,
        })
    }

    /// The plain text `GET /api/logs/previous` serves.
    ///
    /// Plain text and not JSON because that endpoint has always been plain
    /// text: the SPA's `getPreviousLog` reads `response.text()` and renders it
    /// in a `<pre>`, which is what MicroPython's rotated log file was.
    ///
    /// Returns the bytes written, or `None` if the buffer was too small to hold
    /// the whole thing — a partial crash report is worse than none, because it
    /// reads as a complete one.
    pub fn render(&self, out: &mut [u8]) -> Option<usize> {
        // "last abnormal shutdown", not "previous boot". The record is kept
        // until something else dies, so after a clean reboot it describes an
        // *earlier* boot than the last one — and the endpoint's name is the
        // SPA's, which this firmware is replacing the device under, not
        // redesigning. Saying so in the body is what keeps it honest; the
        // uptime and wall-clock lines are how a reader tells which boot it was.
        let mut writer = Renderer { out, len: 0 };
        writer.text("last abnormal shutdown: ")?;
        writer.text(self.cause.as_str())?;
        writer.text(" on core ")?;
        writer.integer(self.core as u32)?;
        writer.text("\nuptime: ")?;
        writer.integer(self.uptime_s)?;
        writer.text(" s\n")?;
        if self.unix_s != 0 {
            writer.text("unix time: ")?;
            writer.integer(self.unix_s)?;
            writer.text("\n")?;
        }
        writer.text("stack high-water: core 0 ")?;
        writer.integer(self.watermarks.core0_used)?;
        writer.text(" of ")?;
        writer.integer(self.watermarks.core0_total)?;
        writer.text(" B, core 1 ")?;
        writer.integer(self.watermarks.core1_used)?;
        writer.text(" of ")?;
        writer.integer(self.watermarks.core1_total)?;
        writer.text(" B\n\n")?;
        writer.text(&self.message)?;
        writer.text("\n")?;
        Some(writer.len)
    }
}

fn read4(bytes: &[u8], at: usize) -> [u8; 4] {
    // Every caller has already proved `bytes.len() >= HEADER_BYTES` and every
    // offset is inside it, so this cannot fail; the `unwrap_or` is what keeps
    // the function total instead of adding a panic to a decode path whose whole
    // point is that it never panics on bad input.
    bytes
        .get(at..at + 4)
        .and_then(|slice| slice.try_into().ok())
        .unwrap_or([0; 4])
}

/// The buffer was too small.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodeError;

/// Why some bytes are not a breadcrumb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// Fewer bytes than a header, or a message that runs off the end.
    Truncated,
    /// Not our record at all — erased flash, or another project's.
    Magic,
    /// Ours, but written by a different firmware layout.
    Version,
    /// A cause byte this firmware does not know.
    Cause,
    /// The message is not UTF-8, or is longer than this build allows.
    Message,
}

impl DecodeError {
    pub const fn as_str(self) -> &'static str {
        match self {
            DecodeError::Truncated => "truncated",
            DecodeError::Magic => "not a breadcrumb",
            DecodeError::Version => "wrong version",
            DecodeError::Cause => "unknown cause",
            DecodeError::Message => "bad message",
        }
    }
}

/// A bounded byte writer for [`Breadcrumb::render`]. Fails the whole render
/// rather than truncating; see that method's docs.
struct Renderer<'a> {
    out: &'a mut [u8],
    len: usize,
}

impl Renderer<'_> {
    fn text(&mut self, text: &str) -> Option<()> {
        let end = self.len.checked_add(text.len())?;
        self.out.get_mut(self.len..end)?.copy_from_slice(text.as_bytes());
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
        let end = self.len + (digits.len() - index);
        self.out.get_mut(self.len..end)?.copy_from_slice(&digits[index..]);
        self.len = end;
        Some(())
    }
}

/// The same never-fails formatter the ring uses, over this module's longer
/// message type.
struct Truncating<'a>(&'a mut String<MAX_MESSAGE>);

impl core::fmt::Write for Truncating<'_> {
    fn write_str(&mut self, fragment: &str) -> core::fmt::Result {
        let room = self.0.capacity() - self.0.len();
        let _ = self.0.push_str(crate::truncate_on_boundary(fragment, room));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Breadcrumb {
        let mut crumb = Breadcrumb::new(Cause::Panic, 1);
        crumb.uptime_s = 4_321;
        crumb.unix_s = 1_754_600_000;
        crumb.watermarks = Watermarks {
            core0_used: 9_000,
            core0_total: 415_520,
            core1_used: 3_348,
            core1_total: 8_192,
        };
        crumb.set_message(format_args!(
            "panicked at src/display_core1.rs:212: {}",
            "index out of bounds"
        ));
        crumb
    }

    #[test]
    fn a_record_survives_the_round_trip() {
        let crumb = sample();
        let mut bytes = [0u8; MAX_BYTES];
        let len = crumb.encode(&mut bytes).expect("encodes");
        assert_eq!(len, HEADER_BYTES + crumb.message.len());
        assert_eq!(Breadcrumb::decode(&bytes[..len]), Ok(crumb));
    }

    #[test]
    fn a_full_message_still_fits_max_bytes() {
        let mut crumb = Breadcrumb::new(Cause::WatchdogStarved, 0);
        crumb.set_message(format_args!("{}", "x".repeat(MAX_MESSAGE * 2)));
        assert_eq!(crumb.message.len(), MAX_MESSAGE);
        let mut bytes = [0u8; MAX_BYTES];
        assert_eq!(crumb.encode(&mut bytes), Ok(MAX_BYTES));
        assert_eq!(Breadcrumb::decode(&bytes), Ok(crumb));
    }

    #[test]
    fn erased_flash_is_not_a_breadcrumb() {
        assert_eq!(
            Breadcrumb::decode(&[0xFF; MAX_BYTES]),
            Err(DecodeError::Magic)
        );
        assert_eq!(Breadcrumb::decode(&[0x00; MAX_BYTES]), Err(DecodeError::Magic));
        assert_eq!(Breadcrumb::decode(&[]), Err(DecodeError::Truncated));
    }

    #[test]
    fn a_record_from_another_firmware_version_is_rejected_not_misread() {
        let mut bytes = [0u8; MAX_BYTES];
        let len = sample().encode(&mut bytes).expect("encodes");
        bytes[4] = VERSION + 1;
        assert_eq!(
            Breadcrumb::decode(&bytes[..len]),
            Err(DecodeError::Version)
        );
    }

    #[test]
    fn a_message_length_running_off_the_end_is_truncated_not_a_panic() {
        let mut bytes = [0u8; MAX_BYTES];
        let len = sample().encode(&mut bytes).expect("encodes");
        bytes[32..34].copy_from_slice(&(MAX_MESSAGE as u16 + 1).to_le_bytes());
        assert_eq!(
            Breadcrumb::decode(&bytes[..len]),
            Err(DecodeError::Truncated)
        );
    }

    #[test]
    fn an_unknown_cause_is_rejected() {
        let mut bytes = [0u8; MAX_BYTES];
        let len = sample().encode(&mut bytes).expect("encodes");
        bytes[5] = 9;
        assert_eq!(Breadcrumb::decode(&bytes[..len]), Err(DecodeError::Cause));
    }

    #[test]
    fn encoding_into_a_short_buffer_fails_rather_than_writing_part_of_one() {
        let crumb = sample();
        let mut bytes = [0u8; HEADER_BYTES];
        assert_eq!(crumb.encode(&mut bytes), Err(EncodeError));
        assert_eq!(bytes, [0u8; HEADER_BYTES], "nothing was written");
    }

    #[test]
    fn the_rendered_text_names_the_cause_the_core_and_the_message() {
        let crumb = sample();
        let mut out = [0u8; 512];
        let len = crumb.render(&mut out).expect("renders");
        let text = core::str::from_utf8(&out[..len]).expect("utf-8");
        assert!(
            text.starts_with("last abnormal shutdown: panic on core 1\n"),
            "{text}"
        );
        assert!(text.contains("uptime: 4321 s\n"), "{text}");
        assert!(text.contains("unix time: 1754600000\n"), "{text}");
        assert!(
            text.contains("stack high-water: core 0 9000 of 415520 B, core 1 3348 of 8192 B\n"),
            "{text}"
        );
        assert!(text.ends_with("index out of bounds\n"), "{text}");
    }

    #[test]
    fn an_unsynced_clock_leaves_the_unix_line_out_entirely() {
        let mut crumb = sample();
        crumb.unix_s = 0;
        let mut out = [0u8; 512];
        let len = crumb.render(&mut out).expect("renders");
        let text = core::str::from_utf8(&out[..len]).expect("utf-8");
        assert!(!text.contains("unix time"), "{text}");
    }

    #[test]
    fn a_render_that_does_not_fit_returns_none_rather_than_half_a_report() {
        let mut out = [0u8; 16];
        assert_eq!(sample().render(&mut out), None);
    }
}
