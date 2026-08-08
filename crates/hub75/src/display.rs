//! RGB565 drawing surface over a frame sink (port of `display.py`'s
//! `Hub75Display`). On the device the sink is [`Hub75Driver`]; with the
//! `simulator` feature it can be [`crate::sim::SimulatorSink`], giving
//! scoreboard-render pixel-assertable frames on the host through the same
//! drawing API.

use crate::driver::Hub75Driver;
use crate::geometry::{HEIGHT, RGB565_FRAME_BYTES, WIDTH};

/// One RGB565 frame, little-endian, `framebuf.RGB565` layout.
pub type FrameBytes = [u8; RGB565_FRAME_BYTES];

/// Where finished frames go on `show()`.
pub trait FrameSink {
    fn show(&mut self, frame: &FrameBytes);
}

impl FrameSink for Hub75Driver {
    fn show(&mut self, frame: &FrameBytes) {
        self.load_rgb565(frame);
        self.flip();
    }
}

/// Drawing surface with a caller-owned back buffer (16,384 B — a static on
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

    /// Fill the whole back buffer with one color.
    pub fn fill(&mut self, color: u16) {
        let [lo, hi] = color.to_le_bytes();
        for pixel in self.buffer.chunks_exact_mut(2) {
            pixel[0] = lo;
            pixel[1] = hi;
        }
    }

    /// Set one pixel; out-of-bounds coordinates are ignored.
    pub fn pixel(&mut self, x: i32, y: i32, color: u16) {
        if (0..WIDTH as i32).contains(&x) && (0..HEIGHT as i32).contains(&y) {
            let index = (y as usize * WIDTH + x as usize) * 2;
            let [lo, hi] = color.to_le_bytes();
            self.buffer[index] = lo;
            self.buffer[index + 1] = hi;
        }
    }

    /// Read one pixel from the back buffer; `None` out of bounds.
    pub fn pixel_at(&self, x: i32, y: i32) -> Option<u16> {
        ((0..WIDTH as i32).contains(&x) && (0..HEIGHT as i32).contains(&y)).then(|| {
            let index = (y as usize * WIDTH + x as usize) * 2;
            u16::from_le_bytes([self.buffer[index], self.buffer[index + 1]])
        })
    }

    /// Blit an RGB565 sprite (`src_width * src_height * 2` bytes, same
    /// little-endian layout) at `(x, y)`, clipping at the edges. Pixels
    /// equal to `key` are transparent.
    pub fn blit(
        &mut self,
        src: &[u8],
        src_width: usize,
        src_height: usize,
        x: i32,
        y: i32,
        key: Option<u16>,
    ) {
        assert_eq!(src.len(), src_width * src_height * 2, "sprite size mismatch");
        for row in 0..src_height as i32 {
            let dst_y = y + row;
            if !(0..HEIGHT as i32).contains(&dst_y) {
                continue;
            }
            for col in 0..src_width as i32 {
                let dst_x = x + col;
                if !(0..WIDTH as i32).contains(&dst_x) {
                    continue;
                }
                let src_index = (row as usize * src_width + col as usize) * 2;
                let color = u16::from_le_bytes([src[src_index], src[src_index + 1]]);
                if key == Some(color) {
                    continue;
                }
                let dst_index = (dst_y as usize * WIDTH + dst_x as usize) * 2;
                self.buffer[dst_index] = src[src_index];
                self.buffer[dst_index + 1] = src[src_index + 1];
            }
        }
    }

    /// The raw back buffer, for bulk operations.
    pub fn buffer(&self) -> &FrameBytes {
        self.buffer
    }

    pub fn buffer_mut(&mut self) -> &mut FrameBytes {
        self.buffer
    }

    /// Push the back buffer to the sink (device: `load_rgb565` + `flip`).
    pub fn show(&mut self) {
        self.sink.show(self.buffer);
    }

    /// Access the sink, e.g. to reach driver controls like `set_brightness`.
    pub fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }
}
