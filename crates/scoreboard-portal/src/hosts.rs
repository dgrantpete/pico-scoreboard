//! Whose name is on the request?
//!
//! Port of `main.py`'s `get_my_hosts` (`:496-511`) and the two comparisons that
//! use it (`:514-558`). In setup mode the DNS responder tells every client that
//! every name is us, so the HTTP server receives requests addressed to
//! `captive.apple.com`, `connectivitycheck.gstatic.com` and whatever else the
//! phone was reaching for. The `Host` header is the only thing that
//! distinguishes those from a real request for the setup page, and the
//! distinction drives the whole portal:
//!
//! - **`GET /`** — a foreign `Host` is redirected to the setup page; ours is
//!   served the SPA. In station mode nothing is redirected: `main.py` guards
//!   that branch on the AP existing, so a station-mode request for `/` always
//!   gets the app.
//! - **Any unknown path** — ours gets `404`, foreign gets `302` to
//!   `http://<ap ip>/#/setup`. The redirect is what turns a probe into an open
//!   browser window.
//!
//! # Two deliberate deviations from `main.py`
//!
//! 1. **The device's own address is always one of its names.** MicroPython
//!    built the set from `app.ap`, which only exists in AP mode, so in station
//!    mode a request for `http://192.168.50.57/some-unknown-path` matched
//!    nothing and was answered with a `302` to `192.168.4.1` — an address that
//!    is not up, on a network the client is not on. `/` was spared only because
//!    its branch is guarded on the AP. [`MyHosts::station`] includes the
//!    station address, so the unknown path 404s like it should.
//! 2. **Matching is ASCII-case-insensitive.** Host names are case-insensitive
//!    per RFC 9110 §4.2, MicroPython's set lookup was not, and mDNS resolvers
//!    do return mixed case — `Scoreboard.local` from a Mac would have been
//!    treated as a hijack and redirected.

use heapless::String;

/// SSIDs cap at 32 bytes, and the device name *is* the AP's SSID, so that is
/// the binding limit on how long a name can be.
pub const MAX_DEVICE_NAME: usize = 32;
/// `255.255.255.255`.
pub const MAX_ADDRESS: usize = 15;

/// The names this device answers to, and whether it is running a portal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MyHosts {
    device_name: String<MAX_DEVICE_NAME>,
    address: String<MAX_ADDRESS>,
    captive: bool,
}

impl MyHosts {
    /// Setup mode: the AP is up and foreign hosts get redirected here.
    pub fn ap(device_name: &str, address: &str) -> MyHosts {
        MyHosts::new(device_name, address, true)
    }

    /// Station mode: joined a network, no portal, nothing is redirected.
    pub fn station(device_name: &str, address: &str) -> MyHosts {
        MyHosts::new(device_name, address, false)
    }

    fn new(device_name: &str, address: &str, captive: bool) -> MyHosts {
        // Truncation rather than rejection: a device whose name is too long
        // should answer to as much of it as fits, not stop answering.
        MyHosts {
            device_name: truncating(device_name),
            address: truncating(address),
            captive,
        }
    }

    /// Is this `Host` header one of ours?
    ///
    /// Accepts the raw header value, port and all — `scoreboard.local:80` is
    /// the same host as `scoreboard.local`, which is why `main.py` split on
    /// `:` before comparing.
    pub fn is_mine(&self, host_header: &str) -> bool {
        let host = strip_port(host_header);
        if host.is_empty() {
            // HTTP/1.1 requires a `Host`; a request without one is not
            // addressed to us in any meaningful sense. `main.py` reached the
            // same answer by defaulting the header to `''`, which was in no set.
            return false;
        }
        if equal_ignoring_ascii_case(host, &self.address) {
            return true;
        }
        if equal_ignoring_ascii_case(host, &self.device_name) {
            return true;
        }
        // `<name>.local` — what mDNS clients ask for. Compared without
        // building the concatenation, which would need a buffer to hold a
        // string this only looks at.
        match host.len().checked_sub(6) {
            Some(split) if host.get(split..).is_some_and(|s| s.eq_ignore_ascii_case(".local")) => {
                equal_ignoring_ascii_case(&host[..split], &self.device_name)
            }
            _ => false,
        }
    }

    /// True in setup mode: a foreign `Host` is a hijacked probe and should be
    /// redirected. False in station mode, where there is no portal to send it
    /// to and `main.py` serves the request instead.
    pub fn captive(&self) -> bool {
        self.captive
    }

    /// The address to build the redirect `Location` from, and the one
    /// `/api/status` reports.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// The AP's SSID in setup mode; the mDNS name's stem in either.
    pub fn device_name(&self) -> &str {
        &self.device_name
    }
}

/// Drop the `:port` suffix. An IPv6 literal (`[::1]:80`) is full of colons and
/// is never one of our names — this stack is IPv4-only — so it is rejected
/// whole rather than mangled by splitting on the first one.
fn strip_port(host_header: &str) -> &str {
    let host = host_header.trim();
    if host.starts_with('[') {
        return "";
    }
    match host.split_once(':') {
        Some((before, _)) => before,
        None => host,
    }
}

fn equal_ignoring_ascii_case(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

fn truncating<const N: usize>(text: &str) -> String<N> {
    let mut out = String::new();
    for character in text.chars() {
        if out.push(character).is_err() {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ap() -> MyHosts {
        MyHosts::ap("scoreboard", "192.168.4.1")
    }

    #[test]
    fn our_names_are_ours() {
        let hosts = ap();
        assert!(hosts.is_mine("scoreboard"));
        assert!(hosts.is_mine("scoreboard.local"));
        assert!(hosts.is_mine("192.168.4.1"));
    }

    #[test]
    fn the_port_is_stripped() {
        let hosts = ap();
        assert!(hosts.is_mine("scoreboard.local:80"));
        assert!(hosts.is_mine("192.168.4.1:8080"));
    }

    #[test]
    fn captive_probes_are_foreign() {
        let hosts = ap();
        for probe in [
            "captive.apple.com",
            "connectivitycheck.gstatic.com",
            "www.msftconnecttest.com",
            "detectportal.firefox.com",
        ] {
            assert!(!hosts.is_mine(probe), "{probe} should not be ours");
        }
    }

    #[test]
    fn a_missing_host_is_not_ours() {
        assert!(!ap().is_mine(""));
        assert!(!ap().is_mine("   "));
    }

    #[test]
    fn case_does_not_matter() {
        // A Mac's mDNS resolver returns the name as advertised, which is not
        // necessarily how it was configured.
        let hosts = ap();
        assert!(hosts.is_mine("Scoreboard.Local"));
        assert!(hosts.is_mine("SCOREBOARD"));
    }

    #[test]
    fn a_suffix_is_not_a_match() {
        let hosts = ap();
        assert!(!hosts.is_mine("notscoreboard"));
        assert!(!hosts.is_mine("notscoreboard.local"));
        assert!(!hosts.is_mine("scoreboard.example.com"));
        assert!(!hosts.is_mine(".local"));
        assert!(!hosts.is_mine("local"));
    }

    #[test]
    fn an_ipv6_literal_is_rejected_whole() {
        // Splitting on the first colon would leave `[`, which matches nothing —
        // the same answer, but by accident. This is the deliberate version.
        let hosts = ap();
        assert!(!hosts.is_mine("[::1]:80"));
        assert!(!hosts.is_mine("[fe80::1]"));
    }

    #[test]
    fn station_mode_answers_to_its_own_address() {
        // The deviation from main.py: in station mode MicroPython's host set
        // held no address at all, so this was a redirect to 192.168.4.1.
        let hosts = MyHosts::station("scoreboard", "192.168.50.57");
        assert!(hosts.is_mine("192.168.50.57"));
        assert!(hosts.is_mine("scoreboard.local"));
        assert!(!hosts.captive(), "station mode redirects nothing");
    }

    #[test]
    fn a_long_device_name_is_truncated_not_rejected() {
        let long = "a".repeat(MAX_DEVICE_NAME + 10);
        let hosts = MyHosts::ap(&long, "192.168.4.1");
        assert_eq!(hosts.device_name().len(), MAX_DEVICE_NAME);
        assert!(hosts.is_mine(hosts.device_name()));
    }
}
