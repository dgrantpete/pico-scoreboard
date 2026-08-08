//! The Rust firmware's OTA surface: `/fw/manifest` and `/fw/image`.
//!
//! Deliberately a **parallel** surface to [`crate::app_update`], not a
//! replacement for it. `/app/*` serves a MicroPython ROMFS image to the gift
//! units in the field, whose `ota.py` describes its request contract as "spoken
//! by every device FOREVER once flashed"; those devices are USB-only, so
//! nothing about that endpoint can ever change. `/fw/*` serves a signed
//! whole-flash image for the Rust firmware's active partition. Two different
//! artifacts for two different fleets, and the day the last gift unit migrates
//! is the day `/app/*` is deleted with nothing to rename.
//!
//! The two also differ in what makes them trustworthy, which is why they do not
//! share an API key — see [`crate::auth::FwApiKey`].

pub mod handler;

pub use handler::{FwImage, FwImages, FwManifest, get_fw_image, get_fw_manifest};
