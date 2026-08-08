//! Bench validation patterns for the hub75 driver crate (task: P1.5
//! hardware validation). Cycles forever through:
//!
//! 1. solid red / green / blue / white (wiring, channel order)
//! 2. per-channel gradient ramps (gamma sweep, BCM plane integrity)
//! 3. a bar moving at 20 FPS (flip/tearing check at render cadence)
//! 4. brightness triangle sweep on solid white (OE duty timing)
//! 5. corner + center markers (geometry, row addressing, scan order)
//!
//! Exists to prove the driver, not to be a product.

#![no_std]
#![no_main]

use defmt::info;
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_time::{Duration, Ticker, Timer};
use hub75::display::{FrameBytes, Hub75Display};
use hub75::driver::{Config, Hub75Driver};
use hub75::geometry::{HEIGHT, RGB565_FRAME_BYTES, WIDTH};
use hub75::rgb565;
use panic_probe as _;
use static_cell::ConstStaticCell;

static FRAME: ConstStaticCell<FrameBytes> = ConstStaticCell::new([0; RGB565_FRAME_BYTES]);

type Display = Hub75Display<'static, Hub75Driver>;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // Embassy owns clock/XOSC bring-up; the driver only needs the number.
    let _peripherals = embassy_rp::init(Default::default());
    let system_clock_hz = embassy_rp::clocks::clk_sys_freq();

    let pac = rp235x_pac::Peripherals::take().unwrap();
    let driver = Hub75Driver::new(pac.PIO0, pac.DMA, Config::defaults(system_clock_hz));
    info!(
        "hub75 driver up: sys {} Hz, refresh {} Hz (target 120)",
        system_clock_hz,
        driver.refresh_rate() as u32
    );

    let mut display = Hub75Display::new(FRAME.take(), driver);
    loop {
        solid_colors(&mut display).await;
        gradient_ramps(&mut display).await;
        moving_bar(&mut display).await;
        brightness_sweep(&mut display).await;
        geometry_markers(&mut display).await;
    }
}

async fn solid_colors(display: &mut Display) {
    let colors = [
        ("red", rgb565(255, 0, 0)),
        ("green", rgb565(0, 255, 0)),
        ("blue", rgb565(0, 0, 255)),
        ("white", rgb565(255, 255, 255)),
    ];
    for (name, color) in colors {
        info!("pattern: solid {}", name);
        display.fill(color);
        display.show();
        Timer::after_millis(2000).await;
    }
}

/// Four horizontal bands (white, R, G, B), each ramping 0..255 left to
/// right. Gamma correctness shows as a perceptually even ramp; a stuck or
/// swapped BCM plane shows as banding.
async fn gradient_ramps(display: &mut Display) {
    info!("pattern: gradient ramps (white / R / G / B)");
    for x in 0..WIDTH {
        let v = (x * 255 / (WIDTH - 1)) as u8;
        let band = [
            rgb565(v, v, v),
            rgb565(v, 0, 0),
            rgb565(0, v, 0),
            rgb565(0, 0, v),
        ];
        for y in 0..HEIGHT {
            display.pixel(x as i32, y as i32, band[y / (HEIGHT / 4)]);
        }
    }
    display.show();
    Timer::after_millis(4000).await;
}

/// A 4-pixel bar sweeping the panel at the scoreboard's render cadence.
/// Tearing or judder here means the flip path is broken.
async fn moving_bar(display: &mut Display) {
    info!("pattern: moving bar at 20 FPS");
    let mut ticker = Ticker::every(Duration::from_millis(50));
    let white = rgb565(255, 255, 255);
    let red = rgb565(255, 0, 0);
    for step in 0..200usize {
        let bar_x = (step * 2) % WIDTH;
        display.fill(0);
        for dx in 0..4 {
            let x = ((bar_x + dx) % WIDTH) as i32;
            for y in 0..HEIGHT as i32 {
                display.pixel(x, y, if dx == 0 { red } else { white });
            }
        }
        display.show();
        ticker.next().await;
    }
}

/// Triangle sweep 0 -> 1 -> 0 on solid white. Steps should be smooth and
/// monotonic; flicker or refresh-rate shifts mean OE timing is off.
async fn brightness_sweep(display: &mut Display) {
    info!("pattern: brightness sweep");
    display.fill(rgb565(255, 255, 255));
    display.show();
    for step in 0..=120u32 {
        let level = if step <= 60 { step } else { 120 - step } as f64 / 60.0;
        display.sink_mut().set_brightness(level);
        Timer::after_millis(50).await;
    }
    display.sink_mut().set_brightness(1.0);
}

/// Distinct single pixels in each corner plus a center cross: proves the
/// full address range, both panel halves, and x/y orientation at once.
/// Corners: red top-left, green top-right, blue bottom-left, white
/// bottom-right.
async fn geometry_markers(display: &mut Display) {
    info!("pattern: geometry markers");
    let right = WIDTH as i32 - 1;
    let bottom = HEIGHT as i32 - 1;
    display.fill(0);
    display.pixel(0, 0, rgb565(255, 0, 0));
    display.pixel(right, 0, rgb565(0, 255, 0));
    display.pixel(0, bottom, rgb565(0, 0, 255));
    display.pixel(right, bottom, rgb565(255, 255, 255));

    let yellow = rgb565(255, 255, 0);
    let (cx, cy) = (WIDTH as i32 / 2, HEIGHT as i32 / 2);
    for offset in -2..=2 {
        display.pixel(cx + offset, cy, yellow);
        display.pixel(cx, cy + offset, yellow);
    }
    display.show();
    Timer::after_millis(4000).await;
}
