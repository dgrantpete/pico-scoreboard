//! Provisioning: the port of `main.py`'s `start_station_mode` / `start_ap_mode`.
//!
//! Three attempts at the configured network, each one a full radio reset, a
//! scan, a join and a DHCP wait; on exhaustion, an open access point with the
//! setup screen and the captive portal behind it. Every retry rule below is
//! `main.py:628-750`'s, and the ones whose mechanism had to change say so.
//!
//! # Status codes, translated
//!
//! MicroPython polled `wlan.status()` every 500 ms and branched on the integer.
//! `cyw43` has no such poll: [`Control::join`] is one await that resolves when
//! the association settles, and the distinction MicroPython read out of the
//! status code arrives as a [`JoinError`] variant instead. The mapping is exact
//! for every code the state machine acted on:
//!
//! | `wlan.status()` | cyw43 | What the flow does with it |
//! |---|---|---|
//! | `-3` LINK_BADAUTH | `JoinError::AuthenticationFailure` | `setup_reason = bad_auth`, next attempt |
//! | `-2` LINK_NONET | `JoinError::NetworkNotFound` | attempt is over |
//! | `-1` LINK_FAIL | `JoinError::JoinFailure(_)` | re-fire the join, ≤2× inside the first 5 s |
//! | `2` LINK_NOIP | `join` returned `Ok`, no address yet | grants the +15 s extension |
//! | `3` LINK_UP | `Stack::wait_config_up` + a real address | success |
//!
//! LINK_NOIP is the interesting one. In MicroPython it was a state the poll
//! observed; here it is a *place in the code* — after `join` returns `Ok` and
//! before the DHCP client reports a lease — and that is precisely the window
//! the extension was written to cover, so the rule survives the change of
//! mechanism unaltered.
//!
//! # Deliberate deviations from `main.py`
//!
//! - **The reset dance is shorter.** `reset_wlan` did `disconnect → deinit →
//!   sleep 1 s → active(True) → sleep 1 s → config(pm=…) → sleep 0.5 s`, and
//!   `deinit()` there means *power the chip down and re-upload its firmware*.
//!   Doing that here would re-upload 231 KB over SPI between attempts. What the
//!   dance was actually for — clearing association state that MicroPython's
//!   WLAN object accumulated — is [`Control::leave`] plus a settle, which is
//!   what [`reset_radio`] does. The stack's IPv4 configuration is cleared in
//!   the same breath, which `deinit` also did and which is the half that
//!   matters for the "valid IP" check below.
//! - **`NetworkNotFound` ends the attempt immediately** rather than spinning
//!   out the remaining timeout. MicroPython kept polling a status that could
//!   not change; the outcome is the same and the retry arrives sooner.
//! - **No country code is set.** `rp2.country('US')` had a counterpart in the
//!   CLM upload, but `cyw43::Control::init` hard-codes `WORLD_WIDE_XX` and
//!   exposes no setter. World-wide is the conservative regulatory domain (it is
//!   a subset of US channels and powers), so this loses two 5 GHz channels the
//!   radio does not support anyway. Worth revisiting only if a join fails
//!   against an AP on channels 12-13.

use core::net::Ipv4Addr;
use core::fmt::Write as _;

use cyw43::{Control, JoinError, JoinOptions, PowerManagementMode, ScanOptions, ScanType};
use embassy_net::{ConfigV4, DhcpConfig, Ipv4Cidr, Stack, StaticConfigV4};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use scoreboard_model::{Publisher, SetupReason, Store};

/// Steps the startup screen counts up to. `main.py` passes 5 to every
/// `set_startup_step` call; the renderer draws "n/5".
pub const STARTUP_STEPS: u8 = 5;

/// `max_retries` (`main.py:648`).
const ATTEMPTS: u8 = 3;
/// The window inside an attempt in which a `LINK_FAIL` re-fires the join rather
/// than ending the attempt (`main.py:715`).
const EARLY_FAIL_WINDOW: Duration = Duration::from_secs(5);
/// How many times, inside that window (`retry_connect_count < 2`).
const EARLY_FAIL_REFIRES: u8 = 2;
/// The pause after a re-fire, so the radio is not asked twice in a millisecond.
const REFIRE_PAUSE: Duration = Duration::from_secs(1);
/// `noip_extension` (`main.py:652`): extra time granted once association has
/// succeeded and only DHCP is outstanding.
const NOIP_EXTENSION: Duration = Duration::from_secs(15);
/// Replaces `reset_wlan`'s 2.5 s of power-cycle sleeps. See the module docs.
const RADIO_SETTLE: Duration = Duration::from_millis(500);
/// A scan is diagnostic, not load-bearing — a timeout shows "Scan failed" and
/// the join proceeds regardless, exactly as `main.py`'s `except` branch does.
const SCAN_TIMEOUT: Duration = Duration::from_secs(10);

/// The AP's own address. `main.py` never sets it: it is what MicroPython's
/// `network.WLAN(AP_IF)` defaults to, and the setup screen and the QR both
/// print it, so it is pinned here rather than read back from the interface.
pub const AP_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 4, 1);
/// The same address as text, for the setup screen and the Host-header check.
/// Formatting it at runtime would need a scratch buffer to hold a constant.
pub const AP_IP_TEXT: &str = "192.168.4.1";
/// `main.py` leaves the AP channel to MicroPython, which uses the cyw43
/// driver's default of 1. `cyw43::Control::start_ap_open` requires it stated.
const AP_CHANNEL: u8 = 1;

/// Where provisioning ended up.
pub enum Provisioned {
    Station { ip: Ipv4Addr },
    Ap { reason: SetupReason, ip: Ipv4Addr },
}

/// Station credentials and the device identity derived from them.
///
/// **Owned, not borrowed.** They used to be `&'static str` out of `build.rs`;
/// they now come from [`crate::config`], which lives behind a lock that must
/// not be held across an `await` — and provisioning is nothing but awaits. So
/// they are copied out once, into the ~130 B this struct costs in the bringup
/// task's arena, and the lock is released before the radio is touched.
pub struct Credentials {
    pub ssid: heapless::String<{ scoreboard_config::MAX_SSID }>,
    pub password: heapless::String<{ scoreboard_config::MAX_PASSWORD }>,
    /// `config.device_name`: the DHCP hostname in station mode, the AP's SSID
    /// in setup mode, and the base of the mDNS name the Host-header check
    /// accepts.
    pub device_name: heapless::String<{ scoreboard_config::MAX_SSID }>,
    /// `config.connect_timeout_seconds`, per attempt, before the NOIP grant.
    pub connect_timeout: Duration,
}

impl Credentials {
    /// Read the running configuration's network section.
    ///
    /// An empty `ssid` is the un-provisioned device: [`provision`] skips the
    /// station attempts and goes straight to setup mode, reason
    /// `no_network_configured`. Three different situations produce it and all
    /// three are supported — a device out of the box, a bench image built with
    /// no `dev.toml`, and one that has just had `POST /api/reset-network`.
    pub fn from_config() -> Credentials {
        crate::config::with(|config| Credentials {
            ssid: config.network.ssid.clone(),
            password: config.network.password.clone(),
            device_name: config.network.device_name.clone(),
            connect_timeout: Duration::from_secs(config.network.connect_timeout_seconds as u64),
        })
    }
}

/// Join the configured network, or become one.
///
/// Publishes startup progress as it goes: the screen is the only feedback a
/// device with no serial port gives during the slowest part of a boot.
pub async fn provision(
    control: &mut Control<'static>,
    stack: Stack<'static>,
    store: &mut Store,
    publisher: &mut Publisher<'static>,
    credentials: &Credentials,
) -> Provisioned {
    match station(control, stack, store, publisher, credentials).await {
        Ok(ip) => Provisioned::Station { ip },
        Err(reason) => {
            let ip = start_ap(control, stack, &credentials.device_name).await;
            Provisioned::Ap { reason, ip }
        }
    }
}

/// The three-attempt station flow. `Err` carries the reason the setup screen
/// will show.
async fn station(
    control: &mut Control<'static>,
    stack: Stack<'static>,
    store: &mut Store,
    publisher: &mut Publisher<'static>,
    credentials: &Credentials,
) -> Result<Ipv4Addr, SetupReason> {
    if credentials.ssid.is_empty() {
        // `main.py:638-640` returns before touching the radio or the screen:
        // there is nothing to attempt and nothing to report about attempting it.
        defmt::info!("wifi: no ssid configured, going straight to setup mode");
        return Err(SetupReason::NoConfig);
    }

    // Sticky across attempts, exactly like `app.setup_reason`: a BADAUTH on
    // attempt 1 still reads as `bad_auth` after attempts 2 and 3 fail some
    // other way, because a wrong password is the more actionable diagnosis.
    let mut reason = SetupReason::ConnectionFailed;

    for attempt in 1..=ATTEMPTS {
        defmt::info!("wifi: connection attempt {}/{}", attempt, ATTEMPTS);
        reset_radio(control, stack).await;

        // `main.py:660-663`: the step counter is monotonic, so a retry cannot
        // walk the bar backwards. It reads as new text plus the attempt dots.
        let mut label = heapless::String::<12>::new();
        let operation = if attempt == 1 {
            None
        } else {
            let _ = write!(label, "Retry {attempt}/{ATTEMPTS}");
            Some(label.as_str())
        };

        let mut detail = heapless::String::<12>::new();
        match scan(control, &credentials.ssid).await {
            Some(found) => {
                let _ = write!(detail, "Found {found}");
            }
            None => {
                let _ = write!(detail, "Scan failed");
            }
        }
        step(
            store,
            publisher,
            2,
            operation.unwrap_or("WiFi scan"),
            &detail,
            attempt,
        );

        // `main.py:683`: the detail line shows at most 20 characters of SSID.
        let shown_ssid = truncate(&credentials.ssid, 20);
        step(
            store,
            publisher,
            3,
            operation.unwrap_or("Connecting"),
            shown_ssid,
            attempt,
        );

        match attempt_join(control, stack, credentials).await {
            Ok(ip) => {
                defmt::info!("wifi: connected, ip {}", ip);
                step_plain(store, publisher, 4, "Connected", &address_text(ip));
                return Ok(ip);
            }
            Err(AttemptError::BadAuth) => {
                defmt::error!("wifi: authentication failed: wrong password for the configured ssid");
                reason = SetupReason::BadAuth;
            }
            Err(AttemptError::Failed) => {}
        }
    }

    defmt::error!("wifi: all {} attempts failed", ATTEMPTS);
    step_plain(store, publisher, 4, "WiFi", "FAILED");
    // `main.py:749`'s `wlan.active(False)`: stop trying, and make sure no stale
    // address survives into AP mode.
    reset_radio(control, stack).await;
    Err(reason)
}

enum AttemptError {
    /// `-3` LINK_BADAUTH. Distinguished because it changes the setup wording.
    BadAuth,
    /// Everything else: association failure, no such network, or the clock ran
    /// out on the join or on DHCP.
    Failed,
}

/// One attempt: join, then wait for an address, under `main.py`'s two-tier
/// timeout.
async fn attempt_join(
    control: &mut Control<'static>,
    stack: Stack<'static>,
    credentials: &Credentials,
) -> Result<Ipv4Addr, AttemptError> {
    // Armed before the join so the client is already listening when the link
    // comes up. `reset_radio` cleared it, so this is a fresh transaction every
    // attempt — the DHCP equivalent of MicroPython's per-attempt `deinit`.
    stack.set_config_v4(ConfigV4::Dhcp(dhcp_config(&credentials.device_name)));

    let started = Instant::now();
    // The un-extended budget. Association is what buys the extension.
    let mut deadline = started + credentials.connect_timeout;
    let mut refires = 0u8;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.as_ticks() == 0 {
            defmt::warn!("wifi: attempt timed out before associating");
            return Err(AttemptError::Failed);
        }

        let joining = control.join(
            &credentials.ssid,
            JoinOptions::new(credentials.password.as_bytes()),
        );
        match with_timeout(remaining, joining).await {
            Ok(Ok(())) => break,
            Ok(Err(JoinError::AuthenticationFailure)) => return Err(AttemptError::BadAuth),
            Ok(Err(JoinError::NetworkNotFound)) => {
                defmt::warn!("wifi: ssid not found in this attempt");
                return Err(AttemptError::Failed);
            }
            Ok(Err(JoinError::JoinFailure(status))) => {
                // `main.py:715-720`, LINK_FAIL: an association that fails in
                // the first few seconds is usually the radio, not the network,
                // and re-firing inside the same attempt recovers it without
                // paying for another full reset.
                if started.elapsed() < EARLY_FAIL_WINDOW && refires < EARLY_FAIL_REFIRES {
                    refires += 1;
                    defmt::warn!(
                        "wifi: early join failure (status {}), re-firing {}/{}",
                        status,
                        refires,
                        EARLY_FAIL_REFIRES
                    );
                    Timer::after(REFIRE_PAUSE).await;
                    continue;
                }
                defmt::warn!("wifi: join failed, status {}", status);
                return Err(AttemptError::Failed);
            }
            Err(_) => {
                defmt::warn!("wifi: join timed out");
                return Err(AttemptError::Failed);
            }
        }
    }

    // Associated: LINK_NOIP. `main.py:723-726` grants the extension from here.
    deadline = started + credentials.connect_timeout + NOIP_EXTENSION;
    let remaining = deadline.saturating_duration_since(Instant::now());
    if with_timeout(remaining, stack.wait_config_up()).await.is_err() {
        defmt::warn!("wifi: associated but dhcp did not complete in time");
        return Err(AttemptError::Failed);
    }

    // `main.py:733-742`: `isconnected()` alone is not success. An address of
    // `0.0.0.0` means the lease is not really there, and the attempt is retried.
    match stack.config_v4().map(|config| config.address.address()) {
        Some(address) if !address.is_unspecified() => Ok(address),
        _ => {
            defmt::warn!("wifi: link up but no valid address, retrying");
            Err(AttemptError::Failed)
        }
    }
}

/// The DHCP client's configuration.
///
/// The hostname is the whole reason this is not `Default::default()`:
/// `network.hostname(config.device_name)` (`main.py:645`) is what makes the
/// router publish `scoreboard` on the LAN, and DHCP option 12 is how that
/// travels. Option 12 caps at 63 bytes; a longer device name is truncated
/// rather than dropped, because a device that is merely misnamed is better
/// than one that is invisible.
fn dhcp_config(device_name: &str) -> DhcpConfig {
    let mut config = DhcpConfig::default();
    let mut hostname = heapless::String::new();
    for character in device_name.chars() {
        if hostname.push(character).is_err() {
            defmt::warn!("wifi: device name too long for dhcp option 12, truncated");
            break;
        }
    }
    config.hostname = Some(hostname);
    config
}

/// `reset_wlan` (`main.py:599-625`), by the mechanism cyw43 offers. See the
/// module docs for why this is 0.5 s rather than 2.5 s.
async fn reset_radio(control: &mut Control<'static>, stack: Stack<'static>) {
    control.leave().await;
    // Drops the lease and any half-finished DHCP transaction, so the next
    // attempt's "is the address real?" check cannot pass on a stale answer.
    stack.set_config_v4(ConfigV4::None);
    Timer::after(RADIO_SETTLE).await;
    // `config(pm=0xa11140)` — MicroPython's documented "disable power
    // management" magic word. The scoreboard is mains-powered and polls on a
    // schedule; sleeping between beacons only adds latency.
    control
        .set_power_management(PowerManagementMode::None)
        .await;
}

/// How many distinct BSSIDs the count can distinguish before it saturates.
///
/// `network_cyw43_scan_cb` (`extmod/network_cyw43.c:156-191`) keeps an
/// unbounded Python list; there is no unbounded list here, so the count stops
/// climbing past this. 64 is well past what a dense neighbourhood puts on the
/// 2.4 GHz band, and the number is a diagnostic on a boot screen — being told
/// "64" when the true answer is 71 costs nothing.
const SCAN_TABLE: usize = 64;

/// Scan, and report how many distinct networks answered. `None` means the scan
/// failed or timed out — diagnostic only, the join proceeds either way.
///
/// Whether the configured SSID was among them is logged rather than acted on,
/// matching `main.py:672-676`: a network that does not answer a probe can still
/// be joined, so refusing to try on a miss would be a regression.
///
/// **The count is of distinct BSSIDs**, because MicroPython's is: its scan
/// callback searches the accumulated list for the BSSID before appending, so
/// `len(networks)` counts access points and not beacons. cyw43 hands every
/// result through, and one AP answers several probes, so without the same
/// de-duplication "Found N" would read about three times too high.
async fn scan(control: &mut Control<'static>, ssid: &str) -> Option<u16> {
    let mut options = ScanOptions::default();
    // MicroPython's `wlan.scan()` transmits probes. Passive would be quieter
    // and slower, and would miss hidden networks entirely.
    options.scan_type = ScanType::Active;
    // **Not `None`.** cyw43 0.7.0 turns a `None` here into `!0u16` and widens
    // it to the `u32` field, so the chip receives `nprobes = 65535` — where the
    // firmware's field is an `int32` whose "use the default" sentinel is `-1`.
    // The CYW43 rejects it and ends the scan in about a millisecond with zero
    // results, which is indistinguishable from "there are no networks here".
    // Measured on the bench: `None` finds 0 every time, `Some(2)` finds 36 in
    // 710 ms. Two probes per channel is the cyw43-driver default.
    options.nprobes = Some(2);

    let mut seen = heapless::Vec::<[u8; 6], SCAN_TABLE>::new();
    let mut target_visible = false;
    let sweep = async {
        let mut scanner = control.scan(options).await;
        // `Scanner::next` returns `None` both for "the scan is over" and for a
        // result whose payload was not a BSS description. cyw43 offers no way
        // to tell them apart, and treating the second as the end of the scan
        // only ever under-counts a diagnostic.
        while let Some(bss) = scanner.next().await {
            if !seen.contains(&bss.bssid) {
                let _ = seen.push(bss.bssid);
            }
            let length = (bss.ssid_len as usize).min(bss.ssid.len());
            if core::str::from_utf8(&bss.ssid[..length]) == Ok(ssid) {
                target_visible = true;
            }
        }
    };

    if with_timeout(SCAN_TIMEOUT, sweep).await.is_err() {
        defmt::warn!("wifi: scan timed out after {} networks", seen.len());
        return None;
    }
    defmt::info!(
        "wifi: scan complete, found {}, target visible {}",
        seen.len(),
        target_visible
    );
    Some(seen.len() as u16)
}

/// `start_ap_mode` (`main.py:561-582`): an **open** network named after the
/// device, on a fixed address, with the captive portal behind it.
///
/// Open is deliberate and load-bearing: the setup QR encodes
/// `WIFI:T:nopass;S:<name>;;`, and a phone that has to be handed a password to
/// reach the page that sets a password is a worse door than an open one on a
/// network that routes nowhere.
async fn start_ap(control: &mut Control<'static>, stack: Stack<'static>, ssid: &str) -> Ipv4Addr {
    control.start_ap_open(ssid, AP_CHANNEL).await;
    stack.set_config_v4(ConfigV4::Static(StaticConfigV4 {
        address: Ipv4Cidr::new(AP_IP, 24),
        // MicroPython's AP `ifconfig` lists itself as gateway and DNS. Nothing
        // reads either — `/api/status` reports `ifconfig()[0]` and no more —
        // and a default route pointing at our own address is a thing smoltcp
        // would have to resolve, so it stays `None`. What clients are *told*
        // is a separate matter, and `dhcp_server` tells them 192.168.4.1 for
        // both, which is what makes the captive portal work.
        gateway: None,
        dns_servers: heapless::Vec::new(),
    }));
    defmt::info!("wifi: ap up, ssid {=str}, ip {}", ssid, AP_IP);
    AP_IP
}

/// Publish one startup step with attempt dots.
fn step(
    store: &mut Store,
    publisher: &mut Publisher<'static>,
    number: u8,
    operation: &str,
    detail: &str,
    attempt: u8,
) {
    store.set_startup_step(
        number,
        STARTUP_STEPS,
        operation,
        detail,
        attempt,
        ATTEMPTS,
    );
    publisher.publish(store.snapshot());
}

/// Publish one startup step without them — `main.py` passes `attempt=0` for the
/// terminal steps, and zero hides the dots.
fn step_plain(
    store: &mut Store,
    publisher: &mut Publisher<'static>,
    number: u8,
    operation: &str,
    detail: &str,
) {
    store.set_startup_step(number, STARTUP_STEPS, operation, detail, 0, 0);
    publisher.publish(store.snapshot());
}

/// Longest address is `255.255.255.255`.
pub type AddressText = heapless::String<15>;

/// An IPv4 address as text, for the screen and for the Host-header check.
pub fn address_text(address: Ipv4Addr) -> AddressText {
    let mut text = AddressText::new();
    let [a, b, c, d] = address.octets();
    let _ = write!(text, "{a}.{b}.{c}.{d}");
    text
}

/// Truncate on a character boundary, so an SSID with a multi-byte character at
/// the limit does not panic the way `&s[..n]` would.
fn truncate(text: &str, limit: usize) -> &str {
    match text.char_indices().nth(limit) {
        Some((index, _)) => &text[..index],
        None => text,
    }
}

/// Station mode's six slots, from the socket table in the module docs. A future
/// consumer that pushes past [`super::SOCKETS`] fails the build here rather
/// than failing to bind at runtime, in AP mode, on a device with no serial port.
const _: () = assert!(super::SOCKETS >= 6, "station mode needs six socket slots");
