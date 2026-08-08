//! The socket half of the captive DNS responder.
//!
//! The answer builder — and its tests — live in
//! [`scoreboard_portal::dns`](scoreboard_portal::dns), because a DNS message is
//! a decision about bytes and belongs where it compiles on the desktop
//! (SPEC §2). What is left here is `dns.py`'s loop, and its two load-bearing
//! rules:
//!
//! - **The task must never die.** `dns.py:19-20` says so in a comment; here it
//!   is the shape of the code — every error path logs and continues, and the
//!   signature is `-> !` so the compiler agrees. If this task ended, captive
//!   portal detection would stop while the AP stayed up and the web server
//!   stayed up, which is the worst of the three failures because it looks like
//!   nothing is wrong.
//! - **Yield after every packet.** `dns.py` calls `sleep_ms(0)` so a burst of
//!   queries cannot starve the web server sharing its event loop. embassy is
//!   the same single-threaded cooperative arrangement, and the server it must
//!   not starve is serving the page the queries exist to open.

use core::net::Ipv4Addr;

use embassy_futures::yield_now;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{IpListenEndpoint, Stack};
use scoreboard_portal::dns;
use static_cell::ConstStaticCell;

/// Payload buffers for the socket.
///
/// 1 KiB each. A probe burst is four to eight queries of ~40 bytes arriving
/// together as a phone's services wake at once, so this holds several bursts;
/// a query at the 512-byte ceiling is pathological and two still fit. Overflow
/// drops the datagram, which every resolver on earth handles by retrying.
static RX_BUFFER: ConstStaticCell<[u8; 1024]> = ConstStaticCell::new([0; 1024]);
static TX_BUFFER: ConstStaticCell<[u8; 1024]> = ConstStaticCell::new([0; 1024]);
/// One slot per queued datagram, so the payload buffer is what runs out first
/// rather than the bookkeeping.
static RX_META: ConstStaticCell<[PacketMetadata; 8]> =
    ConstStaticCell::new([PacketMetadata::EMPTY; 8]);
static TX_META: ConstStaticCell<[PacketMetadata; 8]> =
    ConstStaticCell::new([PacketMetadata::EMPTY; 8]);

/// Answer every query with `address`, forever.
#[embassy_executor::task]
pub async fn serve(stack: Stack<'static>, address: Ipv4Addr) -> ! {
    let mut socket = UdpSocket::new(
        stack,
        RX_META.take(),
        RX_BUFFER.take(),
        TX_META.take(),
        TX_BUFFER.take(),
    );
    // Port 53 on every address the interface has — `0.0.0.0:53` in `dns.py`.
    defmt::unwrap!(socket.bind(IpListenEndpoint {
        addr: None,
        port: 53
    }));
    defmt::info!("captive dns: answering every query with {}", address);

    // Task locals, not statics: they are this task's alone, and an embassy
    // task's frame is a static anyway, so the budget already counts them (they
    // are the bulk of `serve::POOL`).
    let mut query = [0u8; dns::MAX_QUERY];
    let mut response = [0u8; dns::MAX_RESPONSE];

    loop {
        match socket.recv_from(&mut query).await {
            Ok((length, from)) => {
                match dns::answer(&query[..length], address.octets(), &mut response) {
                    Some(length) => {
                        if let Err(error) = socket.send_to(&response[..length], from).await {
                            defmt::warn!("captive dns: reply to {} failed: {}", from.endpoint, error);
                        }
                    }
                    None => defmt::warn!(
                        "captive dns: dropped a malformed {}-byte query from {}",
                        length,
                        from.endpoint
                    ),
                }
            }
            // `Truncated`: longer than the receive buffer. Not something a
            // portal probe sends. Logged and dropped, like everything else.
            Err(error) => defmt::warn!("captive dns: dropped a packet: {}", error),
        }

        // `dns.py`'s `sleep_ms(0)`: hand the executor back between packets.
        yield_now().await;
    }
}
