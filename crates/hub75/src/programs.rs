//! The two PIO programs, 1:1 with driver.py's `data_program` /
//! `address_program` (Binary row addressing), assembled at compile time.
//!
//! Synchronization contract: the address SM fires IRQ 0 ("latch safe") once
//! the row address is on the pins and OE is off; the data SM, which has
//! finished clocking the next row into the panel's shift register, waits on
//! it, raises LAT, and fires IRQ 1 ("latch complete") — the latch triggers
//! on LAT's rising edge, so completion can be signalled before LAT drops.
//! After the handshake the SMs run concurrently: the address SM burns the
//! off/on/off OE delay loops while the data SM clocks the following row.

use pio::{Program, pio_asm};

/// PIO IRQ flag used by the address SM to signal "row switched, latch now".
pub const LATCH_SAFE_IRQ: u8 = 0;
/// PIO IRQ flag used by the data SM to signal "latch has risen".
pub const LATCH_COMPLETE_IRQ: u8 = 1;

/// Data SM: clocks pixel data into the panel's shift register.
///
/// Side-set (2 bits, mandatory): bit 0 = CLK, bit 1 = LAT. Runs at
/// `data_frequency * 2` (one SM cycle per clock edge). `out pins, 8`
/// consumes a byte-aligned 6-bit pixel-pair word (4 per DMA transfer);
/// Y holds `shift_register_depth - 1`, seeded through the TX FIFO before
/// the DMA starts.
pub fn data_program() -> Program<32> {
    pio_asm!(
        ".side_set 2",
        "    out y, 32            side 0", // pixel-count reload value, once
        ".wrap_target",
        "    mov x, y             side 0",
        "write_data:",
        "    out pins, 8          side 0",
        "    jmp x-- write_data   side 1", // pixel clock rises on the loop
        "    wait 1 irq 0         side 0", // latch-safe (consumes the flag)
        "    irq 1                side 2", // LAT rises here; latch-complete
        ".wrap",
    )
    .program
}

/// Address SM: walks row addresses, drives OE, owns all BCM timing.
///
/// Side-set (1 bit, mandatory): OE, active-low (`side 1` = display off).
/// Runs at full system clock. The timing DMA streams interleaved
/// `[off, on]` u32 pairs per bitplane: `out isr, 32` parks the off count in
/// ISR while autopull leaves the on count in OSR for the whole bitplane;
/// `out null, 32` discards the stale on word at each bitplane transition
/// (the first transition jumps past it — nothing is stale yet).
///
/// X counts rows *down* and is written inverted (`mov pins, !x`) so the
/// panel sees addresses counting up from 0. The row count 32 = 1 << 5
/// cannot be loaded with `set` — the 5-bit immediate silently truncates
/// `1 << 5` to 0 (the classic trap, driver.py:855-865) — so a single 1 bit
/// is shifted into position through ISR instead: with `in` shifting right,
/// `in x, 27` computes `ISR = x << (32 - 27)` = `1 << 5`.
pub fn address_program() -> Program<32> {
    pio_asm!(
        ".side_set 1",
        "    jmp initialize            side 1", // first word must not be discarded
        "increment_bitplane:",
        "    out null, 32              side 1",
        "initialize:",
        "    set x, 1                  side 1",
        "    mov isr, null             side 1",
        "    in x, 27                  side 1", // 32 - ROW_ADDRESS_BITS
        "    mov x, isr                side 1", // x = row address count
        "    out isr, 32               side 1", // ISR = off count; OSR = on count
        ".wrap_target",
        "increment_address:",
        "    jmp x-- write_address     side 1",
        "    jmp increment_bitplane    side 1",
        "write_address:",
        "    mov pins, !x              side 1",
        "    irq 0                     side 1", // latch-safe
        "    wait 1 irq 1              side 1", // latch-complete
        "    mov y, isr                side 1",
        "off_delay_before_enable:",
        "    jmp y-- off_delay_before_enable  side 1",
        "    mov y, osr                side 1",
        "on_delay:",
        "    jmp y-- on_delay          side 0", // the lit interval
        "    mov y, isr                side 1",
        "off_delay_after_disable:",
        "    jmp y-- off_delay_after_disable  side 1", // anti-ghosting
        ".wrap",
    )
    .program
}

/// Adjust a program's absolute JMP targets for loading at `origin`. JMP is
/// the only PIO instruction with an in-memory address operand (opcode 000).
pub fn relocate(instruction: u16, origin: u8) -> u16 {
    if instruction >> 13 == 0 {
        let target = (instruction & 0x1F) as u8 + origin;
        debug_assert!(target < 32, "relocated jump target out of program memory");
        (instruction & !0x1F) | target as u16
    } else {
        instruction
    }
}

/// Encode an unconditional `jmp target` carrying `side_bits` in the
/// delay/side-set field, for forcing a state machine's PC via SM_INSTR.
/// `side_bits` is the raw value of the field's top side-set bits — for the
/// address SM (1-bit side-set) pass `1 << 4` to hold OE deasserted.
pub fn jmp_instruction(target: u8, side_bits: u8) -> u16 {
    debug_assert!(target < 32);
    ((side_bits as u16) << 8) | target as u16
}
