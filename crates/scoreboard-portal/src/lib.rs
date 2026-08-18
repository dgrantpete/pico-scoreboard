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
//! - [`mdns`] — the *other* name responder, and the one MicroPython never
//!   needed: lwIP answered `<device_name>.local` for it, embassy-net does not,
//!   and without it the settings page is reachable only by IP address. Same
//!   wire format as [`dns`] and four rules that differ; see its docs.
//! - [`hosts`] — the Host-header check, port of `main.py`'s `get_my_hosts`.
//! - [`conditional`] — the `If-None-Match` check the SPA route makes before it
//!   decides to send 54 KB, port of `main.py:319-336`. It is not part of the
//!   portal proper, but it is the same *kind* of thing: a decision the HTTP
//!   server makes about request bytes before it has consulted any state, and
//!   one that fails silently on a device with no serial port.
//! - [`sntp`] — the SNTP request and the checks a reply has to pass, for the
//!   Phase S firmware that has no backend to ask what time it is. Also not the
//!   portal, and here for [`conditional`]'s reason twice over: it is a UDP
//!   payload in and a rejection out, which is [`dns`] and [`mdns`]'s exact
//!   shape, and a clock that silently accepts a wrong answer is the definition
//!   of failing quietly.
//!
//! Ports of `main.py:496-558` and `dns.py` respectively; each module's docs
//! carry the line references and the deviations.

#![no_std]
#![forbid(unsafe_code)]

pub mod conditional;
pub mod dns;
pub mod hosts;
pub mod mdns;
pub mod sntp;

pub use hosts::MyHosts;
pub use mdns::Responder;
