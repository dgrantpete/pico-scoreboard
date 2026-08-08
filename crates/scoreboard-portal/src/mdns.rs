//! `<device_name>.local`, answered by the device itself.
//!
//! # The parity gap this closes
//!
//! MicroPython got this for free. `network.hostname()` sets the lwIP hostname,
//! and the MicroPython port compiles lwIP with `LWIP_MDNS_RESPONDER`, so a
//! browser asking for `scoreboard.local` was answered by the network stack
//! before any Python ran. embassy-net has no such thing: it sends the hostname
//! in DHCP option 12, which makes the *router* able to resolve the name for
//! clients that ask the router — and does nothing at all for a phone on a
//! network whose router does not do that, or for a laptop that goes straight to
//! multicast. `app/Cargo.toml` said as much when it declined embassy-net's
//! `mdns` feature; this module is the discovered consequence.
//!
//! Without it the settings page is reachable only by IP address, which the
//! owner has to find from the router — for a device whose whole setup story is
//! "join the AP, type your wifi password, done".
//!
//! # Why this is hand-rolled, and what was audited first
//!
//! `edge-mdns` 0.8.0 was evaluated and **rejected**. It passes SPEC §10's
//! actual test — it builds `no_std` for `thumbv8m.main-none-eabihf` with no
//! allocator, which is the enforcement mechanism that section names — so the
//! rejection is not about allocation. It is about proportion:
//!
//! - It brings **six** crates into the audit table (`edge-mdns`, `domain`,
//!   `octseq`, `jiff`, `jiff-core`, `rand_core`, plus the `domain-macros`
//!   proc-macro at build time) to answer one question about one name. `domain`
//!   is a general-purpose DNS library and `jiff` is a datetime library; neither
//!   is doing anything here that this file's 80 lines do not.
//! - Its responder loop is behind the `io` feature, which pulls `edge-nal`. The
//!   codebase has already made exactly this call once, for `edge-dhcp`:
//!   `default-features = false` to take the codec and keep the socket loop
//!   ours, because that is what keeps the never-die-on-a-bad-packet rule
//!   ([`crate::dns`]'s, `dns.py:19-20`'s) the same across every responder.
//!   Taking edge-mdns the same way leaves only its record types, which is the
//!   part that is cheapest to write and easiest to test.
//! - The wire format is the one [`crate::dns`] already parses. This module is
//!   its sibling, not a new subject.
//!
//! The audit line is in SPEC.md's Appendix A so nobody has to re-run it.
//!
//! # Multicast DNS is DNS with four differences, and all four matter
//!
//! 1. **Responses carry no question** (RFC 6762 §6). A multicast answer is
//!    broadcast to a group where most listeners did not ask anything, so
//!    echoing the question is noise — and it means the answer's name cannot be
//!    the `0xC00C` compression pointer [`crate::dns`] uses, because there is
//!    nothing at offset 12 to point at. The name is written out in full.
//! 2. **Except for legacy queries** (§6.7). A query from a source port other
//!    than 5353 came from a resolver using an ephemeral port and expecting
//!    ordinary DNS back: unicast, question echoed, transaction ID echoed. That
//!    is the shape `dig -p 5353` sends and the one this answers when the port
//!    says so.
//! 3. **The QU bit** — the top bit of a question's QCLASS — asks for a unicast
//!    reply even on port 5353. Sent by resolvers that have just booted and do
//!    not yet trust their cache. Ignoring it costs a retry and a second of
//!    latency on the very first lookup, which is the lookup a person is
//!    watching.
//! 4. **The cache-flush bit** — the top bit of an answer's CLASS — says "this
//!    record replaces anything else you have cached for this name". Correct for
//!    a name only this device owns, and what stops a stale address surviving a
//!    DHCP lease change.
//!
//! # What is deliberately not answered
//!
//! **AAAA.** [`crate::dns`] answers every query type with an A record on
//! purpose — a captive-portal probe that gets any answer at all concludes it is
//! behind a portal. Here the opposite is true: a resolver that asks for AAAA
//! and receives an A record has been handed a malformed answer, and the correct
//! response from a host with no IPv6 address is silence, after which the
//! resolver asks for A. (A strictly-correct responder would send an NSEC
//! saying "I own this name and have no AAAA"; that is BACKLOG 18's territory
//! and buys a fraction of a second.)
//!
//! **Known-answer suppression** (§7.1): a query may carry records the asker
//! already holds, and a responder should stay quiet if its own copy is not
//! materially fresher. Skipped. The cost of skipping is one redundant 66-byte
//! datagram per query for one record; the cost of implementing it is parsing
//! the answer section of every query that arrives on a busy multicast group.

use crate::hosts::MAX_DEVICE_NAME;

/// The mDNS multicast group (RFC 6762 §3).
pub const GROUP: [u8; 4] = [224, 0, 0, 251];
/// The mDNS port, for both queries and responses.
pub const PORT: u16 = 5353;

/// The suffix every mDNS name ends in.
const LOCAL: &[u8] = b"local";

/// A query at the Ethernet MTU. mDNS messages may exceed 512 bytes — a query
/// carrying known answers routinely does — so this is not [`crate::dns`]'s
/// 512-byte classic-DNS ceiling.
pub const MAX_QUERY: usize = 1500;

/// The longest response this builds.
///
/// Header 12, then the longest name (`<32-byte device name>.local.` encodes to
/// 40 bytes) or, for a legacy reply, the echoed question (44) plus a two-byte
/// pointer, then the A record's fixed 14 bytes.
pub const MAX_RESPONSE: usize = 12 + (MAX_DEVICE_NAME + 1) + (LOCAL.len() + 1) + 1 + 4 + 2 + 14;

/// The A record's TTL, in seconds. RFC 6762 §10 recommends 120 for records
/// containing a host name.
const TTL_SECONDS: u32 = 120;

/// Where a reply goes, which mDNS decides per query rather than per socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    /// To [`GROUP`]:[`PORT`] — the ordinary case.
    Multicast(usize),
    /// Back to the sender, for a legacy query or one with the QU bit set.
    Unicast(usize),
}

impl Reply {
    pub const fn len(self) -> usize {
        match self {
            Reply::Multicast(length) | Reply::Unicast(length) => length,
        }
    }

    pub const fn is_empty(self) -> bool {
        self.len() == 0
    }
}

/// Answers for one name at one address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Responder {
    /// The first label — the device name, without `.local`.
    name: heapless::String<MAX_DEVICE_NAME>,
    address: [u8; 4],
}

impl Responder {
    /// `device_name` is the bare name: `scoreboard-rs`, not
    /// `scoreboard-rs.local`. A name longer than [`MAX_DEVICE_NAME`] is
    /// truncated, which cannot happen — the same value is the AP's SSID and is
    /// bounded by that.
    pub fn new(device_name: &str, address: [u8; 4]) -> Responder {
        let mut name = heapless::String::new();
        for character in device_name.chars() {
            if name.push(character).is_err() {
                break;
            }
        }
        Responder { name, address }
    }

    pub fn address(&self) -> [u8; 4] {
        self.address
    }

    /// Build the reply to `query`, or `None` to stay silent.
    ///
    /// Silence is the common case and is not an error: most traffic on the
    /// group is other devices' business. `from_port` is the datagram's source
    /// port, which is what distinguishes a legacy query from an mDNS one.
    pub fn answer(
        &self,
        query: &[u8],
        from_port: u16,
        out: &mut [u8; MAX_RESPONSE],
    ) -> Option<Reply> {
        // Header: id, flags, and four section counts.
        let &[id_high, id_low, flags_high, _flags_low, qd_high, qd_low, ..] = query else {
            return None;
        };
        if query.len() < 12 {
            return None;
        }
        // QR set means this is somebody's response, not a question. Answering
        // one would be a broadcast storm with every other responder on the
        // group, so it is the first thing checked.
        if flags_high & 0x80 != 0 {
            return None;
        }
        let questions = u16::from_be_bytes([qd_high, qd_low]);

        // A query may ask several things at once; the answer is the same record
        // whichever of them matched, so the walk stops at the first hit.
        let mut cursor = 12;
        let mut matched = None;
        for _ in 0..questions {
            let (name_end, is_ours) = self.walk_name(query, cursor)?;
            let &[type_high, type_low, class_high, class_low] =
                query.get(name_end..name_end + 4)?
            else {
                return None;
            };
            let query_type = u16::from_be_bytes([type_high, type_low]);
            // The QU bit is the top bit of QCLASS; the class itself is the rest.
            let unicast_requested = class_high & 0x80 != 0;
            let class = u16::from_be_bytes([class_high & 0x7F, class_low]);

            // A (1) and ANY (255) only — see the module docs on AAAA. Class IN,
            // or ANY class, which is what a `dig -c ANY` sends.
            let wanted = matches!(query_type, 1 | 255) && matches!(class, 1 | 255);
            if is_ours && wanted {
                matched = Some((cursor, name_end, unicast_requested));
                break;
            }
            cursor = name_end + 4;
        }
        let (name_start, name_end, unicast_requested) = matched?;

        // §6.7: a query from any port but 5353 is a legacy resolver expecting
        // ordinary DNS — unicast, with the question and the transaction id
        // echoed back.
        let legacy = from_port != PORT;

        let mut written = 0;
        let mut put = |bytes: &[u8]| {
            out[written..written + bytes.len()].copy_from_slice(bytes);
            written += bytes.len();
        };

        if legacy {
            put(&[id_high, id_low]);
        } else {
            // §18.1: a multicast response's id is zero, whatever the query's
            // was. Listeners match on the name, not on a transaction.
            put(&[0, 0]);
        }
        // Response, authoritative. No recursion bits: mDNS has no recursion,
        // and `dns.py`'s 0x8180 would be claiming a resolver this is not.
        put(&[0x84, 0x00]);
        if legacy {
            put(&[0, 1, 0, 1, 0, 0, 0, 0]); // QD=1 AN=1
            put(&query[name_start..name_end + 4]); // the matched question
            put(&[0xC0, 0x0C]); // answer name: pointer back to it
        } else {
            put(&[0, 0, 0, 1, 0, 0, 0, 0]); // QD=0 AN=1 — §6, no question
            // Nothing to point at, so the name goes out in full. Ours rather
            // than the query's: they differ in case, and a resolver caching the
            // asker's spelling is a resolver that caches `SCOREBOARD.local`.
            put(&[self.name.len() as u8]);
            put(self.name.as_bytes());
            put(&[LOCAL.len() as u8]);
            put(LOCAL);
            put(&[0]);
        }
        put(&[0, 1]); // type A
        // Class IN with the cache-flush bit: this device is the only owner of
        // this name, so its answer replaces whatever a listener had cached.
        // Legacy responses must not set it (§6.7) — a plain DNS client has no
        // idea what the bit means and would read the class as 32769.
        put(if legacy { &[0x00, 0x01] } else { &[0x80, 0x01] });
        put(&TTL_SECONDS.to_be_bytes());
        put(&[0, 4]);
        put(&self.address);

        Some(if legacy || unicast_requested {
            Reply::Unicast(written)
        } else {
            Reply::Multicast(written)
        })
    }

    /// Walk a question's name, returning where it ends and whether it is ours.
    ///
    /// `None` for a malformed name, which the caller turns into silence.
    fn walk_name(&self, query: &[u8], start: usize) -> Option<(usize, bool)> {
        let mut cursor = start;
        let mut label = 0;
        let mut ours = true;
        loop {
            let length = *query.get(cursor)? as usize;
            cursor += 1;
            if length == 0 {
                break;
            }
            // Compression pointers are illegal in a question, and a
            // self-referential one makes this walk unbounded — the classic
            // decompression bomb, rejected here exactly as `dns.py` rejects it.
            if length & 0xC0 != 0 {
                return None;
            }
            let bytes = query.get(cursor..cursor + length)?;
            // Keep walking even once it cannot match, so `cursor` still lands
            // on the qtype and the *next* question can be examined.
            ours &= match label {
                0 => equal_ignoring_case(bytes, self.name.as_bytes()),
                1 => equal_ignoring_case(bytes, LOCAL),
                _ => false,
            };
            cursor += length;
            label += 1;
        }
        // `<name>.local.` and nothing longer or shorter.
        Some((cursor, ours && label == 2))
    }
}

/// ASCII case-insensitive comparison.
///
/// DNS names are case-insensitive (RFC 1035 §2.3.3) and mDNS resolvers really
/// do vary the case — `MyHosts` learned the same lesson about the `Host`
/// header, where a `Scoreboard.local` from a Mac was being treated as a
/// hijacked request.
fn equal_ignoring_case(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len() && left.eq_ignore_ascii_case(right)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAME: &str = "scoreboard-rs";
    const ADDRESS: [u8; 4] = [192, 168, 50, 57];

    /// Fixtures are built in a `heapless::Vec` rather than a `Vec`: the crate
    /// is `no_std` and does not link `alloc`, so the type has no name here.
    type Message = heapless::Vec<u8, 256>;

    fn responder() -> Responder {
        Responder::new(NAME, ADDRESS)
    }

    /// A query for `labels` with the given qtype, qclass and transaction id.
    fn query(labels: &[&str], qtype: u16, qclass: u16, id: u16) -> Message {
        let mut message = Message::new();
        let mut put = |bytes: &[u8]| message.extend_from_slice(bytes).unwrap();
        put(&id.to_be_bytes());
        put(&[0x00, 0x00]); // standard query
        put(&[0, 1, 0, 0, 0, 0, 0, 0]); // QD=1
        for label in labels {
            put(&[label.len() as u8]);
            put(label.as_bytes());
        }
        put(&[0]);
        put(&qtype.to_be_bytes());
        put(&qclass.to_be_bytes());
        message
    }

    fn ours(qtype: u16) -> Message {
        query(&[NAME, "local"], qtype, 1, 0)
    }

    #[test]
    fn a_multicast_a_query_is_answered_with_the_address() {
        let mut out = [0u8; MAX_RESPONSE];
        let reply = responder().answer(&ours(1), PORT, &mut out).unwrap();
        let Reply::Multicast(length) = reply else {
            panic!("an mDNS query gets a multicast reply, got {reply:?}");
        };
        let response = &out[..length];

        assert_eq!(&response[0..2], &[0, 0], "multicast responses use id zero");
        assert_eq!(&response[2..4], &[0x84, 0x00], "response, authoritative");
        assert_eq!(&response[4..12], &[0, 0, 0, 1, 0, 0, 0, 0], "no question, one answer");
        // The name, in full, because there is no question to point at.
        assert_eq!(response[12] as usize, NAME.len());
        assert_eq!(&response[13..13 + NAME.len()], NAME.as_bytes());
        let after_name = 13 + NAME.len();
        assert_eq!(response[after_name] as usize, LOCAL.len());
        assert_eq!(&response[after_name + 1..after_name + 6], LOCAL);
        assert_eq!(response[after_name + 6], 0, "root label");

        let record = &response[after_name + 7..];
        assert_eq!(&record[0..2], &[0, 1], "type A");
        assert_eq!(&record[2..4], &[0x80, 0x01], "class IN with cache-flush set");
        assert_eq!(&record[4..8], &TTL_SECONDS.to_be_bytes());
        assert_eq!(&record[8..10], &[0, 4], "rdlength");
        assert_eq!(&record[10..14], &ADDRESS);
        assert_eq!(length, after_name + 7 + 14);
    }

    #[test]
    fn a_legacy_query_gets_ordinary_dns_back_unicast() {
        let mut out = [0u8; MAX_RESPONSE];
        // Source port 51234: an ephemeral port, so a legacy resolver.
        let reply = responder()
            .answer(&query(&[NAME, "local"], 1, 1, 0xBEEF), 51234, &mut out)
            .unwrap();
        let Reply::Unicast(length) = reply else {
            panic!("a legacy query gets a unicast reply, got {reply:?}");
        };
        let response = &out[..length];

        assert_eq!(&response[0..2], &[0xBE, 0xEF], "transaction id echoed");
        assert_eq!(&response[4..12], &[0, 1, 0, 1, 0, 0, 0, 0], "question echoed");
        let question = &query(&[NAME, "local"], 1, 1, 0xBEEF)[12..];
        assert_eq!(&response[12..12 + question.len()], question);
        let answer = 12 + question.len();
        assert_eq!(&response[answer..answer + 2], &[0xC0, 0x0C], "pointer to the question");
        assert_eq!(
            &response[answer + 4..answer + 6],
            &[0x00, 0x01],
            "no cache-flush bit for a legacy client, which would read it as class 32769"
        );
    }

    #[test]
    fn the_qu_bit_asks_for_a_unicast_reply_on_the_mdns_port() {
        let mut out = [0u8; MAX_RESPONSE];
        // QCLASS 0x8001: IN with the unicast-response bit set.
        let reply = responder()
            .answer(&query(&[NAME, "local"], 1, 0x8001, 0), PORT, &mut out)
            .unwrap();
        assert!(matches!(reply, Reply::Unicast(_)), "got {reply:?}");
        // Still the multicast *shape* — no question — because it arrived on
        // 5353. Only the destination changes.
        assert_eq!(&out[4..12], &[0, 0, 0, 1, 0, 0, 0, 0]);
    }

    #[test]
    fn another_devices_name_is_ignored() {
        let mut out = [0u8; MAX_RESPONSE];
        assert!(
            responder()
                .answer(&query(&["someone-else", "local"], 1, 1, 0), PORT, &mut out)
                .is_none()
        );
    }

    #[test]
    fn the_name_matches_regardless_of_case() {
        let mut out = [0u8; MAX_RESPONSE];
        assert!(
            responder()
                .answer(&query(&["Scoreboard-RS", "LOCAL"], 1, 1, 0), PORT, &mut out)
                .is_some()
        );
    }

    #[test]
    fn the_answer_uses_our_spelling_not_the_askers() {
        let mut out = [0u8; MAX_RESPONSE];
        responder()
            .answer(&query(&["SCOREBOARD-RS", "local"], 1, 1, 0), PORT, &mut out)
            .unwrap();
        assert_eq!(
            &out[13..13 + NAME.len()],
            NAME.as_bytes(),
            "a resolver must not cache the asker's capitalisation"
        );
    }

    #[test]
    fn an_aaaa_query_gets_silence_rather_than_an_a_record() {
        // The deliberate difference from `dns::answer`, which answers every
        // type with an A record because lying is the point there.
        let mut out = [0u8; MAX_RESPONSE];
        assert!(responder().answer(&ours(28), PORT, &mut out).is_none());
    }

    #[test]
    fn an_any_query_is_answered() {
        let mut out = [0u8; MAX_RESPONSE];
        assert!(responder().answer(&ours(255), PORT, &mut out).is_some());
    }

    #[test]
    fn a_response_is_never_answered() {
        // QR set. Answering another responder's answer is how a multicast
        // group turns into a broadcast storm.
        let mut message = ours(1);
        message[2] |= 0x80;
        let mut out = [0u8; MAX_RESPONSE];
        assert!(responder().answer(&message, PORT, &mut out).is_none());
    }

    #[test]
    fn a_bare_name_without_local_is_not_ours() {
        let mut out = [0u8; MAX_RESPONSE];
        assert!(responder().answer(&query(&[NAME], 1, 1, 0), PORT, &mut out).is_none());
    }

    #[test]
    fn a_deeper_name_under_ours_is_not_ours() {
        let mut out = [0u8; MAX_RESPONSE];
        assert!(
            responder()
                .answer(&query(&["sub", NAME, "local"], 1, 1, 0), PORT, &mut out)
                .is_none()
        );
        assert!(
            responder()
                .answer(&query(&[NAME, "local", "lan"], 1, 1, 0), PORT, &mut out)
                .is_none()
        );
    }

    #[test]
    fn a_second_question_is_examined_when_the_first_does_not_match() {
        // A resolver batching lookups. The walk has to survive a name it does
        // not own and land on the next question's length byte.
        let mut message = query(&["elsewhere", "local"], 28, 1, 0);
        message[5] = 2; // QDCOUNT = 2
        message.extend_from_slice(&ours(1)[12..]).unwrap();
        let mut out = [0u8; MAX_RESPONSE];
        assert!(responder().answer(&message, PORT, &mut out).is_some());
    }

    #[test]
    fn every_truncation_is_dropped_rather_than_panicking() {
        let full = ours(1);
        let mut out = [0u8; MAX_RESPONSE];
        for cut in 0..full.len() {
            assert!(
                responder().answer(&full[..cut], PORT, &mut out).is_none(),
                "a {cut}-byte prefix should be dropped, not answered"
            );
        }
    }

    #[test]
    fn a_compression_pointer_in_the_question_does_not_loop() {
        let mut message = Message::from_slice(&ours(1)[..12]).unwrap();
        message.extend_from_slice(&[0xC0, 0x0C, 0x00, 0x01, 0x00, 0x01]).unwrap();
        let mut out = [0u8; MAX_RESPONSE];
        assert!(responder().answer(&message, PORT, &mut out).is_none());
    }

    #[test]
    fn a_label_running_past_the_packet_is_dropped() {
        let mut message = Message::from_slice(&ours(1)[..12]).unwrap();
        message.push(60).unwrap();
        message.extend_from_slice(b"short").unwrap();
        let mut out = [0u8; MAX_RESPONSE];
        assert!(responder().answer(&message, PORT, &mut out).is_none());
    }

    #[test]
    fn a_qdcount_larger_than_the_questions_present_cannot_read_past_the_packet() {
        let mut out = [0u8; MAX_RESPONSE];

        // A count that lies about questions *after* one that already matched is
        // harmless: the walk stops at the first hit and never reaches the lie.
        // Answering is the right call — the question that was asked is
        // well-formed and the asker is waiting for it.
        let mut matched_first = ours(1);
        matched_first[5] = 8;
        assert!(responder().answer(&matched_first, PORT, &mut out).is_some());

        // A count that lies about questions after one that did *not* match
        // sends the walk off the end of the datagram, which is exactly the
        // read this has to refuse.
        let mut no_match = query(&["elsewhere", "local"], 1, 1, 0);
        no_match[5] = 8;
        assert!(responder().answer(&no_match, PORT, &mut out).is_none());
    }

    #[test]
    fn the_longest_possible_name_still_fits_the_response_buffer() {
        // The bound MAX_RESPONSE is derived from: a device name at the SSID
        // limit, answered in the legacy shape, which is the longer of the two.
        let long = [b'x'; MAX_DEVICE_NAME];
        let long = core::str::from_utf8(&long).unwrap();
        let responder = Responder::new(long, ADDRESS);
        let mut out = [0u8; MAX_RESPONSE];
        let reply = responder
            .answer(&query(&[long, "local"], 1, 1, 0xBEEF), 51234, &mut out)
            .unwrap();
        assert!(reply.len() <= MAX_RESPONSE);
        // And the multicast shape, which writes the name rather than a pointer.
        let reply = responder
            .answer(&query(&[long, "local"], 1, 1, 0), PORT, &mut out)
            .unwrap();
        assert!(reply.len() <= MAX_RESPONSE);
    }
}
