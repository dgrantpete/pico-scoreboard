//! Minimal A/B trial-boot app for the RP2350 embassy-boot spike.
//!
//! One binary, three demo personalities via features (built by
//! `demo/build.py`):
//!
//! * `a_stager`  — identity A, `confirm`, `stage` (payload = B's image)
//! * `b_confirm` — identity B, `confirm`, `stage` (payload = the bad A)
//! * `a_bad`     — identity A, no `confirm`: on a trial boot it logs and then
//!   hangs without feeding the watchdog, so the bootloader-armed watchdog
//!   resets it and the bootloader rolls back.
//!
//! Boot flow: report identity and bootloader state; handle a trial boot
//! (confirm after a health gate, or play dead); then execute at most one
//! host command from the mailbox sector (staged by probe-rs, see
//! demo/README.md) and idle feeding the watchdog.

#![no_std]
#![no_main]

use core::cell::RefCell;

use defmt::{info, unwrap, warn};
use defmt_rtt as _;
use embassy_boot_rp::{AlignedBuffer, BlockingFirmwareUpdater, FirmwareUpdaterConfig, State};
use embassy_executor::Spawner;
use embassy_rp::flash::{Blocking, Flash};
use embassy_rp::watchdog::Watchdog;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time::{Duration, Timer};
use embedded_storage::nor_flash::NorFlash;
use panic_probe as _;
use spike_layout as layout;

#[cfg(all(feature = "identity-a", feature = "identity-b"))]
compile_error!("pick exactly one of identity-a / identity-b");
#[cfg(not(any(feature = "identity-a", feature = "identity-b")))]
compile_error!("pick exactly one of identity-a / identity-b");

#[cfg(feature = "identity-a")]
const IDENTITY: &str = "A";
#[cfg(feature = "identity-b")]
const IDENTITY: &str = "B";

/// The image this build can stage into the DFU partition, as a raw .bin
/// linked for the active partition (see demo/build.py for the build order
/// that breaks the A-embeds-B-embeds-A cycle).
#[cfg(feature = "stage")]
static PAYLOAD: &[u8] = include_bytes!(env!("SPIKE_PAYLOAD"));

/// Detached ed25519 signature over SHA-512(payload), made by demo/sign.py.
#[cfg(feature = "verify")]
static PAYLOAD_SIG: &[u8; 64] = include_bytes!(env!("SPIKE_PAYLOAD_SIG"));
/// The throwaway dev public key (committed; the private half is not).
#[cfg(feature = "verify")]
static PUBLIC_KEY: &[u8; 32] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../demo/keys/dev_pub.bin"));

const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(8);
/// Stand-in for the production health gate (wifi up / render loop alive):
/// how long a trial image must run before it earns mark_booted().
#[cfg(feature = "confirm")]
const HEALTH_GATE: Duration = Duration::from_secs(2);

type SpikeFlash = Flash<'static, embassy_rp::peripherals::FLASH, Blocking, { layout::FLASH_SIZE as usize }>;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // Before *anything* — defmt-rtt needs no init, so if this line never
    // shows, the executor/entry glue itself is at fault. The boot number is
    // the bootloader-maintained watchdog scratch0 counter; logging it keeps
    // each boot's log bytes unique for the demo poller's reinit detection.
    info!(
        "app {}: entry (boot #{})",
        IDENTITY,
        embassy_rp::pac::WATCHDOG.scratch0().read()
    );
    let p = embassy_rp::init(Default::default());
    info!("app {}: embassy_rp::init done", IDENTITY);

    Timer::after_millis(700).await;
    // The microsecond timestamp jitters between boots, keeping each boot's
    // log bytes unique for the demo poller's reinit detection (the boot
    // counter alone repeats after host-driven resets, which clear it).
    info!(
        "app {}: up at t={=u64:us} (confirm={} stage={} verify={})",
        IDENTITY,
        embassy_time::Instant::now().as_micros(),
        cfg!(feature = "confirm"),
        cfg!(feature = "stage"),
        cfg!(feature = "verify")
    );

    // The bootloader armed the watchdog before jumping here; every path
    // below must either take it over and feed it, or deliberately starve it.
    let mut watchdog = Watchdog::new(p.WATCHDOG);

    let flash = Flash::new_blocking(p.FLASH);
    let flash: Mutex<NoopRawMutex, RefCell<SpikeFlash>> = Mutex::new(RefCell::new(flash));
    let config = FirmwareUpdaterConfig::from_linkerfile_blocking(&flash, &flash);
    // embassy-rp flash has WRITE_SIZE = 1: the state "word" is one byte.
    let mut aligned = AlignedBuffer([0; 1]);
    let mut updater = BlockingFirmwareUpdater::new(config, &mut aligned.0);

    let state = unwrap!(updater.get_state());
    info!("app {}: bootloader state {}", IDENTITY, state);

    match state {
        State::Swap => {
            // Trial boot: the swap happened, nothing confirmed it yet. Any
            // reset before mark_booted() makes the bootloader revert.
            #[cfg(feature = "confirm")]
            {
                watchdog.start(WATCHDOG_TIMEOUT);
                info!(
                    "app {}: TRIAL boot — health gate {} ms, then mark_booted()",
                    IDENTITY,
                    HEALTH_GATE.as_millis()
                );
                Timer::after(HEALTH_GATE).await;
                watchdog.feed(WATCHDOG_TIMEOUT);
                unwrap!(updater.mark_booted());
                info!("app {}: mark_booted() written — this image is now permanent", IDENTITY);
            }
            #[cfg(not(feature = "confirm"))]
            {
                warn!(
                    "app {}: TRIAL boot — playing broken image: no mark_booted(), no watchdog \
                     feeding; expect watchdog reset then bootloader revert",
                    IDENTITY
                );
                loop {
                    cortex_m::asm::nop();
                }
            }
        }
        State::Revert => {
            watchdog.start(WATCHDOG_TIMEOUT);
            warn!(
                "app {}: bootloader state Revert — a trial image failed and was ROLLED BACK to this one",
                IDENTITY
            );
            // Same health gate, then clean Revert back to Boot so the next
            // OTA cycle starts from a confirmed state.
            #[cfg(feature = "confirm")]
            {
                Timer::after(HEALTH_GATE).await;
                watchdog.feed(WATCHDOG_TIMEOUT);
                unwrap!(updater.mark_booted());
                info!("app {}: state cleaned back to Boot", IDENTITY);
            }
        }
        _ => watchdog.start(WATCHDOG_TIMEOUT),
    }

    let cmd = read_command(&flash);
    if cmd != 0xFF {
        info!("app {}: mailbox command {=u8:#04x}", IDENTITY, cmd);
        // Erase before acting: single-shot even if what follows resets us,
        // and — more important — the image swapped in next must not find a
        // stale command and start ping-ponging stages.
        clear_command(&flash);
        run_command(cmd, &mut updater, &mut watchdog);
    }

    info!("app {}: idle, feeding watchdog", IDENTITY);
    loop {
        watchdog.feed(WATCHDOG_TIMEOUT);
        Timer::after_secs(1).await;
    }
}

fn read_command(flash: &Mutex<NoopRawMutex, RefCell<SpikeFlash>>) -> u8 {
    let mut byte = [0u8; 1];
    flash.lock(|f| unwrap!(f.borrow_mut().blocking_read(layout::CMD_OFFSET, &mut byte)));
    byte[0]
}

fn clear_command(flash: &Mutex<NoopRawMutex, RefCell<SpikeFlash>>) {
    flash.lock(|f| {
        unwrap!(
            f.borrow_mut()
                .blocking_erase(layout::CMD_OFFSET, layout::CMD_OFFSET + layout::ERASE_SIZE)
        )
    });
}

#[allow(unused_variables, unused_mut)]
fn run_command<DFU: NorFlash, STATE: NorFlash>(
    cmd: u8,
    updater: &mut BlockingFirmwareUpdater<DFU, STATE>,
    watchdog: &mut Watchdog,
) {
    match cmd {
        #[cfg(all(feature = "stage", not(feature = "verify")))]
        layout::CMD_STAGE => {
            stage_payload(updater, watchdog);
            unwrap!(updater.mark_updated());
            info!("app {}: mark_updated() — bootloader swaps on next boot; resetting", IDENTITY);
            reset_after_drain(watchdog);
        }
        #[cfg(feature = "verify")]
        layout::CMD_STAGE_VERIFIED | layout::CMD_STAGE_BAD_SIG => {
            stage_payload(updater, watchdog);
            let mut sig = *PAYLOAD_SIG;
            if cmd == layout::CMD_STAGE_BAD_SIG {
                warn!("app {}: corrupting signature byte 0 on purpose", IDENTITY);
                sig[0] ^= 0xFF;
            }
            // SHA-512 over the whole payload runs inside; buy it headroom.
            watchdog.feed(WATCHDOG_TIMEOUT);
            match updater.verify_and_mark_updated(PUBLIC_KEY, &sig, PAYLOAD.len() as u32) {
                Ok(()) => {
                    info!(
                        "app {}: signature VERIFIED + mark_updated() — bootloader swaps on next boot; resetting",
                        IDENTITY
                    );
                    reset_after_drain(watchdog);
                }
                Err(e) => {
                    warn!("app {}: signature REJECTED ({}) — not marked, no swap will happen", IDENTITY, e);
                }
            }
        }
        other => {
            warn!("app {}: command {=u8:#04x} not supported by this build; ignored", IDENTITY, other);
        }
    }
}

/// Self-reset, but only after the demo poller has had time to drain our RTT
/// ring: once the bootloader takes over, its header replaces ours at the
/// shared address and unread staging logs would be unobservable. Demo-only,
/// like every other delay in this crate.
#[cfg(feature = "stage")]
fn reset_after_drain(watchdog: &mut Watchdog) -> ! {
    watchdog.feed(WATCHDOG_TIMEOUT);
    cortex_m::asm::delay(375_000_000);
    cortex_m::peripheral::SCB::sys_reset()
}

/// Write the embedded payload into the DFU partition through the real
/// updater API, feeding the watchdog between sectors (staging erases and
/// programs for several seconds).
#[cfg(feature = "stage")]
fn stage_payload<DFU: NorFlash, STATE: NorFlash>(
    updater: &mut BlockingFirmwareUpdater<DFU, STATE>,
    watchdog: &mut Watchdog,
) {
    info!("app {}: staging payload ({} bytes) to DFU", IDENTITY, PAYLOAD.len());
    let mut offset = 0usize;
    for chunk in PAYLOAD.chunks(layout::ERASE_SIZE as usize) {
        watchdog.feed(WATCHDOG_TIMEOUT);
        unwrap!(updater.write_firmware(offset, chunk));
        offset += chunk.len();
    }
    info!("app {}: payload staged", IDENTITY);
}
