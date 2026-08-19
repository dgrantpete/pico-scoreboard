//! The clock, as bytes: an SNTP request and what a reply is allowed to be.
//!
//! Not the portal, and here for the reason [`conditional`](crate::conditional)
//! is: it is a decision about bytes made by code with no socket in it, and the
//! kind of decision that fails silently on a device with no serial port. Its
//! siblings [`dns`](crate::dns) and [`mdns`](crate::mdns) are the same shape —
//! a UDP payload in, a UDP payload or a rejection out, the socket left to the
//! firmware (SPEC §2's crate-boundary rule).
//!
//! # Why the firmware needs this at all
//!
//! The backend answered `GET /time` with the Unix second and a GeoIP-derived
//! UTC offset. Phase S removes the backend, so the second has to come from
//! somewhere else, and that somewhere is the NTP pool over UDP. **Only the
//! second.** NTP has never carried a timezone and does not now; the offset
//! comes from `crate::timezone`'s browser-seeded schedule on the device side,
//! and nothing in this module knows or guesses one.
//!
//! # What a client has to check, and why each one
//!
//! RFC 4330 §5 lists them, and [`reply`] makes all of them because every one
//! rejects an answer that would otherwise look completely reasonable:
//!
//! - **Mode 4.** Anything else is not a server answering a client — a request
//!   looped back, or an unrelated protocol on a reused ephemeral port.
//! - **Version 3 or 4.** NTPv3 servers are still common in the pool and answer
//!   a v4 request with a v3 reply, which is correct and not a fault.
//! - **Leap indicator ≠ 3.** Three is the alarm condition: the server's own
//!   clock is not synchronised. This is the check that matters most, because
//!   such a server sends a *plausible* timestamp it does not stand behind.
//! - **Stratum 1–15.** Zero is a kiss-o'-death, which carries a four-character
//!   complaint where the reference id goes instead of a clock; 16 and up means
//!   reachable but synchronised to nothing.
//! - **The originate timestamp equals the nonce we sent.** The reply's proof of
//!   freshness, and the reason [`request`] puts a random value in a field that
//!   is supposed to hold a transmit time — the device has no clock to put
//!   there, which is the entire point of asking.
//! - **A non-zero transmit timestamp**, which is the "I have never set my
//!   clock" value.
//!
//! # 2036, and why one subtraction handles both eras
//!
//! An NTP timestamp counts seconds from 1900-01-01 in 32 bits, so it wraps on
//! 2036-02-07 06:28:16 UTC. RFC 4330 §3 fixes the interpretation rather than
//! leaving it to the client: high bit set is era 0 (1968–2036, reckoned from
//! 1900), high bit clear is era 1 (2036–2104, reckoned from the wrap).
//! [`unix_seconds`] takes that standard reading and gets it for free —
//! `wrapping_sub` of the 1900→1970 delta in `u32` yields the correct Unix
//! second in *both* eras, because era 1's NTP seconds are already the low 32
//! bits of the count era 0 would have continued into. `u32` Unix seconds wrap
//! in 2106, past era 1's end, so the width the firmware already stores covers
//! the whole range this reading covers.

/// Every SNTP message, request and reply alike, before any optional
/// authenticator. A reply longer than this carries one that this firmware has
/// no key for; the first 48 bytes are still the message.
pub const PACKET_BYTES: usize = 48;

/// RFC 4330 §4. Unchanged since NTPv1.
pub const PORT: u16 = 123;

/// Seconds between 1900-01-01 and 1970-01-01 — 70 years and 17 leap days.
/// RFC 4330 §3's conversion, and the whole of the era handling.
const NTP_UNIX_DELTA: u32 = 2_208_988_800;

/// The version sent, and the lower of the two accepted back. See the module
/// docs on why v3 replies are legitimate.
const VERSION: u8 = 4;
const MIN_VERSION: u8 = 3;

/// Mode 3 is client, mode 4 is server (RFC 4330 §4).
const MODE_CLIENT: u8 = 3;
const MODE_SERVER: u8 = 4;

/// Leap indicator 3: the server's own clock is not synchronised.
const LEAP_ALARM: u8 = 3;

/// The highest stratum that is a usable clock; 16 and up is unsynchronised.
const MAX_STRATUM: u8 = 15;

/// Field offsets, RFC 4330 §4's header diagram top to bottom.
const STRATUM: usize = 1;
const ORIGINATE: usize = 24;
const TRANSMIT: usize = 40;

/// Both timestamp reads sit inside the header, which is what lets
/// [`timestamp`] slice without a bounds test of its own.
const _: () = assert!(ORIGINATE + 8 <= PACKET_BYTES && TRANSMIT + 8 <= PACKET_BYTES);

/// Why a datagram that arrived was not a clock.
///
/// Deliberately not the firmware's `PollError`: that vocabulary is the panel's,
/// and its transport arm would render an SNTP rejection as "bad http response".
/// [`Reject::describe`] is what the log says instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    /// Shorter than a header. Not an SNTP message at all.
    Short,
    /// Not mode 4. A request looped back, or another protocol on our port.
    NotServer,
    /// Older than NTPv3, which this client will not read.
    Version,
    /// Leap indicator 3: the server says its own clock is wrong.
    Unsynchronised,
    /// Stratum 0 — a kiss-o'-death. The pool sends these to clients it wants to
    /// back off, and the only correct response is to stop asking.
    KissOfDeath,
    /// Stratum 16+: reachable, but synchronised to nothing.
    Stratum,
    /// The originate timestamp is not the nonce this exchange sent. An off-path
    /// forgery, or a late reply to an earlier attempt.
    Originate,
    /// A transmit timestamp of zero — "I have never set my clock".
    NoTimestamp,
}

impl Reject {
    /// The rejection in the words a log line wants.
    pub const fn describe(self) -> &'static str {
        match self {
            Reject::Short => "short packet",
            Reject::NotServer => "not a server reply",
            Reject::Version => "unsupported version",
            Reject::Unsynchronised => "server clock unsynchronised",
            Reject::KissOfDeath => "kiss-o'-death",
            Reject::Stratum => "stratum out of range",
            Reject::Originate => "originate mismatch",
            Reject::NoTimestamp => "no transmit timestamp",
        }
    }
}

/// A client request carrying `nonce` as its transmit timestamp.
///
/// RFC 4330 §5: set the version and the mode, zero everything else, and put
/// your own transmit time in the last field. There is no clock to put there —
/// see the module docs — so the field carries the nonce, and the server echoes
/// it into the originate timestamp where [`reply`] checks it.
///
/// A request is almost entirely absence. A stray non-zero stratum or poll would
/// be a client describing itself as a server.
pub fn request(nonce: u64) -> [u8; PACKET_BYTES] {
    let mut packet = [0u8; PACKET_BYTES];
    packet[0] = (VERSION << 3) | MODE_CLIENT;
    packet[TRANSMIT..TRANSMIT + 8].copy_from_slice(&nonce.to_be_bytes());
    packet
}

/// The Unix second a reply carries, or why it is not one.
///
/// Every check the module docs list, in the order that rejects the cheapest way
/// first. `packet` may be longer than [`PACKET_BYTES`] — a reply with an
/// authenticator is still a reply — and only the header is read.
pub fn reply(packet: &[u8], nonce: u64) -> Result<u32, Reject> {
    // The length gate has to come first and produce the fixed-size header the
    // rest of this reads, so no later check can index off the end.
    let Some(header) = packet.first_chunk::<PACKET_BYTES>() else {
        return Err(Reject::Short);
    };

    let flags = header[0];
    if flags & 0b111 != MODE_SERVER {
        return Err(Reject::NotServer);
    }
    if (flags >> 3) & 0b111 < MIN_VERSION {
        return Err(Reject::Version);
    }
    if flags >> 6 == LEAP_ALARM {
        return Err(Reject::Unsynchronised);
    }

    let stratum = header[STRATUM];
    if stratum == 0 {
        return Err(Reject::KissOfDeath);
    }
    if stratum > MAX_STRATUM {
        return Err(Reject::Stratum);
    }

    if timestamp(header, ORIGINATE) != nonce {
        return Err(Reject::Originate);
    }
    let transmit = timestamp(header, TRANSMIT);
    if transmit == 0 {
        return Err(Reject::NoTimestamp);
    }
    Ok(unix_seconds(transmit))
}

/// The 64-bit big-endian NTP timestamp at `at`.
///
/// Total by construction: `at` is one of the two field offsets above and the
/// `const` assertion beside them proves both reads land inside the header.
fn timestamp(header: &[u8; PACKET_BYTES], at: usize) -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&header[at..at + 8]);
    u64::from_be_bytes(bytes)
}

/// An NTP timestamp as a Unix second, in either era. See the module docs.
///
/// The round-trip correction RFC 4330 §5 describes is deliberately not applied:
/// half of a domestic link's round trip to a pool server is tens of
/// milliseconds and this has one-second resolution. Rounding on the fraction's
/// high bit is worth more than the correction would be — it halves the
/// worst-case error, for one comparison.
pub fn unix_seconds(timestamp: u64) -> u32 {
    let seconds = (timestamp >> 32) as u32;
    let fraction = timestamp as u32;
    let seconds = if fraction >= 0x8000_0000 {
        seconds.wrapping_add(1)
    } else {
        seconds
    };
    seconds.wrapping_sub(NTP_UNIX_DELTA)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unix 1_000_000_000 is 2001-09-09T01:46:40Z. Its NTP second has the high
    /// bit set, which is what RFC 4330 §3 reads as era 0.
    const ERA_0_UNIX: u32 = 1_000_000_000;
    const ERA_0_NTP: u64 = ERA_0_UNIX as u64 + NTP_UNIX_DELTA as u64;

    /// NTP second 0 *after* the 2036 wrap is 2036-02-07T06:28:16Z.
    const ERA_1_UNIX: u32 = 2_085_978_496;

    /// The era-0 vector has to actually be in era 0, or the test that uses it
    /// proves nothing about the high-bit rule it is named after.
    const _: () = assert!(ERA_0_NTP & 0x8000_0000 != 0);

    /// A server reply: stratum 2, version 4, which is what most of the pool
    /// answers with.
    fn server_reply(nonce: u64, transmit: u64) -> [u8; PACKET_BYTES] {
        let mut packet = [0u8; PACKET_BYTES];
        packet[0] = (VERSION << 3) | MODE_SERVER;
        packet[STRATUM] = 2;
        packet[ORIGINATE..ORIGINATE + 8].copy_from_slice(&nonce.to_be_bytes());
        packet[TRANSMIT..TRANSMIT + 8].copy_from_slice(&transmit.to_be_bytes());
        packet
    }

    #[test]
    fn a_request_is_flags_and_a_nonce_and_nothing_else() {
        let request = request(0x0123_4567_89ab_cdef);

        assert_eq!(request.len(), PACKET_BYTES);
        assert_eq!(request[0], 0x23, "LI 0, VN 4, mode 3");
        assert_eq!(&request[TRANSMIT..], &0x0123_4567_89ab_cdef_u64.to_be_bytes());
        assert!(
            request[1..TRANSMIT].iter().all(|byte| *byte == 0),
            "a client that filled in stratum or poll would be describing itself as a server"
        );
    }

    #[test]
    fn era_0_is_the_delta_subtracted() {
        assert_eq!(unix_seconds(ERA_0_NTP << 32), ERA_0_UNIX);
    }

    #[test]
    fn era_1_needs_no_branch_of_its_own() {
        // The same wrapping subtraction, on a value whose high bit is clear.
        assert_eq!(unix_seconds(0), ERA_1_UNIX);
        assert_eq!(unix_seconds(100 << 32), ERA_1_UNIX + 100);
    }

    #[test]
    fn the_eras_meet_with_no_gap_and_no_overlap() {
        let last_of_era_0 = unix_seconds(0xffff_ffff_u64 << 32);
        assert_eq!(last_of_era_0 + 1, ERA_1_UNIX);
    }

    #[test]
    fn the_fraction_rounds_to_the_nearest_second() {
        assert_eq!(unix_seconds((ERA_0_NTP << 32) | 0x7fff_ffff), ERA_0_UNIX);
        assert_eq!(unix_seconds((ERA_0_NTP << 32) | 0x8000_0000), ERA_0_UNIX + 1);
    }

    #[test]
    fn a_well_formed_reply_round_trips() {
        let nonce = 0xdead_beef_feed_face;
        let reply = reply(&server_reply(nonce, ERA_0_NTP << 32), nonce);
        assert_eq!(reply, Ok(ERA_0_UNIX));
    }

    #[test]
    fn an_authenticator_on_the_end_is_still_a_reply() {
        let nonce = 0xdead_beef_feed_face;
        let mut long = [0u8; PACKET_BYTES + 20];
        long[..PACKET_BYTES].copy_from_slice(&server_reply(nonce, ERA_0_NTP << 32));
        assert_eq!(reply(&long, nonce), Ok(ERA_0_UNIX));
    }

    #[test]
    fn the_length_gate_fires_before_any_field_is_read() {
        // Zeroed, so it is *also* mode 0 — answering `Short` rather than
        // `NotServer` is what proves nothing indexed past the end first.
        assert_eq!(reply(&[0u8; PACKET_BYTES - 1], 0), Err(Reject::Short));
        assert_eq!(reply(&[], 0), Err(Reject::Short));
    }

    /// Each rejection from the same good reply with exactly one field spoiled,
    /// so a check that stopped working cannot hide behind another one.
    #[test]
    fn every_check_rejects_its_own_case() {
        let nonce = 0xdead_beef_feed_face;
        let good = server_reply(nonce, ERA_0_NTP << 32);

        assert_eq!(reply(&good, nonce ^ 1), Err(Reject::Originate));

        let mut not_server = good;
        not_server[0] = (VERSION << 3) | MODE_CLIENT;
        assert_eq!(reply(&not_server, nonce), Err(Reject::NotServer));

        let mut ancient = good;
        ancient[0] = (2 << 3) | MODE_SERVER;
        assert_eq!(reply(&ancient, nonce), Err(Reject::Version));

        let mut alarm = good;
        alarm[0] = (LEAP_ALARM << 6) | (VERSION << 3) | MODE_SERVER;
        assert_eq!(reply(&alarm, nonce), Err(Reject::Unsynchronised));

        let mut kiss = good;
        kiss[STRATUM] = 0;
        assert_eq!(reply(&kiss, nonce), Err(Reject::KissOfDeath));

        let mut unsynced = good;
        unsynced[STRATUM] = 16;
        assert_eq!(reply(&unsynced, nonce), Err(Reject::Stratum));

        assert_eq!(
            reply(&server_reply(nonce, 0), nonce),
            Err(Reject::NoTimestamp)
        );
    }

    #[test]
    fn a_v3_reply_is_legitimate() {
        let nonce = 0xdead_beef_feed_face;
        let mut v3 = server_reply(nonce, ERA_0_NTP << 32);
        v3[0] = (MIN_VERSION << 3) | MODE_SERVER;
        assert_eq!(reply(&v3, nonce), Ok(ERA_0_UNIX));
    }

    #[test]
    fn stratum_1_and_15_are_both_clocks() {
        let nonce = 0xdead_beef_feed_face;
        for stratum in [1, MAX_STRATUM] {
            let mut packet = server_reply(nonce, ERA_0_NTP << 32);
            packet[STRATUM] = stratum;
            assert_eq!(reply(&packet, nonce), Ok(ERA_0_UNIX), "stratum {stratum}");
        }
    }

    /// Neither degenerate datagram may be read as a clock by accident, and
    /// neither may panic.
    #[test]
    fn garbage_is_rejected() {
        assert_eq!(reply(&[0u8; PACKET_BYTES], 0), Err(Reject::NotServer));
        assert_eq!(
            reply(&[0xffu8; PACKET_BYTES], u64::MAX),
            Err(Reject::NotServer)
        );
    }

    /// A forged reply that guesses the shape but not the nonce is refused, and
    /// that is the whole value of putting a random number in the request.
    #[test]
    fn a_forgery_that_does_not_know_the_nonce_is_refused() {
        let ours = 0x0123_4567_89ab_cdef;
        let theirs = 0xffff_0000_ffff_0000;
        assert_eq!(
            reply(&server_reply(theirs, ERA_0_NTP << 32), ours),
            Err(Reject::Originate)
        );
    }
}
