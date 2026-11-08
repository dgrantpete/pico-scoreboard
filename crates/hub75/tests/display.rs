//! Display drawing API over the simulator sink: what scoreboard-render's
//! host tests will build on.

use hub75::display::Hub75Display;
use hub75::geometry::{HEIGHT, RGB888_FRAME_BYTES, WIDTH};
use hub75::sim::SimulatorSink;

fn display() -> Hub75Display<'static, SimulatorSink> {
    let buffer = Box::leak(Box::new([0u8; RGB888_FRAME_BYTES]));
    Hub75Display::new(buffer, SimulatorSink::new())
}

#[test]
fn pixels_land_where_drawn() {
    let mut display = display();
    let red = [255, 0, 0];
    let cyan = [0, 255, 255];

    display.fill([0, 0, 0]);
    display.pixel(0, 0, red);
    display.pixel(WIDTH as i32 - 1, HEIGHT as i32 - 1, cyan);
    display.show();

    let sink = display.sink_mut();
    assert_eq!(sink.pixel_at(0, 0), red);
    assert_eq!(sink.pixel_at(WIDTH - 1, HEIGHT - 1), cyan);
    assert_eq!(sink.pixel_at(1, 0), [0, 0, 0]);
    assert_eq!(sink.frames_shown(), 1);
}

#[test]
fn pixels_are_three_bytes_in_rgb_order() {
    let mut display = display();
    display.pixel(1, 0, [0x12, 0x34, 0x56]);
    assert_eq!(&display.buffer()[3..6], &[0x12, 0x34, 0x56]);
    assert_eq!(display.pixel_at(1, 0), Some([0x12, 0x34, 0x56]));
}

#[test]
fn out_of_bounds_pixels_clip_silently() {
    let mut display = display();
    let white = [255, 255, 255];
    display.pixel(-1, 0, white);
    display.pixel(0, -1, white);
    display.pixel(WIDTH as i32, 0, white);
    display.pixel(0, HEIGHT as i32, white);
    assert!(display.buffer().iter().all(|&b| b == 0));
    assert_eq!(display.pixel_at(WIDTH as i32, 0), None);
}

#[test]
fn fill_covers_every_pixel() {
    let mut display = display();
    let amber = [255, 191, 0];
    display.fill(amber);
    display.show();
    let sink = display.sink_mut();
    for y in (0..HEIGHT).step_by(7) {
        for x in (0..WIDTH).step_by(11) {
            assert_eq!(sink.pixel_at(x, y), amber);
        }
    }
}
