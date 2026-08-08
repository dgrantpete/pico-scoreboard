//! What `GET /fw/manifest` says, parsed.
//!
//! ```json
//! { "version": "2026.08.08-a1b2c3d", "sha256": "<64 hex>", "size": 712345,
//!   "signature": "<128 hex>", "channel": "stable" }
//! ```
//!
//! # Why this is not `/app/manifest`
//!
//! `firmware/src/ota.py` polls `/app/manifest`, whose body is `{"sha256":
//! ..., "size": ...}` and whose bytes are a MicroPython ROMFS image. That
//! contract is, in its own words, "spoken by every device FOREVER once
//! flashed", and gift units in the field will keep speaking it long after the
//! Rust firmware ships. So the Rust fleet gets a **different noun**, not a
//! version of the same one: `/fw/*` serves a signed whole-flash image for the
//! active partition, `/app/*` serves a ROMFS, and the day the last gift unit
//! migrates is the day `/app/*` is deleted — with nothing to rename.
//!
//! A `/v2/app/manifest` would have invited the opposite: eventually somebody
//! repoints `/app` at the newer thing, and every gift unit downloads a Rust
//! image into its ROMFS partition.
//!
//! # Three fields the MicroPython manifest did not need
//!
//! - **`version`** — the identity. MicroPython used the image's own sha256,
//!   which it could compute because it stored the image in a file. A running
//!   Rust image cannot hash itself without knowing exactly which bytes of its
//!   partition are "it", so identity is stamped in at build time instead
//!   (`app/build.rs`) and the hash goes back to being what it should be: an
//!   integrity check, not a name.
//! - **`signature`** — 64 bytes of detached ed25519 over `SHA-512(image)`.
//!   This is the whole trust root; see [`crate::decide`]'s docs for why the
//!   transport is not.
//! - **`channel`** — echoed back so the device can check it got what it asked
//!   for. A backend that ignored `?channel=dev` would otherwise roll a
//!   staging unit onto the stable image, silently and one poll interval later.

use heapless::String;
use serde::Deserialize;

/// Version strings are `<date>-<git short sha>`; 32 leaves room to grow one
/// without this becoming the limit that breaks.
pub const MAX_VERSION: usize = 32;

pub type Version = String<MAX_VERSION>;

/// Which artifact a device asks for.
///
/// SPEC §8's "the `dev_marker` concept survives as a config flag that pins the
/// device to the staging manifest channel". `ota.channel` in the device
/// configuration selects it; the backend serves a separate artifact per
/// channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Dev,
}

impl Channel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Dev => "dev",
        }
    }

    /// Parse a configured value. Anything unrecognised is [`Channel::Stable`].
    ///
    /// Deliberately lenient where the *backend* is strict (an unknown
    /// `?channel=` is a `400` there). The asymmetry is on purpose: a typo in a
    /// hand-edited configuration must leave the device on the conservative
    /// artifact rather than refusing to update at all, whereas a typo in a
    /// request the firmware built is a bug that should be loud.
    pub fn from_config(value: &str) -> Channel {
        match value {
            "dev" => Channel::Dev,
            _ => Channel::Stable,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub version: Version,
    /// SHA-256 of the image bytes. Checked against what actually arrived over
    /// the wire — see [`crate::decide`]'s docs for why this is kept even
    /// though the signature subsumes it.
    pub sha256: [u8; 32],
    pub size: u32,
    /// Detached ed25519 over `SHA-512(image)`.
    pub signature: [u8; 64],
    pub channel: Channel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestError {
    /// Not JSON, or a field is missing or the wrong type.
    Malformed,
    /// A field was longer than its bound.
    TooLong,
    /// `sha256` or `signature` was not the right number of hex digits.
    BadHex,
    /// The backend answered with a channel other than the one asked for.
    WrongChannel,
    /// `size` is zero, or larger than the partition can hold.
    BadSize,
}

impl ManifestError {
    pub const fn as_str(self) -> &'static str {
        match self {
            ManifestError::Malformed => "manifest did not parse",
            ManifestError::TooLong => "manifest field too long",
            ManifestError::BadHex => "manifest hash or signature malformed",
            ManifestError::WrongChannel => "backend served the wrong channel",
            ManifestError::BadSize => "manifest size is not installable",
        }
    }
}

/// The wire shape, borrowed out of the response buffer.
///
/// `&str` rather than owned strings so nothing is copied twice: serde-json-core
/// can only borrow from JSON with no escape sequences, which is exactly right
/// here — a version or a hex digest containing a backslash is not something
/// this backend produces, and failing to parse it is the correct answer.
#[derive(Deserialize)]
struct Wire<'a> {
    #[serde(borrow)]
    version: &'a str,
    #[serde(borrow)]
    sha256: &'a str,
    size: u32,
    #[serde(borrow)]
    signature: &'a str,
    #[serde(borrow)]
    channel: &'a str,
}

/// Parse a manifest body.
///
/// `asked_for` is the channel the request named, and `max_bytes` is what the
/// active partition can hold — both are checked here rather than at the call
/// site so that a `Manifest` value is, by construction, one this device could
/// install.
pub fn parse(body: &[u8], asked_for: Channel, max_bytes: u32) -> Result<Manifest, ManifestError> {
    let (wire, _) =
        serde_json_core::from_slice::<Wire<'_>>(body).map_err(|_| ManifestError::Malformed)?;

    let version = Version::try_from(wire.version).map_err(|_| ManifestError::TooLong)?;
    if version.is_empty() {
        return Err(ManifestError::Malformed);
    }

    let mut sha256 = [0u8; 32];
    unhex(wire.sha256, &mut sha256)?;
    let mut signature = [0u8; 64];
    unhex(wire.signature, &mut signature)?;

    // The channel the backend *says* it served, compared to what was asked. See
    // the module docs: a backend that ignores the query parameter is a silent
    // downgrade for every device pinned to `dev`. Compared as a string rather
    // than through `Channel::from_config`, which is lenient by design and would
    // read an unrecognised `"nightly"` as stable.
    if wire.channel != asked_for.as_str() {
        return Err(ManifestError::WrongChannel);
    }

    // Zero is not an image, and one that cannot fit the active partition after
    // the swap must be refused *now* rather than after several minutes of
    // download — the device has no way to shrink it later.
    if wire.size == 0 || wire.size > max_bytes {
        return Err(ManifestError::BadSize);
    }

    Ok(Manifest {
        version,
        sha256,
        size: wire.size,
        signature,
        channel: asked_for,
    })
}

/// Decode exactly `out.len() * 2` lowercase hex digits.
///
/// Lowercase only, and length-exact: both are the publishing tool's output, and
/// accepting anything looser would mean the one place that compares a digest is
/// also a place that normalises one.
fn unhex(text: &str, out: &mut [u8]) -> Result<(), ManifestError> {
    let digits = text.as_bytes();
    if digits.len() != out.len() * 2 {
        return Err(ManifestError::BadHex);
    }
    for (byte, pair) in out.iter_mut().zip(digits.chunks_exact(2)) {
        let high = nibble(pair[0]).ok_or(ManifestError::BadHex)?;
        let low = nibble(pair[1]).ok_or(ManifestError::BadHex)?;
        *byte = (high << 4) | low;
    }
    Ok(())
}

fn nibble(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX: u32 = 1536 * 1024;

    fn body(version: &str, size: u32, channel: &str) -> Vec<u8> {
        format!(
            r#"{{"version":"{version}","sha256":"{}","size":{size},"signature":"{}","channel":"{channel}"}}"#,
            "ab".repeat(32),
            "cd".repeat(64)
        )
        .into_bytes()
    }

    #[test]
    fn a_well_formed_manifest_parses_into_bounded_fields() {
        let parsed = parse(
            &body("2026.08.08-a1b2c3d", 712_345, "stable"),
            Channel::Stable,
            MAX,
        )
        .unwrap();
        assert_eq!(parsed.version.as_str(), "2026.08.08-a1b2c3d");
        assert_eq!(parsed.size, 712_345);
        assert_eq!(parsed.sha256, [0xAB; 32]);
        assert_eq!(parsed.signature, [0xCD; 64]);
        assert_eq!(parsed.channel, Channel::Stable);
    }

    #[test]
    fn the_dev_channel_round_trips() {
        let parsed = parse(&body("2026.08.08-a1b2c3d", 1024, "dev"), Channel::Dev, MAX).unwrap();
        assert_eq!(parsed.channel, Channel::Dev);
    }

    #[test]
    fn a_backend_that_ignored_the_channel_is_refused() {
        // Asked for dev, was served stable. Installing it would roll a staging
        // unit onto the stable image.
        assert_eq!(
            parse(&body("v", 1024, "stable"), Channel::Dev, MAX),
            Err(ManifestError::WrongChannel)
        );
        assert_eq!(
            parse(&body("v", 1024, "dev"), Channel::Stable, MAX),
            Err(ManifestError::WrongChannel)
        );
    }

    #[test]
    fn an_unknown_channel_name_is_refused_rather_than_read_as_stable() {
        // `Channel::from_config` is lenient, so this is the check that stops a
        // backend answering `"channel":"nightly"` from looking like stable.
        assert_eq!(
            parse(&body("v", 1024, "nightly"), Channel::Stable, MAX),
            Err(ManifestError::WrongChannel)
        );
    }

    #[test]
    fn a_size_that_cannot_fit_the_partition_is_refused_before_the_download() {
        assert_eq!(
            parse(&body("v", MAX + 1, "stable"), Channel::Stable, MAX),
            Err(ManifestError::BadSize)
        );
        assert_eq!(
            parse(&body("v", 0, "stable"), Channel::Stable, MAX),
            Err(ManifestError::BadSize)
        );
        // Exactly the partition still fits.
        assert!(parse(&body("v", MAX, "stable"), Channel::Stable, MAX).is_ok());
    }

    #[test]
    fn hex_fields_are_length_exact_and_lowercase() {
        let short = br#"{"version":"v","sha256":"ab","size":8,"signature":"cd","channel":"stable"}"#;
        assert_eq!(parse(short, Channel::Stable, MAX), Err(ManifestError::BadHex));

        let upper = format!(
            r#"{{"version":"v","sha256":"{}","size":8,"signature":"{}","channel":"stable"}}"#,
            "AB".repeat(32),
            "cd".repeat(64)
        );
        assert_eq!(
            parse(upper.as_bytes(), Channel::Stable, MAX),
            Err(ManifestError::BadHex)
        );

        let nonhex = format!(
            r#"{{"version":"v","sha256":"{}","size":8,"signature":"{}","channel":"stable"}}"#,
            "zz".repeat(32),
            "cd".repeat(64)
        );
        assert_eq!(
            parse(nonhex.as_bytes(), Channel::Stable, MAX),
            Err(ManifestError::BadHex)
        );
    }

    #[test]
    fn a_version_past_the_bound_is_refused_rather_than_truncated() {
        let long = "x".repeat(MAX_VERSION + 1);
        assert_eq!(
            parse(&body(&long, 1024, "stable"), Channel::Stable, MAX),
            Err(ManifestError::TooLong)
        );
        // A version at exactly the bound is fine.
        let exact = "x".repeat(MAX_VERSION);
        assert!(parse(&body(&exact, 1024, "stable"), Channel::Stable, MAX).is_ok());
    }

    #[test]
    fn an_empty_version_is_not_an_identity() {
        assert_eq!(
            parse(&body("", 1024, "stable"), Channel::Stable, MAX),
            Err(ManifestError::Malformed)
        );
    }

    #[test]
    fn every_truncation_is_rejected_rather_than_panicking() {
        let full = body("2026.08.08-a1b2c3d", 712_345, "stable");
        for cut in 0..full.len() {
            let _ = parse(&full[..cut], Channel::Stable, MAX);
        }
    }

    #[test]
    fn a_body_that_is_not_json_at_all_is_malformed() {
        assert_eq!(
            parse(b"<html>404</html>", Channel::Stable, MAX),
            Err(ManifestError::Malformed)
        );
        assert_eq!(parse(b"", Channel::Stable, MAX), Err(ManifestError::Malformed));
    }
}
