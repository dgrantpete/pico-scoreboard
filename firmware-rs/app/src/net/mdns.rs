//! The socket half of the mDNS responder.
//!
//! The answer builder — and its tests — live in
//! [`scoreboard_portal::mdns`], because a DNS message is a decision about bytes
//! and belongs where it compiles on the desktop (SPEC §2). What is left here is
//! the multicast socket, and it keeps [`captive_dns`](super::captive_dns)'s two
//! rules for the same reasons:
//!
//! - **The task must never die.** Every error path logs and continues, and the
//!   signature is `-> !` so the compiler agrees.
//! - **Yield after every packet.** The mDNS group is chattier than a captive
//!   portal's — every Apple device on the network announces itself on it — and
//!   the server this must not starve is the one serving the page people reach
//!   *by the name this resolves*.
//!
//! # Why it runs in both modes
//!
//! `captive_dns` is setup-mode only; this is not. In station mode it is the
//! whole point: `scoreboard.local` in a browser on the house wifi. In AP
//! mode it is what lets a phone that has joined the setup network reach the
//! page by name as well as by address — and it costs one socket that setup mode
//! has spare.
//!
//! # Joining the group
//!
//! smoltcp needs to be told to accept traffic for `224.0.0.251`, and
//! embassy-net exposes that as [`Stack::join_multicast_group`]. Without it the
//! socket binds and receives nothing at all, which is a failure that looks
//! exactly like "no queries are arriving".
//!
//! # Staying joined, and announcing — BACKLOG 90
//!
//! Answering queries is necessary but was never sufficient. Drill day
//! (2026-08-16) recorded resolution dying **minutes after boot** on Windows
//! and iOS while the responder answered every query form correctly; the
//! 2026-08-22 session pinned the mechanism on live rx logging. The house AP
//! snoops multicast memberships and ages them out, and the LAN has no IGMP
//! querier to refresh them — so a few minutes after the boot-time join, the
//! switch simply stops delivering group traffic to the station and the
//! responder goes deaf mid-conversation. smoltcp answers IGMP queries fine
//! (`ToSpecificQuery` in its multicast state machine); nothing ever asks.
//!
//! Two periodic acts, every [`KEEPALIVE`], both aimed at that network:
//!
//! * **Leave and re-join the group.** A fresh join emits a fresh IGMPv2
//!   membership report — the packet a querier would have solicited — and the
//!   snooping table starts its clock over.
//! * **Announce, unsolicited** ([`Responder::announcement`], RFC 6762 §8.3).
//!   The cadence is half the record's TTL, so every listener whose *own*
//!   membership is alive holds our A record in cache continuously and never
//!   needs its query to reach us at all. On this network that is the leg that
//!   actually resolves names when the query leg is down.

use core::net::Ipv4Addr;

use embassy_futures::select::{Either, select};
use embassy_futures::yield_now;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{IpAddress, IpEndpoint, IpListenEndpoint, Stack};
use embassy_time::{Duration, Ticker, Timer};
use scoreboard_portal::mdns::{self, Reply, Responder};
use static_cell::ConstStaticCell;

/// How often to refresh the group membership and re-announce.
///
/// Half of the responder's record TTL (120 s), so a cached record is renewed
/// well before it can lapse; and comfortably inside every snooping-table
/// timeout observed in the wild (they run minutes).
const KEEPALIVE: Duration = Duration::from_secs(60);

/// Payload buffers.
///
/// 1 KiB each, and sized by [`mdns::MAX_QUERY`] rather than by the classic
/// 512-byte DNS limit: an mDNS query carrying known answers routinely exceeds
/// it. Overflow drops the datagram, which every resolver retries.
static RX_BUFFER: ConstStaticCell<[u8; 1536]> = ConstStaticCell::new([0; 1536]);
static TX_BUFFER: ConstStaticCell<[u8; 512]> = ConstStaticCell::new([0; 512]);
static RX_META: ConstStaticCell<[PacketMetadata; 8]> =
    ConstStaticCell::new([PacketMetadata::EMPTY; 8]);
static TX_META: ConstStaticCell<[PacketMetadata; 4]> =
    ConstStaticCell::new([PacketMetadata::EMPTY; 4]);

/// Answer `<device_name>.local` for as long as the device is up.
#[embassy_executor::task]
pub async fn serve(stack: Stack<'static>, responder: Responder) -> ! {
    let mut socket = UdpSocket::new(
        stack,
        RX_META.take(),
        RX_BUFFER.take(),
        TX_META.take(),
        TX_BUFFER.take(),
    );
    // Every address the interface has, so the same socket serves the station
    // address and the AP's.
    defmt::unwrap!(socket.bind(IpListenEndpoint {
        addr: None,
        port: mdns::PORT,
    }));

    let group = Ipv4Addr::from(mdns::GROUP);
    match stack.join_multicast_group(IpAddress::Ipv4(group)) {
        Ok(()) => defmt::info!("mdns: joined {}, answering for this device's name", group),
        // Not fatal, and not a reason to end the task: unicast queries — the
        // `dig -p 5353` shape, and what some resolvers fall back to — still
        // arrive on the bound port. Multicast discovery is what stops working,
        // and saying so is the difference between diagnosing that in a minute
        // and in an evening.
        Err(error) => defmt::error!(
            "mdns: could not join {}: {}; only unicast queries will be answered",
            group,
            error
        ),
    }

    let mut query = [0u8; mdns::MAX_QUERY];
    let mut response = [0u8; mdns::MAX_RESPONSE];
    let multicast = IpEndpoint::new(IpAddress::Ipv4(group), mdns::PORT);

    // §8.3 wants the boot announcement made more than once — a listener that
    // missed the first (its own membership mid-refresh, a lossy 2.4 GHz
    // moment) catches the second, and after that the keepalive's cadence
    // carries it.
    announce(&socket, &responder, &mut response, multicast).await;
    Timer::after_secs(1).await;
    announce(&socket, &responder, &mut response, multicast).await;

    let mut keepalive = Ticker::every(KEEPALIVE);
    loop {
        match select(socket.recv_from(&mut query), keepalive.next()).await {
            Either::First(Ok((length, from))) => {
                // The source *port* is what separates a modern resolver from a
                // legacy one, so it is passed through rather than discarded.
                let reply = responder.answer(&query[..length], from.endpoint.port, &mut response);
                if let Some(reply) = reply {
                    let target = match reply {
                        Reply::Multicast(_) => multicast,
                        Reply::Unicast(_) => from.endpoint,
                    };
                    if let Err(error) = socket.send_to(&response[..reply.len()], target).await {
                        defmt::warn!("mdns: reply to {} failed: {}", target, error);
                    }
                }
            }
            Either::First(Err(error)) => defmt::warn!("mdns: dropped a packet: {}", error),
            Either::Second(()) => {
                // Leave-then-join rather than join alone: joining a group the
                // stack believes it is already in is a no-op, and the whole
                // point is the fresh IGMP report on the wire.
                let _ = stack.leave_multicast_group(IpAddress::Ipv4(group));
                if let Err(error) = stack.join_multicast_group(IpAddress::Ipv4(group)) {
                    defmt::warn!("mdns: keepalive re-join failed: {}", error);
                }
                announce(&socket, &responder, &mut response, multicast).await;
            }
        }

        // As in `captive_dns`: hand the executor back between packets so a
        // burst on a busy group cannot starve the web server.
        yield_now().await;
    }
}

/// Multicast one unsolicited announcement of our name.
async fn announce(
    socket: &UdpSocket<'_>,
    responder: &Responder,
    response: &mut [u8; mdns::MAX_RESPONSE],
    multicast: IpEndpoint,
) {
    let length = responder.announcement(response);
    if let Err(error) = socket.send_to(&response[..length], multicast).await {
        defmt::warn!("mdns: announcement failed: {}", error);
    }
}
