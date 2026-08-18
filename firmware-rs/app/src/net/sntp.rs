//! The wall clock without a backend: one SNTP exchange, one transient socket.
//!
//! `direct` builds have nowhere to send `GET /time` — that endpoint is the
//! backend's, and S3's whole point is not having one. S3-DESIGN decision 7
//! answers with SNTP over UDP, and this is the socket half of it.
//!
//! The packet codec is [`scoreboard_portal::sntp`], next to the captive-portal
//! DNS and mDNS builders, because it is the same kind of thing they are: a
//! decision about bytes that belongs where a desktop can run it (SPEC §2's
//! crate-boundary rule). Every RFC 4330 §5 check a reply has to pass, and the
//! 2036 era arithmetic, are documented and tested there. What is left here is
//! the name lookup, the datagrams and the timeout.
//!
//! [`timesync`](super::timesync) still owns what the answer *means* — the
//! anchoring against [`Instant`](embassy_time::Instant), the resync cadence,
//! and the rule that a failed sync is logged and never counted against the
//! poll's failure streak. This module answers one question, "what is the Unix
//! second", and holds no state between calls.
//!
//! # What this does not provide
//!
//! **A timezone.** NTP carries UTC and nothing else; there is no offset field
//! and there never was. The backend's `/time` carried both, which is why
//! [`timesync`](super::timesync) stores them together — and under `direct` they
//! become independent sources, because S3-DESIGN decision 7 puts the offset on
//! the browser-seeded schedule in [`crate::timezone`]. SNTP has no business
//! inventing a timezone, so nothing here touches the offset; the seam that
//! still has to close on the other side is written up on
//! [`timesync`](super::timesync)'s `direct` fetch.
//!
//! # Socket budget
//!
//! One UDP socket, held for the length of one exchange and returned by `Drop` —
//! embassy-net's `UdpSocket::drop` removes the handle from the stack's
//! `SocketSet`, so the slot is claimed for a few seconds a day rather than for
//! the life of the firmware. Against [`net`](super)'s table that is station
//! mode's eight plus one, which is the nine that table calls the working
//! ceiling and one below [`SOCKETS`](super::SOCKETS). The DNS lookup costs
//! nothing extra: the resolver's slot is the one embassy-net adds
//! unconditionally, and it is already in the table.
//!
//! The buffers are locals rather than statics for the same reason the socket is
//! transient — they are the poller task's, they exist for one call, and an
//! embassy task's frame is a static already. ~200 bytes, `direct` only.
//!
//! # The pool, and what rotation actually buys
//!
//! [`POOL_HOST`] resolves to a different volunteer server on most lookups, and
//! `smoltcp`'s `DNS_MAX_RESULT_COUNT` is **1** at its default — which this
//! build does not raise — so a lookup yields exactly one address and there is
//! no second address to fail over to inside a single sync. Rotation therefore
//! happens *across* syncs, not within one: a pool member that is down costs
//! this device one sync, and the next lookup, a day or an hour later, almost
//! certainly lands somewhere else. That is why [`ATTEMPTS`] retries the
//! datagram rather than the name — retrying the lookup would very likely return
//! the same cached address, and retrying the datagram is what actually recovers
//! the case UDP makes common and TCP hides: a single dropped packet.

use embassy_net::dns::DnsQueryType;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{IpAddress, IpEndpoint, Stack};
use embassy_rp::clocks::RoscRng;
use embassy_time::{Duration, with_timeout};
use scoreboard_model::poll::{PollError, Transport};
use scoreboard_portal::sntp::{PACKET_BYTES, PORT, Reject, reply, request};

/// The name every SNTP client should be pointing at, and the reason the docs
/// above talk about rotation: it is a DNS round-robin over volunteer servers,
/// so no single operator carries this firmware's traffic and no single outage
/// takes its clock away for good. Not a vendor pool (`*.pool.ntp.org` has
/// per-vendor zones) because this device makes one request a day and the pool's
/// own guidance reserves those for clients that would otherwise be a burden.
pub const POOL_HOST: &str = "pool.ntp.org";

/// Datagrams sent before this sync is given up on.
///
/// Three, because the failure this recovers is a *dropped packet* and a lost
/// sync is expensive: [`timesync::RETRY`](super::timesync::RETRY) is an hour,
/// and an hour with no clock is an hour of pregame cards with no start time.
/// Three datagrams two seconds apart cost six seconds in the worst case,
/// against a poll tick that already allows fifteen for one HTTP request.
const ATTEMPTS: usize = 3;

/// How long one datagram is waited on. Generous for a public pool server on a
/// domestic link, and bounded so the poll loop cannot stall behind it.
const REPLY_TIMEOUT: Duration = Duration::from_secs(2);

/// The largest reply that will be read, and **not** [`PACKET_BYTES`].
///
/// A server may append an authenticator — 4 bytes of key id plus a 16- or
/// 20-byte digest — which this firmware has no key for and ignores. It still
/// has to *fit*: embassy-net answers `RecvError::Truncated` rather than
/// truncating when a datagram is larger than the buffer offered, so receiving
/// into 48 bytes would reject every authenticated reply as malformed and the
/// symptom would be a pool member that mysteriously never works.
const MAX_REPLY_BYTES: usize = 128;

/// The Unix second, from the pool.
///
/// One name lookup, then up to [`ATTEMPTS`] datagrams on one socket. Errors
/// carry [`timesync`](super::timesync)'s vocabulary so a failure logs the same
/// way the backend path's does; the *specific* reason is logged here, where it
/// is still known.
pub async fn epoch(stack: Stack<'static>) -> Result<u32, PollError> {
    let address = resolve(stack).await?;
    let server = IpEndpoint::new(address, PORT);
    defmt::info!("sntp: {=str} resolved to {}", POOL_HOST, address);

    // Task locals. The socket borrows them and is dropped before they are, so
    // the slot is returned at the end of this call whichever way it exits —
    // including the error paths below, which is the property a long-lived
    // socket and a `?` would quietly break.
    let mut rx_meta = [PacketMetadata::EMPTY; 2];
    let mut rx_buffer = [0u8; 2 * MAX_REPLY_BYTES];
    let mut tx_meta = [PacketMetadata::EMPTY; 1];
    let mut tx_buffer = [0u8; PACKET_BYTES];
    let mut socket = UdpSocket::new(
        stack,
        &mut rx_meta,
        &mut rx_buffer,
        &mut tx_meta,
        &mut tx_buffer,
    );
    // Port 0: embassy allocates an ephemeral one. RFC 4330 §5 wants the source
    // port unpredictable, and an off-path forgery has to guess it *and* the
    // nonce to be believed.
    socket
        .bind(0)
        .map_err(|_| PollError::Transport(Transport::Connect))?;

    // Written by `await_reply` as it goes rather than returned by it, because
    // the way that future usually ends is the timeout *cancelling* it — and a
    // cancelled future returns nothing. This is how the reason survives.
    let mut last: Option<Reject> = None;
    for _ in 0..ATTEMPTS {
        // A fresh nonce per datagram, so a reply to the previous attempt
        // arriving late is rejected rather than believed. It is also the only
        // thing in the request that an off-path attacker cannot read.
        let nonce = RoscRng.next_u64();
        if let Err(error) = socket.send_to(&request(nonce), server).await {
            defmt::warn!("sntp: send to {} failed: {}", server, error);
            continue;
        }
        let exchange = await_reply(&socket, server, nonce, &mut last);
        match with_timeout(REPLY_TIMEOUT, exchange).await {
            Ok(Ok(unix_seconds)) => return Ok(unix_seconds),
            // The only `Err` is a kiss-o'-death, and asking again two seconds
            // later is precisely what it asked this client not to do.
            Ok(Err(_)) => break,
            Err(_) => defmt::warn!("sntp: no reply from {} within the timeout", server),
        }
    }

    // One ring-log line naming the reason, because the line `timesync::sync`
    // writes next can only say "bad http response" — the panel's transport
    // vocabulary has no SNTP in it. On a device with no probe attached the ring
    // is the whole story.
    match last {
        Some(reject) => crate::error!("sntp: gave up, last reply {}", reject.describe()),
        None => crate::error!("sntp: gave up, no reply from {}", POOL_HOST),
    }
    Err(PollError::Transport(match last {
        // A reply that arrived and was wrong is a framing failure; silence is
        // the connection never having worked.
        Some(_) => Transport::Framing,
        None => Transport::Connect,
    }))
}

/// One name lookup. `A` only: the stack is built without `proto-ipv6`
/// (`Cargo.toml`'s embassy-net feature list), so an `AAAA` answer would be an
/// address nothing here could route to.
async fn resolve(stack: Stack<'static>) -> Result<IpAddress, PollError> {
    let answers = stack
        .dns_query(POOL_HOST, DnsQueryType::A)
        .await
        .map_err(|error| {
            defmt::warn!("sntp: {=str} did not resolve: {}", POOL_HOST, error);
            PollError::Transport(Transport::Dns)
        })?;
    // An empty answer is a successful query for a name with no address, which
    // the pool does not do — but it is `Ok(vec![])` on the type, so it is
    // handled rather than indexed.
    answers
        .first()
        .copied()
        .ok_or(PollError::Transport(Transport::Dns))
}

/// Receive until one datagram is a clock, or until the caller's timeout drops
/// this future.
///
/// It keeps receiving past a rejection on purpose: a forged packet arriving
/// first must not be able to end the exchange, which is what returning on the
/// first bad datagram would let it do. The only `Err` it can produce is a
/// kiss-o'-death, which is the one rejection that means *stop asking* rather
/// than *try again*; every other one is recorded in `last` and the loop
/// continues, because that is the only way the reason outlives a cancellation.
async fn await_reply(
    socket: &UdpSocket<'_>,
    server: IpEndpoint,
    nonce: u64,
    last: &mut Option<Reject>,
) -> Result<u32, Reject> {
    let mut packet = [0u8; MAX_REPLY_BYTES];
    loop {
        let (length, from) = match socket.recv_from(&mut packet).await {
            Ok(received) => received,
            // `Truncated` is the only variant: a datagram larger than
            // [`MAX_REPLY_BYTES`], which is not a reply to anything sent here.
            Err(error) => {
                defmt::warn!("sntp: dropped a packet: {}", error);
                continue;
            }
        };
        // Cheap, and it is the check that makes the nonce worth having: an
        // attacker now has to forge the source as well as guess the nonce.
        if from.endpoint != server {
            defmt::warn!("sntp: ignored a packet from {}", from.endpoint);
            continue;
        }
        // Sliced to what actually arrived, never the whole buffer: a short
        // datagram leaves the bytes of the *previous* one in the tail, and
        // handing those to the codec would let two truncated packets be read
        // as one well-formed reply.
        match reply(&packet[..length], nonce) {
            Ok(unix_seconds) => return Ok(unix_seconds),
            Err(reject) => {
                defmt::warn!("sntp: rejected a reply: {=str}", reject.describe());
                *last = Some(reject);
                if reject == Reject::KissOfDeath {
                    return Err(reject);
                }
            }
        }
    }
}

/// Station mode's eight slots plus this one, from the table in
/// [`net`](super)'s module docs. A future consumer that pushes past
/// [`SOCKETS`](super::SOCKETS) fails the build here rather than failing to bind
/// once a day, at the one moment nobody is watching.
const _: () = assert!(
    super::SOCKETS >= 9,
    "station mode plus a transient sntp socket needs nine slots"
);
