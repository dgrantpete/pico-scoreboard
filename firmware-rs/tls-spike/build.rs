use std::env;
use std::fs;
use std::path::PathBuf;

/// Credentials come from the app's gitignored `dev.toml` — one secret file,
/// not two. The spike joins the bench network with the same identity the
/// app's dev builds use; a missing file is a hard error because a spike that
/// boots into nothing produces no numbers, silently.
fn credentials() {
    let path = PathBuf::from("../app/dev.toml");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("tls-spike needs {} (see TOOLCHAIN.md, dev.toml): {e}", path.display()));

    let mut ssid = None;
    let mut password = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some((key, value)) = line.split_once('=') {
            let value = value.trim().trim_matches('"');
            match key.trim() {
                "ssid" => ssid = Some(value.to_string()),
                "password" => password = Some(value.to_string()),
                _ => {}
            }
        }
    }
    let ssid = ssid.expect("dev.toml has no ssid");
    let password = password.expect("dev.toml has no password");
    println!("cargo::rustc-env=SPIKE_WIFI_SSID={ssid}");
    println!("cargo::rustc-env=SPIKE_WIFI_PASSWORD={password}");
    println!("cargo::rerun-if-changed=../app/dev.toml");

    // The LAN-local TLS terminator fronting the tools/espn mock. Overridable
    // without editing source so the rig can move machines.
    let terminator =
        env::var("SPIKE_TERMINATOR").unwrap_or_else(|_| "192.168.50.2:8443".to_string());
    println!("cargo::rustc-env=SPIKE_TERMINATOR={terminator}");
    println!("cargo::rerun-if-env-changed=SPIKE_TERMINATOR");
}

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::copy("memory.x", out_dir.join("memory.x")).unwrap();
    println!("cargo::rustc-link-search={}", out_dir.display());
    println!("cargo::rerun-if-changed=memory.x");
    credentials();
}
