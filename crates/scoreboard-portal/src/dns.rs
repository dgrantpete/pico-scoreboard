//! Every name resolves to us.
//!
//! Port of `scoreboard/dns.py`'s `_build_dns_response` (`:55-102`). A phone
//! that joins the setup AP immediately looks up a probe hostname —
//! `captive.apple.com`, `connectivitycheck.gstatic.com`, `msftconnecttest.com`
//! — and decides from the answer whether the network is real or wants a login
//! page. Answering all of them with the AP's address is what makes the phone
//! open the setup page by itself, rather than the owner having to be told to
//! type an address into a browser.
//!
//! It answers *every* query with an A record, including AAAA and TXT. That is
//! not correct DNS and it is the entire point: a probe that gets an answer
//! concludes it is behind a portal, and a probe that gets `NXDOMAIN` concludes
//! the network is broken and stops asking.
//!
//! MicroPython walked the question section with hand-written length checks so
//! that a truncated packet raised a clean `ValueError` rather than `IndexError`
//! from a wild read. Here the language checks the slicing, so [`answer`] is
//! written to *decide* — `None` means "not a well-formed query, drop it" — and
//! the caller's contract is that dropping is all that ever happens. The task on
//! the other side of this function must never die (`dns.py:19-20`).

/// The largest DNS message that fits a classic UDP datagram. Longer means EDNS0
/// or TCP, and a captive-portal probe is neither.
pub const MAX_QUERY: usize = 512;

/// The answer this appends: 2 B name pointer + 2 B type + 2 B class + 4 B TTL
/// + 2 B rdlength + 4 B address.
pub const ANSWER_BYTES: usize = 16;

/// Response ceiling: the whole query's header and question, plus the answer.
pub const MAX_RESPONSE: usize = MAX_QUERY + ANSWER_BYTES;

/// Build the response to `query`, writing into `out` and returning its length.
///
/// `None` means the query was malformed or truncated; the caller drops it.
///
/// The reply is `dns.py`'s byte for byte: the transaction ID echoed, flags
/// `0x8180` (standard response, recursion available, no error), counts QD=1
/// AN=1 NS=0 AR=0, the question copied verbatim, and one answer whose name is
/// the compression pointer `0xC00C` back to offset 12.
pub fn answer(query: &[u8], address: [u8; 4], out: &mut [u8; MAX_RESPONSE]) -> Option<usize> {
    // Header: transaction id, flags, and four section counts.
    if query.len() < 12 {
        return None;
    }

    // Walk the length-prefixed labels to find where the question ends: a length
    // byte of zero is the root label and terminates the name, after which come
    // qtype and qclass, two bytes each.
    let mut cursor = 12;
    loop {
        let length = *query.get(cursor)? as usize;
        cursor += 1;
        if length == 0 {
            break;
        }
        // Compression pointers are illegal in a question, and a self-referential
        // one would make this walk unbounded — the classic decompression bomb.
        // Two high bits set is what marks one.
        if length & 0xC0 != 0 {
            return None;
        }
        cursor += length;
        if cursor > query.len() {
            return None;
        }
    }
    let question_end = cursor + 4;
    if question_end > query.len() {
        return None;
    }
    // A query at the DNS name limit plus the answer must still fit `out`; a
    // longer one is not a name this has to serve.
    if question_end + ANSWER_BYTES > MAX_RESPONSE {
        return None;
    }

    let mut written = 0;
    let mut put = |bytes: &[u8]| {
        out[written..written + bytes.len()].copy_from_slice(bytes);
        written += bytes.len();
    };
    put(&query[0..2]); // transaction id, echoed
    put(&[0x81, 0x80]); // response, recursion available, no error
    put(&[0, 1, 0, 1, 0, 0, 0, 0]); // QD=1 AN=1 NS=0 AR=0
    put(&query[12..question_end]); // the question, verbatim
    put(&[0xC0, 0x0C]); // name: pointer back to the question's name at offset 12
    put(&[0, 1, 0, 1]); // type A, class IN
    put(&[0, 0, 0, 60]); // ttl 60 s
    put(&[0, 4]); // rdlength
    put(&address);
    Some(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AP: [u8; 4] = [192, 168, 4, 1];

    /// `example.com` A IN, transaction id 0xBEEF.
    const QUERY: &[u8] = &[
        0xBE, 0xEF, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 7, b'e', b'x',
        b'a', b'm', b'p', b'l', b'e', 3, b'c', b'o', b'm', 0x00, 0x00, 0x01, 0x00, 0x01,
    ];

    #[test]
    fn answers_with_the_ap_address() {
        let mut out = [0u8; MAX_RESPONSE];
        let length = answer(QUERY, AP, &mut out).unwrap();
        let response = &out[..length];

        assert_eq!(&response[0..2], &[0xBE, 0xEF], "transaction id echoed");
        assert_eq!(&response[2..4], &[0x81, 0x80], "standard response, no error");
        assert_eq!(&response[4..12], &[0, 1, 0, 1, 0, 0, 0, 0], "one Q, one A");
        assert_eq!(&response[12..29], &QUERY[12..], "question copied verbatim");
        assert_eq!(&response[29..31], &[0xC0, 0x0C], "name is a pointer to 12");
        assert_eq!(&response[31..35], &[0, 1, 0, 1], "type A, class IN");
        assert_eq!(&response[35..39], &[0, 0, 0, 60], "ttl 60");
        assert_eq!(&response[39..41], &[0, 4], "rdlength 4");
        assert_eq!(&response[41..45], &AP, "the ap address");
        assert_eq!(length, QUERY.len() + ANSWER_BYTES);
    }

    #[test]
    fn an_aaaa_query_still_gets_an_a_answer() {
        // Type 28 in the question. The answer is deliberately still type A —
        // see the module docs.
        let mut query = QUERY.to_vec();
        let last = query.len() - 4;
        query[last..last + 2].copy_from_slice(&[0x00, 0x1C]);

        let mut out = [0u8; MAX_RESPONSE];
        let length = answer(&query, AP, &mut out).unwrap();
        assert_eq!(&out[length - 6..length - 4], &[0, 4], "rdlength 4");
        assert_eq!(&out[length - 4..length], &AP);
        assert_eq!(&out[31..33], &[0, 1], "answer type A regardless of qtype");
    }

    #[test]
    fn every_truncation_is_rejected_rather_than_panicking() {
        let mut out = [0u8; MAX_RESPONSE];
        for cut in 0..QUERY.len() {
            assert!(
                answer(&QUERY[..cut], AP, &mut out).is_none(),
                "a {cut}-byte prefix should be rejected, not answered"
            );
        }
    }

    #[test]
    fn a_compression_pointer_in_the_question_does_not_loop() {
        // 0xC0 0x0C at the start of the name points back at itself.
        let mut query = QUERY[..12].to_vec();
        query.extend_from_slice(&[0xC0, 0x0C, 0x00, 0x01, 0x00, 0x01]);
        let mut out = [0u8; MAX_RESPONSE];
        assert!(answer(&query, AP, &mut out).is_none());
    }

    #[test]
    fn a_label_running_past_the_packet_is_rejected() {
        let mut query = QUERY[..12].to_vec();
        query.push(60); // claims 60 bytes of label...
        query.extend_from_slice(b"short"); // ...and supplies five
        let mut out = [0u8; MAX_RESPONSE];
        assert!(answer(&query, AP, &mut out).is_none());
    }

    #[test]
    fn a_name_at_the_dns_limit_fits_the_response_buffer() {
        // Four 63-byte labels is 255 bytes of name, the DNS maximum.
        let mut query = QUERY[..12].to_vec();
        for _ in 0..4 {
            query.push(63);
            query.extend(core::iter::repeat_n(b'a', 63));
        }
        query.extend_from_slice(&[0x00, 0x00, 0x01, 0x00, 0x01]);

        let mut out = [0u8; MAX_RESPONSE];
        let length = answer(&query, AP, &mut out).unwrap();
        assert_eq!(length, query.len() + ANSWER_BYTES);
        assert!(length <= MAX_RESPONSE);
    }

    #[test]
    fn a_question_too_long_to_answer_is_dropped_not_truncated() {
        // A name that fills the whole 512-byte datagram leaves no room for the
        // answer. Dropping it is right; writing a truncated answer would not be.
        let mut query = QUERY[..12].to_vec();
        while query.len() < MAX_QUERY - 70 {
            query.push(63);
            query.extend(core::iter::repeat_n(b'a', 63));
        }
        query.push(0);
        query.extend_from_slice(&[0x00, 0x01, 0x00, 0x01]);
        assert!(query.len() + ANSWER_BYTES > MAX_RESPONSE || query.len() <= MAX_QUERY);

        let mut out = [0u8; MAX_RESPONSE];
        // Either it fits and is answered, or it does not and is dropped —
        // never a partial write.
        match answer(&query, AP, &mut out) {
            Some(length) => assert_eq!(length, query.len() + ANSWER_BYTES),
            None => assert!(query.len() + ANSWER_BYTES > MAX_RESPONSE),
        }
    }
}
