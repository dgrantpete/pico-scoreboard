//! RGB888 drawing surface over a frame sink (port of `display.py`'s
//! `Hub75Display`). On the device the sink is [`Hub75Driver`]; with the
//! `simulator` feature it can be [`crate::sim::SimulatorSink`], giving
//! scoreboard-render pixel-assertable frames on the host through the same
//! drawing API.

use crate::driver::Hub75Driver;
use crate::geometry::{HEIGHT, RGB888_FRAME_BYTES, WIDTH};

/// One RGB888 frame: three bytes per pixel (R, G, B), row-major.
pub type FrameBytes = [u8; RGB888_FRAME_BYTES];

/// Where finished frames go on `show()`.
pub trait FrameSink {
    fn show(&mut self, frame: &FrameBytes);
}

impl FrameSink for Hub75Driver {
    fn show(&mut self, frame: &FrameBytes) {
        self.load_rgb888(frame);
        self.flip();
    }
}

/// Drawing surface with a caller-owned back buffer (24,576 B — a static on
/// the device, so constructing the display never moves the frame through
/// the stack; anything local in host tests).
///
/// Coordinates are `i32` and out-of-bounds pixels clip silently, matching
/// MicroPython `framebuf` semantics the render code was written against.
pub struct Hub75Display<'buffer, S: FrameSink> {
    buffer: &'buffer mut FrameBytes,
    sink: S,
}

impl<'buffer, S: FrameSink> Hub75Display<'buffer, S> {
    pub fn new(buffer: &'buffer mut FrameBytes, sink: S) -> Self {
        Hub75Display { buffer, sink }
    }

    pub const fn width(&self) -> usize {
        WIDTH
    }

    pub const fn height(&self) -> usize {
        HEIGHT
    }

    /// Fill the whole back buffer with one `[r, g, b]` color.
    pub fn fill(&mut self, color: [u8; 3]) {
        for pixel in self.buffer.as_chunks_mut::<3>().0 {
            *pixel = color;
        }
    }

    /// Set one pixel; out-of-bounds coordinates are ignored.
    pub fn pixel(&mut self, x: i32, y: i32, color: [u8; 3]) {
        if (0..WIDTH as i32).contains(&x) && (0..HEIGHT as i32).contains(&y) {
            let index = (y as usize * WIDTH + x as usize) * 3;
            self.buffer[index..index + 3].copy_from_slice(&color);
        }
    }

    /// Read one pixel from the back buffer; `None` out of bounds.
    pub fn pixel_at(&self, x: i32, y: i32) -> Option<[u8; 3]> {
        ((0..WIDTH as i32).contains(&x) && (0..HEIGHT as i32).contains(&y)).then(|| {
            let index = (y as usize * WIDTH + x as usize) * 3;
            [
                self.buffer[index],
                self.buffer[index + 1],
                self.buffer[index + 2],
            ]
        })
    }

    /// The raw back buffer, for bulk operations.
    pub fn buffer(&self) -> &FrameBytes {
        self.buffer
    }

    pub fn buffer_mut(&mut self) -> &mut FrameBytes {
        self.buffer
    }

    /// Push the back buffer to the sink (device: `load_rgb888` + `flip`).
    pub fn show(&mut self) {
        self.sink.show(self.buffer);
    }

    /// Access the sink, e.g. to reach driver controls like `set_brightness`.
    pub fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }
}
