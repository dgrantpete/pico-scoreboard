//! The device's trust root.
//!
//! This is the *entire* reason the OTA path can run over plain HTTP. SPEC §8
//! moved authenticity from the transport to the artifact: an image is installed
//! only if `scoreboard_ota::verify` matches it against this key, so a hostile
//! network can substitute whatever bytes it likes and the swap is never armed.
//!
//! # Why it is source and not a `.bin`
//!
//! The Phase 4 spike `include_bytes!`d its throwaway key. A binary blob is the
//! wrong shape for a production trust root, because a change to it is
//! **invisible in a diff** — the one review that most needs to be legible reads
//! as "Binary files differ". Written out as bytes, swapping the key that every
//! deployed device obeys is a reviewable change to a source file, which is what
//! it should be.
//!
//! # Rotating it
//!
//! Do not, casually. Every device already in the field carries the *old* key in
//! its running image, so a new key is only accepted by a device that has
//! already installed an image signed by the old one. In order: publish an image
//! carrying the new key signed with the old, wait for the fleet to take it,
//! then start signing with the new key. Getting that order wrong means a USB
//! flash per unit.
//!
//! `python tools/fwsign.py pubkey` prints the literal below for whichever key
//! `backend/.fw-signing-key` holds.

/// The public half of the key `tools/fwsign.py` signs with.
///
/// Fingerprint: `efdecc6b749df6e30…b62a3a93`. The private half lives in
/// `backend/.fw-signing-key`, is gitignored, and exists nowhere else — losing
/// it means every deployed unit needs a physical flash before it can ever be
/// updated again.
#[cfg_attr(
    not(feature = "link-boot-integrated"),
    allow(dead_code, reason = "only the boot-integrated arm verifies anything; a standalone image has no DFU partition to check")
)]
pub const PUBLIC_KEY: [u8; 32] = [
    0xef, 0xde, 0xcc, 0x6b, 0x74, 0x9d, 0xf6, 0xe3, //
    0x04, 0xfa, 0x0d, 0x3f, 0x1a, 0x06, 0x69, 0x3f, //
    0x95, 0x98, 0x4f, 0xc4, 0x10, 0x13, 0x1d, 0x9a, //
    0x17, 0x01, 0x74, 0x62, 0xb6, 0x2a, 0x3a, 0x93, //
];
