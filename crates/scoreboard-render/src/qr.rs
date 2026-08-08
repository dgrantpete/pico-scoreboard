//! The setup screen's Wi-Fi QR code.
//!
//! # What replaced `miqro`
//!
//! The MicroPython firmware used `lib/miqro`, a first-party library whose
//! encoder ships as opaque precompiled ARM `.mpy` — Reed-Solomon, masking and
//! version selection with no source in the repo. There is nothing to port and
//! nothing to compare against bit-for-bit, so this is a replacement rather than
//! a port, and the parity claim is "it scans", not "the same modules".
//!
//! [`qrcodegen-no-heap`] is Nayuki's reference implementation with every buffer
//! moved to the caller. It passes the SPEC §10 audit outright: `#![no_std]`,
//! `#![forbid(unsafe_code)]`, **zero dependencies**, and no allocation of any
//! kind — the two working buffers are stack arrays here. The alternative was
//! porting a QR encoder by hand, which is several hundred lines of
//! well-specified but easy-to-get-subtly-wrong bit manipulation.
//!
//! [`qrcodegen-no-heap`]: https://crates.io/crates/qrcodegen-no-heap
//!
//! # Sizing
//!
//! The payload is `WIFI:T:nopass;S:<ssid>;;` — 18 bytes plus the SSID, which
//! the snapshot bounds at 40, so 58 bytes worst case. That fits version 4 at
//! medium ECC (62 bytes); [`MAX_VERSION`] is 6 (134 bytes) for headroom. At
//! version 6 the symbol is 41 modules, 49 px with the quiet zone, which leaves
//! 77 px of panel for the setup text beside it.
//!
//! Medium ECC with boosting, not low: at these payload lengths both land on the
//! same version, so the stronger level is free. The mask is chosen by the
//! standard's penalty rules.

use crate::blit::{PixelFormat, Source};
use crate::{BLACK, WHITE};
use qrcodegen_no_heap::{QrCode, QrCodeEcc, Version};

/// Largest symbol version the encoder may choose.
pub const MAX_VERSION: u8 = 6;

/// Quiet zone in modules, per the QR spec's minimum.
pub const QUIET_ZONE: i32 = 4;

/// Modules across the largest allowed symbol.
const MAX_MODULES: i32 = 4 * MAX_VERSION as i32 + 17;

/// Bitmap edge including the quiet zone.
const MAX_SIZE: i32 = MAX_MODULES + QUIET_ZONE * 2;

/// `MONO_HLSB` bytes for the largest bitmap.
const MAX_BITMAP_BYTES: usize = (MAX_SIZE as usize).div_ceil(8) * MAX_SIZE as usize;

/// Working buffer size the encoder requires, per its own documented rule.
const WORK_BYTES: usize = Version::new(MAX_VERSION).buffer_len();

/// The longest payload that can be built.
pub const PAYLOAD_MAX: usize = 64;

const PAYLOAD_PREFIX: &str = "WIFI:T:nopass;S:";
const PAYLOAD_SUFFIX: &str = ";;";

/// Build the Wi-Fi payload for an open access point.
///
/// The AP the device raises has no password, so the WPA form
/// (`WIFI:T:WPA;S:…;P:…;;`) that `state._generate_wifi_qr` also knew how to
/// build was never reachable and is not ported.
///
/// **The SSID is truncated by glyph, never by byte.** It is the one string on
/// the panel this firmware did not author, so it can be any UTF-8 an access
/// point felt like advertising; cutting it mid-sequence would produce an
/// invalid `&str`. The loop below stops before a character that would not fit
/// whole, leaving room for the suffix.
pub fn wifi_payload(ssid: &str) -> heapless::String<PAYLOAD_MAX> {
    let mut payload = heapless::String::new();
    let _ = payload.push_str(PAYLOAD_PREFIX);
    for character in ssid.chars() {
        if payload.len() + character.len_utf8() + PAYLOAD_SUFFIX.len() > PAYLOAD_MAX {
            break;
        }
        let _ = payload.push(character);
    }
    let _ = payload.push_str(PAYLOAD_SUFFIX);
    payload
}

/// A rendered QR symbol with its quiet zone, packed `MONO_HLSB` so one blit
/// puts it on the panel.
///
/// A dark module is a 1 bit and reads as palette index 1; the surrounding quiet
/// zone is left at 0 and reads as index 0. [`QrBitmap::PALETTE`] maps those to
/// black on white, which is the polarity a scanner expects.
#[derive(Debug, Clone)]
pub struct QrBitmap {
    modules: [u8; MAX_BITMAP_BYTES],
    /// Edge length including the quiet zone; 0 when there is no code.
    size: i32,
}

impl QrBitmap {
    /// White background, black modules — matching `state._qr_palette`.
    pub const PALETTE: [u16; 2] = [WHITE, BLACK];

    pub const fn empty() -> Self {
        QrBitmap {
            modules: [0; MAX_BITMAP_BYTES],
            size: 0,
        }
    }

    /// Edge length in pixels, quiet zone included. Zero when empty.
    pub const fn size(&self) -> i32 {
        self.size
    }

    pub const fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// Encode `payload`, replacing whatever was here.
    ///
    /// Returns false — and leaves the bitmap empty — when the payload will not
    /// fit [`MAX_VERSION`]. The MicroPython firmware caught the equivalent
    /// failure, logged it, and drew the setup screen without a QR; so does the
    /// caller here.
    pub fn encode(&mut self, payload: &str) -> bool {
        let mut work = [0u8; WORK_BYTES];
        let mut out = [0u8; WORK_BYTES];
        let Ok(code) = QrCode::encode_text(
            payload,
            &mut work,
            &mut out,
            QrCodeEcc::Medium,
            Version::MIN,
            Version::new(MAX_VERSION),
            None,
            true,
        ) else {
            self.size = 0;
            return false;
        };

        let size = code.size() + QUIET_ZONE * 2;
        let row_bytes = (size as usize).div_ceil(8);
        self.modules[..row_bytes * size as usize].fill(0);
        for y in 0..code.size() {
            for x in 0..code.size() {
                if code.get_module(x, y) {
                    let (px, py) = (x + QUIET_ZONE, y + QUIET_ZONE);
                    self.modules[py as usize * row_bytes + (px as usize >> 3)] |= 0x80 >> (px & 7);
                }
            }
        }
        self.size = size;
        true
    }

    /// This bitmap as a blit source. Empty when there is no code.
    pub fn source(&self) -> Source<'_> {
        Source::new(
            &self.modules,
            self.size,
            self.size,
            PixelFormat::MonoHlsb,
            Some(&Self::PALETTE),
            None,
        )
    }
}

impl Default for QrBitmap {
    fn default() -> Self {
        Self::empty()
    }
}
