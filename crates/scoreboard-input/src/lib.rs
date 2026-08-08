//! Everything the physical world tells the scoreboard.
//!
//! Three sources of input, three modules, and none of them touches hardware:
//!
//! * [`button`] — `lib/button.py`'s PIO debounce program and the *whole* of its
//!   consumer side: decoding a FIFO word into an edge, reconstructing when that
//!   edge happened from the PIO counter, and folding an edge stream into short
//!   and long presses (`main.py`'s `_PressTracker`).
//! * [`menu`] — `menu.py`'s `MenuController`, the league-select session a long
//!   press on button B opens.
//! * [`brightness`] — `brightness.py`, the ambient-light curve. Ambient light is
//!   an input too; the sensor is just slower than a finger.
//!
//! They are together because they are the *same* boundary — the point where
//! something outside the device changes what is on the panel — and because
//! grouping them is what lets the firmware's `inputs.rs` and `brightness.rs`
//! keep nothing but a PIO block, an I2C bus and two tasks. SPEC §2's
//! crate-boundary rule: the decisions live where a desktop can run them.
//!
//! # The PIO program is here too, and that is the point
//!
//! [`button::PROGRAM`] is assembled by the `pio` crate at compile time and
//! links as a `[u16; 26]` of instruction words. The firmware hands those exact
//! words to `embassy_rp::pio`; the test suite hands them to a cycle-accurate
//! interpreter and replays `tools/pio_sim.py`'s scenarios against them. So the
//! oracle tests the shipped program, not a transcription of it — which is the
//! property `pio_sim.py` had over `button.py` and the reason it exists.

#![no_std]
#![forbid(unsafe_code)]

pub mod brightness;
pub mod button;
pub mod menu;

#[cfg(test)]
mod pio_sim;
