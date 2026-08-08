//! A cycle-accurate interpreter for [`button::PROGRAM`], and `tools/pio_sim.py`
//! ported onto it.
//!
//! `pio_sim.py` is the oracle `lib/button.py` was written against: it imports
//! the *real* assembler output and executes it with datasheet semantics, so the
//! timing claims in that file's docstring are tested rather than asserted. The
//! Rust port keeps that property — [`crate::button::program`] is assembled by
//! the `pio` crate and this decodes the resulting instruction words, so a
//! transcription slip in the assembly fails these tests rather than shipping.
//!
//! The four semantics that matter, all from RP2040/RP2350 §3.4 and all things a
//! naive interpreter gets wrong:
//!
//! * `jmp x--` branches on the value **before** the decrement, and decrements
//!   **unconditionally** — 0 wraps to `0xFFFF_FFFF`. That wrap is what produces
//!   the saturation event scenario 4 exercises.
//! * Delay cycles are paid whether or not a branch is taken. The whole
//!   equal-length-loop-path argument rests on this.
//! * `in` with a left shift is `ISR = (ISR << n) | (src & mask(n))`.
//! * `mov ::x` is a 32-bit bit reversal, not a byte swap.
//!
//! What is *not* modelled, because the program does not use it: side-set, the
//! OUT path, autopush/autopull, IRQs, and a TX FIFO deeper than the one word
//! the CPU seeds. A blocking `push` cannot stall here either — the scenarios
//! never fill the FIFO, which is exactly the condition `button.py` requires the
//! consumer to maintain.

use pio::Program;

const M32: u64 = 0xFFFF_FFFF;

/// One decoded instruction. Only the forms [`crate::button::program`] emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    /// `jmp <cond>, addr`
    Jmp { condition: Condition, address: u8 },
    /// `mov dest, <op> source`
    Mov {
        destination: Reg,
        operation: MovOp,
        source: Reg,
    },
    /// `in source, count`
    In { source: Reg, count: u8 },
    Push,
    Pull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Condition {
    Always,
    NotX,
    XDec,
    NotY,
    YDec,
    Pin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reg {
    X,
    Y,
    Null,
    Isr,
    Osr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MovOp {
    None,
    Invert,
    Reverse,
}

fn register(code: u16) -> Reg {
    match code {
        1 => Reg::X,
        2 => Reg::Y,
        3 => Reg::Null,
        6 => Reg::Isr,
        7 => Reg::Osr,
        other => panic!("the button program uses no register {other}"),
    }
}

fn decode(word: u16) -> (Op, u32) {
    let delay = ((word >> 8) & 0x1F) as u32;
    let operands = word & 0xFF;
    let op = match word >> 13 {
        0b000 => Op::Jmp {
            condition: match (operands >> 5) & 0b111 {
                0 => Condition::Always,
                1 => Condition::NotX,
                2 => Condition::XDec,
                3 => Condition::NotY,
                4 => Condition::YDec,
                6 => Condition::Pin,
                other => panic!("the button program uses no jmp condition {other}"),
            },
            address: (operands & 0x1F) as u8,
        },
        0b010 => Op::In {
            source: register((operands >> 5) & 0b111),
            count: match (operands & 0x1F) as u8 {
                0 => 32,
                count => count,
            },
        },
        0b100 => {
            if operands & 0x80 == 0 {
                Op::Push
            } else {
                Op::Pull
            }
        }
        0b101 => Op::Mov {
            destination: register((operands >> 5) & 0b111),
            operation: match (operands >> 3) & 0b11 {
                0 => MovOp::None,
                1 => MovOp::Invert,
                2 => MovOp::Reverse,
                _ => panic!("reserved mov operation"),
            },
            source: register(operands & 0b111),
        },
        other => panic!("the button program uses no opcode {other:#b}"),
    };
    (op, delay)
}

/// A pin waveform: `(level, cycles)` segments, held at the last level after the
/// end. `pio_sim.py`'s `pin_wave`.
pub type Wave = [(u32, u64)];

pub struct Sim<'a> {
    code: heapless::Vec<(Op, u32), 32>,
    wrap_source: usize,
    wrap_target: usize,
    wave: &'a Wave,

    pc: usize,
    cycle: u64,
    x: u64,
    y: u64,
    isr: u64,
    osr: u64,
    tx: Option<u32>,

    /// `(cycle, word)` for every push.
    pub fifo: heapless::Vec<(u64, u32), 64>,
    /// The cycle stamp of every `x` decrement — the tick spacing scenario 2 is
    /// about.
    pub x_decrements: heapless::Vec<u64, 4096>,
}

impl<'a> Sim<'a> {
    pub fn new(program: &Program<32>, debounce_reload: u32, wave: &'a Wave) -> Sim<'a> {
        let mut code = heapless::Vec::new();
        for word in program.code.iter() {
            code.push(decode(*word)).expect("the program fits 32 words");
        }
        Sim {
            code,
            wrap_source: program.wrap.source as usize,
            wrap_target: program.wrap.target as usize,
            wave,
            pc: 0,
            cycle: 0,
            x: 0,
            y: 0,
            isr: 0,
            osr: 0,
            tx: Some(debounce_reload),
            fifo: heapless::Vec::new(),
            x_decrements: heapless::Vec::new(),
        }
    }

    fn pin(&self) -> u32 {
        let mut at = self.cycle;
        for (level, length) in self.wave {
            if at < *length {
                return *level;
            }
            at -= *length;
        }
        self.wave.last().expect("a non-empty wave").0
    }

    fn read(&self, source: Reg) -> u64 {
        match source {
            Reg::X => self.x,
            Reg::Y => self.y,
            Reg::Null => 0,
            Reg::Isr => self.isr,
            Reg::Osr => self.osr,
        }
    }

    fn write(&mut self, destination: Reg, value: u64) {
        match destination {
            Reg::X => self.x = value,
            Reg::Y => self.y = value,
            Reg::Isr => self.isr = value,
            Reg::Osr => self.osr = value,
            Reg::Null => {}
        }
    }

    fn step(&mut self) {
        let (op, delay) = self.code[self.pc];
        let mut next_pc = if self.pc == self.wrap_source {
            self.wrap_target
        } else {
            self.pc + 1
        };
        match op {
            Op::Jmp { condition, address } => {
                let taken = match condition {
                    Condition::Always => true,
                    Condition::NotX => self.x == 0,
                    Condition::NotY => self.y == 0,
                    Condition::XDec => {
                        let taken = self.x != 0;
                        self.x = self.x.wrapping_sub(1) & M32;
                        let _ = self.x_decrements.push(self.cycle);
                        taken
                    }
                    Condition::YDec => {
                        let taken = self.y != 0;
                        self.y = self.y.wrapping_sub(1) & M32;
                        taken
                    }
                    Condition::Pin => self.pin() == 1,
                };
                if taken {
                    next_pc = address as usize;
                }
            }
            Op::Mov {
                destination,
                operation,
                source,
            } => {
                let value = self.read(source);
                let value = match operation {
                    MovOp::None => value,
                    MovOp::Invert => !value & M32,
                    MovOp::Reverse => (value as u32).reverse_bits() as u64,
                };
                self.write(destination, value);
            }
            Op::In { source, count } => {
                let mask = if count >= 32 {
                    M32
                } else {
                    (1u64 << count) - 1
                };
                self.isr = ((self.isr << count) | (self.read(source) & mask)) & M32;
            }
            Op::Push => {
                let _ = self.fifo.push((self.cycle, self.isr as u32));
                self.isr = 0;
            }
            Op::Pull => {
                self.osr = self.tx.take().expect("a pull with an empty TX FIFO blocks forever")
                    as u64;
            }
        }
        self.cycle += 1 + delay as u64;
        self.pc = next_pc;
    }

    pub fn run(&mut self, cycles: u64) {
        while self.cycle < cycles {
            self.step();
        }
    }

    /// Fast-forward the duration counter, so the 2³² saturation wrap can be
    /// reached in a test. `pio_sim.py` scenario 4 pokes the same register.
    pub fn poke_duration_counter(&mut self, value: u32) {
        self.x = value as u64;
    }

    /// `(state, duration_ticks)` for every pushed word.
    pub fn events(&self) -> heapless::Vec<(u32, u32), 64> {
        self.fifo
            .iter()
            .map(|(_, word)| (word >> 31, word & 0x7FFF_FFFF))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::button::{debounce_reload, program};

    /// Cycles per stable-loop iteration. The program's whole timing contract.
    const IT: u64 = 16;
    /// Cycles per reported FIFO tick — two iterations.
    const TICK: u64 = 32;

    /// `reload = 8` iterations = 4 ticks, as every `pio_sim.py` scenario uses.
    const RELOAD: u32 = 8;

    fn wave(segments: &[(u32, u64)]) -> heapless::Vec<(u32, u64), 64> {
        segments.iter().copied().collect()
    }

    fn total(segments: &[(u32, u64)]) -> u64 {
        segments.iter().map(|(_, n)| n).sum()
    }

    #[test]
    fn the_iteration_and_tick_constants_agree_with_the_program() {
        assert_eq!(TICK, 2 * IT);
        assert_eq!(TICK as u32, crate::button::PIO_CYCLES_PER_TICK);
        // reload 8 iterations is 4 ticks, which is what the scenarios assume.
        assert_eq!(debounce_reload(1, 4), Ok(RELOAD));
    }

    /// pio_sim.py scenario 1: a clean press and release, each with bounce
    /// shorter than the window at both edges.
    #[test]
    fn bounce_at_both_edges_produces_exactly_one_press_and_one_release() {
        let segments = wave(&[
            (0, 300 * IT),
            // Press bounce, every excursion under the window.
            (1, IT),
            (0, IT),
            (1, 2 * IT),
            (0, IT),
            (1, 200 * IT),
            // Release bounce.
            (0, IT),
            (1, IT),
            (0, 2 * IT),
            (0, 400 * IT),
        ]);
        let mut sim = Sim::new(&program(), RELOAD, &segments);
        sim.run(total(&segments) - 10);

        let events = sim.events();
        assert_eq!(events.len(), 2, "{events:?}");
        assert_eq!(events[0].0, 1, "the first event is the press (pin HIGH)");
        assert_eq!(events[1].0, 0, "the second is the release");
        // Event 1's duration is the 300 stable low iterations: 150 ticks.
        assert!(events[0].1.abs_diff(150) <= 1, "press duration {}", events[0].1);
        // Event 2 spans the press bounce plus the 200 stable high iterations.
        assert!(
            events[1].1.abs_diff(102) <= 2,
            "release duration {}",
            events[1].1
        );

        // Zero added latency: the accepted edge fires on the sample that saw
        // it, within the transit path's couple of iterations.
        let first_high = 300 * IT;
        let latency = sim.fifo[0].0 - first_high;
        assert!(latency <= 2 * IT + 16, "press latency {latency} cycles");
    }

    /// pio_sim.py scenario 2, "THE MONEY TEST": the duration counter must
    /// decrement on a perfectly even 16-cycle beat in **both** states and in
    /// **both** debounce phases (counting down, and saturated at zero). This is
    /// what the `nop` shim in `saturating_decrement` buys, and the assertion
    /// that fails if a delay is ever dropped from one path.
    #[test]
    fn the_duration_counter_ticks_every_sixteen_cycles_in_steady_state() {
        let segments = wave(&[(0, 500 * IT), (1, 500 * IT)]);
        let mut sim = Sim::new(&program(), RELOAD, &segments);
        sim.run(990 * IT);

        let irregular: heapless::Vec<u64, 16> = sim
            .x_decrements
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .filter(|delta| *delta != IT)
            .collect();
        assert_eq!(
            irregular.as_slice(),
            &[2 * IT],
            "the one accepted transition costs exactly two iterations of \
             transit; everything else must be a flat 16 ({} decrements)",
            sim.x_decrements.len()
        );
    }

    /// pio_sim.py scenario 3: starting with the pin already high pushes
    /// nothing — the PIO converges on the real level within about two
    /// iterations, which is what lets the CPU seed its fold from a plain pin
    /// read.
    #[test]
    fn starting_high_pushes_no_spurious_event() {
        let segments = wave(&[(1, 100 * IT), (0, 100 * IT)]);
        let mut sim = Sim::new(&program(), RELOAD, &segments);
        sim.run(195 * IT);

        let events = sim.events();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].0, 0, "the only event is the release");
        assert!(events[0].1.abs_diff(50) <= 1, "duration {}", events[0].1);
    }

    /// pio_sim.py scenario 4: the ~24.8-day counter wrap pushes a same-state
    /// event whose duration decodes to 0, and the loop then resumes cleanly.
    /// That signature is what `EventDecoder::decode` filters on.
    #[test]
    fn the_saturation_event_is_same_state_with_a_zero_duration() {
        let segments = wave(&[(0, 100_000 * IT)]);
        let mut sim = Sim::new(&program(), RELOAD, &segments);
        sim.run(50 * IT);
        sim.poke_duration_counter(5);
        sim.run(90 * IT);

        let events = sim.events();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].0, 0, "same state as before the wrap");
        assert_eq!(events[0].1, 0, "the documented quirk the decoder filters");

        let tail = &sim.x_decrements[sim.x_decrements.len() - 20..];
        assert!(
            tail.windows(2).all(|pair| pair[1] - pair[0] == IT),
            "ticking resumes on the beat after a wrap"
        );
    }

    /// pio_sim.py scenario 5: a press shorter than the window fires HIGH at
    /// zero latency, its release is rejected because HIGH never armed, and the
    /// next press therefore arrives as a **same-state** HIGH event. This is the
    /// documented trade-off `PressTracker` is written against.
    #[test]
    fn a_sub_debounce_press_is_swallowed_and_surfaces_as_a_same_state_pair() {
        let segments = wave(&[
            (0, 100 * IT),
            (1, 3 * IT),
            (0, 100 * IT),
            (1, 50 * IT),
            (0, 100 * IT),
        ]);
        let mut sim = Sim::new(&program(), RELOAD, &segments);
        sim.run(350 * IT);

        let events = sim.events();
        assert_eq!(events.len(), 3, "{events:?}");
        let states: heapless::Vec<u32, 4> = events.iter().map(|(state, _)| *state).collect();
        assert_eq!(states.as_slice(), &[1, 1, 0], "HIGH, HIGH (swallow), LOW");
        assert!(
            events[1].1.abs_diff(51) <= 2,
            "the second duration spans the swallowed press and the low period, got {}",
            events[1].1
        );
    }

    /// pio_sim.py scenario 6: durations stay exact across many alternations,
    /// with no cumulative drift. The `pio_sim.py` version randomises; this one
    /// uses a fixed pseudo-random sequence so a failure is reproducible without
    /// a seed to chase.
    #[test]
    fn durations_are_exact_over_forty_alternations_with_no_drift() {
        // A small xorshift, so the periods are irregular without a dependency.
        let mut state = 0x2545_F491u32;
        let mut periods = [0u64; 40];
        for period in &mut periods {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *period = 20 + (state % 380) as u64;
        }

        let mut segments: heapless::Vec<(u32, u64), 64> = periods
            .iter()
            .enumerate()
            .map(|(index, period)| ((index % 2) as u32, period * IT))
            .collect();
        let driven = total(&segments);
        segments.push((0, 500 * IT)).expect("room for the tail");

        let mut sim = Sim::new(&program(), RELOAD, &segments);
        sim.run(driven + 100 * IT);

        let events = sim.events();
        assert_eq!(events.len(), periods.len(), "one event per alternation");
        let mut net_error = 0i64;
        for (index, (state, ticks)) in events.iter().enumerate() {
            // Event i fires leaving period i: its duration is that period, and
            // its state is the level of period i+1.
            let expected_ticks = periods[index] / 2;
            let expected_state = ((index + 1) % 2) as u32;
            assert_eq!(*state, expected_state, "event {index} state");
            assert!(
                (*ticks as u64).abs_diff(expected_ticks) <= 2,
                "event {index}: {ticks} ticks, expected about {expected_ticks}"
            );
            net_error += *ticks as i64 - expected_ticks as i64;
        }
        assert!(
            net_error.unsigned_abs() <= events.len() as u64,
            "cumulative drift {net_error} ticks over {} events — the transit \
             path's sub-tick error must not accumulate",
            events.len()
        );
    }
}
