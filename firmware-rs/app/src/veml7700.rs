//! The VEML7700 ambient light sensor, on I²C0 at `0x10`.
//!
//! Port of `lib/veml7700.py`. That file is a fork of a fork of a NodeMCU
//! library and it shows; what matters is that the *numbers it produces* are the
//! numbers `brightness.py`'s curve was tuned against, so this is a transcription
//! and not a re-derivation.
//!
//! # The resolution table is pinned, deviations and all
//!
//! [`RESOLUTION_LUX_PER_COUNT`] is `veml7700.py`'s `gainValues`, copied cell for
//! cell. **Several cells disagree with the datasheet.** At `it=100, gain=1` the
//! Vishay application note gives 0.0288 lux/count, which the table matches — but
//! at `it=200, gain=1` the table repeats 0.0288 where the datasheet halves it to
//! 0.0144, and the `it=800, gain=1/8` cell reads 0.0876 where the ideal
//! progression gives 0.0576. A port that recomputed the table from
//! `0.0036 * (800/it) * (2/gain)` would be *more correct* and would not match
//! the panel standing next to it.
//!
//! Since exactly one configuration is ever used — the one both MicroPython call
//! sites pass — the whole table is here as a documented fact and
//! [`CONFIGURATION`] selects from it, rather than the table being reduced to the
//! one live constant. The deviations are the reason: a future change of
//! integration time must be able to see what it is choosing between.
//!
//! # What this driver deliberately does not do
//!
//! No auto-ranging, no integration-time enforcement, and none of Vishay's
//! high-lux correction polynomial — all three absent from `veml7700.py` too.
//! The consequences are real and bounded: above roughly 1,900 lux the reading
//! saturates, and the curve is already at [`BRI_MAX`] by 300 lux, so saturation
//! is invisible. The caller is responsible for spacing reads by at least one
//! integration time; the 200 ms auto-brightness tick is twice the 100 ms
//! configured here.
//!
//! [`BRI_MAX`]: scoreboard_input::brightness::BRI_MAX

use embassy_rp::i2c::{Async, Error, I2c};
use embassy_rp::peripherals::I2C0;

/// The sensor's fixed I²C address.
pub const ADDRESS: u16 = 0x10;

/// Write registers.
const ALS_CONF_0: u8 = 0x00;
const ALS_WH: u8 = 0x01;
const ALS_WL: u8 = 0x02;
const POWER_SAVE: u8 = 0x03;
/// The ambient-light result register.
const ALS: u8 = 0x04;

/// `ALS_CONF_0` for `it=100, gain=1` — `veml7700.py`'s `confValues[100][1]`,
/// which is two zero bytes.
///
/// The **order** is the table's, not a decision: `writeto_mem` sent the two
/// bytes as they appear in the row. Here both are zero so the order is
/// unobservable, but every other row has a non-zero pair (200 ms at gain 1 is
/// `[0x40, 0x00]`), so a change of integration time must keep sending the
/// row's bytes in the row's order rather than deriving them.
const CONFIGURATION: [u8; 2] = [0x00, 0x00];

/// Lux per raw count at `it=100, gain=1`. See the module docs before changing.
pub const RESOLUTION_LUX_PER_COUNT: f32 = 0.0288;

/// The whole of `veml7700.py`'s `gainValues`, as a documented fact.
///
/// Rows are integration times in milliseconds, columns are gains
/// `[1/8, 1/4, 1, 2]`. Nothing reads it — [`RESOLUTION_LUX_PER_COUNT`] is the
/// one live cell — and that is the point: it is here so a change of integration
/// time is a lookup rather than a derivation, because a derivation would
/// silently disagree with the shipping firmware in four cells.
#[expect(
    dead_code,
    reason = "documentation of the pinned table; see the module docs"
)]
const GAIN_VALUES: [(u16, [f32; 4]); 6] = [
    (25, [1.8432, 0.9216, 0.2304, 0.1152]),
    (50, [0.9216, 0.4608, 0.1152, 0.0576]),
    (100, [0.4608, 0.2304, 0.0288, 0.0144]),
    // Datasheet-ideal would be 0.0144 in the `gain=1` cell, not a repeat of the
    // 100 ms row.
    (200, [0.2304, 0.1152, 0.0288, 0.0144]),
    (400, [0.1152, 0.0576, 0.0144, 0.0072]),
    // Datasheet-ideal would be 0.0576 in the `gain=1/8` cell.
    (800, [0.0876, 0.0288, 0.0072, 0.0036]),
];

/// The sensor, or rather the four register writes that configure one and the
/// one read that uses it.
pub struct Veml7700;

impl Veml7700 {
    /// Configure the part. **Idempotent and re-callable** — both MicroPython
    /// consumers used `init()` as the post-error recovery path, and
    /// [`crate::brightness::LightSensor`] does the same.
    ///
    /// The three zeroing writes are not decoration: the interrupt thresholds
    /// and the power-saving mode are non-volatile-ish in the sense that they
    /// survive a warm reset of the host, so a device that reset while something
    /// had configured them differently would read through a filter nobody set.
    pub async fn init(bus: &mut I2c<'static, I2C0, Async>) -> Result<Veml7700, Error> {
        write(bus, ALS_CONF_0, CONFIGURATION).await?;
        write(bus, ALS_WH, [0x00, 0x00]).await?;
        write(bus, ALS_WL, [0x00, 0x00]).await?;
        write(bus, POWER_SAVE, [0x00, 0x00]).await?;
        Ok(Veml7700)
    }

    /// One reading, in lux.
    ///
    /// A plain linear scale of the raw count, exactly as `read_lux` had it. The
    /// register is little-endian and the scale is [`RESOLUTION_LUX_PER_COUNT`];
    /// there is no correction curve and no ranging.
    pub async fn read_lux(&self, bus: &mut I2c<'static, I2C0, Async>) -> Result<f32, Error> {
        let mut raw = [0u8; 2];
        bus.write_read_async(ADDRESS, [ALS], &mut raw).await?;
        let counts = u16::from_le_bytes(raw);
        Ok(counts as f32 * RESOLUTION_LUX_PER_COUNT)
    }
}

async fn write(
    bus: &mut I2c<'static, I2C0, Async>,
    register: u8,
    value: [u8; 2],
) -> Result<(), Error> {
    bus.write_async(ADDRESS, [register, value[0], value[1]]).await
}
