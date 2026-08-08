//! Host-side frame sink: `show()` lands frames in a plain in-memory RGB565
//! buffer instead of the panel, so render code driving a
//! [`Hub75Display`](crate::display::Hub75Display) can be pixel-asserted on
//! the desktop.

use crate::display::{FrameBytes, FrameSink};
use crate::geometry::{HEIGHT, RGB565_FRAME_BYTES, WIDTH};

pub struct SimulatorSink {
    front: FrameBytes,
    frames_shown: usize,
}

impl SimulatorSink {
    pub fn new() -> Self {
        SimulatorSink {
            front: [0; RGB565_FRAME_BYTES],
            frames_shown: 0,
        }
    }

    /// The last frame `show()`n.
    pub fn front(&self) -> &FrameBytes {
        &self.front
    }

    /// Read a pixel from the last shown frame (panics out of bounds — the
    /// simulator is for tests, where that is a bug worth failing loudly).
    pub fn pixel_at(&self, x: usize, y: usize) -> u16 {
        assert!(x < WIDTH && y < HEIGHT, "pixel out of bounds");
        let index = (y * WIDTH + x) * 2;
        u16::from_le_bytes([self.front[index], self.front[index + 1]])
    }

    pub fn frames_shown(&self) -> usize {
        self.frames_shown
    }
}

impl Default for SimulatorSink {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameSink for SimulatorSink {
    fn show(&mut self, frame: &FrameBytes) {
        self.front = *frame;
        self.frames_shown += 1;
    }
}
