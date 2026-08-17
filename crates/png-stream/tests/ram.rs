//! RAM inventory: the numbers the report and BUDGET.md cite. The crate
//! itself compile-time-asserts `Scratch` ≤ 64 KiB on every target; this
//! test prints the host-measured exact sizes (`--nocapture` to see them).

use png_stream::{RowDecoder, Scratch, SpriteDecoder};

#[test]
fn scratch_fits_the_documented_budget() {
    let scratch = core::mem::size_of::<Scratch>();
    let sprite_dec = core::mem::size_of::<SpriteDecoder>();
    let row_dec = core::mem::size_of::<RowDecoder>();
    println!("Scratch:       {scratch} bytes");
    println!("SpriteDecoder: {sprite_dec} bytes (borrows Scratch)");
    println!("RowDecoder:    {row_dec} bytes (borrows Scratch)");
    assert!(scratch <= 64 * 1024);
    // The decoder handles are ephemeral view structs, not buffers.
    assert!(sprite_dec <= 256);
    assert!(row_dec <= 192);
}
