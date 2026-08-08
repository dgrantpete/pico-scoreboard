//! The scoreboard's bootloader: swap, revert, jump.
//!
//! SPEC §8, productionized from `firmware-rs/boot-spike/boot`, which
//! demonstrated every path below on this hardware on 2026-08-07
//! (`boot-spike/PARTITIONS.md`). It is deliberately the most boring binary in
//! the repository, and there are only three things to know about it.
//!
//! # It makes no decisions
//!
//! Every choice is `embassy_boot`'s. It reads the state partition's magic; if a
//! swap was requested it swaps active and DFU page by page, resuming from the
//! progress array if power failed mid-swap; if a swap already ran and the app
//! never confirmed with `mark_booted()`, it reverts. Then it jumps into
//! whatever the active partition now holds. This file supplies the partition
//! addresses and a log line.
//!
//! # It arms a watchdog that the app cannot turn off
//!
//! `WatchdogFlash` starts an 8 s hardware watchdog before the first flash
//! access and it **stays armed across the jump**. That is the entire automatic
//! rollback mechanism: a trial image that wedges before confirming never feeds
//! it, gets reset, and the bootloader reverts on the way back up.
//!
//! The consequence for the app is not optional and is easy to get wrong: under
//! this bootloader an RP2350 watchdog is already counting when the app's first
//! instruction runs, and an armed watchdog cannot be disarmed. So the
//! boot-integrated app **always** runs a feeder, whatever `watchdog.enabled`
//! says — that flag survives as "may the health gate deliberately starve it",
//! which is a different question. `app/src/supervise.rs` carries the same note
//! from the other side.
//!
//! [`WATCHDOG_TIMEOUT`] is therefore also a hard budget on the app's boot: every
//! blocking step between reset and the first feed has to fit inside it. The one
//! that nearly did not is the storage region's one-time erase, which is why
//! `app/src/storage.rs` erases sector by sector and feeds between them instead
//! of calling `erase_all`.
//!
//! # It is never updated over the air
//!
//! Nothing in the OTA path writes the boot partition, and that is a design
//! decision rather than an omission: an interrupted bootloader write is the one
//! failure A/B cannot recover from, because the thing that would recover it is
//! what was being written. Changing this binary means a physical flash — USB or
//! probe. `tools/build.py publish-fw` refuses to put it in an artifact.

#![no_std]
#![no_main]

use core::cell::RefCell;

use cortex_m_rt::entry;
use defmt::info;
use defmt_rtt as _;
use embassy_boot_rp::{BootLoader, BootLoaderConfig, WatchdogFlash};
use embassy_sync::blocking_mutex::Mutex;
use embassy_time::Duration;
use panic_probe as _;
use scoreboard_layout as layout;

/// How long the app has, from this bootloader's jump, to feed the watchdog.
///
/// 8 s, the spike's value, and just under the RP2350's 8.388 s ceiling (the
/// counter is 24 bits of microseconds, halved). There is no headroom to buy by
/// raising it — this is the maximum the silicon offers.
const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(8);

/// Settling time before the first flash access.
///
/// The upstream embassy bootloader example warns that touching flash too early
/// after reset can hard-fault **with a debugger attached**, which is exactly the
/// configuration every drill runs in. The spike used 2.5 s so its SWD poller
/// could capture each phase; nothing here is being watched that closely, so this
/// is the ~100 ms that note said production should trim to. At 150 MHz that is
/// 15,000,000 cycles.
const SETTLE_CYCLES: u32 = 15_000_000;

#[entry]
fn main() -> ! {
    let p = embassy_rp::init(Default::default());
    info!(
        "boot: arming the watchdog ({} s) and reading the state partition",
        WATCHDOG_TIMEOUT.as_secs()
    );
    cortex_m::asm::delay(SETTLE_CYCLES);

    let flash = WatchdogFlash::<{ layout::FLASH_SIZE as usize }>::start(
        p.FLASH,
        p.WATCHDOG,
        WATCHDOG_TIMEOUT,
    );
    let flash = Mutex::new(RefCell::new(flash));

    // The three partitions, from the `__bootloader_*` symbols `build.rs`
    // generated out of the layout crate — so this binary and the app cannot
    // disagree about where anything is.
    let config = BootLoaderConfig::from_linkerfile_blocking(&flash, &flash, &flash);
    let active_offset = config.active.offset();

    // The swap or revert, if there is one, happens inside `prepare`. With
    // `embassy_boot` at trace (see .cargo/config.toml) its "Swapping" and
    // "Reverting" lines land in RTT, which is the only direct evidence of what
    // this boot chose.
    let bl: BootLoader = BootLoader::prepare(config);

    info!(
        "boot: state {}, jumping to the active partition at {=u32:#010x}",
        bl.state,
        layout::FLASH_BASE + active_offset
    );

    // SAFETY: the address is the active partition's, taken from the config the
    // bootloader just prepared, and `prepare` has left that partition holding a
    // complete image. Everything after this point is the app's.
    unsafe { bl.load(layout::FLASH_BASE + active_offset) }
}
