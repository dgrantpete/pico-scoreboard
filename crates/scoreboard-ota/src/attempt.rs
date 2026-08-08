//! The record that stops one bad image being installed every day forever.
//!
//! # The loop this exists to break
//!
//! A/B OTA guarantees that a broken image is *reverted*, not that it is not
//! *re-downloaded*. Without a memory of what was tried, the sequence is:
//!
//! 1. The manifest offers version V. The device downloads it, verifies it,
//!    marks it and reboots.
//! 2. V boots, fails its health gate, never calls `mark_booted`.
//! 3. The watchdog resets. The bootloader reverts. The old image is back.
//! 4. The old image checks the manifest, which still offers V.
//!
//! That is an infinite loop with a several-minute period, and every lap costs
//! two full partition swaps plus a download. The bootloader is doing exactly
//! its job the whole time; nothing is corrupt; the panel is dark for most of
//! every lap.
//!
//! So the device remembers. [`Attempt`] names the version being installed and
//! counts how many times installing it has been started; the boot that finds
//! `State::Revert` marks it [`reverted`](Attempt::reverted), and [`decide`]
//! refuses to install that version again.
//!
//! # Why it survives the swap
//!
//! The record lives in the storage region (SPEC §9), which is outside both the
//! active and DFU partitions — so a swap does not touch it and a revert does
//! not lose it. The old image writes "installing V"; the trial image either
//! confirms it or dies; whichever image is running afterwards reads the same
//! bytes back. That is the entire mechanism, and it works only because the
//! partition table put storage outside the pair.
//!
//! # Why it is not a config field
//!
//! SPEC §9 argued the OTA flag belongs in the configuration document, and it
//! does — `ota.enabled` and `ota.channel` are things a person sets. This is
//! not: nothing outside this module writes it, `GET /api/config` has no
//! business returning it, and a `PUT` that reset it would re-arm the loop
//! above. It also changes on a different clock — once per update attempt
//! rather than once per settings save — and folding it into the configuration
//! document would rewrite the whole document, wifi password and all, every
//! time an update started.
//!
//! It is therefore the **third** storage key, and the first thing SPEC §9's
//! "two, not four" has had to make room for.

use crate::manifest::{MAX_VERSION, Version};

/// How many times installing one version may be started before the device
/// gives up on it.
///
/// Three, not one: the failures this counts are not all the image's fault. A
/// download interrupted by a power cut, or a verify that the watchdog cut
/// short, leaves the count incremented and the image entirely innocent. Two
/// retries covers the transient causes; a fourth attempt at the same version
/// has never been anything but the same failure again.
///
/// A [`reverted`](Attempt::reverted) record needs no retries at all — the image
/// was installed, ran, and failed its own health gate, which is a verdict
/// rather than an accident.
pub const MAX_ATTEMPTS: u8 = 3;

/// What the device has tried to install, and how that went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    /// The version being installed.
    pub target: Version,
    /// How many times a download of `target` has been started.
    pub attempts: u8,
    /// The bootloader rolled `target` back: it was installed, booted, and
    /// never confirmed.
    pub reverted: bool,
}

/// The record's encoding version, leading every record.
///
/// A firmware that changes the shape below bumps this, and [`Attempt::decode`]
/// answers `None` for anything else — which reads as "no record", the same as
/// a fresh device. Losing the record across a firmware change is the correct
/// trade: the alternative is a decoder that has to be right about bytes written
/// by a version of itself that no longer exists.
const FORMAT: u8 = 1;

/// `FORMAT` + attempts + reverted + version length + the version.
pub const MAX_BYTES: usize = 4 + MAX_VERSION;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooLong;

impl Attempt {
    /// A first attempt at `target`.
    pub fn first(target: &str) -> Result<Attempt, TooLong> {
        Ok(Attempt {
            target: Version::try_from(target).map_err(|_| TooLong)?,
            attempts: 1,
            reverted: false,
        })
    }

    /// Whether this record is about `version`.
    pub fn is_about(&self, version: &str) -> bool {
        self.target == version
    }

    /// Whether `version` may still be installed, given this record.
    ///
    /// A record about a *different* version says nothing about this one: the
    /// backend publishing a new image is exactly the event that should clear a
    /// stuck device, and it does so without anyone having to remember to.
    pub fn permits(&self, version: &str) -> bool {
        !self.is_about(version) || (!self.reverted && self.attempts < MAX_ATTEMPTS)
    }

    /// Record that another install of `target` is starting.
    ///
    /// Saturating rather than wrapping: a count that rolled over to zero would
    /// re-arm the loop this module exists to break, which is the one failure
    /// mode worth being careful about here.
    pub fn again(&mut self) {
        self.attempts = self.attempts.saturating_add(1);
    }

    pub fn encode(&self, out: &mut [u8]) -> Result<usize, TooLong> {
        let version = self.target.as_bytes();
        let length = 4 + version.len();
        if out.len() < length {
            return Err(TooLong);
        }
        out[0] = FORMAT;
        out[1] = self.attempts;
        out[2] = u8::from(self.reverted);
        out[3] = version.len() as u8;
        out[4..length].copy_from_slice(version);
        Ok(length)
    }

    /// `None` for any record this firmware cannot read — see [`FORMAT`].
    pub fn decode(record: &[u8]) -> Option<Attempt> {
        let &[format, attempts, reverted, length, ref rest @ ..] = record else {
            return None;
        };
        if format != FORMAT {
            return None;
        }
        let version = rest.get(..length as usize)?;
        Some(Attempt {
            target: Version::try_from(core::str::from_utf8(version).ok()?).ok()?,
            attempts,
            // Any non-zero byte, not just 1: the field is a boolean and a
            // decoder that only accepted 1 would read a corrupt 2 as "not
            // reverted", which is the unsafe direction.
            reverted: reverted != 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_record_permits_its_own_target() {
        let record = Attempt::first("2026.08.08-a1b2c3d").unwrap();
        assert!(record.permits("2026.08.08-a1b2c3d"));
    }

    #[test]
    fn the_count_runs_out_after_max_attempts() {
        let mut record = Attempt::first("v1").unwrap();
        assert_eq!(record.attempts, 1);
        record.again();
        assert!(record.permits("v1"), "a second attempt is allowed");
        record.again();
        assert_eq!(record.attempts, MAX_ATTEMPTS);
        assert!(!record.permits("v1"), "the third attempt was the last");
    }

    #[test]
    fn a_revert_ends_it_immediately_whatever_the_count() {
        let mut record = Attempt::first("v1").unwrap();
        record.reverted = true;
        assert!(
            !record.permits("v1"),
            "an image that ran and failed its gate gets no retries"
        );
    }

    #[test]
    fn a_new_version_clears_a_stuck_device_without_anyone_intervening() {
        let mut record = Attempt::first("v1").unwrap();
        record.reverted = true;
        record.attempts = MAX_ATTEMPTS;
        assert!(
            record.permits("v2"),
            "publishing a fix is what unblocks a device stuck on a bad image"
        );
    }

    #[test]
    fn the_count_saturates_rather_than_wrapping_back_into_permitting() {
        let mut record = Attempt::first("v1").unwrap();
        for _ in 0..300 {
            record.again();
        }
        assert_eq!(record.attempts, u8::MAX);
        assert!(!record.permits("v1"));
    }

    #[test]
    fn a_record_round_trips_through_storage() {
        let mut record = Attempt::first("2026.08.08-a1b2c3d").unwrap();
        record.again();
        record.reverted = true;

        let mut bytes = [0u8; MAX_BYTES];
        let length = record.encode(&mut bytes).unwrap();
        assert_eq!(Attempt::decode(&bytes[..length]), Some(record));
    }

    #[test]
    fn the_longest_version_still_fits_the_buffer() {
        let long = "x".repeat(MAX_VERSION);
        let record = Attempt::first(&long).unwrap();
        let mut bytes = [0u8; MAX_BYTES];
        let length = record.encode(&mut bytes).unwrap();
        assert_eq!(length, MAX_BYTES);
        assert_eq!(Attempt::decode(&bytes[..length]).unwrap().target, long.as_str());
    }

    #[test]
    fn a_version_past_the_bound_is_refused_rather_than_truncated() {
        assert_eq!(Attempt::first(&"x".repeat(MAX_VERSION + 1)), Err(TooLong));
    }

    #[test]
    fn a_record_from_another_format_reads_as_no_record() {
        let record = Attempt::first("v1").unwrap();
        let mut bytes = [0u8; MAX_BYTES];
        let length = record.encode(&mut bytes).unwrap();
        bytes[0] = FORMAT + 1;
        assert_eq!(Attempt::decode(&bytes[..length]), None);
    }

    #[test]
    fn every_truncation_decodes_to_none_rather_than_panicking() {
        let record = Attempt::first("2026.08.08-a1b2c3d").unwrap();
        let mut bytes = [0u8; MAX_BYTES];
        let length = record.encode(&mut bytes).unwrap();
        for cut in 0..length {
            assert_eq!(
                Attempt::decode(&bytes[..cut]),
                None,
                "a {cut}-byte prefix is not a record"
            );
        }
    }

    #[test]
    fn a_corrupt_reverted_byte_reads_as_reverted() {
        let record = Attempt::first("v1").unwrap();
        let mut bytes = [0u8; MAX_BYTES];
        let length = record.encode(&mut bytes).unwrap();
        bytes[2] = 0x7F;
        assert!(
            Attempt::decode(&bytes[..length]).unwrap().reverted,
            "a garbled flag must fail towards refusing the install"
        );
    }
}
