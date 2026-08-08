//! Setup mode's DHCP server: hands a joining phone an address and points it at
//! us for DNS.
//!
//! MicroPython got this free — `network.WLAN(AP_IF).active(True)` starts
//! lwIP's `shared/netutils/dhcpserver.c` inside the port. embassy-net has a
//! DHCP *client* and no server, so the behaviour has to be reproduced, and
//! reproduced faithfully: the captive portal only works because a client is
//! told 192.168.4.1 is its DNS server, and a phone that gets an address but no
//! DNS pointer will sit on the setup network showing "no internet" and never
//! open a page.
//!
//! # Parity with `dhcpserver.c`
//!
//! | Behaviour | MicroPython | Here |
//! |---|---|---|
//! | Address pool | `.16` – `.23` (`DHCPS_BASE_IP`, `DHCPS_MAX_IP`) | the same |
//! | Lease | 24 h (`DEFAULT_LEASE_TIME_S`) | the same |
//! | Subnet | `255.255.255.0` | the same |
//! | Router (opt 3) | the AP's own address | the same |
//! | DNS (opt 6) | hard-coded `192.168.4.1` | the AP's own address |
//! | Reply destination | always broadcast | RFC 2131 §4.1: relay, else `ciaddr` unicast, else broadcast |
//!
//! Eight addresses is not many, and it is deliberately not more: it is what
//! today's firmware offers, a setup AP serves one phone at a time, and a ninth
//! simultaneous client getting nothing is behaviour that already ships.
//!
//! The reply-destination row is the one real difference, and it is
//! [`edge_dhcp`]'s rule rather than an invention — always-broadcast is legal
//! but noisy, and unicasting to a client that already has an address is what
//! every other server does.
//!
//! # Why edge-dhcp, and only half of it
//!
//! The crate is pulled in with `default-features = false`, which drops its
//! `io` module and with it `edge-nal` — so what links is the packet codec and
//! a `heapless::LinearMap` of leases, no allocation and no socket abstraction.
//! The loop below is this firmware's, which is what keeps the never-die rule
//! identical to [`super::captive_dns`]'s: a malformed packet is logged and
//! dropped, and the task's `-> !` says the compiler agrees it cannot end.

use core::net::Ipv4Addr;

use edge_dhcp::server::{Server, ServerOptions};
use edge_dhcp::{Options, Packet};
use embassy_futures::yield_now;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{IpEndpoint, IpListenEndpoint, Stack};
use embassy_time::Instant;
use static_cell::ConstStaticCell;

/// `DHCPS_BASE_IP` — the last octet the pool starts at.
const POOL_FIRST: u8 = 16;
/// `DHCPS_MAX_IP` — eight addresses, `.16` through `.23`.
const POOL_SIZE: u8 = 8;
/// `DEFAULT_LEASE_TIME_S`.
const LEASE_SECONDS: u32 = 24 * 60 * 60;

const SERVER_PORT: u16 = 67;
const CLIENT_PORT: u16 = 68;

/// One lease slot per poolable address, so the table can never be the thing
/// that runs out before the pool does.
const LEASES: usize = POOL_SIZE as usize;

/// A DHCP message is 236 bytes of fixed BOOTP plus options; RFC 2131 sets 576
/// as the size every implementation must accept. 1 KiB clears that with room
/// for a client that sends a long vendor-class or parameter-request list.
static RX_BUFFER: ConstStaticCell<[u8; 1024]> = ConstStaticCell::new([0; 1024]);
static TX_BUFFER: ConstStaticCell<[u8; 1024]> = ConstStaticCell::new([0; 1024]);
/// A setup AP serves one client at a time; four queued datagrams covers a
/// DISCOVER/REQUEST pair from two phones arriving together.
static RX_META: ConstStaticCell<[PacketMetadata; 4]> =
    ConstStaticCell::new([PacketMetadata::EMPTY; 4]);
static TX_META: ConstStaticCell<[PacketMetadata; 4]> =
    ConstStaticCell::new([PacketMetadata::EMPTY; 4]);

/// edge-dhcp's lease expiry is compared against a caller-supplied clock. Boot
/// time is fine: leases only have to outlive a setup session, and a device that
/// reboots has forgotten the AP anyway.
fn now_seconds() -> u64 {
    Instant::now().as_secs()
}

/// Serve DHCP on the AP interface, forever.
#[embassy_executor::task]
pub async fn serve(stack: Stack<'static>, address: Ipv4Addr) -> ! {
    let mut socket = UdpSocket::new(
        stack,
        RX_META.take(),
        RX_BUFFER.take(),
        TX_META.take(),
        TX_BUFFER.take(),
    );
    // Bound to every address, because a client with no address yet sends from
    // 0.0.0.0 to 255.255.255.255 and the datagram has to be accepted anyway.
    defmt::unwrap!(socket.bind(IpListenEndpoint {
        addr: None,
        port: SERVER_PORT
    }));

    let octets = address.octets();
    let mut server: Server<fn() -> u64, LEASES> = Server::new(now_seconds, address);
    server.range_start = Ipv4Addr::new(octets[0], octets[1], octets[2], POOL_FIRST);
    server.range_end = Ipv4Addr::new(octets[0], octets[1], octets[2], POOL_FIRST + POOL_SIZE - 1);

    // Built once and borrowed by every reply. `ServerOptions::new` fills the
    // gateway buffer with our own address (option 3, "router"); the DNS list is
    // set below, and is the option the captive portal actually turns on —
    // `dhcpserver.c` hard-codes 192.168.4.1 there for the same reason.
    let mut gateway = [address];
    let dns = [address];
    let mut options = ServerOptions::new(address, Some(&mut gateway));
    options.subnet = Some(Ipv4Addr::new(255, 255, 255, 0));
    options.dns = &dns;
    options.lease_duration_secs = LEASE_SECONDS;
    // RFC 8910's captive-portal option is deliberately not sent: it advertises
    // an RFC 8908 JSON API this firmware does not serve, and a client that
    // follows the pointer and finds HTML is worse off than one that falls back
    // to probing. BACKLOG, alongside task #10.
    options.captive_url = None;

    defmt::info!(
        "dhcp server: pool {} - {}, lease {} s, dns {}",
        server.range_start,
        server.range_end,
        LEASE_SECONDS,
        address
    );

    let mut request_buffer = [0u8; 1024];
    let mut reply_buffer = [0u8; 1024];

    loop {
        let (length, from) = match socket.recv_from(&mut request_buffer).await {
            Ok(received) => received,
            Err(error) => {
                defmt::warn!("dhcp server: dropped a packet: {}", error);
                yield_now().await;
                continue;
            }
        };

        let request = match Packet::decode(&request_buffer[..length]) {
            Ok(request) => request,
            Err(_) => {
                defmt::warn!(
                    "dhcp server: dropped a malformed {}-byte packet from {}",
                    length,
                    from.endpoint
                );
                yield_now().await;
                continue;
            }
        };

        let mut option_buffer = Options::buf();
        let Some(reply) = server.handle_request(&mut option_buffer, &options, &request) else {
            // A RELEASE or DECLINE, or a message this server has nothing to say
            // about. The lease table has already been updated.
            yield_now().await;
            continue;
        };

        let destination = reply_destination(&request);
        match reply.encode(&mut reply_buffer) {
            Ok(encoded) => {
                let length = encoded.len();
                if let Err(error) = socket.send_to(&reply_buffer[..length], destination).await {
                    defmt::warn!("dhcp server: reply to {} failed: {}", destination, error);
                }
            }
            Err(_) => defmt::warn!("dhcp server: reply would not fit its buffer, dropped"),
        }

        // Same reason as the DNS responder: a burst of clients must not starve
        // the HTTP server they are joining in order to reach.
        yield_now().await;
    }
}

/// RFC 2131 §4.1's destination rules, as every real server implements them.
///
/// The one that matters is the last: **never unicast to `yiaddr`.** The address
/// being offered is not configured on the client yet, so a unicast to it is a
/// packet nobody is listening for — the offer is lost and the client retries
/// until it gives up.
fn reply_destination(request: &Packet<'_>) -> IpEndpoint {
    if !request.giaddr.is_unspecified() {
        // Through a relay agent, which answers on the server port.
        return IpEndpoint::new(request.giaddr.into(), SERVER_PORT);
    }
    if !request.ciaddr.is_unspecified() && !request.broadcast {
        // The client already holds this address and asked not to be broadcast
        // at — a renewal.
        return IpEndpoint::new(request.ciaddr.into(), CLIENT_PORT);
    }
    // Everything else, which is every first-time join: broadcast, because the
    // client has no address to be reached at.
    IpEndpoint::new(Ipv4Addr::BROADCAST.into(), CLIENT_PORT)
}
