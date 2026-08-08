//! A bench instrument: one plain-HTTP GET, to prove the stack moves bytes.
//!
//! Compiled only under `--features net-probe`, and never in a shipped build.
//! It answers one question that nothing else in this task can — does a TCP
//! connection to the real backend open, carry a request, and bring a response
//! back — without waiting for the client in task #11.
//!
//! **It is not the time sync.** `main.py:453-488` fetches the same endpoint,
//! parses `timestamp` and `utc_offset`, sets the RTC from the former, and is
//! careful that a `utc_offset` of `0` is a real answer and `None` is not. All
//! of that is task #11's, on top of reqwless. This reads the socket and logs
//! it.
//!
//! Plain HTTP, not TLS, deliberately: SPEC §7.4 and `api_client.py:94`. Score
//! polling is unauthenticated and a persistent TLS session would hold ~21 KB
//! of record buffers for its whole life. The device's integrity story is the
//! signed OTA image (SPEC §8), not the transport.

use embassy_net::Stack;
use embassy_net::dns::DnsQueryType;
use embassy_net::tcp::TcpSocket;
use embassy_time::{Duration, with_timeout};
// `read` is inherent on `TcpSocket`; `write_all` is the trait's.
use embedded_io_async::Write;

/// `main.py` wraps its time fetch in a 15 s `wait_for`; same budget here.
const TIMEOUT: Duration = Duration::from_secs(15);

/// Fetch `{DEV_API_URL}/time` once and log the result.
#[embassy_executor::task]
pub async fn fetch_time(stack: Stack<'static>) {
    let url = env!("DEV_API_URL");
    let Some((host, port, base_path)) = split_url(url) else {
        defmt::error!("net probe: DEV_API_URL is not a plain-http url: {=str}", url);
        return;
    };

    let addresses = match with_timeout(TIMEOUT, stack.dns_query(host, DnsQueryType::A)).await {
        Ok(Ok(addresses)) => addresses,
        Ok(Err(error)) => {
            defmt::error!("net probe: dns lookup of {=str} failed: {}", host, error);
            return;
        }
        Err(_) => {
            defmt::error!("net probe: dns lookup of {=str} timed out", host);
            return;
        }
    };
    let Some(&address) = addresses.first() else {
        defmt::error!("net probe: {=str} resolved to nothing", host);
        return;
    };
    defmt::info!("net probe: {=str} resolved to {}", host, address);

    let mut rx = [0u8; 2048];
    let mut tx = [0u8; 512];
    let mut socket = TcpSocket::new(stack, &mut rx, &mut tx);
    socket.set_timeout(Some(TIMEOUT));

    if let Err(error) = with_timeout(TIMEOUT, socket.connect((address, port))).await {
        defmt::error!("net probe: connect to {} timed out: {}", address, error);
        return;
    }
    defmt::info!("net probe: connected to {}:{}", address, port);

    // HTTP/1.1 with an explicit `Connection: close`, so the response ends at
    // EOF and this needs no chunked or content-length parsing to be a valid
    // reachability test.
    let mut request = heapless::String::<256>::new();
    if core::fmt::Write::write_fmt(
        &mut request,
        format_args!(
            "GET {base_path}/time HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\
             Accept: application/json\r\n\r\n"
        ),
    )
    .is_err()
    {
        defmt::error!("net probe: request line does not fit its buffer");
        return;
    }

    if let Err(error) = socket.write_all(request.as_bytes()).await {
        defmt::error!("net probe: write failed: {}", error);
        return;
    }

    let mut body = [0u8; 512];
    let mut filled = 0;
    loop {
        match socket.read(&mut body[filled..]).await {
            Ok(0) => break,
            Ok(read) => {
                filled += read;
                if filled == body.len() {
                    break;
                }
            }
            Err(error) => {
                defmt::error!("net probe: read failed after {} bytes: {}", filled, error);
                return;
            }
        }
    }
    socket.close();

    match core::str::from_utf8(&body[..filled]) {
        Ok(text) => defmt::info!("net probe: {} bytes back:\n{=str}", filled, text),
        Err(_) => defmt::info!("net probe: {} bytes back, not utf-8", filled),
    }
}

/// Split `http://host[:port][/path]` into its parts. `None` for anything that
/// is not plain HTTP — including an `https://` URL, which would silently not
/// work rather than loudly not work.
fn split_url(url: &str) -> Option<(&str, u16, &str)> {
    let rest = url.strip_prefix("http://")?;
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], rest[index..].trim_end_matches('/')),
        None => (rest, ""),
    };
    match authority.split_once(':') {
        Some((host, port)) => Some((host, port.parse().ok()?, path)),
        None => Some((authority, 80, path)),
    }
}
