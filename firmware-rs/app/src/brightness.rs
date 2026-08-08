//! Auto-brightness: the one owner of the panel's brightness.
//!
//! `main.py`'s `LightSensor` and `auto_brightness_loop`. The curve is
//! [`scoreboard_input::brightness`], host-tested against the Python's values;
//! what is here is the I²C bus, the retry policy, and the 5 Hz tick.
//!
//! # Sole ownership, and why it is worth naming
//!
//! `auto_brightness_loop` was the only caller of `driver.set_brightness()` in
//! the MicroPython firmware, and this is the only writer of
//! [`crate::display_core1::BRIGHTNESS`]. Anything else that adjusted brightness
//! — a settings save, a diagnostic — would be fighting a 5 Hz ramp it cannot
//! see, and the visible result is a panel that drifts back over a few seconds.
//! `PUT /api/config` therefore changes `display.brightness`, which this reads
//! **every tick** as the user preference, rather than setting a level directly.
//!
//! # An absent sensor is a supported configuration
//!
//! The bench unit has no VEML7700 attached, and neither will some gift units.
//! Two rules make that a non-event:
//!
//! * **No reading ever taken means a bright room**, not a dark one. Falling
//!   back to the bottom of the curve would leave a device with no sensor at
//!   5 % brightness, which reads as broken. The rule lives in
//!   [`AutoBrightness`](scoreboard_input::brightness::AutoBrightness) and is
//!   host-tested there.
//! * **Only transitions are logged.** This is a deliberate deviation:
//!   `LightSensor._try_init` logged an error on *every* attempt, and with a
//!   retry every 15 ticks that is a line every three seconds — twenty a minute,
//!   which evicts the 200-slot ring log in ten minutes and takes the history
//!   worth reading with it. Here the first failure is logged, the recovery is
//!   logged, and the thousands of attempts in between are silent.

use embassy_rp::i2c::{self, Async, I2c};
use embassy_rp::peripherals::{I2C0, PIN_0, PIN_1};
use embassy_rp::{Peri, bind_interrupts};
use embassy_time::{Duration, Ticker};
use scoreboard_input::brightness::{AutoBrightness, TICK_MS};

use crate::display_core1::BRIGHTNESS;
use crate::veml7700::Veml7700;

bind_interrupts!(struct Irqs {
    I2C0_IRQ => i2c::InterruptHandler<I2C0>;
});

/// `main.py:1235` — `I2C(0, sda=Pin(0), scl=Pin(1), freq=100000)`.
const BUS_FREQUENCY_HZ: u32 = 100_000;

/// Ticks between re-initialisation attempts while the sensor is unavailable.
/// `LightSensor.RETRY_TICKS` — 15 ticks is 3 s at the 200 ms tick.
const RETRY_TICKS: u8 = 15;

/// The sensor's silicon.
pub struct SensorPeripherals {
    pub i2c: Peri<'static, I2C0>,
    pub sda: Peri<'static, PIN_0>,
    pub scl: Peri<'static, PIN_1>,
}

/// What was last said about the sensor, so only changes are said again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reported {
    /// Nothing said yet — the first outcome, whichever it is, is worth a line.
    Unknown,
    Working,
    Unavailable,
}

/// The sensor plus its retry state. `main.py`'s `LightSensor`.
struct LightSensor {
    bus: I2c<'static, I2C0, Async>,
    sensor: Option<Veml7700>,
    retry_ticks: u8,
    reported: Reported,
}

impl LightSensor {
    /// One tick's reading, or `None` while the sensor is unavailable.
    async fn read_lux(&mut self) -> Option<f32> {
        let Some(sensor) = self.sensor.as_ref() else {
            self.retry_ticks += 1;
            if self.retry_ticks >= RETRY_TICKS {
                self.retry_ticks = 0;
                match Veml7700::init(&mut self.bus).await {
                    Ok(sensor) => self.sensor = Some(sensor),
                    Err(error) => self.report_unavailable("init failed", error),
                }
            }
            return None;
        };
        match sensor.read_lux(&mut self.bus).await {
            Ok(lux) => {
                if self.reported != Reported::Working {
                    self.reported = Reported::Working;
                    crate::debug!("brightness: sensor ok, veml7700 reading {} lux", lux as u32);
                }
                Some(lux)
            }
            Err(error) => {
                // Drop the handle so the retry path re-runs `init`, which is
                // what recovers a part that browned out or was unplugged and
                // plugged back in. `main.py` kept the object and only retried
                // the read; re-initialising is strictly more recovery for the
                // same four register writes.
                self.sensor = None;
                self.retry_ticks = 0;
                self.report_unavailable("read failed", error);
                None
            }
        }
    }

    fn report_unavailable(&mut self, what: &str, error: i2c::Error) {
        if self.reported == Reported::Unavailable {
            return;
        }
        self.reported = Reported::Unavailable;
        crate::error!(
            "brightness: veml7700 {} ({}); assuming a bright room, retrying every {} ms",
            what,
            I2cComplaint(error),
            RETRY_TICKS as u32 * TICK_MS as u32
        );
    }
}

/// Read ambient light and drive the panel. Runs in both modes, always.
#[embassy_executor::task]
pub async fn auto_brightness(p: SensorPeripherals) -> ! {
    let mut config = i2c::Config::default();
    config.frequency = BUS_FREQUENCY_HZ;
    let mut bus = I2c::new_async(p.i2c, p.scl, p.sda, Irqs, config);

    // One attempt before the loop, so a working sensor is reported at boot
    // rather than three seconds into it.
    let sensor = Veml7700::init(&mut bus).await.ok();
    let mut light = LightSensor {
        bus,
        sensor,
        // Start ready to retry: with no sensor at boot the first tick tries
        // again rather than waiting out a full retry window.
        retry_ticks: RETRY_TICKS,
        reported: Reported::Unknown,
    };

    let preference = crate::config::with(|config| config.display.brightness);
    let mut auto = AutoBrightness::new(preference);
    crate::debug!("brightness: auto-brightness up, preference {}", preference);

    let mut ticker = Ticker::every(Duration::from_millis(TICK_MS));
    loop {
        let lux = light.read_lux().await;
        // Re-read every tick, so a `PUT /api/config` moves the panel without a
        // reboot — which is what `poller.py` and this loop both got for free
        // from holding the `Config` object the API route mutated.
        let preference = crate::config::with(|config| config.display.brightness);
        let level = auto.tick(lux, preference);
        BRIGHTNESS.store(AutoBrightness::quantize(level), core::sync::atomic::Ordering::Relaxed);
        ticker.next().await;
    }
}

/// An I²C error in words both log channels can carry.
struct I2cComplaint(i2c::Error);

impl I2cComplaint {
    fn name(&self) -> &'static str {
        match self.0 {
            // What an absent part looks like: nobody drives the ACK bit.
            i2c::Error::Abort(i2c::AbortReason::NoAcknowledge) => "no acknowledge",
            i2c::Error::Abort(i2c::AbortReason::ArbitrationLoss) => "arbitration lost",
            i2c::Error::Abort(i2c::AbortReason::TxNotEmpty(_)) => "transmit fifo not empty",
            i2c::Error::Abort(i2c::AbortReason::Other(_)) => "aborted",
            i2c::Error::InvalidReadBufferLength => "invalid read length",
            i2c::Error::InvalidWriteBufferLength => "invalid write length",
            // The address is a constant, so the two address variants are
            // unreachable; a wildcard also keeps this compiling across
            // embassy-rp releases that add or deprecate one.
            _ => "bad address",
        }
    }
}

impl defmt::Format for I2cComplaint {
    fn format(&self, formatter: defmt::Formatter<'_>) {
        defmt::write!(formatter, "{=str}", self.name());
    }
}

impl core::fmt::Display for I2cComplaint {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.name())
    }
}
