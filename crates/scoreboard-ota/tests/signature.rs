//! Does `tools/fwsign.py` produce what the device will accept?
//!
//! This is the only place the two halves of the signing pipeline are ever
//! compared, and the failure it exists to catch is expensive and silent: a
//! signature made under the wrong scheme is indistinguishable from a correct
//! one until a device has downloaded ~700 KB, spent seconds hashing it, and
//! refused. On a deployed unit that shows up as "updates never install", with
//! nothing in the log more specific than `Signature`.
//!
//! The scheme is dictated by embassy-boot's ed25519 adapter, which
//! `BlockingFirmwareUpdater::verify_and_mark_updated` calls:
//!
//! ```text
//! signature = Ed25519_sign(private, SHA512(image))
//! ```
//!
//! Plain Ed25519 over the 64-byte digest — not Ed25519ph, and not Ed25519 over
//! the image itself. The verification below is written to be that call and
//! nothing more: `VerifyingKey::verify` over the digest, exactly as the adapter
//! performs it, against a vector `tools/fwsign.py` produced.
//!
//! `python tools/fwsign.py selftest` checks the same vector from the other
//! side, so a change to either half fails one of the two.

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha512};

const IMAGE: &[u8] = include_bytes!("vector/image.bin");
const PUBLIC: &[u8] = include_bytes!("vector/public.bin");
const SIGNATURE_HEX: &str = include_str!("vector/signature.hex");

fn key() -> VerifyingKey {
    VerifyingKey::from_bytes(PUBLIC.try_into().expect("32-byte public key"))
        .expect("the vector's public key is a valid ed25519 point")
}

fn signature() -> Signature {
    let bytes = hex::decode(SIGNATURE_HEX.trim()).expect("the vector's signature is hex");
    Signature::from_bytes(&bytes.try_into().expect("64-byte signature"))
}

/// The adapter's operation, spelled out.
fn verify(image: &[u8], signature: &Signature) -> bool {
    key().verify(&Sha512::digest(image), signature).is_ok()
}

#[test]
fn the_python_signer_produces_what_embassy_boot_verifies() {
    assert!(
        verify(IMAGE, &signature()),
        "tools/fwsign.py and embassy-boot's ed25519 adapter disagree about the \
         signing scheme. Every published image would be refused by every device."
    );
}

#[test]
fn a_single_flipped_bit_anywhere_in_the_image_is_refused() {
    // Not a formality: it is the proof that the signature is bound to the image
    // and not merely to something correlated with it. First byte, last byte,
    // and the middle, because a digest fed the wrong length would still pass
    // one of the three.
    for index in [0, IMAGE.len() / 2, IMAGE.len() - 1] {
        let mut tampered = IMAGE.to_vec();
        tampered[index] ^= 0x01;
        assert!(
            !verify(&tampered, &signature()),
            "a bit flipped at byte {index} still verified"
        );
    }
}

#[test]
fn a_truncated_image_is_refused() {
    // The case a `size` field lying about the image would produce: the right
    // prefix, verified against the whole image's signature.
    assert!(!verify(&IMAGE[..IMAGE.len() - 1], &signature()));
}

#[test]
fn a_corrupted_signature_is_refused_rather_than_panicking() {
    let mut bytes = signature().to_bytes();
    bytes[0] ^= 0xFF;
    assert!(!verify(IMAGE, &Signature::from_bytes(&bytes)));
}

#[test]
fn signing_the_raw_image_is_not_the_scheme() {
    // The single most likely way to get the tool wrong — sign the image
    // directly rather than its digest — and the assertion that says so. If
    // this ever starts passing, the adapter changed and `fwsign.py` must too.
    assert!(
        key().verify(IMAGE, &signature()).is_err(),
        "the vector was signed over the raw image, not over SHA-512(image)"
    );
}
