//! The backend client: plain HTTP, one request at a time, into a buffer the
//! caller owns.
//!
//! Port of `api_client.py` (SPEC §7.4) on reqwless. Four endpoints, all `GET`:
//! a league's games list, one game's detail, a team crest, and the clock.
//!
//! # Plain HTTP, and no API key
//!
//! `api_client.py:94` rewrites the configured `https://` URL to `http://` for
//! score polling and this keeps the rewrite, so a config written by the
//! MicroPython firmware migrates unchanged. The reasoning has changed shape but
//! not direction: MicroPython's was that a persistent TLS session costs ~21 KB
//! of mbedTLS record buffers, and here it is SPEC §8 — the device's integrity
//! root is the signed OTA image, not the transport. Either way **no API key is
//! sent**: these routes are unauthenticated backend-side, and a cleartext key
//! would leak. `config.api.key` stays in the configuration for OTA.
//!
//! # One in-flight request, proved rather than checked
//!
//! `api_client.py:103-129` raises `RuntimeError` when a second request starts
//! while one is running, because both would land in the same shared buffer and
//! the first caller would parse the second's response. Here every request takes
//! `&mut self` and hands back a borrow of the caller's buffer, so a second
//! request *while a response is still being read* does not compile. The runtime
//! guard has no counterpart because the condition it detected is unreachable.
//!
//! That is also what makes the receive buffer's aliasing rule enforceable. The
//! MicroPython client returned a `memoryview` "valid only until the next
//! request" and relied on callers not to await in between; the borrow checker
//! now says exactly that, and [`crate::poller`] splits its one buffer
//! ([`poll::RESPONSE_BYTES`]) when it genuinely needs a detail and a crest
//! alive at once.
//!
//! # A connection per request
//!
//! **Deviation.** `aiohttp.ClientSession` held one connection open across polls
//! and closed it on timeout; this opens one per request and drops it after.
//!
//! Three reasons. The poll interval is 30 s by default, which is past the idle
//! timeout of every proxy between here and the backend — the "persistent"
//! session was reconnecting on most polls anyway, it just could not tell you
//! so. Dropping the connection on timeout stops being a special case
//! (`_with_timeout`'s `session.close()`) and becomes what always happens, so
//! the wedged-mid-request state that motivated it cannot persist. And the
//! socket is only held while a request is in flight, which is what lets the
//! whole poller fit in the single socket `net`'s budget reserved for it.
//!
//! The cost is one TCP handshake per request. Measured against the deployed
//! backend it is ~60 ms, against a tick that makes at most four requests every
//! 30 s.

use core::fmt::Write as _;

use embassy_net::Stack;
use embassy_net::dns::DnsSocket;
use embassy_net::tcp::client::{TcpClient, TcpClientState};
use embassy_time::{Duration, with_timeout};
use reqwless::client::HttpClient;
use reqwless::request::{Method, RequestBuilder as _};
use scoreboard_model::poll::{self, MAX_HEADER_BLOCK, PollError, Transport};
use scoreboard_wire::STRUCT_CONTENT_TYPE;
use static_cell::ConstStaticCell;

/// The crest format the panel blits directly (`display.py`'s `LogoPool`).
pub const LOGO_CONTENT_TYPE: &str = "image/x-rgb565";

/// A base URL plus the longest path the poller builds — a college-football
/// crest request, which carries the resize query string.
pub const URL_BYTES: usize = 256;

/// An `ETag`, stored **with its quotes** so it can be echoed verbatim as
/// `If-None-Match`: the backend does a strict string comparison and does not
/// recognise a stripped-quote form (`api_client.py:209-231`). The deployed
/// backend sends an 18-byte `"0b81674f50a5ebf9"`; this leaves room for a weak
/// validator and a wider hash.
pub type Etag = heapless::String<48>;

/// `main.py:453-488` and `api_client.py:32`: every request, 15 s, no
/// exceptions. Without it a wedged TCP connection stalls the poller forever —
/// the watchdog guards the render loop, not the network.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Per-connection socket buffers. One connection, because
/// [`net`](crate::net)'s socket budget reserves one slot for the poller and a
/// request is never concurrent with another.
const TCP_RX_BYTES: usize = 1536;
const TCP_TX_BYTES: usize = 512;

static TCP_STATE: ConstStaticCell<TcpClientState<1, TCP_TX_BYTES, TCP_RX_BYTES>> =
    ConstStaticCell::new(TcpClientState::new());

/// One response, borrowing the buffer it was read into.
pub struct Fetched<'buf> {
    pub status: u16,
    /// The `ETag` header, verbatim. Absent on responses that carry none.
    pub etag: Option<Etag>,
    /// Empty for a `304`, which carries no body at all.
    pub body: &'buf [u8],
}

/// The backend clock. `main.py:453-488`.
#[derive(Debug, Clone, Copy)]
pub struct BackendTime {
    /// Unix seconds, UTC.
    pub unix_seconds: u32,
    /// Seconds to add for local display. **`Some(0)` is not `None`**: UTC is a
    /// legitimate offset, and a device that has never synced omits start times
    /// rather than showing one from the wrong timezone.
    pub utc_offset_s: i32,
}

/// The client. Holds no buffer of its own — see the module docs.
pub struct ApiClient {
    stack: Stack<'static>,
    state: &'static TcpClientState<1, TCP_TX_BYTES, TCP_RX_BYTES>,
}

impl ApiClient {
    /// Panics on a second call: there is one socket, and two clients would
    /// both claim it.
    pub fn new(stack: Stack<'static>) -> ApiClient {
        ApiClient {
            stack,
            state: TCP_STATE.take(),
        }
    }

    /// A league's games list, with the conditional request `poller.py` sends on
    /// every refresh but the first.
    ///
    /// A `304` comes back with an empty body and the ETag echoed; the caller
    /// keeps its cached slate.
    pub async fn game_list<'buf>(
        &mut self,
        url: &str,
        if_none_match: Option<&str>,
        buf: &'buf mut [u8],
    ) -> Result<Fetched<'buf>, PollError> {
        let fetched = self
            .get(url, STRUCT_CONTENT_TYPE, if_none_match, buf)
            .await?;
        if fetched.status >= 400 {
            return Err(error_from_body(fetched.status, fetched.body));
        }
        Ok(fetched)
    }

    /// One game's detail, or `None` when the game left today's scoreboard
    /// between the list refresh and this fetch.
    ///
    /// `get_game_state`'s `404 → None` (`api_client.py:249-252`), and *only*
    /// for a detail: a `404` on a games list is a real error, because a
    /// configured league that does not exist is a configuration to fix.
    pub async fn game_detail<'buf>(
        &mut self,
        url: &str,
        buf: &'buf mut [u8],
    ) -> Result<Option<&'buf [u8]>, PollError> {
        let fetched = self.get(url, STRUCT_CONTENT_TYPE, None, buf).await?;
        if fetched.status == 404 {
            return Ok(None);
        }
        if fetched.status >= 400 {
            return Err(error_from_body(fetched.status, fetched.body));
        }
        Ok(Some(fetched.body))
    }

    /// A team crest as raw RGB565.
    ///
    /// `get_team_logo_raw` returned `(status, body)` and left the status to the
    /// pool, which logged a miss and cached nothing. Same here: a non-200 is
    /// `Ok(None)`, not an error, because a league with no crest for one team
    /// must not count as a failed poll.
    pub async fn team_logo<'buf>(
        &mut self,
        url: &str,
        buf: &'buf mut [u8],
    ) -> Result<Option<&'buf [u8]>, PollError> {
        let fetched = self.get(url, LOGO_CONTENT_TYPE, None, buf).await?;
        if fetched.status != 200 {
            crate::error!("logo: fetch failed, status {}", fetched.status);
            return Ok(None);
        }
        Ok(Some(fetched.body))
    }

    /// `GET /time`. JSON, not the struct format — the endpoint predates it.
    pub async fn time(&mut self, url: &str, buf: &mut [u8]) -> Result<BackendTime, PollError> {
        let fetched = self.get(url, "application/json", None, buf).await?;
        if fetched.status != 200 {
            return Err(error_from_body(fetched.status, fetched.body));
        }
        #[derive(serde::Deserialize)]
        struct TimeBody {
            timestamp: u32,
            /// Absent or `null` reads as UTC, which is `main.py`'s
            /// `data.get('utc_offset') or 0`.
            #[serde(default)]
            utc_offset: Option<i32>,
        }
        let (body, _) = serde_json_core::from_slice::<TimeBody>(fetched.body)
            .map_err(|_| PollError::Transport(Transport::Framing))?;
        Ok(BackendTime {
            unix_seconds: body.timestamp,
            utc_offset_s: body.utc_offset.unwrap_or(0),
        })
    }

    /// The one request path. Everything above is a status-code policy over it.
    async fn get<'buf>(
        &mut self,
        url: &str,
        accept: &str,
        if_none_match: Option<&str>,
        buf: &'buf mut [u8],
    ) -> Result<Fetched<'buf>, PollError> {
        let started = embassy_time::Instant::now();
        // Dropping this future on a timeout drops the connection with it, and
        // `TcpConnection::drop` closes the socket — `_with_timeout`'s
        // `session.close()`, arrived at by construction rather than by an
        // `except` arm.
        let fetched = with_timeout(REQUEST_TIMEOUT, self.fetch(url, accept, if_none_match, buf))
            .await
            .map_err(|_| PollError::Timeout)?;
        if let Ok(fetched) = &fetched {
            // `_log_api` (`api_client.py:52-56`): every request, with its status
            // and elapsed time, at DEBUG. It goes to the ring as well as to
            // defmt because that is where it went in MicroPython, and because
            // "which request was slow" is the question `/api/logs` exists to
            // answer on a device with no probe attached. `log.level` is what
            // turns it off, exactly as it was.
            // The *path*, not the URL, exactly as `_log_api` had it. The host is
            // the same on every line and the ring's message cap is 128 bytes —
            // a full crest URL spends 96 of them saying `fly.dev` again and
            // then truncates away the status, which is the half worth keeping.
            crate::debug!(
                "api: GET {} -> {} in {} ms",
                path_of(url),
                fetched.status,
                started.elapsed().as_millis()
            );
        }
        fetched
    }

    async fn fetch<'buf>(
        &mut self,
        url: &str,
        accept: &str,
        if_none_match: Option<&str>,
        buf: &'buf mut [u8],
    ) -> Result<Fetched<'buf>, PollError> {
        let tcp = TcpClient::new(self.stack, self.state);
        let dns = DnsSocket::new(self.stack);
        let mut client = HttpClient::new(&tcp, &dns);

        let mut headers: heapless::Vec<(&str, &str), 2> = heapless::Vec::new();
        let _ = headers.push(("Accept", accept));
        if let Some(etag) = if_none_match {
            let _ = headers.push(("If-None-Match", etag));
        }

        let mut request = client
            .request(Method::GET, url)
            .await
            .map_err(transport_error)?
            .headers(&headers);
        let response = request.send(buf).await.map_err(transport_error)?;

        let status = response.status.0;
        // Before `body()`, which moves the body over the header bytes this
        // scans. Case-insensitively, because the header's case is the server's
        // choice — `api_client.py:216-218` scanned the same way, for the same
        // reason, against a different HTTP library.
        let etag = response
            .headers()
            .find(|(name, _)| name.eq_ignore_ascii_case("etag"))
            .and_then(|(_, value)| core::str::from_utf8(value).ok())
            .and_then(|value| Etag::try_from(value).ok());

        // A `304` has no body, and reqwless does not know that: with no
        // `Content-Length` and no chunked encoding it would read to end of
        // connection, which on a keep-alive socket means waiting out the 15 s
        // timeout for zero bytes. `api_client.py:221-223` returned early for
        // the same reason, phrased as "without reading a body".
        if status == 304 {
            return Ok(Fetched {
                status,
                etag,
                body: &[],
            });
        }

        let body = response
            .body()
            .read_to_end()
            .await
            .map_err(transport_error)?;
        Ok(Fetched { status, etag, body })
    }
}

/// The path and query of an absolute URL, for a log line. The whole URL when it
/// has no path — a base with no trailing path is a configuration to look at, so
/// the log should show what it actually is.
fn path_of(url: &str) -> &str {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    match after_scheme.find('/') {
        Some(index) => &after_scheme[index..],
        None => url,
    }
}

/// Map reqwless's error onto the transport vocabulary the panel speaks.
fn transport_error(error: reqwless::Error) -> PollError {
    use embedded_io::ErrorKind;
    PollError::Transport(match error {
        reqwless::Error::Dns => Transport::Dns,
        reqwless::Error::InvalidUrl(_) => Transport::BadUrl,
        reqwless::Error::BufferTooSmall => Transport::TooLarge,
        reqwless::Error::Codec => Transport::Framing,
        reqwless::Error::ConnectionAborted => Transport::Io,
        // `AlreadySent` and `IncorrectBodyWritten` are misuse of the library,
        // not conditions the network produces; there is nowhere better to put
        // them than the connection bucket.
        reqwless::Error::AlreadySent | reqwless::Error::IncorrectBodyWritten => Transport::Io,
        // embassy-net answers `ConnectionReset` for a refused connect and for a
        // pool with no free socket alike. Separating them would need the
        // socket state, which the client does not have.
        reqwless::Error::Network(ErrorKind::ConnectionRefused | ErrorKind::ConnectionReset) => {
            Transport::Connect
        }
        reqwless::Error::Network(_) => Transport::Io,
    })
}

/// `_raise_api_error` (`api_client.py:59-70`): error bodies are always JSON,
/// whatever `Accept` asked for, and the `error` field is what reaches the
/// panel. A body that is not a JSON object reads as `unknown_error` rather
/// than as a parse failure — the status code is the useful half either way.
fn error_from_body(status: u16, body: &[u8]) -> PollError {
    #[derive(serde::Deserialize)]
    struct ErrorBody<'a> {
        #[serde(default, borrow)]
        error: Option<&'a str>,
    }
    let code = serde_json_core::from_slice::<ErrorBody<'_>>(body)
        .ok()
        .and_then(|(parsed, _)| parsed.error)
        .unwrap_or("");
    PollError::http(status, code)
}

/// The base URL every request is built on: the configured value with its
/// scheme downgraded and its trailing slash removed.
///
/// `config.api_url.rstrip('/').replace('https://', 'http://', 1)`, verbatim —
/// including that it only rewrites a *leading* `https://`, so a URL with the
/// string elsewhere is left alone.
pub fn base_url(configured: &str) -> heapless::String<URL_BYTES> {
    let trimmed = configured.trim_end_matches('/');
    let mut base = heapless::String::new();
    match trimmed.strip_prefix("https://") {
        Some(rest) => {
            let _ = base.push_str("http://");
            let _ = base.push_str(rest);
        }
        None => {
            let _ = base.push_str(trimmed);
        }
    }
    base
}

/// `{base}{path}`, or `None` if the result does not fit [`URL_BYTES`].
///
/// A URL that does not fit is [`Transport::BadUrl`] at the call site rather
/// than a truncated request to somewhere unintended.
pub fn url(base: &str, path: core::fmt::Arguments<'_>) -> Result<heapless::String<URL_BYTES>, PollError> {
    let mut url = heapless::String::new();
    url.push_str(base)
        .map_err(|_| PollError::Transport(Transport::BadUrl))?;
    url.write_fmt(path)
        .map_err(|_| PollError::Transport(Transport::BadUrl))?;
    Ok(url)
}

/// The receive buffer, in one place so BUDGET.md has one symbol to point at.
pub type ResponseBuffer = [u8; poll::RESPONSE_BYTES];

const _: () = assert!(MAX_HEADER_BLOCK < poll::DETAIL_BYTES);
