//! embassy-boot bootloader for the RP2350 A/B spike.
//!
//! Boot-time flow (all decisions are embassy_boot's, we just report them):
//! read the state partition's magic → if a swap was requested, swap active
//! and DFU page by page (resumable via the progress array if power fails);
//! if a swap already ran but the app never confirmed with `mark_booted()`,
//! revert it — then jump to whatever the active partition now holds.
//!
//! The watchdog is armed *here* (8 s) and stays armed across the jump: an
//! app that wedges before feeding it gets reset, which is exactly what turns
//! an unconfirmed trial boot into an automatic rollback.

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
use spike_layout as layout;

const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(8);

#[entry]
fn main() -> ! {
    let p = embassy_rp::init(Default::default());

    // Boot counter in watchdog scratch0. Survives watchdog and SCB resets;
    // measured on the bench: probe-rs reset clears it, so it restarts at 1
    // after every host-driven reset — hence the extra nonce below.
    let boot_n = embassy_rp::pac::WATCHDOG.scratch0().read().wrapping_add(1);
    embassy_rp::pac::WATCHDOG.scratch0().write_value(boot_n);

    // Per-boot random nonce from the ring oscillator so every boot's log
    // bytes are unique even when the counter repeats: that uniqueness is
    // what lets the demo poller detect RTT re-initialization between two
    // otherwise identical boots.
    let mut nonce: u16 = 0;
    for _ in 0..16 {
        nonce = (nonce << 1) | embassy_rp::pac::ROSC.randombit().read().randombit() as u16;
    }

    info!(
        "spike-boot: boot #{} nonce {=u16:#06x}, arming watchdog ({} s) and reading state",
        boot_n,
        nonce,
        WATCHDOG_TIMEOUT.as_secs()
    );

    // ~2.5 s at 150 MHz between banner and first flash access. Two jobs: the
    // demo's SWD poller needs a couple of ticks inside every bootloader
    // phase so the banner is captured before the app's first log
    // re-initializes the RTT header, and the upstream RP2040 bootloader
    // example warns that touching flash too early after boot can hard-fault
    // with a debugger attached. Production trims this to ~100 ms.
    cortex_m::asm::delay(375_000_000);

    let flash = WatchdogFlash::<{ layout::FLASH_SIZE as usize }>::start(p.FLASH, p.WATCHDOG, WATCHDOG_TIMEOUT);
    let flash = Mutex::new(RefCell::new(flash));

    let config = BootLoaderConfig::from_linkerfile_blocking(&flash, &flash, &flash);
    let active_offset = config.active.offset();

    // Swap/revert (if any) happens inside prepare(); with DEFMT_LOG routing
    // embassy_boot at trace, its "Swapping"/"Reverting" lines land in RTT.
    let bl: BootLoader = BootLoader::prepare(config);

    info!(
        "spike-boot: state {}, jumping to active @ {=u32:#010x}",
        bl.state,
        layout::FLASH_BASE + active_offset
    );
    // Same reasoning as the banner delay: give the poller a beat to read the
    // decision line before the app's first log takes over the RTT header.
    cortex_m::asm::delay(300_000_000);
    unsafe { bl.load(layout::FLASH_BASE + active_offset) }
}
