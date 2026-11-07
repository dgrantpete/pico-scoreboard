//! The onboard LED's vocabulary: five looks, which one the device shows, and
//! why there are five.
//!
//! The Pico 2 W's LED is a GPIO on the CYW43 radio, not on the RP2350, so the
//! firmware can drive it only once the radio firmware is up and only through
//! the radio's command path. That decides what the LED is *for*: not a boot
//! progress bar — the panel shows step 1 of 5 before the radio has finished
//! loading — but a second liveness channel that owes nothing to the panel,
//! core 1 or the network. It is read with the enclosure open, by whoever is
//! debugging, and the question it answers is "what is this device doing when
//! the panel cannot tell me".
//!
//! | Look | Meaning |
//! |---|---|
//! | off | no power, the bootloader swapping or reverting, or the app before the radio is up. The one look this module never chooses, which is what makes it mean something |
//! | solid | radio up, provisioning: scan, join, DHCP or starting the access point. Bounded; always ends in one of the rows below |
//! | fast blink | an update is downloading, verifying, or armed and counting down. Do not unplug |
//! | inverted heartbeat | a trial or rolled-back boot that has not yet earned its confirmation |
//! | slow blink | attention: setup mode, or the health gate is unhappy — a request left unanswered, or core 1 stalled. Ends in a phone or a reset |
//! | heartbeat | healthy per the supervisor: station mode, nothing in flight |
//!
//! # The rules the vocabulary follows
//!
//! - **State, not events.** Every look persists as long as its condition holds.
//!   A one-shot flash is missed by anyone not staring at the board.
//! - **The healthy look animates, at low duty.** A hung core leaves the LED
//!   wherever it was, and the radio's command path has no timeout, so a wedged
//!   radio freezes it too. A static LED must therefore never read as healthy;
//!   [`Pattern::Heartbeat`] is mostly off, so a freeze lands on "dark" or
//!   "stuck on", and both are correctly alarming. Solid is reserved for the one
//!   bounded phase in which the firmware cannot animate it at all.
//! - **Shape, not rate.** Nobody can tell 1 Hz from 1.5 Hz across a bench.
//!   The five looks differ in rhythm and duty; the test suite proves no two
//!   coincide over a common period.
//! - **Priority, not last-writer-wins.** Subsystems publish facts, as they
//!   already do for `/api/status`; [`Pattern::choose`] turns the facts into one
//!   look, most urgent first. Nothing writes the pin directly.
//! - **Off is rare, so off is informative.** The firmware never chooses it.
//!
//! What this crate holds is the pure half — the facts, the priority and the
//! waveforms — so it is host-tested like [`crate::poll::gate`]. Sampling the
//! facts and toggling the radio GPIO is the app's.

/// How often the owner samples the facts and, if the level changed, toggles
/// the LED. Every waveform below is a period measured in these ticks.
///
/// 100 ms is the coarsest tick at which a blip still reads as a blip, and it
/// bounds the radio traffic: the busiest look toggles once a tick, so the LED
/// never costs more than ten commands a second on the radio's shared path.
pub const TICK_MS: u64 = 100;

/// Heartbeat: two 100 ms blips at ticks 0 and 3, then quiet to the second.
/// 20 % duty — the Linux `heartbeat` trigger's pulse, rounded to the tick.
const HEARTBEAT_PERIOD: u32 = 10;
/// Slow blink: one second on, one second off.
const SLOW_PERIOD: u32 = 20;
/// Fast blink: 5 Hz, the "updating, do not unplug" of every router.
const FAST_PERIOD: u32 = 2;

/// What the LED knows, sampled once a tick by whoever owns it.
///
/// Four booleans rather than the subsystems' own enums, because this crate
/// knows nothing about OTA states or provisioning outcomes and should not:
/// the app decides which of its states count, in one place, and says so there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    /// An update is downloading, verifying, or armed and counting down.
    pub ota_busy: bool,
    /// This boot is a trial or a rollback that has not yet earned its
    /// confirmation.
    pub needs_confirm: bool,
    /// Provisioning ended in the access point.
    pub setup_mode: bool,
    /// The health gate would feed the watchdog right now.
    pub healthy: bool,
}

/// One of the five looks. The order of the variants is the priority
/// [`Pattern::choose`] applies, most urgent first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pattern {
    /// On. Provisioning — the firmware's only static look, and never chosen
    /// by [`Pattern::choose`], because by the time there are facts to choose
    /// from, provisioning is over.
    Solid,
    /// 5 Hz, 50 % duty.
    FastBlink,
    /// The heartbeat's complement: on, with two short dark blips a second.
    InvertedHeartbeat,
    /// 0.5 Hz, 50 % duty.
    SlowBlink,
    /// Two short bright blips a second, dark otherwise.
    Heartbeat,
}

impl Pattern {
    /// The look for these facts.
    ///
    /// An update outranks everything: it is the one state in which a human
    /// can do harm by acting. An unconfirmed trial outranks health, because
    /// "this image is on probation" is the fact a drill needs with the panel
    /// dark, and a trial that is also unhealthy reverts within seconds either
    /// way. Setup mode and an unhappy gate share a look: both mean "the
    /// network needs attention", and what follows tells them apart — setup
    /// mode persists until a phone or the retry, an unhappy gate ends in a
    /// reset within a few intervals.
    pub const fn choose(facts: Facts) -> Pattern {
        if facts.ota_busy {
            Pattern::FastBlink
        } else if facts.needs_confirm {
            Pattern::InvertedHeartbeat
        } else if facts.setup_mode || !facts.healthy {
            Pattern::SlowBlink
        } else {
            Pattern::Heartbeat
        }
    }

    /// The waveform's period, in ticks of [`TICK_MS`].
    pub const fn period(self) -> u32 {
        match self {
            Pattern::Solid => 1,
            Pattern::FastBlink => FAST_PERIOD,
            Pattern::InvertedHeartbeat | Pattern::Heartbeat => HEARTBEAT_PERIOD,
            Pattern::SlowBlink => SLOW_PERIOD,
        }
    }

    /// Whether the LED is lit at this tick. `tick` is the owner's running
    /// count; the waveform is its phase within [`Pattern::period`], so a
    /// change of look needs no phase reset and the count is free to wrap.
    pub const fn level(self, tick: u32) -> bool {
        let phase = tick % self.period();
        match self {
            Pattern::Solid => true,
            Pattern::FastBlink => phase == 0,
            Pattern::SlowBlink => phase < SLOW_PERIOD / 2,
            Pattern::Heartbeat => phase == 0 || phase == 3,
            Pattern::InvertedHeartbeat => !(phase == 0 || phase == 3),
        }
    }

    /// For the log line a change of look earns.
    pub const fn as_str(self) -> &'static str {
        match self {
            Pattern::Solid => "solid",
            Pattern::FastBlink => "fast blink",
            Pattern::InvertedHeartbeat => "inverted heartbeat",
            Pattern::SlowBlink => "slow blink",
            Pattern::Heartbeat => "heartbeat",
        }
    }
}
