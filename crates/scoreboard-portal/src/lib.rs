//! The captive portal, minus the network.
//!
//! Setup mode has two halves that lie to a phone in complementary ways: a DNS
//! responder that resolves every name to the device, and an HTTP catch-all that
//! redirects anything addressed elsewhere to the setup page. Both halves are
//! decisions about bytes, and both are exactly the kind of thing that fails
//! silently on a device with no serial port — so both live here, where they
//! compile and test on the desktop, and the firmware keeps only the sockets
//! (SPEC §2's crate-boundary rule).
//!
//! - [`dns`] — the answer builder, port of `scoreboard/dns.py`.
//! - [`hosts`] — the Host-header check, port of `main.py`'s `get_my_hosts`.
//!
//! Ports of `main.py:496-558` and `dns.py` respectively; each module's docs
//! carry the line references and the deviations.

#![no_std]
#![forbid(unsafe_code)]

pub mod dns;
pub mod hosts;

pub use hosts::MyHosts;
