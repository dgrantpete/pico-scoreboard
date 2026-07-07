//! OTA app updates for the Pico fleet.
//!
//! Serves the current device app — a ROMFS image produced by
//! `tools/build.py publish-app` and baked into the deploy at
//! `app_dist/pico.romfs` — plus a tiny manifest the firmware polls daily.
//! The device's update decision is pure content identity: if the
//! manifest's sha256 differs from the sha of the image it is running
//! (`/app_version` on the device), it downloads and applies. Rollbacks are
//! therefore just publishing an older image.

pub mod handler;

pub use handler::{AppImage, AppManifest, get_app_image, get_app_manifest};
