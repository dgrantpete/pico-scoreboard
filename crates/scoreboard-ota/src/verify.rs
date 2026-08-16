//! The signature check, and the token that proves it happened.
//!
//! # Why this is ours and not `embassy-boot`'s
//!
//! `BlockingFirmwareUpdater::verify_and_mark_updated` does exactly what this
//! does, was demonstrated on this hardware by the Phase 4 spike, and — once
//! `embassy-boot`'s `ed25519-dalek` feature is on — is the *only* way to mark
//! an image, because the feature deletes `mark_updated`. That deletion is a
//! genuinely valuable property: with it, no amount of wrong code in the OTA
//! client can arm a swap for an unverified image, because there is no call that
//! would do it.
//!
//! It was still not takeable, for one reason:
//!
//! ```text
//! let mut chunk_buf = [0; 2];
//! self.hash::<Sha512>(_update_len, &mut chunk_buf, &mut message)?;
//! ```
//!
//! Two bytes. Hashing a ~700 KB image that way is ~350,000 round trips through
//! a mutex, a `RefCell`, two bounds checks and a two-byte `memcpy`, inside a
//! **single blocking call that cannot feed the watchdog**. Under the bootloader
//! an 8 s watchdog is already running and cannot be disarmed
//! (`firmware-rs/boot`), so if that call overruns, the device resets in the
//! middle of verifying, comes back, and does it again — a reboot loop on every
//! update, which is the one outcome A/B OTA exists to make impossible.
//!
//! The spike measured verification as sub-second at 26 KB and warned that
//! ~800 KB might cost "tens of seconds". That extrapolation spans both sides of
//! the 8 s limit, and it could not be resolved without the hardware. Drill day
//! (2026-08-16) resolved it: **the wrong side** — a single blocking hash of the
//! 1.1 MB image overran the window and reset the device at the end of every
//! install, even with a 4 KB chunk buffer. The chunk size was never the
//! decisive variable; the inability to feed mid-call was.
//!
//! # What is taken instead
//!
//! The app walks the DFU partition itself — the same SHA-512 over the same
//! bytes, read 4 KB at a time through the partition's own bounds — feeding the
//! watchdog and yielding to the executor between chunks, so the walk can take
//! as long as the flash makes it take. Then this function does the ed25519
//! check that `verify_and_mark_updated` would have done, on the digest that
//! produced.
//!
//! The deleted-`mark_updated` guarantee is rebuilt here as a type: [`Verified`]
//! is a token with a private field, so the only way to obtain one is to call
//! [`verify`] and have it succeed, and `ota::arm_swap` demands one. Arming a
//! swap for an unverified image does not compile.
//!
//! # The scheme
//!
//! Identical to the adapter's, because it has to be:
//!
//! ```text
//! Ed25519_verify(public_key, SHA512(image), signature)
//! ```
//!
//! and `tests/signature.rs` checks this very function against a vector
//! `tools/fwsign.py` produced, so the device path and the publishing path are
//! pinned to each other rather than to two readings of the same paragraph.

use ed25519_dalek::{Signature, VerifyingKey};

/// Proof that an image's signature was checked and matched.
///
/// Constructible only by [`verify`]. `ota::arm_swap` takes one by value, which
/// is what replaces the structural guarantee `embassy-boot`'s verify feature
/// would have given by deleting `mark_updated` — see the module docs.
#[derive(Debug, PartialEq, Eq)]
pub struct Verified {
    /// Private, and the whole point: no other module can build this.
    _private: (),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureError {
    /// The manifest's key is not a point on the curve. Only reachable if the
    /// compiled-in public key is corrupt, since that is the only key used.
    BadKey,
    /// The image in DFU is not what the publisher signed.
    Mismatch,
}

impl SignatureError {
    pub const fn as_str(self) -> &'static str {
        match self {
            SignatureError::BadKey => "the compiled-in public key is not a valid ed25519 key",
            SignatureError::Mismatch => "the image's signature did not verify",
        }
    }
}

/// Check `signature` over `digest`, which must be `SHA-512(image)`.
///
/// # `verify_strict`, not `verify`
///
/// embassy-boot's adapter calls the permissive `verify`. This calls
/// `verify_strict`, which additionally rejects small-order public keys and
/// small-order `R` values, and the difference is not academic — it was found by
/// the `garbage_is_refused_rather_than_panicking` test below:
///
/// > an all-zero public key verifies an all-zero signature over any message.
///
/// A device whose compiled-in key had been zeroed would therefore accept an
/// image signed with 64 zero bytes, which is a signature anybody can produce.
/// That key is a source-file constant and there is no plausible path that
/// zeroes it, so this is defence against a corruption rather than against an
/// attacker — but the check costs one curve comparison per update, runs at most
/// once a day, and turns "the trust root failed open" into "the trust root
/// failed closed". A signature `tools/fwsign.py` produced with a real key
/// passes strict verification unchanged.
pub fn verify(
    digest: &[u8; 64],
    public_key: &[u8; 32],
    signature: &[u8; 64],
) -> Result<Verified, SignatureError> {
    let key = VerifyingKey::from_bytes(public_key).map_err(|_| SignatureError::BadKey)?;
    key.verify_strict(digest, &Signature::from_bytes(signature))
        .map_err(|_| SignatureError::Mismatch)?;
    Ok(Verified { _private: () })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn garbage_is_refused_rather_than_panicking() {
        // Every argument here is attacker-influenced in the real path: the
        // signature comes off the wire, the digest is whatever landed in DFU,
        // and the key is bytes out of a source file. None of the three may
        // panic a device that is otherwise showing a scoreboard.
        //
        // Note that most 32-byte strings *are* valid ed25519 points — the
        // encoding is a y-coordinate plus a sign bit — so these come back as
        // `Mismatch` rather than `BadKey`. `BadKey` is kept because
        // `VerifyingKey::from_bytes` is genuinely fallible and the alternative
        // was an `unwrap` on the compiled-in key.
        //
        // `0x00` is the one that matters and the reason `verify` above is
        // `verify_strict`: an all-zero key is the identity point, and under the
        // permissive `verify` it accepts an all-zero signature over anything.
        for pattern in [0x00, 0x01, 0x7F, 0x80, 0xFF] {
            let outcome = verify(&[pattern; 64], &[pattern; 32], &[pattern; 64]);
            assert!(outcome.is_err(), "pattern {pattern:#04x} verified: {outcome:?}");
        }
    }

    #[test]
    fn the_two_failures_are_told_apart() {
        // "your key is broken" and "this image is not ours" send an operator to
        // completely different places, and the log line is all they get.
        assert_ne!(
            SignatureError::BadKey.as_str(),
            SignatureError::Mismatch.as_str()
        );
    }
}
