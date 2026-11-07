//! The bench harness: `--features diag`, absent from every shipped image.
//!
//! A scoreboard whose panel shows whatever core 0 was told over HTTP, on the
//! same radio, HTTP server and panel driver the real image runs — so a panel,
//! a ribbon, a power supply or a pack bug can be cornered one controlled
//! frame at a time, without a reflash per experiment, and with two readbacks
//! that split the question three ways:
//!
//! * `GET /api/diag/frame` returns the 16 KB RGB565 frame core 1 last drew,
//!   copied at a frame boundary. If that is right and the panel is wrong, the
//!   fault is below the renderer.
//! * `GET /api/diag/lanes` counts the set bits per data lane — R1, G1, B1,
//!   R2, G2, B2 — over the bitplane buffer the DMA chain is scanning right
//!   now. If the frame is right and the counts are right, the fault is on the
//!   wire or in the panel; if the counts are wrong, it is the pack.
//!
//! # Endpoints
//!
//! | Route | Body | Effect |
//! |---|---|---|
//! | `PUT /api/diag/frame?offset=N` | raw RGB565, little-endian | Written at byte N of the staged frame. The chunk that reaches the end publishes it and takes the panel |
//! | `POST /api/diag/fill` | `{"pattern":"black"}` or `{"x","y","w","h","rgb":[r,g,b]}` | Painted onto the staged frame, then published |
//! | `POST /api/diag/exit` | — | The panel goes back to the scoreboard |
//! | `GET /api/diag/frame` | — | The last drawn frame, 16,384 B |
//! | `GET /api/diag/lanes` | — | `{"r1":n,…,"b2":n,"bytes":N}` |
//! | `POST /api/diag/led` | `{"toggles":N,"interval_ms":M}` | The LED task fires N on/off IOCTL pairs at that spacing |
//!
//! The frame is staged in chunks because the HTTP request buffer is 4 KB;
//! `tools/diag.py` does the chunking, the image conversion and the PNG on the
//! way back.
//!
//! # Ownership across the cores
//!
//! Two 16 KB frames live here, and the atomics say whose they are at every
//! instant — the same shape as the snapshot channel, without the slots:
//!
//! * [`STAGE`] belongs to core 0 while [`PENDING`] is false — its writers take
//!   [`WRITER`] among themselves — and to core 1 from the moment a writer sets
//!   [`PENDING`] until core 1 has copied it out and cleared the flag.
//! * [`SHOT`] is written by core 1 only while [`SHOT_REQ`] is set, and only the
//!   holder of [`READER`] sets it; that holder reads the bytes only after
//!   [`SHOT_SEQ`] has moved, and keeps the lock for as long as it is reading.
//!
//! Core 1 keeps no cross-frame harness state: [`service`] decides everything
//! from the atomics at the top of the frame, which is what the core-1
//! mutation contract asks of anything on the render path.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::{Mutex, MutexGuard};
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer, with_timeout};
use hub75::display::{FrameBytes, Hub75Display};
use hub75::driver::Hub75Driver;
use hub75::geometry::{HEIGHT, RGB565_FRAME_BYTES, WIDTH};
use hub75::rgb565;

struct Frame(UnsafeCell<FrameBytes>);
// SAFETY: every access is arbitrated by the atomics in the module docs —
// `PENDING` for `STAGE`, `SHOT_REQ`/`SHOT_SEQ` for `SHOT` — so no two cores
// hold a reference to the same frame at once.
unsafe impl Sync for Frame {}

static STAGE: Frame = Frame(UnsafeCell::new([0; RGB565_FRAME_BYTES]));
static SHOT: Frame = Frame(UnsafeCell::new([0; RGB565_FRAME_BYTES]));

/// `STAGE` is core 1's to copy. Set by a writer, cleared by core 1.
static PENDING: AtomicBool = AtomicBool::new(false);
/// The harness owns the panel. Set by the first publish, cleared by core 1 on
/// a release.
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// `POST /api/diag/exit` asked for the panel back.
static RELEASE: AtomicBool = AtomicBool::new(false);
static SHOT_REQ: AtomicBool = AtomicBool::new(false);
static SHOT_SEQ: AtomicU32 = AtomicU32::new(0);
static LANES_REQ: AtomicBool = AtomicBool::new(false);
static LANES_SEQ: AtomicU32 = AtomicU32::new(0);
/// Six lane counts and, last, the number of bytes they were counted over.
static LANES: [AtomicU32; 7] = [const { AtomicU32::new(0) }; 7];
static WRITER: Mutex<CriticalSectionRawMutex, ()> = Mutex::new(());
static READER: Mutex<CriticalSectionRawMutex, ()> = Mutex::new(());
static LED_STORM: Signal<CriticalSectionRawMutex, LedStorm> = Signal::new();

/// How long core 0 waits for core 1 to take a frame or answer a request. A
/// dozen frames: a render loop that has not come round in that long is not
/// going to, and the request reports it rather than hanging a connection.
const CORE1_WAIT: Duration = Duration::from_millis(250);

// ---------------------------------------------------------------------------
// Core 0: staging frames
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// Core 1 has not taken the previous frame within [`CORE1_WAIT`].
    Busy,
    /// The chunk would run past the end of the frame.
    OutOfBounds,
}

/// Block until core 1 has taken the previous frame, so the stage is core 0's.
async fn await_stage() -> Result<(), Refused> {
    with_timeout(CORE1_WAIT, async {
        while PENDING.load(Ordering::Acquire) {
            Timer::after_millis(2).await;
        }
    })
    .await
    .map_err(|_| Refused::Busy)
}

fn publish() {
    ACTIVE.store(true, Ordering::Relaxed);
    PENDING.store(true, Ordering::Release);
}

/// `PUT /api/diag/frame?offset=N`. Returns whether this chunk completed the
/// frame, in which case it has been published.
pub async fn write_chunk(offset: usize, bytes: &[u8]) -> Result<bool, Refused> {
    let end = offset
        .checked_add(bytes.len())
        .filter(|&end| end <= RGB565_FRAME_BYTES)
        .ok_or(Refused::OutOfBounds)?;
    let _writer = WRITER.lock().await;
    await_stage().await?;
    // SAFETY: `PENDING` is false and `WRITER` is held, so core 0 owns `STAGE`
    // and this is its only writer until the guard drops.
    let stage = unsafe { &mut *STAGE.0.get() };
    stage[offset..end].copy_from_slice(bytes);
    let complete = end == RGB565_FRAME_BYTES;
    if complete {
        publish();
    }
    Ok(complete)
}

/// A whole-frame test card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pattern {
    Black,
    White,
    Red,
    Green,
    Blue,
    /// Rows 32–63 blue, the rest black: the lower-half blue lane alone.
    LowerBlue,
    /// Rows 0–31 blue, the rest black.
    UpperBlue,
    /// Upper half white, lower half black.
    Halves,
    /// Eight vertical bars: white, yellow, cyan, green, magenta, red, blue, black.
    Bars,
}

impl Pattern {
    pub fn parse(name: &str) -> Option<Pattern> {
        Some(match name {
            "black" => Pattern::Black,
            "white" => Pattern::White,
            "red" => Pattern::Red,
            "green" => Pattern::Green,
            "blue" => Pattern::Blue,
            "lower_blue" => Pattern::LowerBlue,
            "upper_blue" => Pattern::UpperBlue,
            "halves" => Pattern::Halves,
            "bars" => Pattern::Bars,
            _ => return None,
        })
    }
}

/// A rectangle to paint onto the staged frame. Clipped to the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    pub rgb: [u8; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Pattern(Pattern),
    Fill(Rect),
}

/// `POST /api/diag/fill`: paint onto the stage, then publish it.
pub async fn draw(command: Command) -> Result<(), Refused> {
    let _writer = WRITER.lock().await;
    await_stage().await?;
    // SAFETY: as in `write_chunk`.
    let stage = unsafe { &mut *STAGE.0.get() };
    match command {
        Command::Pattern(pattern) => paint(stage, pattern),
        Command::Fill(rect) => fill(stage, rect),
    }
    publish();
    Ok(())
}

fn solid(stage: &mut FrameBytes, color: u16) {
    let bytes = color.to_le_bytes();
    for pixel in stage.as_chunks_mut::<2>().0 {
        *pixel = bytes;
    }
}

fn fill(stage: &mut FrameBytes, rect: Rect) {
    let color = rgb565(rect.rgb[0], rect.rgb[1], rect.rgb[2]).to_le_bytes();
    let x1 = (rect.x as usize).min(WIDTH);
    let y1 = (rect.y as usize).min(HEIGHT);
    let x2 = (rect.x as usize + rect.w as usize).min(WIDTH);
    let y2 = (rect.y as usize + rect.h as usize).min(HEIGHT);
    for y in y1..y2 {
        for x in x1..x2 {
            let index = (y * WIDTH + x) * 2;
            stage[index..index + 2].copy_from_slice(&color);
        }
    }
}

fn paint(stage: &mut FrameBytes, pattern: Pattern) {
    let half = (HEIGHT / 2) as u16;
    let full = Rect {
        x: 0,
        y: 0,
        w: WIDTH as u16,
        h: HEIGHT as u16,
        rgb: [0; 3],
    };
    match pattern {
        Pattern::Black => solid(stage, 0),
        Pattern::White => solid(stage, rgb565(255, 255, 255)),
        Pattern::Red => solid(stage, rgb565(255, 0, 0)),
        Pattern::Green => solid(stage, rgb565(0, 255, 0)),
        Pattern::Blue => solid(stage, rgb565(0, 0, 255)),
        Pattern::LowerBlue => {
            solid(stage, 0);
            fill(stage, Rect { y: half, h: half, rgb: [0, 0, 255], ..full });
        }
        Pattern::UpperBlue => {
            solid(stage, 0);
            fill(stage, Rect { h: half, rgb: [0, 0, 255], ..full });
        }
        Pattern::Halves => {
            solid(stage, 0);
            fill(stage, Rect { h: half, rgb: [255, 255, 255], ..full });
        }
        Pattern::Bars => {
            const BARS: [[u8; 3]; 8] = [
                [255, 255, 255],
                [255, 255, 0],
                [0, 255, 255],
                [0, 255, 0],
                [255, 0, 255],
                [255, 0, 0],
                [0, 0, 255],
                [0, 0, 0],
            ];
            let bar = (WIDTH / BARS.len()) as u16;
            for (index, rgb) in BARS.iter().enumerate() {
                fill(stage, Rect { x: index as u16 * bar, w: bar, rgb: *rgb, ..full });
            }
        }
    }
}

/// `POST /api/diag/exit`: core 1 hands the panel back at its next frame.
pub fn exit() {
    RELEASE.store(true, Ordering::Release);
}

// ---------------------------------------------------------------------------
// Core 0: reading back
// ---------------------------------------------------------------------------

/// The frame core 1 last drew. Holds [`READER`] for as long as it lives, so
/// the bytes cannot change under a response that is still streaming them.
pub struct Shot(#[allow(dead_code, reason = "held for its lock, never read")] MutexGuard<'static, CriticalSectionRawMutex, ()>);

impl Shot {
    pub fn bytes(&self) -> &FrameBytes {
        // SAFETY: core 1 writes `SHOT` only while `SHOT_REQ` is set, only the
        // `READER` holder sets it, this guard is that holder, and `capture`
        // returned it only after `SHOT_SEQ` moved — the write is complete.
        unsafe { &*SHOT.0.get() }
    }
}

/// Ask core 1 for its frame. `None` if it did not answer within
/// [`CORE1_WAIT`], which means the render loop is not running.
pub async fn capture() -> Option<Shot> {
    let guard = READER.lock().await;
    let before = SHOT_SEQ.load(Ordering::Acquire);
    SHOT_REQ.store(true, Ordering::Release);
    let answered = with_timeout(CORE1_WAIT, async {
        while SHOT_SEQ.load(Ordering::Acquire) == before {
            Timer::after_millis(2).await;
        }
    })
    .await
    .is_ok();
    if !answered {
        // Withdrawn, so a render loop that comes back later does not write
        // under a future reader.
        SHOT_REQ.store(false, Ordering::Relaxed);
        return None;
    }
    Some(Shot(guard))
}

/// Set-bit counts per data lane over the bitplane buffer being scanned, in
/// the lane order R1, G1, B1, R2, G2, B2, followed by the number of bytes
/// counted. `None` if core 1 did not answer.
pub async fn lanes() -> Option<[u32; 7]> {
    let before = LANES_SEQ.load(Ordering::Acquire);
    LANES_REQ.store(true, Ordering::Release);
    with_timeout(CORE1_WAIT, async {
        while LANES_SEQ.load(Ordering::Acquire) == before {
            Timer::after_millis(2).await;
        }
    })
    .await
    .ok()?;
    let mut counts = [0u32; 7];
    for (count, slot) in counts.iter_mut().zip(LANES.iter()) {
        *count = slot.load(Ordering::Relaxed);
    }
    Some(counts)
}

// ---------------------------------------------------------------------------
// Core 0: the LED storm
// ---------------------------------------------------------------------------

/// `POST /api/diag/led`: `toggles` on/off pairs, `interval_ms` apart. The LED
/// task plays it in place of its pattern and then resumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedStorm {
    pub toggles: u16,
    pub interval_ms: u16,
}

pub fn request_led_storm(storm: LedStorm) {
    LED_STORM.signal(storm);
}

/// The LED task's end. A second request before the first has played
/// replaces it, which is what a storm wants.
pub fn take_led_storm() -> Option<LedStorm> {
    LED_STORM.try_take()
}

// ---------------------------------------------------------------------------
// Core 1
// ---------------------------------------------------------------------------

/// What [`service`] decided this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The scoreboard draws as usual.
    Inactive,
    /// The harness has the panel; the scoreboard's draw is skipped.
    Owned,
    /// The harness just gave the panel back: the scoreboard's memo is stale
    /// and its next frame must redraw.
    Released,
}

/// Core 1, once per frame, before the scoreboard's own draw.
pub fn service(display: &mut Hub75Display<'static, Hub75Driver>) -> Verdict {
    if PENDING.load(Ordering::Acquire) {
        // SAFETY: `PENDING` set means the writer finished and handed `STAGE`
        // over; no core-0 writer touches it again until the store below.
        let stage = unsafe { &*STAGE.0.get() };
        display.buffer_mut().copy_from_slice(stage);
        PENDING.store(false, Ordering::Release);
        display.show();
    }
    if SHOT_REQ.load(Ordering::Acquire) {
        // SAFETY: the `READER` holder asked and reads only after the
        // `fetch_add` below; nobody else touches `SHOT`.
        let shot = unsafe { &mut *SHOT.0.get() };
        shot.copy_from_slice(display.buffer());
        SHOT_REQ.store(false, Ordering::Relaxed);
        SHOT_SEQ.fetch_add(1, Ordering::Release);
    }
    if LANES_REQ.load(Ordering::Acquire) {
        let planes = display.sink_mut().active_planes();
        let mut counts = [0u32; 7];
        for &byte in planes {
            for (lane, count) in counts[..6].iter_mut().enumerate() {
                *count += u32::from((byte >> lane) & 1);
            }
        }
        counts[6] = planes.len() as u32;
        for (slot, count) in LANES.iter().zip(counts) {
            slot.store(count, Ordering::Relaxed);
        }
        LANES_REQ.store(false, Ordering::Relaxed);
        LANES_SEQ.fetch_add(1, Ordering::Release);
    }
    if RELEASE.swap(false, Ordering::AcqRel) && ACTIVE.swap(false, Ordering::Relaxed) {
        return Verdict::Released;
    }
    if ACTIVE.load(Ordering::Relaxed) {
        Verdict::Owned
    } else {
        Verdict::Inactive
    }
}
