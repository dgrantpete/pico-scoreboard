//! `GET /api/status` — the three shapes, and what replaced the GC numbers.
//!
//! Port of `main.get_network_status` (`:390-450`). The network half is a
//! faithful copy: three shapes keyed on `mode` (`ap` / `station` / `unknown`),
//! every field present in all three so the SPA never has to test for a key, and
//! `configured_ssid` populated **only** when the setup reason is a failure —
//! `connection_failed` or `bad_auth` — because "here is the network you asked
//! for and it did not work" is useful and "here is the network you never
//! configured" is noise.
//!
//! # The GC numbers, and what they become
//!
//! MicroPython reported `memory_used` / `memory_free` from `gc.mem_alloc()` and
//! `gc.mem_free()`, and `flash_used` / `flash_free` from `os.statvfs('/')`.
//! Deliberately without collecting first, so observing memory did not change
//! behaviour — which meant the numbers sawtoothed, and reading them told you
//! how much garbage had accumulated since the last collection rather than how
//! much memory the firmware needed.
//!
//! None of that survives the port, because none of it exists: there is no
//! allocator (SPEC §10), no garbage, and no filesystem. But the *question* the
//! numbers were there to answer does survive — **is this device running out of
//! room, and where?** — and on this firmware it has a better answer than it
//! ever had on MicroPython, because the two things that can actually exhaust
//! are both measurable exactly:
//!
//! | MicroPython | Here | What it now means |
//! |---|---|---|
//! | `memory_used` — heap in use, sawtoothing | Statically allocated RAM | Everything `.data` and `.bss` claim. A **constant** for a given image: this is the number BUDGET.md tracks, read off the running device rather than the ELF. |
//! | `memory_free` — heap free, sawtoothing | RAM left for the stacks | What is not static. With flip-link, core 0's stack grows down through exactly this space. |
//! | `flash_used` | Image size | The bytes this image occupies in its partition — and therefore what an OTA has to move. |
//! | `flash_free` | Partition remainder | How much an image can still grow before it stops fitting. |
//!
//! and four fields that have no MicroPython counterpart at all, because they
//! measure the thing that replaced the heap as the way this firmware runs out
//! of room — **the stacks**:
//!
//! - `core0_stack_used` / `core0_stack_total`
//! - `core1_stack_used` / `core1_stack_total`
//!
//! Both are **high-water marks**, not instantaneous depths: the region is
//! painted with a sentinel before the core starts and the deepest byte ever
//! touched is the first one that is no longer painted. So the number answers
//! "how close has this device *ever* come to overflowing", which is the
//! question, rather than "how deep is it right now", which is noise. A stack
//! overflow on either core is a hard fault by construction (MSPLIM on both);
//! these fields are how you find out you are heading for one before it happens.
//! [`crate::supervise`] measures them on a 10 s tick and this route reads the
//! result, so a request costs nothing and cannot itself perturb what it reports
//! — the one property `get_memory_stats`'s comment was most careful about.
//!
//! Two more replace the third thing a MicroPython device could run out of, log
//! space, which was a flash file and is now the RAM ring: `log_entries` (how
//! many the ring is holding) and `log_latest_seq` (the newest sequence number,
//! which is also the cursor a client would resume from).

use serde::Serialize;

use crate::net::status::NetStatus;
use crate::{ringlog, supervise};

/// The version string `main.py` reported from `ota.current_version()`.
///
/// That was the sha256 of the deployed ROMFS bundle, or `None` on a dev
/// littlefs deploy. Here it is the version `build.rs` stamped in — `"dev"`
/// unless `publish-fw` built the image — plus the profile it was linked for.
///
/// **The SPA compares this before and after an update** to decide whether one
/// landed (`StatusCard.svelte` polls until it changes), so it has to be the
/// thing that actually changes across an install. The link profile rides along
/// because a probe-flashed image and an OTA'd one are otherwise indistinguishable
/// from the settings page.
pub const APP_VERSION: &str = concat!(env!("FW_VERSION"), "+", env!("LINK_PROFILE"));

/// The response body. Field order is the response's; the SPA reads by name.
#[derive(Debug, Serialize)]
pub struct Status {
    pub mode: &'static str,
    pub connected: bool,
    pub setup_mode: bool,
    pub setup_reason: Option<&'static str>,
    pub configured_ssid: Option<heapless::String<32>>,
    pub ip: Option<heapless::String<15>>,
    pub hostname: Option<heapless::String<39>>,
    /// What the OTA client is doing: `idle`, `checking`, `downloading`,
    /// `verifying`, `restarting`, `trial` or `rolled_back`.
    ///
    /// The last two are the ones worth having on a deployed unit. `trial` means
    /// this boot has not yet earned `mark_booted` and a reset would roll it
    /// back; `rolled_back` means one already did. Neither is visible anywhere
    /// else once the ring log has wrapped.
    pub ota_state: &'static str,
    /// Download progress, 0..=100. Meaningful only while `ota_state` is
    /// `downloading`; the settings page shows the panel's own bar rather than
    /// this, and it is here so a browser can follow an update it cannot see.
    pub ota_progress: u8,
    pub ap_ip: Option<heapless::String<15>>,
    pub ap_ssid: Option<heapless::String<32>>,

    // The four legacy keys, redefined. See the module docs.
    pub memory_used: u32,
    pub memory_free: u32,
    pub flash_used: u32,
    pub flash_free: u32,

    // The Rust-specific readouts.
    pub core0_stack_used: u32,
    pub core0_stack_total: u32,
    pub core1_stack_used: u32,
    pub core1_stack_total: u32,
    pub log_entries: u32,
    pub log_latest_seq: u32,

    pub app_version: &'static str,
}

impl Status {
    /// Read the current status.
    pub fn read() -> Status {
        let memory = supervise::memory();
        let (log_entries, log_latest_seq) = ringlog::stats();
        let (ota_state, ota_progress) = crate::ota::status();

        let mut status = Status {
            mode: "unknown",
            connected: false,
            setup_mode: false,
            setup_reason: None,
            configured_ssid: None,
            ip: None,
            hostname: None,
            ap_ip: None,
            ap_ssid: None,
            memory_used: memory.static_ram,
            memory_free: memory.ram_free,
            flash_used: memory.image_bytes,
            flash_free: memory.partition_free,
            core0_stack_used: memory.core0_stack_used,
            core0_stack_total: memory.core0_stack_total,
            core1_stack_used: memory.core1_stack_used,
            core1_stack_total: memory.core1_stack_total,
            log_entries,
            log_latest_seq,
            app_version: APP_VERSION,
            ota_state: ota_state.as_str(),
            ota_progress,
        };

        match crate::net::status::read() {
            // `mode: "unknown"` is reachable for a real window: the server does
            // not start until provisioning finishes, but a reset-network call
            // clears the published status underneath a live connection.
            None => status,
            Some(NetStatus::Station {
                ip,
                device_name,
                configured_ssid,
            }) => {
                status.mode = "station";
                status.connected = true;
                status.ip = Some(ip);
                // `<name>.local` — the name DHCP option 12 registered with the
                // router, which is how the SPA offers a link that survives the
                // address changing.
                let mut hostname = heapless::String::new();
                let _ = hostname.push_str(&device_name);
                let _ = hostname.push_str(".local");
                status.hostname = Some(hostname);
                let _ = configured_ssid;
                status
            }
            Some(NetStatus::Ap {
                reason,
                ap_ip,
                ap_ssid,
                configured_ssid,
            }) => {
                status.mode = "ap";
                status.setup_mode = true;
                status.setup_reason = Some(crate::net::reason_name(reason));
                // Only for the two failure reasons. `no_network_configured`
                // means there is nothing to report and reporting an empty
                // string would render as a blank "tried to join:" line.
                status.configured_ssid = matches!(
                    reason,
                    scoreboard_model::SetupReason::ConnectionFailed
                        | scoreboard_model::SetupReason::BadAuth
                )
                .then_some(configured_ssid);
                status.ap_ip = Some(ap_ip);
                status.ap_ssid = Some(ap_ssid);
                status
            }
        }
    }

    /// Serialize into a caller-owned buffer.
    pub fn to_json(&self, out: &mut [u8]) -> Result<usize, ()> {
        serde_json_core::to_slice(self, out).map_err(|_| ())
    }
}
