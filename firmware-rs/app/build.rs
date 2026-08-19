use std::env;
use std::fs;
use std::path::PathBuf;

use scoreboard_layout::Profile;

fn main() {
    let standalone = env::var_os("CARGO_FEATURE_LINK_STANDALONE").is_some();
    let integrated = env::var_os("CARGO_FEATURE_LINK_BOOT_INTEGRATED").is_some();
    let profile = match (standalone, integrated) {
        (true, false) => Profile::Standalone,
        (false, true) => Profile::BootIntegrated,
        (true, true) => panic!(
            "link-standalone and link-boot-integrated are both on. Features are additive, so \
             the second profile needs --no-default-features --features link-boot-integrated"
        ),
        (false, false) => panic!("one of link-standalone / link-boot-integrated must be enabled"),
    };

    // Put the generated memory.x where cortex-m-rt's link.x INCLUDE finds it.
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(out.join("memory.x"), scoreboard_layout::memory_x(profile)).unwrap();
    println!("cargo::rustc-link-search={}", out.display());

    // So the firmware can say at boot which address it was linked for — the
    // one fact a probe-flashed image and an OTA'd image disagree about.
    println!("cargo::rustc-env=LINK_PROFILE={profile:?}");

    emit_dev_config();
    emit_spa_etag();
    emit_firmware_version();

    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=../layout/src/lib.rs");
    // The direct feed's replay lever (S3-DESIGN decision 13): `poller::direct`
    // reads this with `option_env!`, which cargo does not track by itself — a
    // changed override without this line would keep serving the old base out
    // of an incremental build, which is exactly the kind of stale-image
    // confusion a bench session cannot afford.
    println!("cargo::rerun-if-env-changed=SCOREBOARD_ESPN_BASE");
}

/// Stamp in the version this image answers to, which is also its OTA identity.
///
/// `tools/build.py publish-fw` sets `SCOREBOARD_FW_VERSION`; nothing else does.
/// That asymmetry is the whole mechanism behind
/// `scoreboard_ota::decide`'s `DevBuild` arm: a locally-built image carries
/// `"dev"`, knows it was never published, and refuses to "update" itself to the
/// published one — which would be a rollback, and was a real incident on
/// 2026-07-11 under the MicroPython firmware's `/ota_dev` marker file.
///
/// A marker file could be missed. A constant compiled into the image cannot.
fn emit_firmware_version() {
    println!("cargo::rerun-if-env-changed=SCOREBOARD_FW_VERSION");
    let version = env::var("SCOREBOARD_FW_VERSION").unwrap_or_default();
    let version = if version.is_empty() {
        String::from("dev")
    } else {
        assert!(
            !version.starts_with("dev"),
            "SCOREBOARD_FW_VERSION must not start with `dev` ({version:?}): that prefix is what              tells a device its image was never published, and an image carrying it will refuse              every future update"
        );
        version
    };
    println!("cargo::rustc-env=FW_VERSION={version}");
}

/// Hash the embedded web bundle into the ETag the SPA route serves it under.
///
/// Port of `main.py:319-336`'s `_compute_index_etag`, moved from boot to build:
/// SHA-1 of the gzip, first 8 bytes as lowercase hex. MicroPython recomputed it
/// at every startup by streaming the file through `hashlib` in 512-byte chunks,
/// because on a filesystem the bundle could change under a running device. Here
/// the bundle is `include_bytes!`d into the image, so its hash is a property of
/// the build and there is nothing to recompute — the boot cost goes to zero and
/// the value cannot disagree with the bytes it names.
fn emit_spa_etag() {
    println!("cargo::rerun-if-changed={SPA_ASSET}");

    let bundle = fs::read(SPA_ASSET).unwrap_or_else(|error| {
        panic!(
            "{SPA_ASSET} is missing ({error}). It is a committed build artifact — see \
             assets/README.md for how to regenerate it from `frontend/`."
        )
    });
    let digest = const_sha1::sha1(&bundle).as_bytes();
    let mut etag = String::with_capacity(16);
    for byte in &digest[..8] {
        etag.push_str(&format!("{byte:02x}"));
    }
    println!("cargo::rustc-env=SPA_ETAG={etag}");
}

/// The web bundle, relative to this crate's root. Named here so `build.rs` and
/// the `include_bytes!` in `http::spa` cannot drift.
const SPA_ASSET: &str = "assets/index.html.gz";

/// Every key `dev.toml` may set, as `(section, key, env var, default)`.
///
/// The default is what the firmware sees when the file is absent, and every
/// default here is the un-provisioned device's behaviour: no SSID means
/// `net::wifi` skips the station attempts entirely and boots into AP setup
/// mode, which is exactly the fresh-out-of-the-box path.
const DEV_KEYS: &[(&str, &str, &str, &str)] = &[
    ("station", "ssid", "DEV_WIFI_SSID", ""),
    ("station", "password", "DEV_WIFI_PASSWORD", ""),
    ("station", "device_name", "DEV_DEVICE_NAME", "scoreboard"),
    (
        "station",
        "connect_timeout_seconds",
        "DEV_CONNECT_TIMEOUT_SECONDS",
        "20",
    ),
    ("api", "url", "DEV_API_URL", ""),
];

/// Read the gitignored `dev.toml` into compile-time env vars.
///
/// **This is a bench seam, not the product.** Device config storage over the
/// flash region (SPEC §9) is task #12's, and it replaces exactly one function:
/// `net::wifi::Credentials::from_dev_build`. Until then a probe-flashed image
/// has no other way to know a network, and typing a passphrase into a tracked
/// file is the failure mode worth engineering against — hence a file that is
/// ignored before it exists (see `.gitignore`) and `dev.example.toml` as the
/// tracked template.
///
/// Values reach the firmware through `env!`, so they end up in the image — as
/// `.rodata` literals or, at opt-level 3, as instruction immediates that no
/// grep will find but any disassembler will. And published artifacts are NOT
/// built in some clean CI: `tools/build.py publish-fw` runs this build on
/// whatever machine invokes it, dev.toml and all. Drill day 2026-08-16 proved
/// the consequence — a published image carried the bench wifi passphrase in
/// recoverable form. So the seed is now keyed off the same signal that already
/// separates published from bench: `SCOREBOARD_FW_VERSION` set means a
/// publish, and a publish gets every `DEV_*` key empty, dev.toml or not.
///
/// The parser is deliberately minimal — `key = "value"` under `[section]`
/// headers, `#` comments — rather than a `toml` build-dependency. It reads five
/// keys from a file one developer writes by hand.
fn emit_dev_config() {
    println!("cargo::rerun-if-changed=dev.toml");
    println!("cargo::rerun-if-env-changed=SCOREBOARD_FW_VERSION");

    // A published artifact must not carry the bench's secrets — see the doc
    // above. Version set = publish = the seed reads as an absent dev.toml.
    let publishing = !env::var("SCOREBOARD_FW_VERSION")
        .unwrap_or_default()
        .is_empty();
    let text = if publishing {
        String::new()
    } else {
        fs::read_to_string("dev.toml").unwrap_or_default()
    };
    let mut section = String::new();
    let mut found: Vec<(String, String, String)> = Vec::new();

    for (number, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim().to_string();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            panic!("dev.toml:{}: expected `key = value`, got `{raw}`", number + 1);
        };
        let key = key.trim().to_string();
        let value = value.trim().trim_matches('"').to_string();
        found.push((section.clone(), key, value));
    }

    for (section, key, var, default) in DEV_KEYS {
        let value = found
            .iter()
            .find(|(s, k, _)| s == section && k == key)
            .map(|(_, _, v)| v.as_str())
            .unwrap_or(default);
        println!("cargo::rustc-env={var}={value}");
    }

    // A typo in a key name would otherwise read as "the default applies", and
    // the symptom — a device that boots into setup mode — looks identical to
    // having no file at all.
    for (section, key, _) in &found {
        let known = DEV_KEYS
            .iter()
            .any(|(s, k, _, _)| s == section && k == key);
        assert!(known, "dev.toml: unknown key `{section}.{key}`");
    }
}
