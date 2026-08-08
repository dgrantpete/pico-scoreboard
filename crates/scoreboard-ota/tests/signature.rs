//! Does `tools/fwsign.py` produce what the device will accept?
//!
//! This is the only place the two halves of the signing pipeline are ever
//! compared, and the failure it exists to catch is expensive and silent: a
//! signature made under the wrong scheme is indistinguishable from a correct
//! one until a device has downloaded ~700 KB, hashed it, and refused. On a
//! deployed unit that shows up as "updates never install", with nothing in the
//! log more specific than a signature mismatch.
//!
//! The scheme is dictated by embassy-boot's ed25519 adapter, which
//! `verify_and_mark_updated` would have called:
//!
//! ```text
//! signature = Ed25519_sign(private, SHA512(image))
//! ```
//!
//! Plain Ed25519 over the 64-byte digest — not Ed25519ph, and not Ed25519 over
//! the image itself.
//!
//! What is exercised below is [`scoreboard_ota::verify`] — **the same function
//! the device calls**, not a re-statement of it. The device supplies a digest
//! that `BlockingFirmwareUpdater::hash` computed over the DFU partition; this
//! supplies one computed over a file. Everything after that is shared.
//!
//! `python tools/fwsign.py selftest` checks the same vector from the other
//! side, so a change to either half fails one of the two.

use scoreboard_ota::{SignatureError, verify};
use sha2::{Digest, Sha512};

const IMAGE: &[u8] = include_bytes!("vector/image.bin");
const PUBLIC_KEY: &[u8; 32] = include_bytes!("vector/public.bin");
const SIGNATURE_HEX: &str = include_str!("vector/signature.hex");

fn signature() -> [u8; 64] {
    hex::decode(SIGNATURE_HEX.trim())
        .expect("the vector's signature is hex")
        .try_into()
        .expect("64 bytes")
}

fn digest_of(image: &[u8]) -> [u8; 64] {
    Sha512::digest(image).into()
}

#[test]
fn the_python_signer_produces_what_the_device_verifies() {
    assert!(
        verify(&digest_of(IMAGE), PUBLIC_KEY, &signature()).is_ok(),
        "tools/fwsign.py and scoreboard_ota::verify disagree about the signing \
         scheme. Every published image would be refused by every device."
    );
}

#[test]
fn a_single_flipped_bit_anywhere_in_the_image_is_refused() {
    // Not a formality: it is the proof that the signature is bound to the image
    // and not merely to something correlated with it. First byte, middle and
    // last, because a digest fed the wrong length would still catch only one.
    for index in [0, IMAGE.len() / 2, IMAGE.len() - 1] {
        let mut tampered = IMAGE.to_vec();
        tampered[index] ^= 0x01;
        assert_eq!(
            verify(&digest_of(&tampered), PUBLIC_KEY, &signature()),
            Err(SignatureError::Mismatch),
            "a bit flipped at byte {index} still verified"
        );
    }
}

#[test]
fn a_truncated_image_is_refused() {
    // The case a manifest lying about `size` would produce: the right prefix,
    // checked against the whole image's signature. The device hashes exactly
    // `manifest.size` bytes of DFU, so this is the shape of that attack.
    assert_eq!(
        verify(&digest_of(&IMAGE[..IMAGE.len() - 1]), PUBLIC_KEY, &signature()),
        Err(SignatureError::Mismatch)
    );
}

#[test]
fn a_corrupted_signature_is_refused_rather_than_panicking() {
    let mut corrupted = signature();
    corrupted[0] ^= 0xFF;
    assert_eq!(
        verify(&digest_of(IMAGE), PUBLIC_KEY, &corrupted),
        Err(SignatureError::Mismatch)
    );
}

#[test]
fn signing_the_raw_image_is_not_the_scheme() {
    // The single most likely way to get `fwsign.py` wrong — sign the image
    // directly rather than its digest. The device hashes before verifying, so
    // a tool that signed the raw bytes would produce signatures nothing
    // accepts. If this ever starts passing, the two halves have drifted.
    let mut not_a_digest = [0u8; 64];
    not_a_digest.copy_from_slice(&IMAGE[..64]);
    assert_eq!(
        verify(&not_a_digest, PUBLIC_KEY, &signature()),
        Err(SignatureError::Mismatch)
    );
}
