use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    // Put memory.x where the cortex-m-rt link.x include can find it.
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::copy("memory.x", out_dir.join("memory.x")).unwrap();
    println!("cargo::rustc-link-search={}", out_dir.display());
    println!("cargo::rerun-if-changed=memory.x");
}
