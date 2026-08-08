//! The OTA client, minus the flash and the socket.
//!
//! SPEC §8. The device half of an update is four decisions and one piece of
//! arithmetic, and every one of them is the kind of thing that fails silently
//! on a device with no serial port — so all of it lives here, where it compiles
//! and tests on the desktop, and `app/src/ota.rs` keeps only the I/O
//! (SPEC §2's crate-boundary rule, the same split
//! [`scoreboard_portal`](https://docs.rs) makes for the captive portal).
//!
//! - [`manifest`] — the backend's answer, parsed into bounded fields.
//! - [`decide`] — *should this device install that image?* Four ways to say no,
//!   and each one has a failure behind it.
//! - [`attempt`] — the record that stops a bad image being installed forever.
//! - [`progress`] — the percent accounting behind the updating screen.
//!
//! # What is deliberately not here
//!
//! **The signature check.** `embassy-boot`'s `verify_and_mark_updated` is the
//! only path that can mark an image for swap once its `ed25519-dalek` feature
//! is on, and verification happens inside it, against the bytes already in the
//! DFU partition. Reimplementing it here would mean owning a second copy of the
//! trust decision, and the copy that ran would be the one in the bootloader
//! crate anyway. What this crate does own is a *test* that the signing tool and
//! that adapter agree on the scheme — see `tests/signature.rs`, which is the
//! only place the two halves of the pipeline are ever compared.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod attempt;
pub mod decide;
pub mod manifest;
pub mod progress;
pub mod verify;

pub use attempt::{Attempt, MAX_ATTEMPTS};
pub use decide::{Decision, Local, decide};
pub use manifest::{Channel, MAX_VERSION, Manifest, ManifestError, Version};
pub use progress::Progress;
pub use verify::{SignatureError, Verified, verify};

/// The version string a locally-built image carries.
///
/// `app/build.rs` stamps this into every image that `tools/build.py publish-fw`
/// did not build, and [`decide`] refuses to update away from it. See
/// [`decide::Decision::DevBuild`] for the incident that rule comes from.
pub const DEV_VERSION: &str = "dev";

/// Whether `version` names an image that was never published.
///
/// The prefix rather than equality, so a future `dev-<something>` stays covered
/// without this becoming a list to maintain: every unpublished version starts
/// this way and no published one may (`publish-fw` refuses).
pub fn is_dev_build(version: &str) -> bool {
    version.starts_with(DEV_VERSION)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dev_prefix_covers_the_bare_marker_and_anything_built_on_it() {
        assert!(is_dev_build("dev"));
        assert!(is_dev_build("dev-a1b2c3d"));
        assert!(is_dev_build("dev+dirty"));
    }

    #[test]
    fn a_published_version_is_not_a_dev_build() {
        assert!(!is_dev_build("2026.08.08-a1b2c3d"));
        assert!(!is_dev_build("1.0.0"));
        // The one that would bite: a published version merely *containing*
        // "dev" is not a dev build, because the rule is a prefix.
        assert!(!is_dev_build("2026.08.08-deadbee"));
    }
}
