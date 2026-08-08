//! Should this device install that image?
//!
//! Five answers, four of them "no". Each `no` is a failure that happened to a
//! real device, and the arm that prevents it says which.
//!
//! # What is *not* a reason to refuse: the transport
//!
//! `firmware/src/ota.py` fetches its manifest and its image over TLS, and
//! treats having done so as part of the trust story. Nothing here does.
//! SPEC §8 moved authenticity from the transport to the artifact: the backend
//! signs the image with ed25519 and the device verifies it against a public key
//! compiled into itself, so a hostile network can substitute bytes all it likes
//! and `verify_and_mark_updated` will refuse to mark them. That is what buys
//! the removal of device-side TLS — 21 KB of standing record buffers and a
//! certificate store this firmware has nowhere to put.
//!
//! The consequence worth stating plainly: **the API key the device sends to
//! `/fw/*` travels in cleartext.** That is why `/fw/*` has a key of its own and
//! does not share the MicroPython fleet's — see `backend/src/auth.rs`.

use crate::attempt::Attempt;
use crate::is_dev_build;
use crate::manifest::Manifest;

/// What this device knows about itself when it reads a manifest.
#[derive(Debug, Clone, Copy)]
pub struct Local<'a> {
    /// The version stamped into this image at build time.
    pub running: &'a str,
    /// `ota.enabled` from the device configuration.
    pub enabled: bool,
    /// The stored [`Attempt`], if there is one.
    pub record: Option<&'a Attempt>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// `ota.enabled` is off.
    Disabled,
    /// This image was never published, and installing the manifest's image
    /// would be a **rollback**.
    ///
    /// `ota.py` solved this with the `/ota_dev` marker file, written by
    /// `build.py flash --release` whenever the flashed image's sha was not the
    /// published one — after the 2026-07-11 incident where a locally-built
    /// device "updated" itself backwards to the older published app and the
    /// littlefs and ROMFS halves ended up from different builds.
    ///
    /// The Rust firmware needs no marker file and no comparison against the
    /// manifest, because the image knows what it is: `publish-fw` is the only
    /// thing that stamps a real version, so anything else carries
    /// [`DEV_VERSION`](crate::DEV_VERSION) and says so about itself forever.
    /// A marker that had to be *written* could be missed; a prefix compiled
    /// into the image cannot.
    DevBuild,
    /// The manifest offers what is already running.
    Current,
    /// The stored [`Attempt`] refuses this version. See [`crate::attempt`].
    Blocked { reverted: bool, attempts: u8 },
    /// Download it.
    Install,
}

impl Decision {
    /// The `status` field `POST /api/check-update` answers with.
    ///
    /// These strings are the SPA's contract (`frontend/src/lib/api/types.ts`'s
    /// `CheckUpdateResponse`), and they are `api_routes.py`'s before that —
    /// `dev_deploy` is named for the marker file that no longer exists,
    /// because renaming it would mean shipping a settings page that cannot
    /// read an older device. The sixth status, `no_network`, is not decided
    /// here: it is the answer when there is no poller to ask, which is a fact
    /// about the device's mode rather than about the manifest.
    pub const fn status(self) -> &'static str {
        match self {
            Decision::Disabled => "disabled",
            Decision::DevBuild => "dev_deploy",
            Decision::Current => "current",
            Decision::Blocked { .. } => "error",
            Decision::Install => "updating",
        }
    }

    /// Whether the caller should go on to download.
    pub const fn installs(self) -> bool {
        matches!(self, Decision::Install)
    }
}

/// The whole decision, in the order the reasons stop mattering.
///
/// `enabled` first because a device with updates switched off should not even
/// be described as "current" — the manifest is none of its business. The dev
/// guard before the version compare because a dev build is *always* a different
/// version from the published one, so the compare would always say "install"
/// and always be wrong.
pub fn decide(manifest: &Manifest, local: &Local<'_>) -> Decision {
    if !local.enabled {
        return Decision::Disabled;
    }
    if is_dev_build(local.running) {
        return Decision::DevBuild;
    }
    if manifest.version == local.running {
        return Decision::Current;
    }
    if let Some(record) = local.record
        && !record.permits(&manifest.version)
    {
        return Decision::Blocked {
            reverted: record.reverted,
            attempts: record.attempts,
        };
    }
    Decision::Install
}

/// Why a [`Decision::Blocked`] blocked, in the words the log line and the
/// settings page both use.
///
/// The two cases are genuinely different faults — one image ran and failed, the
/// other never finished arriving — and this is the only place the difference is
/// ever reported.
pub fn blocked_message(reverted: bool) -> &'static str {
    if reverted {
        "this version was installed once and rolled back; publish a fix to retry"
    } else {
        "this version failed to install repeatedly; publish a fix to retry"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attempt::MAX_ATTEMPTS;
    use crate::manifest::{Channel, Version};

    fn offering(version: &str) -> Manifest {
        Manifest {
            version: Version::try_from(version).unwrap(),
            sha256: [0; 32],
            size: 1024,
            signature: [0; 64],
            channel: Channel::Stable,
        }
    }

    fn local<'a>(running: &'a str, record: Option<&'a Attempt>) -> Local<'a> {
        Local {
            running,
            enabled: true,
            record,
        }
    }

    #[test]
    fn a_newer_version_installs() {
        let decision = decide(&offering("v2"), &local("v1", None));
        assert_eq!(decision, Decision::Install);
        assert_eq!(decision.status(), "updating");
        assert!(decision.installs());
    }

    #[test]
    fn the_running_version_is_current() {
        let decision = decide(&offering("v1"), &local("v1", None));
        assert_eq!(decision, Decision::Current);
        assert_eq!(decision.status(), "current");
        assert!(!decision.installs());
    }

    #[test]
    fn a_disabled_device_never_looks_further() {
        let mut it = local("v1", None);
        it.enabled = false;
        assert_eq!(decide(&offering("v2"), &it), Decision::Disabled);
        // Even when it is already current: "disabled" is the honest answer.
        assert_eq!(decide(&offering("v1"), &it), Decision::Disabled);
    }

    #[test]
    fn a_dev_build_refuses_to_roll_itself_back() {
        // The 2026-07-11 incident, in one assertion. Without this arm the
        // version compare says "install" and the device downgrades itself to
        // the published image.
        assert_eq!(
            decide(&offering("2026.08.08-a1b2c3d"), &local("dev", None)),
            Decision::DevBuild
        );
        assert_eq!(
            decide(&offering("2026.08.08-a1b2c3d"), &local("dev-a1b2c3d", None)),
            Decision::DevBuild
        );
    }

    #[test]
    fn the_dev_guard_outranks_the_version_compare_but_not_the_enable_flag() {
        let mut it = local("dev", None);
        it.enabled = false;
        assert_eq!(decide(&offering("v2"), &it), Decision::Disabled);
    }

    #[test]
    fn a_reverted_version_is_blocked() {
        let mut record = Attempt::first("v2").unwrap();
        record.reverted = true;
        let decision = decide(&offering("v2"), &local("v1", Some(&record)));
        assert_eq!(
            decision,
            Decision::Blocked {
                reverted: true,
                attempts: 1
            }
        );
        assert_eq!(decision.status(), "error");
        assert!(!decision.installs());
    }

    #[test]
    fn a_version_that_never_finished_installing_is_blocked_after_the_last_try() {
        let mut record = Attempt::first("v2").unwrap();
        record.attempts = MAX_ATTEMPTS;
        assert_eq!(
            decide(&offering("v2"), &local("v1", Some(&record))),
            Decision::Blocked {
                reverted: false,
                attempts: MAX_ATTEMPTS
            }
        );
    }

    #[test]
    fn a_record_about_another_version_does_not_block_this_one() {
        let mut record = Attempt::first("v2").unwrap();
        record.reverted = true;
        assert_eq!(
            decide(&offering("v3"), &local("v1", Some(&record))),
            Decision::Install,
            "publishing a fix is what unblocks a stuck device"
        );
    }

    #[test]
    fn a_record_about_the_running_version_still_reads_as_current() {
        // The record left over from a successful install of v1. It names v1,
        // and v1 is what is running, so the answer is `Current` and not
        // `Blocked` — the version compare comes first for exactly this.
        let mut record = Attempt::first("v1").unwrap();
        record.attempts = MAX_ATTEMPTS;
        assert_eq!(
            decide(&offering("v1"), &local("v1", Some(&record))),
            Decision::Current
        );
    }

    #[test]
    fn the_two_blocked_messages_name_different_faults() {
        assert_ne!(blocked_message(true), blocked_message(false));
    }
}
