//! The onboard LED: a liveness channel that owes nothing to the panel, core 1
//! or the network.
//!
//! The vocabulary — five looks, what each means, and the rules that keep them
//! readable — is [`scoreboard_model::led`], host-tested there. This is the
//! wiring: one task that owns the radio's control handle, samples the facts
//! once a tick and toggles the pin when the waveform says to. It is read with
//! the enclosure open, by whoever is debugging, so there is no setting to turn
//! it off and nothing about it reaches the settings page.
//!
//! # It is a radio GPIO, and that decides most things
//!
//! On the Pico 2 W the LED is `WL_GPIO0`, a pin on the CYW43, so every toggle
//! is an IOCTL over the radio's SPI — `Control::gpio_set` — and the LED exists
//! only while the radio firmware is loaded, the `cyw43` runner is alive and
//! the executor is running. Four consequences, all deliberate:
//!
//! * **The bootloader cannot light it.** The radio firmware is 231 KB against
//!   a 32 KB boot partition, and WL_ON drops at every reset. The swap and
//!   revert windows — the 30 to 90 s of dark panel an update costs — are dark
//!   here too, and that is what "off" means: no power, or the bootloader.
//! * **No panic or fault handler can touch it.** Those paths are synchronous
//!   and reset within seconds; the trace of a crash is the LED going dark and
//!   restarting its boot sequence, and the record is the breadcrumb.
//! * **It cannot animate during provisioning.** `wifi::provision` holds the
//!   one `&mut Control` across its joins and scans, so [`show_provisioning`]
//!   sets it solid the moment the radio is up and leaves it there until the
//!   arm that won hands the handle to [`run`]. Solid is the only static look,
//!   and this bounded phase is its only use.
//! * **An IOCTL has no timeout.** A wedged radio stops this task at its next
//!   toggle, with the LED wherever it was. So this task holds nothing anyone
//!   else needs, is never an input to the watchdog gate, and the healthy look
//!   is mostly dark — a freeze lands on "off" or "stuck on", both of which
//!   read as trouble.
//!
//! # Facts, not decisions
//!
//! The look is derived each tick from what the subsystems already publish
//! for `/api/status`: the OTA state, whether this boot still owes the
//! bootloader a confirmation, which provisioning arm won, and the health gate.
//! The gate is [`scoreboard_model::poll::gate`] called with the same inputs the
//! feeder gives it — core 1's frame counter over a 2 s window and the poller's
//! health — rather than a verdict the feeder would have to publish. The two
//! agree by construction, and under `link-standalone` with the watchdog off,
//! where there is no feeder at all, the LED still shows health because it
//! computes it. Nothing anywhere writes the pin directly.

use core::sync::atomic::Ordering;

use cyw43::Control;
use embassy_time::{Duration, Ticker};
use scoreboard_model::led::{Facts, Pattern, TICK_MS};
use scoreboard_model::poll::gate;

use crate::display_core1::FRAME_SEQ;
use crate::ota::{self, State};
use crate::poller;

/// `WL_GPIO0`. The Pico W's LED is wired to the radio, not to the RP2350; the
/// RP2350's own GPIO 25, the LED on a plain Pico, is the radio's chip select.
const LED: u8 = 0;

/// How often `healthy` is re-evaluated, in ticks: every 2 s, which is the
/// feeder's cadence at the shipped 8 s timeout (`timeout / 4`), so "core 1
/// ticked" means the same thing to the LED as it does to the watchdog.
const HEALTH_EVERY: u32 = 2_000 / TICK_MS as u32;

/// Light the LED the moment the radio can: solid, for the whole of
/// provisioning. Called by `net::bringup` right after `Control::init`, the
/// first instant the pin exists.
pub async fn show_provisioning(control: &mut Control<'static>) {
    control.gpio_set(LED, true).await;
}

/// Own the radio's control handle for the life of the program and play the
/// look the facts call for.
///
/// `station` is which provisioning arm won. In setup mode there is no poller,
/// so the gate is asked about the render loop alone — the same `None` the
/// watchdog task receives — and the look is the attention blink regardless.
#[embassy_executor::task]
pub async fn run(mut control: Control<'static>, station: bool) -> ! {
    // Read once, as `net::arm_watchdog` reads it for the feeder: the two
    // should age together, and a config save does not re-arm either.
    let poll_interval_s =
        station.then(|| crate::config::with(|config| config.display.poll_interval_seconds));

    let mut ticker = Ticker::every(Duration::from_millis(TICK_MS));
    let mut tick: u32 = 0;
    let mut previous_frame = FRAME_SEQ.load(Ordering::Relaxed);
    // Until the first window closes, nothing has been shown to be wrong.
    let mut healthy = true;
    // What `show_provisioning` left, for the first transition line.
    let mut shown = Pattern::Solid;
    // `None` until this task has set the pin itself: it never assumes what
    // provisioning left behind.
    let mut level: Option<bool> = None;

    loop {
        ticker.next().await;
        tick = tick.wrapping_add(1);

        // The bench harness's IOCTL storm: the one way to put radio command
        // traffic on demand next to whatever the panel is doing.
        #[cfg(feature = "diag")]
        if let Some(storm) = crate::diag::take_led_storm() {
            crate::debug!(
                "led: diag storm, {} toggles {} ms apart",
                storm.toggles,
                storm.interval_ms
            );
            for _ in 0..storm.toggles {
                control.gpio_set(LED, true).await;
                embassy_time::Timer::after_millis(storm.interval_ms as u64).await;
                control.gpio_set(LED, false).await;
                embassy_time::Timer::after_millis(storm.interval_ms as u64).await;
            }
            level = Some(false);
        }

        if tick.is_multiple_of(HEALTH_EVERY) {
            let current = FRAME_SEQ.load(Ordering::Relaxed);
            let ticked = current != previous_frame;
            previous_frame = current;
            healthy = gate(ticked, poll_interval_s, &poller::health()).is_none();
        }

        // `Checking` is deliberately not busy: it is a manifest GET once a
        // day, and a one-second flicker at 5 Hz would teach the eye that the
        // fast blink means nothing.
        let (ota_state, _) = ota::status();
        let facts = Facts {
            ota_busy: matches!(
                ota_state,
                State::Downloading | State::Verifying | State::Restarting
            ),
            needs_confirm: ota::needs_confirm(),
            setup_mode: !station,
            healthy,
        };

        let pattern = Pattern::choose(facts);
        if pattern != shown {
            crate::debug!("led: {} -> {}", shown.as_str(), pattern.as_str());
            shown = pattern;
        }

        // One IOCTL per edge, not per tick: solid and the long halves of the
        // blinks cost the radio nothing.
        let wanted = pattern.level(tick);
        if level != Some(wanted) {
            control.gpio_set(LED, wanted).await;
            level = Some(wanted);
        }
    }
}
