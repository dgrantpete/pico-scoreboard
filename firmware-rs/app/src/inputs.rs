//! The two buttons: PIO1, two state machines, and a 50 ms drain.
//!
//! `main.py`'s `init_buttons` and `button_input_loop`. Everything that decides
//! what a FIFO word *means* — the debounce program itself, the timestamp
//! reconstruction, the short/long fold, and the routing through the league menu
//! — is [`scoreboard_input`], where it is host-tested against
//! `tools/pio_sim.py`'s scenarios. What is here is the hardware: a PIO block,
//! two GPIOs, and the loop that empties the FIFOs.
//!
//! | | GPIO | state machine | closed-menu action |
//! |---|---|---|---|
//! | A | 10 | PIO1 SM0 | short: next game · long: next league |
//! | B | 22 | PIO1 SM1 | short: rotation lock · long: open the menu |
//!
//! PIO1 because PIO0 belongs to `hub75` and PIO2 to the radio — the resource
//! map is in [`crate::net`]'s module docs, and this is the last unclaimed block.
//!
//! # The program is loaded once and both machines run it
//!
//! 26 instructions of a 32-word instruction memory, so a *second* program would
//! not fit — which is fine, because the two buttons differ only in which GPIO
//! they watch, and that is per-machine configuration (`jmp_pin` and `in_base`),
//! not per-program. `button.py` arranged it the same way and for the same
//! reason.
//!
//! # Where the presses go
//!
//! Into [`crate::poller`]'s command channel, and nowhere else. The poller owns
//! the `Store`, the `Slate` and the skip machine — the arm/reject decision for a
//! press is about *its* state and `poller.py` made it in the same place — so
//! this task decodes and forwards, and never touches display state.
//!
//! The cost of that, stated plainly: a press lands between the poller's ticks
//! rather than the instant it arrives. Most of the time the poller is asleep on
//! the poll interval and the command wakes it immediately; when it is inside a
//! request the press waits for that request, which is bench-measured at
//! 60-300 ms and bounded by the 15 s request timeout. PARITY.md records it.
//!
//! # Init failure was non-fatal, and now cannot happen
//!
//! `init_buttons` wrapped everything in a `try` and returned `(None, None)` —
//! buttons are an enhancement and must never block a boot. Nothing in this
//! version can fail: `Pio::new` takes the block by ownership rather than looking
//! one up, and the program's fit is a compile-time fact (asserted by
//! `scoreboard_input`'s own test). The *supported* state that remains is a
//! device with **no buttons physically attached**, which is what the bench unit
//! is: the pins idle high on their pull-ups, the state machines run, and no
//! event is ever pushed. Nothing logs, because nothing is wrong.

use embassy_rp::gpio::{Input, Pull};
use embassy_rp::peripherals::{PIN_10, PIN_22, PIO1};
use embassy_rp::pio::{
    Config, Direction, InterruptHandler, Pio, ShiftConfig, ShiftDirection, StateMachine,
};
use embassy_rp::{Peri, bind_interrupts};
use embassy_time::{Duration, Instant, Ticker};
use fixed::traits::ToFixed;
use scoreboard_input::button::{
    DEBOUNCE_MS, EventDecoder, POLL_PERIOD_MS, PressTracker, TICK_PERIOD_MS, debounce_reload,
    frequency_hz, program,
};
use scoreboard_input::menu::Button;

use crate::poller::{self, Command};

// The program never raises an IRQ — it communicates entirely through the RX
// FIFO — but `Pio::new` requires the binding regardless, because constructing
// the block is what installs the handler that would service one.
bind_interrupts!(struct Irqs {
    PIO1_IRQ_0 => InterruptHandler<PIO1>;
});

/// The buttons' silicon, taken from `Peripherals` in `main` so the resource map
/// stays decided in one place.
pub struct InputPeripherals {
    pub pio: Peri<'static, PIO1>,
    /// Button A — skip. Active low, internal pull-up.
    pub a: Peri<'static, PIN_10>,
    /// Button B — lock. Active low, internal pull-up.
    pub b: Peri<'static, PIN_22>,
}

/// Buttons are wired to ground through a switch, so a pressed button reads LOW.
const ACTIVE_LOW: bool = true;

/// Drain both buttons every [`POLL_PERIOD_MS`] and forward what they said.
#[embassy_executor::task]
pub async fn run(mut p: InputPeripherals, system_clock_hz: u32) -> ! {
    let Pio {
        mut common,
        mut sm0,
        mut sm1,
        ..
    } = Pio::new(p.pio, Irqs);

    // Sampled through a plain input before the pins become PIO pins, so the
    // fold starts from the boundary condition `Button.__init__` captured: the
    // real level, and the time it was read. The PIO converges on the same state
    // within about two iterations without pushing anything, so the seed and the
    // event stream agree without being synchronised.
    let a_pressed = Input::new(p.a.reborrow(), Pull::Up).is_low();
    let b_pressed = Input::new(p.b.reborrow(), Pull::Up).is_low();
    let seeded_at = Instant::now().as_millis();

    let mut a_pin = common.make_pio_pin(p.a);
    let mut b_pin = common.make_pio_pin(p.b);
    a_pin.set_pull(Pull::Up);
    b_pin.set_pull(Pull::Up);

    // One load, two machines. See the module docs.
    let loaded = common.load_program(&program());
    let reload = defmt::unwrap!(debounce_reload(TICK_PERIOD_MS, DEBOUNCE_MS).ok());
    let divider = system_clock_hz as f64 / frequency_hz(TICK_PERIOD_MS) as f64;

    let configure = |config: &mut Config<'static, PIO1>, pin: &_| {
        config.use_program(&loaded, &[]);
        config.clock_divider = divider.to_fixed();
        // Left, because the program packs the duration into the ISR's high bits
        // and the state bit last. Autopush off: every push in the program is
        // explicit, and an autopush at 32 bits would fire a word early.
        config.shift_in = ShiftConfig {
            auto_fill: false,
            threshold: 32,
            direction: ShiftDirection::Left,
        };
        config.set_in_pins(&[pin]);
        config.set_jmp_pin(pin);
    };

    let mut a_config = Config::default();
    configure(&mut a_config, &a_pin);
    let mut b_config = Config::default();
    configure(&mut b_config, &b_pin);

    sm0.set_config(&a_config);
    sm1.set_config(&b_config);
    sm0.set_pin_dirs(Direction::In, &[&a_pin]);
    sm1.set_pin_dirs(Direction::In, &[&b_pin]);
    // The blocking `pull` at the top of the program is waiting for this. Seeded
    // before the machines start, which the TX FIFO holds either way.
    sm0.tx().push(reload);
    sm1.tx().push(reload);
    sm0.set_enable(true);
    sm1.set_enable(true);

    crate::debug!(
        "input: buttons up on PIO1 — A=skip(GPIO10) B=lock(GPIO22), {} ms debounce",
        DEBOUNCE_MS
    );

    let mut a = ButtonState::new(a_pressed, seeded_at, "A");
    let mut b = ButtonState::new(b_pressed, seeded_at, "B");

    let mut ticker = Ticker::every(Duration::from_millis(POLL_PERIOD_MS as u64));
    loop {
        ticker.next().await;
        let now = Instant::now().as_millis();
        a.drain(&mut sm0, Button::A, now);
        b.drain(&mut sm1, Button::B, now);
    }
}

/// One button's fold state.
struct ButtonState {
    decoder: EventDecoder,
    tracker: PressTracker,
    /// `"A"` or `"B"`, for the log line. `_PressTracker` carried the same.
    name: &'static str,
}

impl ButtonState {
    fn new(pressed: bool, at_ms: u64, name: &'static str) -> ButtonState {
        ButtonState {
            decoder: EventDecoder::new(pressed, at_ms, TICK_PERIOD_MS, ACTIVE_LOW),
            tracker: PressTracker::new(pressed),
            name,
        }
    }

    /// Empty one FIFO, fold what came out, and check the hold threshold.
    ///
    /// The threshold check is not optional and cannot move into the decoder:
    /// the PIO emits nothing at all while a button is steadily held, so a long
    /// press has no event to hang off and is found by asking the clock.
    fn drain<const SM: usize>(
        &mut self,
        machine: &mut StateMachine<'static, PIO1, SM>,
        button: Button,
        now_ms: u64,
    ) {
        while let Some(word) = machine.rx().try_pull() {
            let Some(event) = self.decoder.decode(word) else {
                continue;
            };
            if let Some(press) = self.tracker.event(event) {
                self.send(button, press);
            }
        }
        if let Some(press) = self.tracker.poll(now_ms) {
            self.send(button, press);
        }
    }

    fn send(&self, button: Button, press: scoreboard_input::button::Press) {
        crate::debug!(
            "input: button {} {} press",
            self.name,
            match press {
                scoreboard_input::button::Press::Short => "short",
                scoreboard_input::button::Press::Long => "long",
            }
        );
        poller::command(Command::Press(button, press));
    }
}
