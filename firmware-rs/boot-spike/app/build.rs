use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(out.join("memory.x"), spike_layout::memory_x(spike_layout::Region::Application)).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../layout/src/lib.rs");

    // The staged payload and its signature are compile-time inputs
    // (include_bytes!); make cargo notice when they change.
    for var in ["SPIKE_PAYLOAD", "SPIKE_PAYLOAD_SIG"] {
        println!("cargo:rerun-if-env-changed={var}");
        if let Some(path) = env::var_os(var) {
            println!("cargo:rerun-if-changed={}", PathBuf::from(path).display());
        }
    }
}
