//! Display drawing API over the simulator sink: what scoreboard-render's
//! host tests will build on.

use hub75::display::Hub75Display;
use hub75::geometry::{HEIGHT, RGB565_FRAME_BYTES, WIDTH};
use hub75::rgb565;
use hub75::sim::SimulatorSink;

fn display() -> Hub75Display<'static, SimulatorSink> {
    let buffer = Box::leak(Box::new([0u8; RGB565_FRAME_BYTES]));
    Hub75Display::new(buffer, SimulatorSink::new())
}

#[test]
fn pixels_land_where_drawn() {
    let mut display = display();
    let red = rgb565(255, 0, 0);
    let cyan = rgb565(0, 255, 255);

    display.fill(rgb565(0, 0, 0));
    display.pixel(0, 0, red);
    display.pixel(WIDTH as i32 - 1, HEIGHT as i32 - 1, cyan);
    display.show();

    let sink = display.sink_mut();
    assert_eq!(sink.pixel_at(0, 0), red);
    assert_eq!(sink.pixel_at(WIDTH - 1, HEIGHT - 1), cyan);
    assert_eq!(sink.pixel_at(1, 0), 0);
    assert_eq!(sink.frames_shown(), 1);
}

#[test]
fn out_of_bounds_pixels_clip_silently() {
    let mut display = display();
    display.pixel(-1, 0, 0xFFFF);
    display.pixel(0, -1, 0xFFFF);
    display.pixel(WIDTH as i32, 0, 0xFFFF);
    display.pixel(0, HEIGHT as i32, 0xFFFF);
    assert!(display.buffer().iter().all(|&b| b == 0));
    assert_eq!(display.pixel_at(WIDTH as i32, 0), None);
}

#[test]
fn blit_clips_and_keys() {
    let mut display = display();
    let key = 0xF81F;
    let solid = rgb565(0, 255, 0);

    // 2x2 sprite: one keyed (transparent) pixel.
    let mut sprite = Vec::new();
    for color in [solid, key, solid, solid] {
        sprite.extend_from_slice(&color.to_le_bytes());
    }

    display.blit(&sprite, 2, 2, 0, 0, Some(key));
    assert_eq!(display.pixel_at(0, 0), Some(solid));
    assert_eq!(display.pixel_at(1, 0), Some(0), "keyed pixel must not draw");
    assert_eq!(display.pixel_at(0, 1), Some(solid));
    assert_eq!(display.pixel_at(1, 1), Some(solid));

    // Straddling the right edge: off-screen columns clip, on-screen draw.
    display.blit(&sprite, 2, 2, WIDTH as i32 - 1, 0, None);
    assert_eq!(display.pixel_at(WIDTH as i32 - 1, 0), Some(solid));
    assert_eq!(display.pixel_at(WIDTH as i32 - 1, 1), Some(solid));
}

#[test]
fn fill_covers_every_pixel() {
    let mut display = display();
    let amber = rgb565(255, 191, 0);
    display.fill(amber);
    display.show();
    let sink = display.sink_mut();
    for y in (0..HEIGHT).step_by(7) {
        for x in (0..WIDTH).step_by(11) {
            assert_eq!(sink.pixel_at(x, y), amber);
        }
    }
}
