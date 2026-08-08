//! The poller's pure half: what a failure means, when a skip is accepted, and
//! how big a receive buffer has to be.
//!
//! [`Slate`](crate::Slate) already owns the rotation and [`Store`] owns the
//! commits, so what is left of `poller.py` splits cleanly in two. The sockets,
//! the ETags and the sleep are the app's — they are I/O. Everything here is a
//! decision the app *makes* about that I/O, and every one of them is a decision
//! a firmware bug hides in, so they live where a desktop can run them (SPEC §2's
//! crate-boundary rule).
//!
//! Three things, then:
//!
//! - [`PollError`] and [`friendly`] — `_friendly_error`'s mapping onto the four
//!   lines the panel has room for.
//! - [`FailureTracker`] — `MAX_FAILURES`, the streak, and the "failing for N m"
//!   line. **There is no exponential backoff**, here or in `poller.py`: the
//!   sleep is always `poll_interval_seconds`. A scoreboard that has been failing
//!   for an hour should recover within one interval of the backend returning,
//!   not within an hour.
//! - [`SkipMachine`] — the arm/reject/in-flight rules a button press meets.
//!
//! plus [`RESPONSE_BYTES`] and its split, which is a decision about the *wire*
//! and belongs next to the code that reads it.

use scoreboard_wire::DecodeError;

use crate::snapshot::{ERROR_LINES, LINE, Millis};
use crate::store::Store;
use crate::text::{Text, set_capped, set_line, set_plain, write_args};

// ---------------------------------------------------------------------------
// Receive buffer sizing
// ---------------------------------------------------------------------------

/// The one receive buffer every backend response lands in.
///
/// `api_client.py:22-27` picked 4 KB and justified it as "~3.5× the largest
/// body (24×24 RGB565 logo = 1,152 B)". The constant is right and the
/// derivation is not: **the games list is the largest body, not the logo**, and
/// it is the only one that scales with anything.
///
/// A list entry is `u8 state` + `u8 length` + the id
/// ([`list`](scoreboard_wire::list)), and the count is a `u8`, so the format's
/// own ceiling is [`MAX_GAMES`](scoreboard_wire::MAX_GAMES) entries. ESPN game
/// ids are nine digits across every league the corpus covers
/// ([`MAX_GAME_ID_BYTES`], asserted against it), so:
///
/// | | bytes |
/// |---|---:|
/// | list body at the format's ceiling: `2 + 255 × (2 + 9)` | 2,807 |
/// | HTTP response header block, measured against the deployed backend | 386 |
/// | **worst case** | **3,193** |
/// | [`RESPONSE_BYTES`] | 4,096 |
/// | spare | 903 |
///
/// The header block counts because reqwless reads headers and body into the
/// same caller-owned buffer — it parses the headers in place, then moves the
/// body down to the front — so the peak is their sum, not the larger of the
/// two. 386 B is what `pico-scoreboard-api-dgrantpete.fly.dev` sends today
/// (status line, content-type, ETag, two `vary`s, CORS, content-length, date,
/// server, two `via`s and a Fly request id); [`MAX_HEADER_BLOCK`] is the
/// allowance.
///
/// Overflow is loud, not silent: reqwless answers `BufferTooSmall`, which
/// reaches the panel as an error screen. `api_client.py` promised the same
/// ("readinto will fail loudly") for the same reason.
pub const RESPONSE_BYTES: usize = 4096;

/// The receive buffer's front half, during a game-detail poll.
///
/// A detail and a crest are live *at the same time* — the commit needs the
/// decoded game and both crest handles — so the buffer is split for that phase
/// rather than used whole. A list refresh and a detail poll never overlap
/// (`_refresh_lists` completes before `_poll_current`), so the list gets all of
/// it and this split costs nothing.
///
/// 2,048 B against a corpus maximum of 148 B and a computed worst case near
/// 800 B (a soccer live game carrying a 255-byte commentary line), plus the
/// header block.
pub const DETAIL_BYTES: usize = 2048;

/// The receive buffer's back half: one 24×24 RGB565 crest, 1,152 B, plus the
/// header block. 510 B spare.
pub const LOGO_BYTES: usize = RESPONSE_BYTES - DETAIL_BYTES;

/// Allowance for an HTTP response's status line and headers. Measured at 386 B
/// against the deployed backend; this is 1.3× that, and an intermediary that
/// exceeds it fails the request loudly rather than truncating a body.
pub const MAX_HEADER_BLOCK: usize = 512;

/// The longest game id the corpus carries. Asserted, not assumed — see
/// [`RESPONSE_BYTES`], which is derived from it.
pub const MAX_GAME_ID_BYTES: usize = 9;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// An error code from a backend error body — `{"error": "not_found"}`.
pub const ERROR_CODE: usize = 32;

/// The kind line, the first of the four the error screen shows.
pub const ERROR_KIND: usize = 16;

/// Both detail lines' worth of text, before it is split.
pub const ERROR_DETAIL: usize = 64;

/// Characters per error-screen line — one line of `spleen_5x8` across the
/// panel, which is what `poller.py` sliced at.
pub const DETAIL_LINE_CHARS: usize = 25;

/// The title `set_error` is always called with. The *kind* is a line, not the
/// title: the title says which subsystem failed and the kind says how.
pub const ERROR_TITLE: &str = "API ERROR";

/// Polls that must fail in a row before the panel says so.
///
/// `poller.py:243`. Five polls is 2.5 minutes at the default 30 s interval —
/// long enough that a backend restart or a Wi-Fi hiccup passes unremarked, short
/// enough that a genuinely dead feed does not leave a stale score on the wall.
pub const MAX_FAILURES: u32 = 5;

/// Why a request did not produce a payload.
///
/// The transport half of `_friendly_error`'s `isinstance` chain, as a type.
/// MicroPython matched on `asyncio.TimeoutError`, `ApiError`, `DeserializeError`
/// and `OSError` in that order and fell through to the exception's class name;
/// the fall-through arm has no counterpart here because this enumerates every
/// case, which is the point of doing it with an enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollError {
    /// The 15 s request timeout expired. `asyncio.TimeoutError`.
    Timeout,
    /// A 4xx or 5xx. `code` is the error body's `error` field — error bodies are
    /// always JSON, whatever the `Accept` header asked for — or
    /// `unknown_error` when the body was not a JSON object.
    Http { status: u16, code: Text<ERROR_CODE> },
    /// The payload was not the wire format. `DeserializeError`.
    Decode(DecodeError),
    /// Everything `OSError` covered: DNS, connect, the socket, and the framing
    /// of the response around the body.
    Transport(Transport),
}

/// The transport failures, named rather than left as an errno.
///
/// `str(OSError)` gave `[Errno 113] EHOSTUNREACH`, which is 24 characters of
/// which four are useful on a 25-character line. These say the same thing in
/// the words the owner of a scoreboard could act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// The backend's name did not resolve.
    Dns,
    /// No TCP connection.
    Connect,
    /// The connection dropped mid-request.
    Io,
    /// A response that is not HTTP, or headers that did not fit
    /// [`MAX_HEADER_BLOCK`].
    Framing,
    /// The body did not fit the receive buffer. See [`RESPONSE_BYTES`].
    TooLarge,
    /// `api.url` is not a URL. Reachable only from a hand-written config.
    BadUrl,
}

impl Transport {
    pub const fn detail(self) -> &'static str {
        match self {
            Transport::Dns => "cannot resolve backend",
            Transport::Connect => "cannot reach backend",
            Transport::Io => "connection lost",
            Transport::Framing => "bad http response",
            Transport::TooLarge => "response too large",
            Transport::BadUrl => "api url is not valid",
        }
    }
}

impl PollError {
    /// A 4xx/5xx whose body named no error code.
    pub fn http(status: u16, code: &str) -> PollError {
        let mut text = Text::new();
        set_plain(
            &mut text,
            if code.is_empty() { "unknown_error" } else { code },
        );
        PollError::Http { status, code: text }
    }
}

/// The two strings `_friendly_error` returns: what went wrong, and the detail
/// under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Friendly {
    pub kind: Text<ERROR_KIND>,
    pub detail: Text<ERROR_DETAIL>,
}

/// Map an error onto the panel's vocabulary. `poller.py:73-83`, arm for arm.
pub fn friendly(error: &PollError) -> Friendly {
    let mut kind = Text::new();
    let mut detail = Text::new();
    match error {
        PollError::Timeout => {
            set_plain(&mut kind, "Timeout");
            set_plain(&mut detail, "backend not responding");
        }
        PollError::Http { status, code } => {
            write_args(&mut kind, format_args!("HTTP {status}"));
            set_plain(&mut detail, code.as_str());
        }
        PollError::Decode(error) => {
            set_plain(&mut kind, "Bad response");
            // `@29: truncated inside game_id: need 9 bytes, have 3` — the
            // offset is often the only clue a device can give, which is why
            // `DecodeError` carries it and why it leads the line.
            write_args(&mut detail, format_args!("{error}"));
        }
        PollError::Transport(transport) => {
            set_plain(&mut kind, "Network error");
            set_plain(&mut detail, transport.detail());
        }
    }
    Friendly { kind, detail }
}

/// The four lines a sustained failure puts on the panel.
///
/// `poller.py:377-383`: the kind, then the detail split across up to two
/// 25-character lines, then how long it has been failing. Four lines is
/// [`ERROR_LINES`] exactly, which is why the detail gets two and not three.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorScreen {
    lines: heapless::Vec<Text<LINE>, ERROR_LINES>,
}

impl ErrorScreen {
    fn build(friendly: &Friendly, failing_minutes: u32) -> ErrorScreen {
        let mut lines = heapless::Vec::new();
        let mut push = |text: &str| {
            let mut line = Text::new();
            set_line(&mut line, text);
            let _ = lines.push(line);
        };
        push(friendly.kind.as_str());

        let mut first = Text::<LINE>::new();
        set_capped(&mut first, friendly.detail.as_str(), DETAIL_LINE_CHARS);
        push(first.as_str());
        // The tail, if the detail did not fit one line. Character counts, not
        // bytes: a folded name can carry a multi-byte codepoint and the panel
        // measures glyphs.
        let mut overflow = Text::<LINE>::new();
        for character in friendly
            .detail
            .chars()
            .skip(DETAIL_LINE_CHARS)
            .take(DETAIL_LINE_CHARS)
        {
            let _ = overflow.push(character);
        }
        if !overflow.is_empty() {
            push(overflow.as_str());
        }

        let mut age = Text::<LINE>::new();
        write_args(&mut age, format_args!("failing for {failing_minutes}m"));
        push(age.as_str());
        ErrorScreen { lines }
    }

    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().map(Text::as_str)
    }

    /// Put this screen on the panel.
    ///
    /// A method rather than a `&[&str]` accessor because building that slice
    /// needs somewhere to put the borrows, and every caller would build the
    /// same fixed-size array. [`Store::set_error`] takes the title as an
    /// argument; [`ERROR_TITLE`] is the only one this crate ever passes.
    pub fn commit(&self, store: &mut Store) {
        let mut refs: [&str; ERROR_LINES] = [""; ERROR_LINES];
        for (slot, line) in refs.iter_mut().zip(self.lines()) {
            *slot = line;
        }
        store.set_error(ERROR_TITLE, &refs[..self.lines.len()]);
    }
}

/// The consecutive-failure count, and the screen it eventually produces.
///
/// `GamePoller._consecutive_failures` / `_first_failure_ms`, which were two
/// fields updated at three call sites. Here the update rule is the type's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FailureTracker {
    streak: u32,
    first_failure_ms: Millis,
}

/// What a failed tick did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// How many polls have now failed in a row.
    pub streak: u32,
    /// The panel's answer, once the streak reaches [`MAX_FAILURES`]. Present on
    /// *every* failure from there on, not only the fifth, so the "failing for
    /// N m" line keeps counting.
    pub screen: Option<ErrorScreen>,
}

impl FailureTracker {
    pub const fn new() -> FailureTracker {
        FailureTracker {
            streak: 0,
            first_failure_ms: 0,
        }
    }

    pub const fn streak(&self) -> u32 {
        self.streak
    }

    /// True once the panel is showing the error screen — the point past which
    /// a success is worth an ERROR-level "recovered" line.
    pub const fn failing(&self) -> bool {
        self.streak >= MAX_FAILURES
    }

    /// A tick succeeded. Returns the streak it ended, or `None` if there was
    /// nothing to recover from.
    pub fn record_success(&mut self) -> Option<u32> {
        let streak = self.streak;
        self.streak = 0;
        (streak > 0).then_some(streak)
    }

    /// A tick failed.
    pub fn record_failure(&mut self, now_ms: Millis, error: &PollError) -> Failure {
        if self.streak == 0 {
            self.first_failure_ms = now_ms;
        }
        self.streak += 1;
        let screen = (self.streak >= MAX_FAILURES).then(|| {
            let minutes = now_ms.saturating_sub(self.first_failure_ms) / 60_000;
            ErrorScreen::build(&friendly(error), minutes as u32)
        });
        Failure {
            streak: self.streak,
            screen,
        }
    }
}

// ---------------------------------------------------------------------------
// The skip machine
// ---------------------------------------------------------------------------

/// What a button asked the rotation to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipKind {
    /// Button A, short press: the next game.
    Game,
    /// Button A, long press: the next league.
    League,
}

/// Whether a press was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipVerdict {
    /// Taken. The caller shows the sticky spinner and wakes the poll loop.
    Armed,
    /// Refused, because one is already armed or in flight. The caller dims the
    /// visible toast one cycle — the press is **rejected, not queued**, which
    /// is what stops a burst of presses from walking the rotation.
    Rejected,
}

/// `poller.py`'s skip state, which was three booleans and a comment.
///
/// # Why this is not a lock-free anything
///
/// MicroPython's argument was that the poll loop, the button hooks and this
/// machine all ran on Core 0's single asyncio loop, so the flags could be plain
/// booleans that only ever interleave at `await` points. That argument is true
/// of the Rust firmware too — every task that touches the rotation runs on core
/// 0's one executor — but it is **not** the argument being relied on, because
/// it is an argument about where tasks happen to be spawned and it fails
/// silently if one moves.
///
/// Instead this is an ordinary value owned by the poller task, mutated through
/// `&mut self`, and a press reaches it as a message. There is no shared state
/// to reason about: a second owner would not compile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SkipMachine {
    requested: Option<SkipKind>,
    in_flight: bool,
}

impl SkipMachine {
    pub const fn new() -> SkipMachine {
        SkipMachine {
            requested: None,
            in_flight: false,
        }
    }

    /// A press arrived.
    pub fn request(&mut self, kind: SkipKind) -> SkipVerdict {
        if self.requested.is_some() || self.in_flight {
            return SkipVerdict::Rejected;
        }
        self.requested = Some(kind);
        SkipVerdict::Armed
    }

    /// Take whatever is armed, at the top of a tick. A consumed skip owns the
    /// sticky spinner for exactly that tick.
    pub fn consume(&mut self) -> Option<SkipKind> {
        let requested = self.requested.take();
        self.in_flight = requested.is_some();
        requested
    }

    /// The tick is over, however it ended.
    ///
    /// Returns whether a skip was in flight, which is the signal to tear the
    /// spinner down. `poller.py` did this in a `finally` so that success, an
    /// empty slate, a 404 and a mid-flight exception all released the toast;
    /// here the caller runs it on every path for the same reason, and the
    /// return value is what makes forgetting visible.
    pub fn finish(&mut self) -> bool {
        core::mem::take(&mut self.in_flight)
    }

    pub const fn armed(&self) -> bool {
        self.requested.is_some()
    }
}
