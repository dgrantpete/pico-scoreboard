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

// ---------------------------------------------------------------------------
// Supervision: what the watchdog is allowed to conclude
// ---------------------------------------------------------------------------

/// Poll intervals a request may wait for an answer before the link is presumed
/// dead.
///
/// Three, so a single failed poll and its retry both pass unremarked. At the
/// default 30 s interval that is 90 s — long enough that a Wi-Fi hiccup or a
/// backend restart never reaches it, short enough that a scoreboard which has
/// genuinely fallen off the network is back within two minutes of trying to use
/// it.
pub const SILENCE_INTERVALS: u32 = 3;

/// The link's evidence, as the API client observes it: when something last
/// answered, and since when the oldest request still without an answer has been
/// waiting. Uptime seconds throughout.
///
/// # Waiting, not silence
///
/// The gate's first two versions measured *silence* — seconds since anything
/// answered. That reads a poller that did not **ask** exactly like a link that
/// did not **answer**, and the poller has three legitimate reasons to ask
/// nothing for longer than the gate's patience:
///
/// - **An empty slate.** With no game in any enabled league the tick makes no
///   request, and the lists refresh only once per `game_rotation_seconds` — so
///   any rotation more than twice the poll interval read as a dead link on
///   every day without a game, and all winter on an MLB-only unit.
/// - **No sports enabled.** The only requests left are the daily clock sync and
///   the daily update check.
/// - **No usable `api.url`.** [`Transport::BadUrl`] is decided before anything
///   touches the network.
///
/// None of the three could bite while the gate was opt-in. Making it
/// unconditional on fleet builds (2026-10-14, BACKLOG 69) would have turned each
/// into a reboot loop, so the clock now starts when a request **leaves** and
/// stops when anything **answers**: a poller that is quiet by design is never
/// waiting, and a link that has died is caught by the first request sent into
/// it.
///
/// # Leaving, answering, and keeping the earliest
///
/// A request leaves when the client hands a parsed URL to DNS and TCP — which is
/// why [`withdraw`](LinkClock::withdraw) exists: the client cannot tell a URL
/// that will not parse from one that will until it has already announced the
/// request, and a request that never left is not evidence of anything.
///
/// A request is answered when a status line and headers arrive, whatever the
/// status: a 404 and a 500 prove the link as well as a 200 does (BACKLOG 70).
/// A streamed body is one request per read, so a transfer that stalls part-way
/// waits like any other request and a slow one that keeps moving never does.
///
/// A request that fails leaves the clock running, and the next one does **not**
/// restart it. The gate measures the *oldest* unanswered request, so a dead
/// link's patience runs from the first request sent into it rather than being
/// reset by every retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LinkClock {
    last_answer_s: Option<u32>,
    waiting_since_s: Option<u32>,
}

impl LinkClock {
    /// Nothing asked, nothing answered.
    pub const fn new() -> LinkClock {
        LinkClock {
            last_answer_s: None,
            waiting_since_s: None,
        }
    }

    /// A request is about to leave. Starts the clock unless an older request
    /// is still waiting, and returns whether this call started it — the one
    /// fact [`withdraw`](LinkClock::withdraw) needs.
    #[must_use = "a request that turns out never to have left must withdraw what it started"]
    pub fn ask(&mut self, now_s: u32) -> bool {
        if self.waiting_since_s.is_some() {
            return false;
        }
        self.waiting_since_s = Some(now_s);
        true
    }

    /// The request [`ask`](LinkClock::ask) announced never left — its URL did
    /// not parse, which reqwless decides before it touches the network. Takes
    /// the stamp back only if that `ask` made it: an older request that is still
    /// waiting keeps its claim.
    pub fn withdraw(&mut self, started: bool) {
        if started {
            self.waiting_since_s = None;
        }
    }

    /// Something answered at the HTTP layer. Every waiting request has its
    /// answer, because the only question any of them was evidence for — is the
    /// link alive — just got one.
    pub fn answer(&mut self, now_s: u32) {
        self.last_answer_s = Some(now_s);
        self.waiting_since_s = None;
    }

    /// Seconds since anything answered, or `None` if nothing ever has.
    pub fn since_answer_s(&self, now_s: u32) -> Option<u32> {
        self.last_answer_s.map(|at| now_s.saturating_sub(at))
    }

    /// How long the oldest unanswered request has been waiting, or `None` when
    /// nothing is.
    pub fn unanswered_s(&self, now_s: u32) -> Option<u32> {
        self.waiting_since_s.map(|at| now_s.saturating_sub(at))
    }
}

/// What the poller knows about the network, for the watchdog's health gate.
///
/// # The failure this exists for
///
/// BACKLOG 69: the bench unit fell off the Wi-Fi overnight and **kept rendering
/// at a perfect 20 FPS**. No link-down event fired — embassy-net still believed
/// its IPv4 configuration was up — so a frame counter, which is the obvious
/// thing for a render-loop watchdog to read, proved nothing at all. The poller
/// is the only part of the firmware that finds out, because it is the only part
/// that talks to anything. The same failure took a friend's unit off the air
/// three times on 2026-10-12/13, each time until somebody pulled the plug.
///
/// # Three clocks, and only one of them may starve a watchdog
///
/// [`unanswered_s`](Health::unanswered_s) is the *link's* clock,
/// [`since_success_s`](Health::since_success_s) is the *backend's*, and
/// [`since_answer_s`](Health::since_answer_s) is a record of the last time the
/// link was proved. Conflating the link with the backend was the gate's first
/// version, and it was wrong in a way that took a live outage to see clearly: a
/// backend that is up and returning 500s, or a path that 404s, is a **working
/// network**, and rebooting the device over it gives a reboot loop for as long
/// as the outage lasts. MicroPython showed the error screen and sat there,
/// which is the better behaviour. Keying on time since the last answer was the
/// second version, and it was wrong the other way — see [`LinkClock`].
///
/// So the gate keys on `unanswered_s` alone. The streak still exists, still
/// counts, and still puts [`ERROR_TITLE`] on the panel at [`MAX_FAILURES`] —
/// it just never starves anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Health {
    /// Consecutive failed ticks. Zero means the last tick succeeded. Drives the
    /// error screen; deliberately **not** an input to [`gate`].
    pub streak: u32,
    /// Seconds since a poll last completed end to end — fetched, decoded and
    /// committed. `None` if that has never happened.
    pub since_success_s: Option<u32>,
    /// Seconds since the backend last **answered at the HTTP layer**, whatever
    /// it said. `None` if it never has.
    ///
    /// Not a gate input any more — a quiet poller lets it grow without bound,
    /// which is [`LinkClock`]'s whole argument. What still reads it is the OTA
    /// confirm, whose question really is "has the backend answered at all this
    /// boot".
    pub since_answer_s: Option<u32>,
    /// How long the oldest request still without an answer has been waiting.
    /// `None` when nothing is — including a poller with nothing to ask.
    ///
    /// "Answered" means a response reached the client: a status line and
    /// headers, any status, including a 404 or a 500 and including a body that
    /// then fails to decode. All of them prove DNS resolved, TCP connected and
    /// bytes crossed in both directions — which is the whole question the
    /// watchdog is asking.
    ///
    /// **Why the HTTP layer and not TCP.** A connection *refused* is, in
    /// principle, equally good evidence: a RST came back, so something is alive
    /// out there. It is not usable evidence here, for two measured reasons.
    /// embassy-net answers `ConnectionReset` for a refused connect and for an
    /// exhausted socket pool alike, so the firmware's `Transport::Connect`
    /// already cannot tell them apart. And on the bench, pointing `api.url` at a
    /// closed port produced a *timeout*, not a connect error at all — so a gate
    /// keyed on "refused" would have starved in exactly the case it was written
    /// to exempt. The discrimination is not there to key on.
    ///
    /// Against the deployed backend the distinction is nearly vacuous anyway: a
    /// dead app behind Fly's edge answers 502, which is an HTTP answer. The
    /// residual is a hand-typed `api.url` that parses and points at a
    /// reachable-but-refusing address, which reads as link death and resets the
    /// device every couple of minutes — a misconfiguration, explained at
    /// `/api/logs/previous` and fixable from the settings page between resets.
    /// An `api.url` that does not parse at all never sends anything, never
    /// waits, and shows its error screen instead.
    pub unanswered_s: Option<u32>,
}

impl Health {
    /// Nothing has happened yet.
    pub const fn new() -> Health {
        Health {
            streak: 0,
            since_success_s: None,
            since_answer_s: None,
            unanswered_s: None,
        }
    }
}

impl Default for Health {
    fn default() -> Health {
        Health::new()
    }
}

/// Why the health gate refused to feed the watchdog. `None` is a healthy
/// device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unhealthy {
    /// Core 1's frame counter did not move across a whole feed interval. A
    /// *quiet* render loop is not a stalled one — an idle screen skips every
    /// frame and still counts ticks — so this really is "the loop stopped".
    RenderLoopStalled,
    /// A request has waited [`SILENCE_INTERVALS`] poll intervals for an answer
    /// and nothing at all has answered meanwhile. The BACKLOG 69 failure;
    /// `seconds` is how long the oldest request had waited.
    LinkSilent { seconds: u32 },
}

impl Unhealthy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Unhealthy::RenderLoopStalled => "render loop stalled",
            Unhealthy::LinkSilent { .. } => "link silent",
        }
    }
}

/// Decide whether the watchdog has earned a feed.
///
/// `ticked` is whether core 1's frame counter moved since the last check.
/// `poll_interval_s` is `None` in **setup mode**, where there is no poller by
/// design — gating on its health there would reset the device every few seconds
/// while somebody was typing a Wi-Fi password into the settings page, so only
/// the render loop counts.
///
/// There is no fallback for a clock that has never started. A device that has
/// sent nothing has proved nothing either way, and the first request it sends
/// starts the count — so a unit that joins a network and finds it already dead
/// starves [`SILENCE_INTERVALS`] intervals after its first poll, which is the
/// same patience a running one gets.
///
/// # What this no longer catches, and why that is acceptable
///
/// Silence also caught a poller that had *stopped*: a task wedged somewhere
/// stopped being answered because it stopped asking. Keyed on waiting
/// requests, a poller wedged outside a request is never waiting and is fed.
/// The loss is bounded by the shape of the poll loop rather than by anything
/// here: every `await` in it is a request (under the client's 15 s timeout, and
/// under this clock), a streamed download (read by read under this clock, and
/// under its own ceiling), a timer, or a command channel raced against a timer.
/// There is nothing left for it to wedge on that this clock does not see. A
/// hang of the whole executor stops the feeder task with it, and the hardware
/// watchdog needs no gate for that.
pub fn gate(ticked: bool, poll_interval_s: Option<u32>, health: &Health) -> Option<Unhealthy> {
    if !ticked {
        return Some(Unhealthy::RenderLoopStalled);
    }
    let poll_interval_s = poll_interval_s?;
    let waiting = health.unanswered_s?;
    if waiting > SILENCE_INTERVALS.saturating_mul(poll_interval_s) {
        return Some(Unhealthy::LinkSilent { seconds: waiting });
    }
    None
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
