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

    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=layout/src/lib.rs");
}
