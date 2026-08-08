use std::env;
use std::fs;
use std::path::PathBuf;

use scoreboard_layout::Profile;

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(
        out.join("memory.x"),
        scoreboard_layout::memory_x(Profile::Bootloader),
    )
    .unwrap();
    println!("cargo::rustc-link-search={}", out.display());
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=../layout/src/lib.rs");
}
