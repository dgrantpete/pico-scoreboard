//! The direct feed's transport: ESPN over TLS 1.3, one connection held open,
//! bodies streamed past the caller a slice at a time.
//!
//! `direct` builds have no backend, so this stands where
//! [`api_client`](super::api_client) stands on the proxy build's data plane:
//! `GET`s against `site.api.espn.com` for the scoreboards and `a.espncdn.com`
//! for the crests, over the stack `firmware-rs/tls-spike` proved on this
//! silicon in S2 — reqwless over embedded-tls, TLS 1.3, AES-128-GCM, P-256.
//! Every number quoted below is measured and lives in BUDGET.md "Phase S2".
//!
//! # Nothing is verified, and that was ruled rather than overlooked
//!
//! [`TlsVerify::None`]: the session is encrypted, the server is not
//! authenticated. ESPN and the CDN serve RSA-only chains, embedded-tls's RSA
//! verification hard-requires `alloc`, and SPEC §10's no-alloc contract does
//! not get sold for this — the owner's ruling, recorded in
//! PHASE-S-CHECKLIST.md under S2. It is still strictly more than the proxy
//! build has, whose data plane is plain HTTP on purpose.
//!
//! The corollary belongs here, where somebody will come looking for it: **the
//! parser and the PNG decoder are the boundary for hostile payloads**, not
//! this module. Nothing that arrives through here has been authenticated, and
//! that is exactly why the things downstream of it are no-alloc, bounded, and
//! swept for panics.
//!
//! # One connection, held between fetches
//!
//! The handshake costs 311 ms against ESPN and a kept-alive request costs
//! 50–110 ms, so keep-alive is not a nicety: the crest pipeline makes dozens
//! of small CDN fetches in a row, and paying a handshake for each one would be
//! most of the time it takes. The 28.5 h S2 soak — 2,890 polls, ~29
//! reconnects, every one successful — is why holding is safe to do and why the
//! reconnect below is worth having.
//!
//! One connection at a time, not a pool: a fetch to a host other than the held
//! one closes and redials, because two live connections would want two socket
//! slots and two sets of record buffers, and the poll loop never has two
//! fetches in flight. A fetch that fails on a *reused* connection is retried
//! once on a fresh one — ESPN's idle drop is invisible until you write to it —
//! but only while the sink has seen no bytes yet. Past that point the caller's
//! parser is mid-document and feeding it the same body twice would corrupt
//! what a reconnect was supposed to save.
//!
//! # A dropped fetch takes the connection with it
//!
//! [`EspnClient::fetch`] *takes* the held connection out of the client for the
//! length of the call and puts it back only after a clean finish. A future
//! dropped mid-body — a caller's `select`, or the timeout in here — drops the
//! connection with it, so the next fetch dials afresh instead of reading the
//! tail of a response nobody wanted. The invariant is ownership rather than a
//! flag somebody has to remember to clear.
//!
//! # Buffers, and the one finding that sizes them
//!
//! S2's surprise was that transfers are **TCP-window-limited, not
//! crypto-limited**: with `api_client`'s 1,536 B receive buffer a 215 KB
//! scoreboard took ~6 s at ~36 KB/s, window over round trip, while the crypto
//! never showed up as a bulk-rate ceiling at all. [`TCP_RX_BYTES`] is
//! therefore **16,384 B** — the "16 KB rx ⇒ sub-second polls" lever, and the
//! +14.8 KB the budget accepted for it. The TLS record buffers are the
//! spike's: 16,640 B read (a full TLS record plus its overhead) and 4,096 B
//! write.
//!
//! Bodies never exist in one piece. A 300–450 KB scoreboard has nowhere to sit
//! on a 512 KB device, and reqwless's whole-body `read_to_end` answers
//! `BufferTooSmall` for exactly that reason, so the sink is fed slice by slice
//! as the records arrive. The slices come straight out of embedded-tls's
//! decrypted record buffer — reqwless's body reader prefers it over a copy —
//! so streaming costs no buffer of its own beyond [`HEADER_BYTES`], which
//! holds the response headers and whatever body bytes shared their record.
//!
//! # Why there is an `unsafe` in here
//!
//! reqwless hands back a connection borrowed from the client that opened it,
//! and that client borrows the record buffers: `HttpResource<'res, _>` carries
//! the `'res` of the `&'res mut HttpClient` it came from. A connection that
//! lives *between* calls has to be `HttpResource<'static, _>`, which needs a
//! `&'static mut HttpClient`, which needs `&'static mut` record buffers — and
//! safe Rust hands those out exactly once, which is `ConstStaticCell::take`'s
//! entire contract. Reconnecting needs them a second time.
//!
//! So they live in [`SESSION`] behind `UnsafeCell`s and are lent to one
//! connection at a time: [`crate::http::scratch`]'s pattern with a single
//! holder instead of a pool. [`Lend::claim`] wins an `AtomicBool`, and every
//! reference it derives dies with the [`Connection`] that took it, because the
//! token releases the flag in `Drop` — after the connection's own fields,
//! which is what the field order there is for. Two live lends are not
//! improbable, they are unreachable: the second claim fails and is reported as
//! a connect failure rather than aliasing anything.
//!
//! # Socket budget
//!
//! One TCP slot, and unlike [`sntp`](super::sntp)'s it is *held* — between
//! polls, not just during one. That is the tenth of the slots in
//! [`net`](super)'s table and the reason [`SOCKETS`](super::SOCKETS) is 12
//! under `direct`.
//!
//! # The liveness clock
//!
//! Every answer stamps [`LAST_ANSWER_S`], the same evidence
//! [`api_client`](super::api_client) stamps for the same reason: headers back
//! from ESPN prove DNS resolved, TCP connected, TLS negotiated and bytes
//! crossed in both directions, which is the whole of the question
//! [`Health::since_answer_s`](scoreboard_model::poll::Health) asks. Every body
//! chunk stamps it too, so a slow 450 KB transfer cannot look like a device
//! that fell off the Wi-Fi.
//!
//! It is a *second* clock rather than `api_client`'s, because that one is
//! private to its module and a `direct` build still uses it for OTA. The
//! poller is what merges them — the newer of the two is the link's liveness —
//! and until it does, a `direct` build that never talks to a backend reads as
//! "never answered" to
//! [`poll::gate`](scoreboard_model::poll::gate), which resets the device on
//! the silence rule.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use embassy_net::Stack;
use embassy_net::dns::DnsSocket;
use embassy_net::tcp::client::{TcpClient, TcpClientState, TcpConnection};
use embassy_rp::clocks::RoscRng;
use embassy_time::{Instant, with_timeout};
use embedded_io_async::BufRead;
use reqwless::client::{HttpClient, HttpResource, TlsConfig, TlsVerify};
use reqwless::headers::TransferEncoding;
use reqwless::request::RequestBuilder as _;
use scoreboard_model::poll::{PollError, Transport};
use static_cell::{ConstStaticCell, StaticCell};

/// What ESPN's edge sees, verbatim: the prefix the backend allowlists
/// (`backend/config/default.toml`). Empirical rather than decorative — S2 sent
/// exactly this string on every real fetch and never saw the 403 an unknown
/// agent earns.
const USER_AGENT: &str = "python-requests/2.32.3 pico-scoreboard/1.0";

/// The only scheme this client speaks. A plain-`http` URL is a configuration
/// or a feed href to look at, not something to quietly downgrade to.
const SCHEME: &str = "https://";

/// The receive window, and the whole point of it. See the module docs: 16 KB
/// is what turns a six-second scoreboard fetch into a sub-second one, because
/// the transfer is bounded by window over round trip and nothing else.
const TCP_RX_BYTES: usize = 16_384;
/// Requests are a request line and three headers. `api_client` and the spike
/// both send them through 512 B.
const TCP_TX_BYTES: usize = 512;

/// One whole TLS record plus its overhead — embedded-tls's own documented
/// ceiling, and what S2 measured against every host this firmware talks to.
const TLS_READ_BYTES: usize = 16_640;
/// Bounds one outgoing record. Nothing this client sends comes close; the
/// handshake is what actually needs the room.
const TLS_WRITE_BYTES: usize = 4_096;

/// The header block, and the first body bytes that arrive in the same record.
///
/// reqwless answers `BufferTooSmall` when a response's headers do not fit, and
/// ESPN's edge is generous with them. 4,096 B is what the spike passed to
/// every real fetch it made, which makes it the size that has actually been
/// proved against these hosts rather than the size that looks sufficient.
const HEADER_BYTES: usize = 4_096;

/// `host` or `host:port`, bounded. Longer than either name this firmware
/// fetches from by a wide margin, and short enough that a malformed href
/// cannot spend a kilobyte proving it.
const AUTHORITY_BYTES: usize = 64;
/// `https://` plus an authority: what the connection is opened on.
const CONNECT_URL_BYTES: usize = AUTHORITY_BYTES + 8;

/// How much of a `404` body is read away to keep the connection usable.
///
/// ESPN's are a line of JSON, and a 404 is the one non-200 worth keeping a
/// connection for — the crest pipeline meets several in a refill. A body past
/// this is not worth the milliseconds: the connection is dropped instead, and
/// the next fetch dials.
const DRAIN_LIMIT: usize = 8 * 1024;

/// Uptime seconds at the last answer from ESPN; [`NEVER`] until the first one.
static LAST_ANSWER_S: AtomicU32 = AtomicU32::new(NEVER);

/// [`LAST_ANSWER_S`] before anything has answered. Not zero, for the reason
/// `api_client`'s says: zero is a real uptime.
const NEVER: u32 = u32::MAX;

/// Uptime seconds at the last answer from ESPN, or `None` if nothing ever has.
///
/// The `direct` half of what the poller assembles into
/// [`Health`](scoreboard_model::poll::Health) — see the module docs' last
/// section for why there are two of these and what happens if only one is
/// read.
pub fn last_answer_uptime_s() -> Option<u32> {
    let at = LAST_ANSWER_S.load(Ordering::Relaxed);
    (at != NEVER).then_some(at)
}

/// The link answered, at this instant.
fn stamp() {
    LAST_ANSWER_S.store(Instant::now().as_secs() as u32, Ordering::Relaxed);
}

/// The pooled socket's buffers. One connection, because that is what the
/// socket table budgeted and what the poll loop uses.
static TCP_STATE: ConstStaticCell<TcpClientState<1, TCP_TX_BYTES, TCP_RX_BYTES>> =
    ConstStaticCell::new(TcpClientState::new());
/// The connector itself. It has to outlive every connection it makes, which is
/// what putting it in a static says.
static TCP_CLIENT: StaticCell<TcpClient<'static, 1, TCP_TX_BYTES, TCP_RX_BYTES>> = StaticCell::new();
/// A handle onto the stack's resolver, not a socket of its own — the DNS slot
/// in [`net`](super)'s table is the one embassy-net always adds.
static DNS: StaticCell<DnsSocket<'static>> = StaticCell::new();
/// Response headers. Reborrowed per request; never lent to a connection, so it
/// needs none of [`SESSION`]'s machinery.
static HEADERS: ConstStaticCell<[u8; HEADER_BYTES]> = ConstStaticCell::new([0; HEADER_BYTES]);

/// The socket type one connection runs on.
type Socket = TcpConnection<'static, 1, TCP_TX_BYTES, TCP_RX_BYTES>;
/// The reqwless client, alive for exactly as long as the connection it opened.
type Client = HttpClient<'static, TcpClient<'static, 1, TCP_TX_BYTES, TCP_RX_BYTES>, DnsSocket<'static>>;
/// A connection, scoped to a host: reqwless's "resource" is a base URL plus an
/// open connection, which is precisely what keep-alive against one host is.
type Resource = HttpResource<'static, Socket>;

// ---------------------------------------------------------------------------
// The lend
// ---------------------------------------------------------------------------

/// Everything one connection borrows for its whole life. See the module docs'
/// "Why there is an `unsafe` in here".
struct Session {
    lent: AtomicBool,
    read: UnsafeCell<[u8; TLS_READ_BYTES]>,
    write: UnsafeCell<[u8; TLS_WRITE_BYTES]>,
    /// The URL the resource was opened on. It has to outlive the resource,
    /// because the resource keeps its host and base path as slices of it.
    url: UnsafeCell<heapless::String<CONNECT_URL_BYTES>>,
    /// The client the connection came from. Nothing reads it again — it exists
    /// so the connection's borrow has somewhere `'static` to point.
    client: UnsafeCell<Option<Client>>,
}

// SAFETY: every field is reached only through the references [`Lend::claim`]
// derives, and `claim` hands them out only to the winner of `lent` — one
// holder at a time, released in `Drop` after the connection built on them has
// been dropped. `http::scratch` makes the same argument for the same reason.
unsafe impl Sync for Session {}

static SESSION: Session = Session {
    lent: AtomicBool::new(false),
    read: UnsafeCell::new([0; TLS_READ_BYTES]),
    write: UnsafeCell::new([0; TLS_WRITE_BYTES]),
    url: UnsafeCell::new(heapless::String::new()),
    client: UnsafeCell::new(None),
};

/// Exclusive use of [`SESSION`], released when dropped.
struct Lend;

/// What one lend hands over, derived exactly once per claim.
struct Parts {
    read: &'static mut [u8],
    write: &'static mut [u8],
    url: &'static mut heapless::String<CONNECT_URL_BYTES>,
    client: &'static mut Option<Client>,
}

impl Lend {
    /// Take the session, or `None` if a connection still holds it.
    ///
    /// `None` is unreachable by construction — [`EspnClient::connect`] is the
    /// only caller and it drops the held connection first — and it is an
    /// `Option` anyway, because "still lent" is a reason to refuse a
    /// connection and never a reason to alias 20 KB of TLS state.
    fn claim() -> Option<(Lend, Parts)> {
        SESSION
            .lent
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()?;

        // SAFETY: this thread won the flag, so no other reference into
        // `SESSION` exists.
        let client = unsafe { &mut *SESSION.client.get() };
        // Before the record buffers are handed out again: the client left here
        // by the previous connection still holds the previous borrow of them,
        // and it has to be gone before the new one exists. Dropping it is free
        // — it is references and a seed — but the *order* is not decorative.
        *client = None;

        // SAFETY: as above, and the previous holder of these two is now gone.
        let (read, write, url) = unsafe {
            (
                &mut *SESSION.read.get(),
                &mut *SESSION.write.get(),
                &mut *SESSION.url.get(),
            )
        };

        Some((
            Lend,
            Parts {
                read,
                write,
                url,
                client,
            },
        ))
    }
}

impl Drop for Lend {
    fn drop(&mut self) {
        SESSION.lent.store(false, Ordering::Release);
    }
}

/// One open connection to one host.
struct Connection {
    /// Dropped first, and that is the point: it owns the TLS session and the
    /// socket, both of which borrow what [`Lend`] below is holding.
    resource: Resource,
    /// `host` or `host:port`, as the URL spelled it.
    authority: heapless::String<AUTHORITY_BYTES>,
    /// Declared last so the session is released after everything borrowing it
    /// has been dropped.
    #[allow(
        dead_code,
        reason = "never read, held for its Drop — the release side of the lend"
    )]
    lend: Lend,
}

// ---------------------------------------------------------------------------
// The client
// ---------------------------------------------------------------------------

/// What a fetch found. A `404` is a value here rather than an error: a game
/// that left today's scoreboard between the list and the detail is an ordinary
/// event, and so is a team with no crest on the CDN.
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum Fetched {
    /// A `200`, streamed to the sink in full.
    Complete,
    /// A `404`. The sink saw nothing.
    NotFound,
}

/// The ESPN client. Holds one connection and the statics behind it.
pub struct EspnClient {
    tcp: &'static TcpClient<'static, 1, TCP_TX_BYTES, TCP_RX_BYTES>,
    dns: &'static DnsSocket<'static>,
    headers: &'static mut [u8; HEADER_BYTES],
    /// `None` between hosts, after a failure, and after any fetch whose future
    /// was dropped.
    connection: Option<Connection>,
}

impl EspnClient {
    /// Panics on a second call: there is one set of record buffers and one
    /// socket, and two clients would both claim them.
    pub fn new(stack: Stack<'static>) -> EspnClient {
        let tcp: &'static TcpClient<'static, 1, TCP_TX_BYTES, TCP_RX_BYTES> =
            TCP_CLIENT.init(TcpClient::new(stack, TCP_STATE.take()));
        let dns: &'static DnsSocket<'static> = DNS.init(DnsSocket::new(stack));
        EspnClient {
            tcp,
            dns,
            headers: HEADERS.take(),
            connection: None,
        }
    }

    /// `GET url`, streaming the `200` body into `sink` as it arrives.
    ///
    /// `sink` is handed each slice exactly once, in order, and returns `false`
    /// to abort — the caller keeps its own reason, because a `bool` is all
    /// this layer needs to know (`api_client::download`'s bargain, for the
    /// same reason). An abort surfaces here as an `Err` and closes the
    /// connection, since a body that was not read to its end leaves nothing
    /// reusable behind it.
    ///
    /// A `404` is [`Fetched::NotFound`] and leaves the connection usable. Any
    /// other non-`200`, and every transport failure, is an `Err`.
    ///
    /// # The fifteen seconds
    ///
    /// One [`REQUEST_TIMEOUT`](super::api_client::REQUEST_TIMEOUT) covers the
    /// *whole* call — the dial, the handshake, the request, the streamed body
    /// and the one redial below — rather than one per leg. `api_client`'s
    /// convention is "every request, 15 s, no exceptions", and a per-leg
    /// ceiling would quietly turn that into 45 s on the redial path, which is
    /// a poll and a half at the default cadence. The measured worst case
    /// inside it is a 490 KB body over the LAN at ~3 s plus a 311 ms
    /// handshake, so the margin is fivefold and the redial fits.
    pub async fn fetch(
        &mut self,
        url: &str,
        sink: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Result<Fetched, PollError> {
        let (authority, path) = split_url(url)?;
        let started = Instant::now();
        let mut delivered = false;

        // Dropping this future on the timeout drops whatever connection it was
        // holding, and `TcpConnection::drop` closes the socket —
        // `api_client`'s `_with_timeout` teardown, arrived at by construction
        // rather than by an `except` arm. Nothing needs clearing afterwards:
        // `exchange` took the connection out of `self` on its way in.
        let outcome = with_timeout(
            super::api_client::REQUEST_TIMEOUT,
            self.exchange(authority, path, sink, &mut delivered),
        )
        .await
        .unwrap_or(Err(PollError::Timeout));

        match outcome {
            Ok(outcome) => {
                // `api_client`'s line, in the same words: the path, the status
                // and the time, at debug, because "which request was slow" is
                // what the ring log exists to answer on a device with no probe
                // attached.
                crate::debug!(
                    "espn: GET {} -> {} in {} ms",
                    path,
                    outcome.status,
                    started.elapsed().as_millis()
                );
                Ok(outcome.fetched)
            }
            Err(error) => {
                crate::error!(
                    "espn: GET {} failed after {} ms",
                    path,
                    started.elapsed().as_millis()
                );
                Err(error)
            }
        }
    }

    /// The connection half of a fetch: reuse or dial, ask, and redial once if
    /// asking on a kept-alive connection did not work.
    ///
    /// The connection is put back into `self` only by the arm that finished
    /// cleanly. Every other way out of here — an error, or this future being
    /// dropped — leaves it owned by a local that is about to be dropped, which
    /// closes it.
    async fn exchange(
        &mut self,
        authority: &str,
        path: &str,
        sink: &mut dyn FnMut(&[u8]) -> bool,
        delivered: &mut bool,
    ) -> Result<Outcome, PollError> {
        // Taken, not borrowed. A connection to another host is dropped right
        // here, which is also what frees the session for the dial below.
        let held = self
            .connection
            .take()
            .filter(|held| held.authority.eq_ignore_ascii_case(authority));
        let reused = held.is_some();
        let mut connection = match held {
            Some(connection) => connection,
            None => self.connect(authority).await?,
        };

        let first = request(&mut self.headers[..], &mut connection, path, sink, delivered).await;
        let (connection, outcome) = match first {
            // The idle drop the S2 soak kept hitting: the connection looked
            // fine and was not, which is only discoverable by writing to it.
            // One redial, and only while the sink is still untouched — past
            // that the caller's parser is mid-document and a second copy of
            // the body would corrupt what the redial was meant to save.
            Err(error) if reused && !*delivered && worth_redialing(&error) => {
                defmt::warn!("espn: kept-alive request failed, redialing once");
                // Before `connect`, which claims the session this is holding.
                drop(connection);
                let mut fresh = self.connect(authority).await?;
                let retry =
                    request(&mut self.headers[..], &mut fresh, path, sink, delivered).await;
                (fresh, retry)
            }
            first => (connection, first),
        };

        let outcome = outcome?;
        if outcome.reusable {
            self.connection = Some(connection);
        } else {
            defmt::warn!("espn: response was not delimited, connection closed");
        }
        Ok(outcome)
    }

    /// Hold a connection to `url`'s host, dialing if the held one is absent
    /// or elsewhere. A no-op when the right connection is already held.
    ///
    /// `fetch` dials lazily and does not need this. It exists for the stack:
    /// a TLS 1.3 handshake is the deepest synchronous work this module does,
    /// and the poller's streaming phases carry ~20 KB poll-frame allocas of
    /// their own — the S3 bring-up faulted exactly where the two met. Called
    /// from a small frame before the fat phases, the handshake's depth and
    /// the extractors' never stack. A failure is logged and swallowed: the
    /// fetch that follows will dial for itself and report properly.
    pub async fn connect_to(&mut self, url: &str) {
        let Ok((authority, _)) = split_url(url) else {
            return;
        };
        if self
            .connection
            .as_ref()
            .is_some_and(|held| held.authority.eq_ignore_ascii_case(authority))
        {
            return;
        }
        self.connection = None;
        match with_timeout(super::api_client::REQUEST_TIMEOUT, self.connect(authority)).await {
            Ok(Ok(connection)) => self.connection = Some(connection),
            Ok(Err(_)) | Err(_) => {
                defmt::debug!("espn: pre-connect failed, the fetch will redial");
            }
        }
    }

    /// Drop the held connection, if there is one.
    ///
    /// Nothing needs this to be correct — a fetch to another host redials by
    /// itself, and dropping the client closes what it holds. It is for the
    /// caller that knows it is finished for a while, such as the crest
    /// pipeline after a refill: an idle connection ESPN will drop anyway is
    /// worth less than the socket slot it sits in.
    #[allow(
        dead_code,
        reason = "that caller has not landed; whether the warmer takes this is \
                  a field-trial question — the held connection's idle behavior \
                  against the CDN is a thing to observe, not assume"
    )]
    pub fn close(&mut self) {
        if self.connection.take().is_some() {
            defmt::debug!("espn: connection closed by the caller");
        }
    }

    /// Dial a host: DNS, TCP, and the TLS 1.3 handshake.
    ///
    /// The caller must have dropped any previous connection — the claim below
    /// is what enforces it, and refusing is what it does instead of aliasing.
    async fn connect(&mut self, authority: &str) -> Result<Connection, PollError> {
        let (lend, parts) = Lend::claim().ok_or_else(|| {
            crate::error!("espn: the tls session is still lent out, refusing to connect");
            PollError::Transport(Transport::Connect)
        })?;
        let Parts {
            read,
            write,
            url: url_slot,
            client: client_slot,
        } = parts;

        url_slot.clear();
        if url_slot.push_str(SCHEME).is_err() || url_slot.push_str(authority).is_err() {
            return Err(PollError::Transport(Transport::BadUrl));
        }
        // `&'static str`, because the resource keeps its host and base path as
        // slices of this for as long as it lives. That is what the lend is
        // for: a local would not survive the return.
        let url: &'static str = url_slot;

        // A fresh seed per handshake, out of the ROSC rather than out of one
        // boot-time value: it is what the client random and the key share are
        // drawn from, and there is no reason for two connections to share it.
        let tls = TlsConfig::new(RoscRng.next_u64(), read, write, TlsVerify::None);
        let client: &'static mut Client =
            Option::insert(client_slot, HttpClient::new_with_tls(self.tcp, self.dns, tls));

        // Unbounded here on purpose: the whole fetch is already inside one
        // `with_timeout`, and a second ceiling around the dial would only be a
        // second number to keep in step with the first.
        let started = Instant::now();
        let resource: Resource = client.resource(url).await.map_err(transport_error)?;
        // At `info`, not `debug`: the bench runs `DEFMT_LOG=info`, where a
        // `debug` line does not exist, and how long a handshake took is the
        // first thing anyone asks when a poll feels slow. Probe only — the
        // ring's 128-byte lines are spent on the per-fetch line below.
        defmt::info!(
            "espn: connected to {=str} in {=u64} ms",
            authority,
            started.elapsed().as_millis()
        );

        let mut name = heapless::String::new();
        name.push_str(authority)
            .map_err(|_| PollError::Transport(Transport::BadUrl))?;
        Ok(Connection {
            resource,
            authority: name,
            lend,
        })
    }
}

/// What one request produced, plus what it says about the connection.
struct Outcome {
    fetched: Fetched,
    status: u16,
    /// Whether the connection can carry another request: the body was
    /// delimited by a length or by chunks, and it was read to its end.
    reusable: bool,
}

/// One request/response cycle on an open connection.
async fn request(
    headers: &mut [u8],
    connection: &mut Connection,
    path: &str,
    sink: &mut dyn FnMut(&[u8]) -> bool,
    delivered: &mut bool,
) -> Result<Outcome, PollError> {
    let response = connection
        .resource
        .get(path)
        .headers(&[("User-Agent", USER_AGENT)])
        .send(headers)
        .await
        .map_err(transport_error)?;
    // Headers back is already the evidence the liveness clock wants: every
    // hop between here and ESPN answered.
    stamp();

    let status = response.status.0;
    // A body with neither a length nor chunked framing ends when the
    // connection does (RFC 7230 §3.3.3), so reading it to the end is also
    // closing it. Rare from ESPN, and cheaper to notice than to rediscover as
    // a mysterious failure on the next fetch.
    //
    // A server that answered `Connection: close` is *not* checked for, and
    // deliberately: reading it back costs a 1 KB header iterator in this
    // future, against the one wasted request that `exchange`'s redial already
    // heals.
    let delimited = response.content_length.is_some()
        || response
            .transfer_encoding
            .contains(&TransferEncoding::Chunked);
    let mut reader = response.body().reader();

    match status {
        200 => {
            stream(&mut reader, sink, delivered).await?;
            Ok(Outcome {
                fetched: Fetched::Complete,
                status,
                reusable: delimited,
            })
        }
        404 => {
            // Drained rather than dropped: a 404 is an ordinary event on both
            // hosts, and paying a handshake for one would make the ordinary
            // case the expensive one.
            let drained = discard(&mut reader, DRAIN_LIMIT).await?;
            Ok(Outcome {
                fetched: Fetched::NotFound,
                status,
                reusable: delimited && drained,
            })
        }
        _ => {
            // Not drained, unlike the 404 above: this connection is not going
            // to be reused — `exchange` drops what a failed request was made
            // on — so reading a body nobody wants would be bytes off the
            // window and nothing else. Nor is the body parsed: unlike the
            // backend's, ESPN's error bodies are not a contract, and the
            // status is the whole of what is actionable.
            Err(PollError::http(status, ""))
        }
    }
}

/// Hand the body to `sink`, slice by slice, until it ends.
///
/// The slices are embedded-tls's own decrypted record buffer wherever reqwless
/// can manage it, so nothing here copies and nothing here allocates a chunk.
async fn stream<R>(
    reader: &mut R,
    sink: &mut dyn FnMut(&[u8]) -> bool,
    delivered: &mut bool,
) -> Result<(), PollError>
where
    R: BufRead<Error = reqwless::Error>,
{
    loop {
        let chunk = reader.fill_buf().await.map_err(transport_error)?;
        if chunk.is_empty() {
            return Ok(());
        }
        let length = chunk.len();
        // Bytes arriving *are* the evidence that the link works, and a 450 KB
        // body takes long enough that a clock stamped only at the headers
        // would let the health gate call a working download a dead network.
        stamp();
        // Set before the sink answers, not after: a sink that refuses the
        // first slice has still seen it, and a redial would show it the same
        // bytes again.
        *delivered = true;
        if !sink(chunk) {
            return Err(PollError::Transport(Transport::Io));
        }
        reader.consume(length);
    }
}

/// Read a body away and throw it out. `false` if it was longer than `limit`,
/// in which case it has *not* been read to its end and the connection is spent.
async fn discard<R>(reader: &mut R, limit: usize) -> Result<bool, PollError>
where
    R: BufRead<Error = reqwless::Error>,
{
    let mut total = 0;
    loop {
        let chunk = reader.fill_buf().await.map_err(transport_error)?;
        if chunk.is_empty() {
            return Ok(true);
        }
        let length = chunk.len();
        total += length;
        if total > limit {
            defmt::warn!("espn: error body past {=usize} B, connection closed", limit);
            return Ok(false);
        }
        reader.consume(length);
    }
}

/// Whether a failure is the kind a fresh connection can heal.
///
/// An HTTP status is not: the server answered, it just answered badly, and
/// asking twice would only make a struggling edge answer twice. Everything
/// else here is the connection itself, which is exactly what redialing
/// replaces — the S2 soak's idle drop arrives as a timeout or as an `Io`
/// depending on whether the peer bothered with a RST.
fn worth_redialing(error: &PollError) -> bool {
    matches!(
        error,
        PollError::Timeout
            | PollError::Transport(Transport::Io | Transport::Framing | Transport::Connect)
    )
}

/// Split `https://authority/path` the way reqwless's own parser will.
///
/// Only the authority and the path are wanted: one to decide whether the held
/// connection is the right one, the other to put in the request line. A
/// non-`https` URL is refused here rather than downgraded, and an authority
/// too long for [`AUTHORITY_BYTES`] is refused rather than truncated into a
/// request to somewhere unintended.
fn split_url(url: &str) -> Result<(&str, &str), PollError> {
    let rest = url
        .strip_prefix(SCHEME)
        .ok_or(PollError::Transport(Transport::BadUrl))?;
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    if authority.is_empty() || authority.len() > AUTHORITY_BYTES {
        return Err(PollError::Transport(Transport::BadUrl));
    }
    Ok((authority, path))
}

/// Map reqwless's error onto the vocabulary the panel speaks, logging the
/// reqwless one on the way past.
///
/// The log is the point of doing it in one place: [`Transport`] has a single
/// bucket for a dozen distinct faults, and `Error::Tls` is the arm that hides
/// the most — a handshake that failed on a signature scheme and one that
/// failed on a closed socket are the same word by the time the panel sees
/// them. At `warn`, because most of what arrives here is a kept-alive
/// connection the far end had already dropped, and the redial heals it; the
/// `error` line worth grepping for is the one [`EspnClient::fetch`] writes
/// when the whole fetch, redial included, has failed. Arm for arm this mirrors
/// `api_client`'s mapping, which is private to that module.
fn transport_error(error: reqwless::Error) -> PollError {
    use embedded_io::ErrorKind;

    defmt::warn!("espn: transport error {}", error);
    PollError::Transport(match error {
        reqwless::Error::Dns => Transport::Dns,
        reqwless::Error::InvalidUrl(_) => Transport::BadUrl,
        reqwless::Error::BufferTooSmall => Transport::TooLarge,
        reqwless::Error::Codec => Transport::Framing,
        reqwless::Error::ConnectionAborted => Transport::Io,
        reqwless::Error::AlreadySent | reqwless::Error::IncorrectBodyWritten => Transport::Io,
        reqwless::Error::Network(ErrorKind::ConnectionRefused | ErrorKind::ConnectionReset) => {
            Transport::Connect
        }
        // No usable session with the host: the Connect bucket's meaning. The
        // line above is where the detail that would justify a finer split
        // actually lives.
        reqwless::Error::Tls(_) => Transport::Connect,
        reqwless::Error::Network(_) => Transport::Io,
    })
}

/// Station mode's eight slots, `sntp`'s transient ninth, and this connection's
/// held tenth — the table in [`net`](super)'s module docs. A future consumer
/// that pushes past [`SOCKETS`](super::SOCKETS) fails the build here rather
/// than failing to connect on a Sunday afternoon.
const _: () = assert!(
    super::SOCKETS >= 10,
    "station mode, a transient sntp socket and a held espn connection need ten slots"
);
