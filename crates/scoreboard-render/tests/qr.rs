//! The setup QR: payload construction, and the encoded symbol against an
//! independent implementation.

mod goldens;

use goldens::{QR_BY_MASK, QR_PAYLOAD, QR_SIZE, QR_VERSION};
use scoreboard_render::qr::{self, QUIET_ZONE, QrBitmap};

fn module(bitmap: &QrBitmap, x: i32, y: i32) -> bool {
    let source = bitmap.source();
    let row_bytes = source.format.row_bytes(source.stride as usize);
    let byte = source.data[y as usize * row_bytes + (x as usize >> 3)];
    byte & (0x80 >> (x & 7)) != 0
}

#[test]
fn the_payload_is_the_open_network_form() {
    assert_eq!(qr::wifi_payload("pico-scoreboard"), QR_PAYLOAD);
    assert_eq!(qr::wifi_payload(""), "WIFI:T:nopass;S:;;");
}

#[test]
fn a_long_ssid_truncates_at_a_character_boundary() {
    // An SSID is the one string on the panel this firmware did not author, so it
    // can be any UTF-8 an access point advertises. Cutting one mid-sequence
    // would not be a `&str` at all.
    let ssid = "ネットワーク".repeat(8);
    let payload = qr::wifi_payload(&ssid);
    assert!(payload.len() <= qr::PAYLOAD_MAX);
    assert!(payload.starts_with("WIFI:T:nopass;S:"));
    assert!(payload.ends_with(";;"), "the suffix always survives");
    // Round-tripping through &str is the check: an invalid cut would not have
    // compiled into the String in the first place, so assert the visible
    // consequence — whole characters only.
    let ssid_part = payload
        .trim_start_matches("WIFI:T:nopass;S:")
        .trim_end_matches(";;");
    assert!(ssid.starts_with(ssid_part));
    assert!(ssid_part.chars().count() * 3 == ssid_part.len());
}

#[test]
fn the_encoded_symbol_matches_the_reference_implementation() {
    // The goldens come from `segno`, encoding the same payload at the same ECC
    // level in byte mode, once per mask pattern. Matching one of them means the
    // codewords, the Reed-Solomon blocks, the interleaving, the module
    // placement, the mask XOR and the format and version bits all agree; which
    // mask an encoder picks is a free choice the spec's penalty rules leave open
    // and implementations genuinely differ on.
    let mut bitmap = QrBitmap::empty();
    assert!(bitmap.encode(QR_PAYLOAD));
    assert_eq!(
        bitmap.size(),
        QR_SIZE + QUIET_ZONE * 2,
        "expected a version {QR_VERSION} symbol plus its quiet zone"
    );

    let matches: Vec<usize> = QR_BY_MASK
        .iter()
        .enumerate()
        .filter(|(_, golden)| {
            golden.iter().enumerate().all(|(y, row)| {
                row.iter().enumerate().all(|(x, dark)| {
                    module(&bitmap, x as i32 + QUIET_ZONE, y as i32 + QUIET_ZONE) == *dark
                })
            })
        })
        .map(|(mask, _)| mask)
        .collect();

    // Exactly one: zero would mean the payload encoded differently (a raised ECC
    // level from boosting, say, or a different segmentation), and more than one
    // would mean the goldens are not distinguishing masks at all.
    assert_eq!(
        matches.len(),
        1,
        "expected the symbol to match exactly one mask, matched {matches:?}"
    );
}

#[test]
fn the_quiet_zone_is_light_all_the_way_round() {
    let mut bitmap = QrBitmap::empty();
    assert!(bitmap.encode(QR_PAYLOAD));
    let size = bitmap.size();
    for offset in 0..size {
        for band in 0..QUIET_ZONE {
            assert!(!module(&bitmap, offset, band), "top band");
            assert!(!module(&bitmap, offset, size - 1 - band), "bottom band");
            assert!(!module(&bitmap, band, offset), "left band");
            assert!(!module(&bitmap, size - 1 - band, offset), "right band");
        }
    }
}

#[test]
fn the_finder_patterns_are_where_a_scanner_looks() {
    let mut bitmap = QrBitmap::empty();
    assert!(bitmap.encode(QR_PAYLOAD));
    let last = bitmap.size() - 1 - QUIET_ZONE;
    for (corner_x, corner_y) in [
        (QUIET_ZONE, QUIET_ZONE),
        (last - 6, QUIET_ZONE),
        (QUIET_ZONE, last - 6),
    ] {
        // 7x7 finder: dark ring, light ring, 3x3 dark core.
        assert!(module(&bitmap, corner_x, corner_y));
        assert!(!module(&bitmap, corner_x + 1, corner_y + 1));
        assert!(module(&bitmap, corner_x + 3, corner_y + 3));
    }
}

#[test]
fn the_worst_case_ssid_still_fits() {
    // The snapshot bounds an SSID at 40 bytes, so 58 bytes of payload is the
    // most the encoder can ever be handed. If this stops fitting, MAX_VERSION
    // is too small and the setup screen would silently lose its QR.
    let ssid = "W".repeat(40);
    let mut bitmap = QrBitmap::empty();
    assert!(bitmap.encode(&qr::wifi_payload(&ssid)));
    assert!(
        bitmap.size() <= 64,
        "the symbol must fit the panel's 64 rows"
    );
}

#[test]
fn an_empty_bitmap_reports_itself_empty() {
    let bitmap = QrBitmap::empty();
    assert!(bitmap.is_empty());
    assert_eq!(bitmap.size(), 0);
}
