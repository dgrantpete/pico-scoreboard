use std::path::Path;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, header};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use utoipa::{IntoParams, ToSchema};

use crate::AppState;
use crate::auth::FwApiKey;
use crate::error::AppError;

/// Log the device-identity headers the Rust firmware sends.
///
/// Separate from `app_update`'s despite doing the same job: the two fleets send
/// different header sets (`X-Fw-Version` here, `X-Mpy` and `X-Romfs-Bytes`
/// there), and merging them would couple two contracts that must be free to
/// diverge — one of them is frozen forever and the other is not.
fn log_device_meta(endpoint: &str, channel: Channel, headers: &HeaderMap) {
    let get = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("-")
            .to_string()
    };
    tracing::info!(
        endpoint,
        channel = channel.as_str(),
        device_id = get("x-device-id"),
        fw_version = get("x-fw-version"),
        context = get("x-ota-context"),
        "fw device request"
    );
}

/// Which artifact a device is asking for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Stable,
    Dev,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Dev => "dev",
        }
    }
}

/// `?channel=stable|dev`, defaulting to stable.
#[derive(Debug, Default, Deserialize, IntoParams)]
pub struct ChannelQuery {
    channel: Option<String>,
}

impl ChannelQuery {
    /// Resolve the parameter.
    ///
    /// An unrecognised value is an error rather than a silent fall back to
    /// stable. The device sends the channel it is pinned to, so a typo that
    /// quietly resolved to stable would ship the stable image to a unit that
    /// asked for staging — and it would do it silently, one poll interval
    /// later, with the device's own logs saying it asked for `dev`.
    fn resolve(&self) -> Result<Channel, AppError> {
        match self.channel.as_deref() {
            None | Some("") | Some("stable") => Ok(Channel::Stable),
            Some("dev") => Ok(Channel::Dev),
            Some(other) => {
                tracing::warn!(channel = other, "unknown fw channel requested");
                Err(AppError::InvalidFwChannel)
            }
        }
    }
}

/// The sidecar `publish-fw` writes next to an image.
///
/// It carries only what cannot be derived from the bytes. The size and the
/// hash deliberately are *not* in it — see [`FwImage::load`].
#[derive(Debug, Deserialize)]
struct Sidecar {
    version: String,
    /// 64-byte detached ed25519 signature over `SHA-512(image)`, lowercase hex.
    signature: String,
}

/// One channel's artifact, loaded at startup.
pub struct FwImage {
    pub bytes: Vec<u8>,
    pub version: String,
    pub signature: String,
    /// Computed here, from `bytes`. Never read from the sidecar.
    pub sha256: String,
}

impl FwImage {
    /// Load `<dir>/image.bin` and `<dir>/manifest.json`.
    ///
    /// Returns `None` — with a log line — for every failure, so the endpoints
    /// 404 rather than serving something half-formed. That matches
    /// `AppImage::load`'s dev-friendly behaviour and matters more here: a
    /// device that downloads a megabyte and then rejects the signature has
    /// spent a minute of dark panel to learn what this could have said at
    /// startup.
    pub fn load(dir: &str) -> Option<Self> {
        let dir = Path::new(dir);
        let bytes = match std::fs::read(dir.join("image.bin")) {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::warn!(
                    dir = %dir.display(),
                    error = %e,
                    "no firmware image — /fw endpoints will 404 for this channel"
                );
                return None;
            }
        };

        let sidecar = std::fs::read_to_string(dir.join("manifest.json"))
            .map_err(|e| e.to_string())
            .and_then(|text| serde_json::from_str::<Sidecar>(&text).map_err(|e| e.to_string()));
        let sidecar = match sidecar {
            Ok(sidecar) => sidecar,
            Err(e) => {
                tracing::error!(
                    dir = %dir.display(),
                    error = %e,
                    "firmware image present but its manifest.json is unreadable"
                );
                return None;
            }
        };

        // Validated at load, not per request. A signature of the wrong length
        // or the wrong alphabet cannot verify on any device, so serving it
        // would cost every unit in the fleet a download to discover that.
        if sidecar.version.is_empty() {
            tracing::error!(dir = %dir.display(), "firmware manifest.json has an empty version");
            return None;
        }
        let signature_ok = sidecar.signature.len() == 128
            && sidecar
                .signature
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !signature_ok {
            tracing::error!(
                dir = %dir.display(),
                len = sidecar.signature.len(),
                "firmware manifest.json signature is not 128 lowercase hex digits"
            );
            return None;
        }

        let sha256 = hex::encode(Sha256::digest(&bytes));
        tracing::info!(
            dir = %dir.display(),
            version = sidecar.version,
            size = bytes.len(),
            sha256,
            "firmware image loaded"
        );
        Some(Self {
            bytes,
            version: sidecar.version,
            signature: sidecar.signature,
            sha256,
        })
    }
}

/// Both channels' artifacts.
#[derive(Default)]
pub struct FwImages {
    pub stable: Option<FwImage>,
    pub dev: Option<FwImage>,
}

impl FwImages {
    pub fn load(stable_dir: &str, dev_dir: &str) -> Self {
        Self {
            stable: FwImage::load(stable_dir),
            dev: FwImage::load(dev_dir),
        }
    }

    fn get(&self, channel: Channel) -> Option<&FwImage> {
        match channel {
            Channel::Stable => self.stable.as_ref(),
            Channel::Dev => self.dev.as_ref(),
        }
    }
}

/// What the firmware polls to decide whether to update.
#[derive(Serialize, ToSchema)]
pub struct FwManifest {
    /// The image's identity, stamped into it at build time.
    pub version: String,
    /// Hex sha256 of the image bytes, computed from what `/fw/image` serves.
    pub sha256: String,
    pub size: usize,
    /// Hex ed25519 signature over `SHA-512(image)`.
    pub signature: String,
    /// Echoed so the device can check it got the channel it asked for.
    pub channel: Channel,
}

/// Get the current firmware manifest
#[utoipa::path(
    get,
    path = "/fw/manifest",
    tag = "fw",
    params(ChannelQuery),
    responses(
        (status = 200, description = "Current firmware image identity", body = FwManifest),
        (status = 400, description = "Unknown channel", body = crate::error::ErrorResponse),
        (status = 404, description = "No firmware published on this channel", body = crate::error::ErrorResponse),
        (status = 401, description = "Missing or invalid API key", body = crate::error::ErrorResponse),
    ),
    security(("api_key" = []))
)]
pub async fn get_fw_manifest(
    _auth: FwApiKey,
    State(state): State<Arc<AppState>>,
    Query(query): Query<ChannelQuery>,
    headers: HeaderMap,
) -> Result<Json<FwManifest>, AppError> {
    let channel = query.resolve()?;
    log_device_meta("manifest", channel, &headers);
    let image = state
        .fw_images
        .get(channel)
        .ok_or(AppError::FwImageUnavailable)?;
    Ok(Json(FwManifest {
        version: image.version.clone(),
        sha256: image.sha256.clone(),
        size: image.bytes.len(),
        signature: image.signature.clone(),
        channel,
    }))
}

/// Download the current firmware image
#[utoipa::path(
    get,
    path = "/fw/image",
    tag = "fw",
    params(ChannelQuery),
    responses(
        (status = 200, description = "Signed firmware image bytes", content_type = "application/octet-stream"),
        (status = 400, description = "Unknown channel", body = crate::error::ErrorResponse),
        (status = 404, description = "No firmware published on this channel", body = crate::error::ErrorResponse),
        (status = 401, description = "Missing or invalid API key", body = crate::error::ErrorResponse),
    ),
    security(("api_key" = []))
)]
pub async fn get_fw_image(
    _auth: FwApiKey,
    State(state): State<Arc<AppState>>,
    Query(query): Query<ChannelQuery>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    let channel = query.resolve()?;
    log_device_meta("image", channel, &headers);
    let image = state
        .fw_images
        .get(channel)
        .ok_or(AppError::FwImageUnavailable)?;
    Ok((
        [(header::CONTENT_TYPE, "application/octet-stream")],
        image.bytes.clone(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(channel: Option<&str>) -> ChannelQuery {
        ChannelQuery {
            channel: channel.map(str::to_string),
        }
    }

    #[test]
    fn an_absent_or_empty_channel_is_stable() {
        assert_eq!(query(None).resolve().unwrap(), Channel::Stable);
        assert_eq!(query(Some("")).resolve().unwrap(), Channel::Stable);
    }

    #[test]
    fn the_two_known_channels_resolve() {
        assert_eq!(query(Some("stable")).resolve().unwrap(), Channel::Stable);
        assert_eq!(query(Some("dev")).resolve().unwrap(), Channel::Dev);
    }

    #[test]
    fn an_unknown_channel_is_refused_rather_than_read_as_stable() {
        // Including a near miss in case: a device pinned to staging that got
        // the stable image would be silently downgraded.
        for bad in ["Dev", "DEV", "prod", "nightly", "stable "] {
            assert!(
                query(Some(bad)).resolve().is_err(),
                "{bad:?} should not resolve"
            );
        }
    }

    /// A throwaway directory holding one channel's artifacts.
    struct Fixture(std::path::PathBuf);

    impl Fixture {
        fn new(name: &str, image: &[u8], sidecar: &str) -> Fixture {
            let dir = std::env::temp_dir().join(format!(
                "fw-{}-{}-{}",
                name,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("image.bin"), image).unwrap();
            std::fs::write(dir.join("manifest.json"), sidecar).unwrap();
            Fixture(dir)
        }

        fn load(&self) -> Option<FwImage> {
            FwImage::load(self.0.to_str().unwrap())
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn sidecar(version: &str, signature: &str) -> String {
        format!(r#"{{"version":"{version}","signature":"{signature}"}}"#)
    }

    const GOOD_SIG: &str = "ab12cd34ef567890ab12cd34ef567890ab12cd34ef567890ab12cd34ef567890ab12cd34ef567890ab12cd34ef567890ab12cd34ef567890ab12cd34ef567890";

    #[test]
    fn the_manifest_describes_the_bytes_that_are_served() {
        // The property the whole design turns on: `sha256` and `size` are
        // computed from the image, so a manifest cannot describe anything but
        // what `/fw/image` hands over.
        let image = b"not really a firmware image, but it hashes the same way";
        let fixture = Fixture::new("good", image, &sidecar("2026.08.08-a1b2c3d", GOOD_SIG));
        let loaded = fixture.load().expect("a well-formed channel loads");

        assert_eq!(loaded.bytes, image);
        assert_eq!(loaded.bytes.len(), image.len());
        assert_eq!(loaded.sha256, hex::encode(Sha256::digest(image)));
        assert_eq!(loaded.version, "2026.08.08-a1b2c3d");
        assert_eq!(loaded.signature, GOOD_SIG);
    }

    #[test]
    fn a_malformed_sidecar_makes_the_channel_absent() {
        let image = b"image";
        let cases = [
            ("short", sidecar("v", &GOOD_SIG[..127])),
            ("long", sidecar("v", &format!("{GOOD_SIG}a"))),
            ("uppercase", sidecar("v", &GOOD_SIG.to_uppercase())),
            ("nonhex", sidecar("v", &"z".repeat(128))),
            ("empty version", sidecar("", GOOD_SIG)),
            ("not json", "{".to_string()),
        ];
        for (name, text) in cases {
            let fixture = Fixture::new("bad", image, &text);
            assert!(
                fixture.load().is_none(),
                "{name} should have made the channel 404, not served"
            );
        }
    }

    #[test]
    fn a_missing_image_makes_the_channel_absent() {
        let dir = std::env::temp_dir().join(format!("fw-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _ = std::fs::remove_file(dir.join("image.bin"));
        assert!(FwImage::load(dir.to_str().unwrap()).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
