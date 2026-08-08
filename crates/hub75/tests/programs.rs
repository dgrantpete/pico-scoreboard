//! Structural assertions pinning the assembled PIO programs to the exact
//! instruction stream the cycle model in src/timing.rs was derived from
//! (encodings hand-assembled from the RP2350 datasheet PIO ISA).

use hub75::programs::{address_program, data_program, jmp_instruction, relocate};

#[test]
fn data_program_encodes_as_derived() {
    let program = data_program();
    assert_eq!(
        program.code.as_slice(),
        &[
            0x6040, // out y, 32        side 0
            0xA022, // mov x, y         side 0   <- wrap target
            0x6008, // out pins, 8      side 0
            0x0842, // jmp x-- 2        side 1 (CLK)
            0x20C0, // wait 1 irq 0     side 0
            0xD001, // irq 1            side 2 (LAT)
        ],
        "data program instruction stream changed; re-derive the cycle model"
    );
    assert_eq!((program.wrap.source, program.wrap.target), (5, 1));
    assert!(!program.side_set.optional(), "side-set must be mandatory");
    assert_eq!(program.origin, None);
}

#[test]
fn address_program_encodes_as_derived() {
    let program = address_program();
    assert_eq!(
        program.code.as_slice(),
        &[
            0x1002, // jmp initialize           side 1
            0x7060, // out null, 32             side 1   <- increment_bitplane
            0xF021, // set x, 1                 side 1   <- initialize
            0xB0C3, // mov isr, null            side 1
            0x503B, // in x, 27                 side 1
            0xB026, // mov x, isr               side 1
            0x70C0, // out isr, 32              side 1
            0x1049, // jmp x-- write_address    side 1   <- wrap target
            0x1001, // jmp increment_bitplane   side 1
            0xB009, // mov pins, !x             side 1   <- write_address
            0xD000, // irq 0                    side 1
            0x30C1, // wait 1 irq 1             side 1
            0xB046, // mov y, isr               side 1
            0x108D, // jmp y-- 13               side 1 (off before enable)
            0xB047, // mov y, osr               side 1
            0x008F, // jmp y-- 15               side 0 (OE asserted: lit)
            0xB046, // mov y, isr               side 1
            0x1091, // jmp y-- 17               side 1 (off after disable)
        ],
        "address program instruction stream changed; re-derive the cycle model"
    );
    assert_eq!((program.wrap.source, program.wrap.target), (17, 7));
    assert!(!program.side_set.optional(), "side-set must be mandatory");
}

#[test]
fn programs_fit_one_pio_block() {
    assert!(data_program().code.len() + address_program().code.len() <= 32);
}

#[test]
fn relocation_adjusts_only_jumps() {
    let origin = data_program().code.len() as u8;
    let address = address_program();
    let relocated: Vec<u16> = address
        .code
        .iter()
        .map(|&instruction| relocate(instruction, origin))
        .collect();

    // Jump targets shift by the origin; everything else is untouched.
    assert_eq!(relocated[0], 0x1002 + origin as u16);
    assert_eq!(relocated[7], 0x1049 + origin as u16);
    assert_eq!(relocated[8], 0x1001 + origin as u16);
    assert_eq!(relocated[13], 0x108D + origin as u16);
    assert_eq!(relocated[15], 0x008F + origin as u16);
    assert_eq!(relocated[17], 0x1091 + origin as u16);
    for index in [1, 2, 3, 4, 5, 6, 9, 10, 11, 12, 14, 16] {
        assert_eq!(relocated[index], address.code[index], "instruction {index}");
    }
}

#[test]
fn forced_jump_encodings() {
    // Data SM: plain jmp to 0 with both side bits low.
    assert_eq!(jmp_instruction(0, 0), 0x0000);
    // Address SM: jmp to its origin holding OE deasserted (side bit = 1 in
    // the top bit of the 5-bit delay/side field).
    assert_eq!(jmp_instruction(6, 1 << 4), 0x1006);
}
