//! The debounced button: a PIO program, a FIFO word, and two folds over it.
//!
//! Port of `lib/button.py` plus `main.py`'s `_PressTracker`. The PIO half is
//! transcribed instruction for instruction — see [`PROGRAM`] — and the CPU half
//! is [`EventDecoder`] (what `Button.read()` did) and [`PressTracker`].
//!
//! # The contract, unchanged from `button.py`
//!
//! A state machine watches one GPIO continuously and pushes a 32-bit word on
//! every *accepted* edge. An edge is accepted only if the previous state was
//! held for the full debounce window, so:
//!
//! * an accepted edge fires on the very sample it is seen — **zero added
//!   latency** — while the bounce tail after it is rejected, because every
//!   rejected crossing reloads the debounce counter; and
//! * a press or release shorter than the window is **swallowed**, and surfaces
//!   as two consecutive events with the same `pressed` value. Consumers must
//!   not assume alternation. [`PressTracker`] is written against that: it
//!   compares each event to the previous *debounced* state and a same-state
//!   pair produces no edge at all.
//!
//! # Time comes out of the PIO, not off the clock
//!
//! [`EventDecoder::decode`] never reads a clock. Each word carries the duration
//! of the *previous* state in ticks, and the decoder advances a private anchor
//! by that duration — so the timestamps of a burst of events are spaced by what
//! the PIO measured, not by when the CPU got round to draining the FIFO. That
//! is what makes an 800 ms hold threshold mean 800 ms of hold rather than
//! 800 ms of "hold, plus however long the poll loop was busy".
//!
//! The one place a clock is unavoidable is the long press, which fires
//! *mid-hold* — the PIO emits nothing while a button is steadily held, so
//! [`PressTracker::poll`] is given the time and compares. That is `main.py`'s
//! arrangement exactly.
//!
//! The MicroPython version did this arithmetic in `time.ticks_ms` algebra,
//! because its clock is a 30-bit counter that wraps every ~6.2 days. Embassy's
//! `Instant` is 64-bit microseconds since boot, so the milliseconds here are a
//! plain `u64` and the wrap rules are not ported — there is nothing to wrap.
//!
//! # The rollover marker
//!
//! After ~24.8 days of one unbroken state the PIO's counter wraps and pushes a
//! same-state event whose duration field decodes to 0. That signature is unique
//! because [`debounce_reload`] refuses anything below 2, which guarantees a real
//! event spans at least one tick. [`EventDecoder::decode`] filters it — and
//! still advances the anchor by its (zero) duration, so the arithmetic stays
//! identical whether or not the marker was there.

use pio::{Program, pio_asm};

/// SM cycles per FIFO duration tick.
///
/// One stable-loop iteration is padded to exactly half this — 16 cycles, via
/// `[3]` on each of the four instructions on every loop path — and the FIFO word
/// drops the counter's LSB, so one reported tick is two iterations. Change the
/// padding and this changes with it.
pub const PIO_CYCLES_PER_TICK: u32 = 32;

/// What one FIFO duration tick means in real time. `button.py`'s default.
///
/// Must stay in `[1, 16]`: the state-machine clock is 32 kHz / this, and the
/// RP2350's divider bottoms out near 1.9 kHz.
pub const TICK_PERIOD_MS: u32 = 1;

/// `button.py`'s default debounce window, and what both buttons use.
pub const DEBOUNCE_MS: u32 = 20;

/// Hold this long for the league-level action. `main.py`'s `_LONG_PRESS_MS`.
pub const LONG_PRESS_MS: u64 = 800;

/// How often the input loop drains the FIFOs. `main.py`'s `button_input_loop`.
///
/// The RX FIFO holds four events — two full press/release cycles — and the PIO
/// *blocks* when it is full rather than dropping events, but blocked time is
/// uncounted and skews every subsequent timestamp. `button.py` states the
/// requirement as "at least every ~4× debounce_ms", which is 80 ms; 50 ms is
/// well inside it and is also the frame period, so a press can never be more
/// than one frame stale.
pub const POLL_PERIOD_MS: u32 = 50;

/// Bit 31 of a FIFO word: the raw pin level, 1 = HIGH.
///
/// The PIO is polarity-agnostic — it reports the pin, and `active_low` is
/// applied on this side. That split is `button.py`'s and it is why one loaded
/// program serves a pull-up button and a pull-down one without a recompile.
const FIFO_STATE_BIT: u32 = 0x8000_0000;
/// Bits 30..0: how long the *previous* state lasted, in ticks.
const FIFO_DURATION_MASK: u32 = 0x7FFF_FFFF;

/// The debounce program, assembled.
///
/// Instruction for instruction from `button.py:38-163`, with the two
/// `saturating_decrement` macro expansions written out. Every label name is the
/// Python one except the four the macro generated anonymously, which are named
/// here for the state they belong to.
///
/// **The constant-time saturating decrement is the whole trick.** Both paths of
/// `max(y - 1, 0)` are exactly two instructions: the nonzero path is
/// `jmp !y` (not taken) then `jmp y--` (taken), and the zero path is `jmp !y`
/// (taken) landing on a `nop` that falls through to the same join. Without the
/// `nop` shim the zero path is one instruction shorter, and because the first
/// `jmp` runs either way no delay assignment can equalise them — `button.py`
/// records that asymmetry as the original timing bug. The cost of getting it
/// wrong is a tick that means two different amounts of real time depending on
/// whether the debounce counter had bottomed out, which decodes as durations
/// that drift.
///
/// The other subtlety is the *packing*. `x` counts **down** from all-ones, so
/// complementing it turns it into elapsed iterations; reversing the bits before
/// `in x, 31` and un-reversing the ISR afterwards is how the counter's LSB is
/// dropped without a shift instruction — which is what makes one reported tick
/// two iterations and keeps the wrap at 2³² iterations continuous instead of
/// discontinuous.
pub fn program() -> Program<32> {
    pio_asm!(
        // Seeded by the CPU before the state machine starts; see
        // `debounce_reload`. Outside the wrap, so it runs exactly once.
        "    pull block",
        ".wrap_target",
        // Shared by first entry and by every post-push return: x is the
        // duration counter and is reset to all-ones here.
        "    mov x, ~null",
        // y is still all-ones or all-zeroes; this rejoins the loop the previous
        // state was in with the debounce count reset. y can never be 0 after
        // this decrement, so the `!y` tests below cannot mis-fire.
        "    jmp y--, high_edge",
        "low_edge:",
        "    jmp !y, transition_low",
        "    mov y, osr",
        "low_stable:",
        "    jmp pin, high_edge      [3]",
        "    jmp !y, low_saturated   [3]",
        "    jmp y--, low_counted    [3]",
        "low_saturated:",
        "    nop                     [3]",
        "low_counted:",
        "    jmp x--, low_stable     [3]",
        // Reached only when x wrapped: 2^32 iterations of one unbroken state.
        "    jmp transition_low",
        "high_edge:",
        "    jmp !y, transition_high",
        "    mov y, osr",
        "high_stable:",
        "    jmp pin, high_held      [3]",
        "    jmp low_edge",
        "high_held:",
        "    jmp !y, high_saturated  [3]",
        "    jmp y--, high_counted   [3]",
        "high_saturated:",
        "    nop                     [3]",
        "high_counted:",
        "    jmp x--, high_stable    [3]",
        // Falls through from the wrapped counter above, which saves a jmp.
        "transition_high:",
        // y is guaranteed 0 on every path that reaches here, so this makes it
        // all-ones — the state bit, below.
        "    mov y, ~y",
        "transition_low:",
        "    mov x, ~x",
        "    mov x, ::x",
        "    in x, 31",
        "    in y, 1",
        "    mov isr, ::isr",
        "    push",
        ".wrap",
    )
    .program
}

/// The state-machine clock for a given tick period, derived from the
/// loop-cycles-per-tick contract.
///
/// 32 kHz at the 1 ms default. Well above the RP2350's ~1.9 kHz floor, and at
/// 150 MHz the divider lands on 4687.5 — exactly representable in the
/// hardware's 16.8 fixed point, so a tick is a tick and not a tick and a bit.
pub const fn frequency_hz(tick_period_ms: u32) -> u32 {
    1_000 * PIO_CYCLES_PER_TICK / tick_period_ms
}

/// The debounce reload the CPU seeds into the OSR, in loop iterations.
///
/// Iterations, not ticks: the counter decrements once per iteration and there
/// are two per tick, so the millisecond figure doubles. The floor of 2 — one
/// full tick — is load-bearing rather than defensive: below it a real event's
/// duration could decode to 0 and be mistaken for the rollover marker
/// [`EventDecoder::decode`] filters on.
pub const fn debounce_reload(tick_period_ms: u32, debounce_ms: u32) -> Result<u32, ConfigError> {
    if tick_period_ms < 1 {
        return Err(ConfigError::TickPeriod);
    }
    let reload = (2 * debounce_ms) / tick_period_ms;
    if reload < 2 || reload >= (1 << 30) {
        return Err(ConfigError::Debounce);
    }
    Ok(reload)
}

/// A button configuration that cannot be run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigError {
    /// `tick_period_ms` below 1.
    TickPeriod,
    /// `debounce_ms` below one tick, or absurdly long.
    Debounce,
}

/// One debounced edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ButtonEvent {
    /// The debounced state *after* this edge, with `active_low` applied.
    pub pressed: bool,
    /// Milliseconds since boot at which that state began, reconstructed from
    /// the PIO counter. See the module docs.
    pub at_ms: u64,
}

/// The CPU half of `Button`: turns FIFO words into [`ButtonEvent`]s.
///
/// Holds the fold state `Button.read()` kept privately — the timestamp anchor
/// and the last emitted state — and nothing else. The FIFO itself belongs to the
/// firmware, because it is hardware.
#[derive(Debug, Clone)]
pub struct EventDecoder {
    tick_period_ms: u32,
    active_low: bool,
    anchor_ms: u64,
    last_pressed: bool,
}

impl EventDecoder {
    /// Seed the fold from the boundary condition `Button.__init__` captured:
    /// the pin as the CPU sampled it, and the time it sampled it at.
    ///
    /// The PIO converges on the same state within about two iterations without
    /// pushing an event, which is why this seed and the event stream agree
    /// even though nothing synchronises them.
    pub const fn new(
        initial_pressed: bool,
        now_ms: u64,
        tick_period_ms: u32,
        active_low: bool,
    ) -> EventDecoder {
        EventDecoder {
            tick_period_ms,
            active_low,
            anchor_ms: now_ms,
            last_pressed: initial_pressed,
        }
    }

    /// Decode one FIFO word. `None` is the rollover marker, which is an
    /// implementation detail consumers never see.
    pub fn decode(&mut self, word: u32) -> Option<ButtonEvent> {
        let pressed = ((word & FIFO_STATE_BIT) != 0) != self.active_low;
        let duration_ticks = word & FIFO_DURATION_MASK;
        // Advances for the filtered marker too — a no-op, since its duration is
        // zero, and keeping it unconditional is what makes the filter provably
        // free of arithmetic consequences.
        self.anchor_ms = self
            .anchor_ms
            .saturating_add(duration_ticks as u64 * self.tick_period_ms as u64);
        if duration_ticks == 0 && pressed == self.last_pressed {
            return None;
        }
        self.last_pressed = pressed;
        Some(ButtonEvent {
            pressed,
            at_ms: self.anchor_ms,
        })
    }
}

/// What a fold over one button's edges produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    /// Fired on the **release** edge, and only if the hold stayed under
    /// [`LONG_PRESS_MS`].
    Short,
    /// Fired **mid-hold**, the moment the threshold passes.
    Long,
}

/// `main.py`'s `_PressTracker`: one button's edge stream, folded.
///
/// The two halves fire at different moments on purpose. A long press fires
/// while the button is still down, because immediate feedback is what tells the
/// user the hold registered; the short press therefore has to move to the
/// release edge so that a long hold cannot fire both. Consuming the long press
/// clears the timestamp, which is the mechanism: with nothing recorded, the
/// release has no press to complete.
#[derive(Debug, Clone, Copy)]
pub struct PressTracker {
    /// The last debounced state, which is what an edge is measured against.
    pressed: bool,
    /// When the current press began; `None` means released *or already
    /// consumed by a long press*. The two are deliberately the same state.
    press_ms: Option<u64>,
}

impl PressTracker {
    pub const fn new(initial_pressed: bool) -> PressTracker {
        PressTracker {
            pressed: initial_pressed,
            press_ms: None,
        }
    }

    /// Fold one edge in. Returns [`Press::Short`] on a release that completes a
    /// short press.
    pub fn event(&mut self, event: ButtonEvent) -> Option<Press> {
        let press = match (event.pressed, self.pressed) {
            (true, false) => {
                self.press_ms = Some(event.at_ms);
                None
            }
            (false, true) => {
                // `None` here is a press whose long action already fired.
                let short = self.press_ms.map(|_| Press::Short);
                self.press_ms = None;
                short
            }
            // A same-state pair: a sub-debounce blip was swallowed, so there is
            // no edge. See the module docs.
            _ => None,
        };
        self.pressed = event.pressed;
        press
    }

    /// Check the hold threshold. Call every poll — the PIO emits nothing while
    /// a button is steadily held, so this is the only thing that can notice.
    pub fn poll(&mut self, now_ms: u64) -> Option<Press> {
        let started = self.press_ms?;
        if !self.pressed || now_ms.saturating_sub(started) < LONG_PRESS_MS {
            return None;
        }
        // Consumed: the release will not also fire short.
        self.press_ms = None;
        Some(Press::Long)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The word the PIO pushes for a HIGH state that lasted `ticks`.
    const fn high(ticks: u32) -> u32 {
        FIFO_STATE_BIT | ticks
    }

    #[test]
    fn the_program_fits_one_pio_block_and_leaves_room_for_nothing_else() {
        let program = program();
        assert_eq!(
            program.code.len(),
            26,
            "the instruction count is a documented fact: both state machines \
             share PIO1's 32-word memory and a second program would not fit"
        );
    }

    #[test]
    fn the_clock_matches_button_pys_derivation() {
        assert_eq!(frequency_hz(1), 32_000);
        assert_eq!(frequency_hz(2), 16_000);
    }

    #[test]
    fn the_debounce_reload_is_iterations_not_ticks() {
        // pio_sim.py scenario 7: `2 * 20 // 1 = 40`.
        assert_eq!(debounce_reload(1, 20), Ok(40));
        assert_eq!(debounce_reload(2, 20), Ok(20));
    }

    #[test]
    fn a_debounce_below_one_tick_is_rejected() {
        // The floor exists so a real event can never decode to duration 0 and
        // be mistaken for the rollover marker.
        assert_eq!(debounce_reload(1, 0), Err(ConfigError::Debounce));
        assert_eq!(debounce_reload(4, 1), Err(ConfigError::Debounce));
        assert_eq!(debounce_reload(0, 20), Err(ConfigError::TickPeriod));
    }

    /// pio_sim.py scenario 7, the decode half: four words, including the
    /// same-state one a swallowed release produces.
    #[test]
    fn words_decode_and_the_anchor_advances_by_measured_duration() {
        let mut decoder = EventDecoder::new(false, 1_000, 1, false);
        let words = [high(500), 200, high(100), high(50)];
        let events: heapless::Vec<_, 4> = words
            .iter()
            .filter_map(|word| decoder.decode(*word))
            .collect();
        assert_eq!(
            events.as_slice(),
            &[
                ButtonEvent { pressed: true, at_ms: 1_500 },
                ButtonEvent { pressed: false, at_ms: 1_700 },
                ButtonEvent { pressed: true, at_ms: 1_800 },
                // The swallow: HIGH again with no LOW between.
                ButtonEvent { pressed: true, at_ms: 1_850 },
            ]
        );
    }

    #[test]
    fn the_rollover_marker_is_filtered_and_the_anchor_still_advances() {
        let mut decoder = EventDecoder::new(false, 1_000, 1, false);
        for word in [high(500), 200, high(100), high(50)] {
            decoder.decode(word);
        }
        // Same state, zero duration: the ~24.8-day counter wrap.
        assert_eq!(decoder.decode(high(0)), None);
        assert_eq!(
            decoder.decode(300),
            Some(ButtonEvent { pressed: false, at_ms: 2_150 }),
            "the marker consumed no time, so the next event lands where it would have"
        );
    }

    #[test]
    fn active_low_inverts_the_pin_and_nothing_else() {
        let mut low = EventDecoder::new(false, 0, 1, true);
        assert_eq!(
            low.decode(high(10)),
            Some(ButtonEvent { pressed: false, at_ms: 10 }),
            "pin high with a pull-up means released"
        );
        assert_eq!(
            low.decode(10),
            Some(ButtonEvent { pressed: true, at_ms: 20 })
        );
    }

    #[test]
    fn a_short_press_fires_on_release() {
        let mut tracker = PressTracker::new(false);
        assert_eq!(tracker.event(ButtonEvent { pressed: true, at_ms: 100 }), None);
        assert_eq!(tracker.poll(500), None, "still inside the threshold");
        assert_eq!(
            tracker.event(ButtonEvent { pressed: false, at_ms: 600 }),
            Some(Press::Short)
        );
        assert_eq!(tracker.poll(2_000), None, "nothing is held any more");
    }

    #[test]
    fn a_long_press_fires_mid_hold_and_the_release_does_not_double_fire() {
        let mut tracker = PressTracker::new(false);
        tracker.event(ButtonEvent { pressed: true, at_ms: 100 });
        assert_eq!(tracker.poll(100 + LONG_PRESS_MS - 1), None);
        assert_eq!(tracker.poll(100 + LONG_PRESS_MS), Some(Press::Long));
        assert_eq!(
            tracker.poll(100 + LONG_PRESS_MS + 50),
            None,
            "a held button fires its long action once, not once per poll"
        );
        assert_eq!(
            tracker.event(ButtonEvent { pressed: false, at_ms: 3_000 }),
            None,
            "consuming the long press is what stops the release firing short"
        );
    }

    #[test]
    fn a_swallowed_blip_produces_no_edge() {
        let mut tracker = PressTracker::new(false);
        assert_eq!(tracker.event(ButtonEvent { pressed: true, at_ms: 100 }), None);
        // The release was swallowed, so the next accepted edge repeats HIGH.
        assert_eq!(
            tracker.event(ButtonEvent { pressed: true, at_ms: 150 }),
            None,
            "same-state events must not be read as a release"
        );
        assert_eq!(
            tracker.event(ButtonEvent { pressed: false, at_ms: 200 }),
            Some(Press::Short)
        );
    }

    #[test]
    fn a_same_state_release_pair_does_not_fire_twice() {
        let mut tracker = PressTracker::new(true);
        tracker.event(ButtonEvent { pressed: false, at_ms: 10 });
        assert_eq!(
            tracker.event(ButtonEvent { pressed: false, at_ms: 20 }),
            None,
            "a released button releasing again is not a press"
        );
    }

    #[test]
    fn the_long_threshold_is_measured_from_the_pios_timestamp_not_the_poll() {
        // A press whose edge the PIO stamped 900 ms ago is already long the
        // first time it is polled, even though the CPU has only just seen it.
        let mut tracker = PressTracker::new(false);
        tracker.event(ButtonEvent { pressed: true, at_ms: 1_000 });
        assert_eq!(tracker.poll(1_900), Some(Press::Long));
    }
}
