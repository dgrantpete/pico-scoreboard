//! Events: a full-screen animated clip that takes the panel over on a cadence
//! while a date window is open — a friend's birthday, say.
//!
//! Three pieces, all pure, all host-tested (`tests/event.rs`):
//!
//! * [`Clip`] — the `.sbev` format and its decoder. The format is specified
//!   here, normatively; `tools/events/encode_clip.py` is its only encoder.
//! * [`Window`] — which local calendar days an event is live on.
//! * [`EventPlayer`] — when a clip plays, what each render-loop tick shows, and
//!   how bright. Cross-frame state, so it lives in the app's loop-local
//!   `LoopState` beside the [`FrameRail`](crate::time::FrameRail) and
//!   [`SkipMemo`](crate::SkipMemo), and no renderer can reach it.
//!
//! What stays in the app is the wiring: which events exist and where their
//! bytes live, the atomics core 0 uses to say "this event is live" and "a
//! button was pressed", and the driver calls.
//!
//! # The clip format (`.sbev`, version 1)
//!
//! All integers little-endian.
//!
//! | offset | size | field |
//! |---|---|---|
//! | 0 | 4 | magic `SBEV` |
//! | 4 | 1 | version, `1` |
//! | 5 | 1 | fps — must divide [`FPS`] so every clip frame lasts whole ticks |
//! | 6 | 2 | frame count, ≥ 1 |
//! | 8 | 2 | palette length `P`, `1..=256` |
//! | 10 | 1 | width, must be [`WIDTH`] |
//! | 11 | 1 | height, must be [`HEIGHT`] |
//! | 12 | `2P` | palette: RGB565, the panel's own byte order |
//! | … | `4(F+1)` | frame offsets into the op data; first `0`, last = its length |
//! | … | | op data: one stream per frame |
//!
//! A frame's op stream rewrites the *previous* frame into this one, walking a
//! pixel cursor row-major from 0 to exactly `WIDTH × HEIGHT`. Each op is one
//! control byte — the top two bits the op, the low six the run length minus
//! one (so 1..=64):
//!
//! | bits | op | payload |
//! |---|---|---|
//! | `00` | skip — pixels unchanged from the previous frame | none |
//! | `01` | fill — `run` pixels of one colour | 1 palette index |
//! | `10` | literal — `run` pixels, one index each | `run` palette indices |
//! | `11` | reserved; decoding fails | |
//!
//! Frame 0 may not skip: it is the keyframe, and whatever was on the surface
//! before it belongs to another screen.
//!
//! ## Why this shape and not deflate
//!
//! The decoder writes straight into the render loop's existing 16 KB frame
//! surface, reading the clip in place from flash, and needs no RAM of its own —
//! the palette is looked up where it lies. An inflater needs ~11 KB of tables
//! plus a history window, against ~14.5 KB of static-RAM headroom (BUDGET.md).
//! For the first clip the difference was 324 KB against ~250 KB; flash is the
//! resource there was room in.
//!
//! # Playback
//!
//! ```text
//!  waiting ──due & interruptible──▶ leaving ──▶ playing ──▶ returning ──▶ waiting
//!             (screen, full)         (screen,    (clip,      (screen,
//!                                    fading out)  fades in   fading in)
//!                                                 and out)
//! ```
//!
//! The fades are the panel's brightness, not pixels: the clip carries no fade
//! frames, because a fade rewrites every pixel of every frame it covers and
//! baking one in measured at ~150 KB. The app multiplies the user's brightness
//! by [`Tick::fade`].
//!
//! Clip frames advance on **ticks**, not wall time — continuous motion, so
//! under the crate's rail rule ("a stall stretches motion but consumes
//! waiting") it belongs on the frame rail, and counting ticks is that rail.
//! There is a second reason it could not be otherwise: every frame is a delta
//! on the one before, so a frame can never be skipped to catch up.
//!
//! The *cadence* is the other rail: minutes between plays are waiting, so they
//! are measured on the wall clock ([`WallMs`]), from the end of the last play.

use scoreboard_model::{Millis, Mode};

use crate::geometry::{HEIGHT, WIDTH};
use crate::time::{FPS, WallMs};

// ------------------------------------------------------------------ the clip

const MAGIC: &[u8; 4] = b"SBEV";
const VERSION: u8 = 1;
const HEADER_BYTES: usize = 12;
const PIXELS: usize = (WIDTH * HEIGHT) as usize;
const SURFACE_BYTES: usize = PIXELS * 2;

const OP_MASK: u8 = 0xC0;
const OP_SKIP: u8 = 0x00;
const OP_FILL: u8 = 0x40;
const OP_LITERAL: u8 = 0x80;

/// Why a clip could not be parsed or a frame could not be decoded.
///
/// Nothing here panics: a clip is data, and a bad one costs the event, never
/// the render loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipError {
    /// Shorter than its header, palette or offset table says it is.
    Truncated,
    BadMagic,
    UnsupportedVersion(u8),
    /// Not 128×64.
    BadGeometry,
    /// Zero, or does not divide [`FPS`].
    BadFps(u8),
    /// Zero frames or zero palette entries.
    Empty,
    /// The offset table does not start at 0, runs backwards, or does not end
    /// exactly at the end of the op data.
    BadOffsets,
    /// A frame index past the end of the clip.
    NoSuchFrame(u16),
    /// The surface is not one RGB565 frame.
    BadSurface,
    /// A frame's op stream is malformed: a reserved op, a palette index out of
    /// range, a skip in the keyframe, or a cursor that does not land exactly on
    /// the last pixel. The surface holds a partial frame.
    Corrupt(u16),
}

/// How a clip spends render-loop ticks — everything [`EventPlayer`] needs to
/// know about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipTiming {
    pub frames: u16,
    /// Render-loop ticks each clip frame stays up: `FPS / clip fps`.
    pub ticks_per_frame: u8,
}

impl ClipTiming {
    /// Ticks the clip plays for, start to finish.
    pub const fn ticks(self) -> u32 {
        self.frames as u32 * self.ticks_per_frame as u32
    }
}

/// A parsed `.sbev` clip, borrowing its bytes — on the device, straight out of
/// flash.
#[derive(Debug, Clone, Copy)]
pub struct Clip<'a> {
    timing: ClipTiming,
    palette: &'a [u8],
    offsets: &'a [u8],
    ops: &'a [u8],
}

impl<'a> Clip<'a> {
    /// Validate the header and the offset table.
    ///
    /// The op streams themselves are checked as they decode, one frame at a
    /// time, so parsing touches a few hundred bytes rather than the whole clip.
    /// The clips that ship are also decoded end to end on the host
    /// (`tests/event.rs`), against hashes the encoder wrote.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ClipError> {
        let header = bytes.get(..HEADER_BYTES).ok_or(ClipError::Truncated)?;
        if &header[0..4] != MAGIC {
            return Err(ClipError::BadMagic);
        }
        if header[4] != VERSION {
            return Err(ClipError::UnsupportedVersion(header[4]));
        }
        let fps = header[5];
        if fps == 0 || !FPS.is_multiple_of(fps as u32) {
            return Err(ClipError::BadFps(fps));
        }
        let frames = u16::from_le_bytes([header[6], header[7]]);
        let palette_len = u16::from_le_bytes([header[8], header[9]]) as usize;
        if header[10] as i32 != WIDTH || header[11] as i32 != HEIGHT {
            return Err(ClipError::BadGeometry);
        }
        if frames == 0 || palette_len == 0 || palette_len > 256 {
            return Err(ClipError::Empty);
        }

        let rest = &bytes[HEADER_BYTES..];
        let (palette, rest) = split(rest, palette_len * 2)?;
        let (offsets, ops) = split(rest, (frames as usize + 1) * 4)?;

        let clip = Clip {
            timing: ClipTiming {
                frames,
                ticks_per_frame: (FPS / fps as u32) as u8,
            },
            palette,
            offsets,
            ops,
        };
        let mut previous = 0;
        for index in 0..=frames as usize {
            let offset = clip.offset(index);
            if (index == 0 && offset != 0) || offset < previous {
                return Err(ClipError::BadOffsets);
            }
            previous = offset;
        }
        if previous != ops.len() {
            return Err(ClipError::BadOffsets);
        }
        Ok(clip)
    }

    pub const fn timing(&self) -> ClipTiming {
        self.timing
    }

    /// Decode frame `frame` onto `surface`, which must hold frame `frame - 1`
    /// (or anything at all, for frame 0).
    ///
    /// Writes the palette's bytes as they lie, so a clip frame and a rendered
    /// frame are the same RGB565 layout by construction.
    pub fn decode_into(&self, frame: u16, surface: &mut [u8]) -> Result<(), ClipError> {
        if frame >= self.timing.frames {
            return Err(ClipError::NoSuchFrame(frame));
        }
        if surface.len() != SURFACE_BYTES {
            return Err(ClipError::BadSurface);
        }
        let stream = &self.ops[self.offset(frame as usize)..self.offset(frame as usize + 1)];
        let corrupt = ClipError::Corrupt(frame);
        let keyframe = frame == 0;

        let mut cursor = 0usize;
        let mut at = 0usize;
        while at < stream.len() {
            let control = stream[at];
            at += 1;
            let run = (control & !OP_MASK) as usize + 1;
            let end = cursor + run;
            if end > PIXELS {
                return Err(corrupt);
            }
            match control & OP_MASK {
                OP_SKIP if !keyframe => {}
                OP_FILL => {
                    let colour = self
                        .colour(*stream.get(at).ok_or(corrupt)?)
                        .ok_or(corrupt)?;
                    at += 1;
                    for pixel in surface[cursor * 2..end * 2].chunks_exact_mut(2) {
                        pixel.copy_from_slice(&colour);
                    }
                }
                OP_LITERAL => {
                    let indices = stream.get(at..at + run).ok_or(corrupt)?;
                    at += run;
                    let pixels = surface[cursor * 2..end * 2].chunks_exact_mut(2);
                    for (pixel, &index) in pixels.zip(indices) {
                        pixel.copy_from_slice(&self.colour(index).ok_or(corrupt)?);
                    }
                }
                _ => return Err(corrupt),
            }
            cursor = end;
        }
        if cursor != PIXELS {
            return Err(corrupt);
        }
        Ok(())
    }

    fn offset(&self, index: usize) -> usize {
        let b = &self.offsets[index * 4..index * 4 + 4];
        u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize
    }

    fn colour(&self, index: u8) -> Option<[u8; 2]> {
        let b = self
            .palette
            .get(index as usize * 2..index as usize * 2 + 2)?;
        Some([b[0], b[1]])
    }
}

fn split(bytes: &[u8], at: usize) -> Result<(&[u8], &[u8]), ClipError> {
    if bytes.len() < at {
        return Err(ClipError::Truncated);
    }
    Ok(bytes.split_at(at))
}

// ---------------------------------------------------------------- the window

/// Days since 1970-01-01 for a proleptic Gregorian date — Howard Hinnant's
/// `days_from_civil`, the inverse of `scoreboard-model`'s `civil_from_unix`.
///
/// `const` so an event's window is a compile-time pair of integers and the
/// device never does calendar arithmetic: "is today in the window" is two
/// integer compares against [`local_day`].
pub const fn days_from_civil(year: i32, month: u8, day: u8) -> i32 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = (if year >= 0 { year } else { year - 399 }) / 400;
    let year_of_era = year - era * 400;
    let month = month as i32;
    let day_of_year =
        (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day as i32 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The local calendar day for a Unix instant, as days since 1970-01-01.
///
/// An unknown offset reads as UTC. That is a deliberate departure from the
/// pregame line, which omits a start time rather than guess: an event's window
/// is whole days, so a guessed offset moves its edges by hours at worst, where
/// refusing to guess would lose the whole event on a device whose offset
/// lookup failed.
pub const fn local_day(epoch_s: u32, utc_offset_s: Option<i32>) -> i32 {
    let offset = match utc_offset_s {
        Some(offset) => offset as i64,
        None => 0,
    };
    (epoch_s as i64 + offset).div_euclid(86_400) as i32
}

/// The local days an event is live on, inclusive at both ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    first_day: i32,
    last_day: i32,
}

impl Window {
    /// From `(year, month, day)` to `(year, month, day)`, both inclusive.
    pub const fn new(first: (i32, u8, u8), last: (i32, u8, u8)) -> Self {
        Window {
            first_day: days_from_civil(first.0, first.1, first.2),
            last_day: days_from_civil(last.0, last.1, last.2),
        }
    }

    pub const fn contains(&self, day: i32) -> bool {
        self.first_day <= day && day <= self.last_day
    }
}

// ---------------------------------------------------------------- the player

/// How often a live event plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cadence {
    /// From the moment an event goes live — which after a reboot is the first
    /// poll after the clock syncs — to its first play. Short, so the person it
    /// is for sees it soon after the scoreboard powers up.
    pub first_ms: Millis,
    /// Between plays, end to start, on any screen but a live game's.
    pub interval_ms: Millis,
    /// Between plays while a game is live. Longer, because thirteen seconds is
    /// long enough to miss a scoring play.
    pub live_interval_ms: Millis,
}

pub const CADENCE: Cadence = Cadence {
    first_ms: 15_000,
    interval_ms: 3 * 60_000,
    live_interval_ms: 10 * 60_000,
};

/// Fade durations, in ticks at [`FPS`].
const LEAVE_TICKS: u32 = ticks(300);
const CLIP_FADE_IN_TICKS: u32 = ticks(600);
const CLIP_FADE_OUT_TICKS: u32 = ticks(700);
const RETURN_TICKS: u32 = ticks(300);

const fn ticks(ms: u32) -> u32 {
    ms * FPS / 1_000
}

/// What one render-loop tick puts on the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Draw {
    /// The ordinary screen, through the skip memo as usual. `redraw` means the
    /// clip has been on the surface, so the memo must be invalidated — a
    /// static screen has no commit coming to repaint it.
    Screen { redraw: bool },
    /// Decode this clip frame onto the surface and show it. Frames arrive in
    /// order, each exactly once, starting from 0.
    ClipFrame(u16),
    /// The clip frame already on the panel stays up; draw nothing.
    Hold,
}

/// One tick's instructions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tick {
    pub draw: Draw,
    /// Scale for the user's brightness, `0..=255` (255 = unchanged).
    pub fade: u8,
    /// Whether a button press right now should dismiss the event instead of
    /// doing its usual job. True while the screen is going or the clip is up —
    /// a press is always *about* what the person is looking at.
    pub captures_input: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Waiting,
    Leaving { tick: u32, timing: ClipTiming },
    Playing { tick: u32, timing: ClipTiming },
    Returning { tick: u32 },
}

/// When the live event plays and what each tick shows. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventPlayer {
    phase: Phase,
    /// When the current event went live, on the wall rail.
    armed_ms: Option<Millis>,
    /// When the last play finished, on the wall rail. Survives the event
    /// ending, which is harmless: the next event goes live days later.
    finished_ms: Option<Millis>,
}

impl EventPlayer {
    pub const fn new() -> Self {
        EventPlayer {
            phase: Phase::Waiting,
            armed_ms: None,
            finished_ms: None,
        }
    }

    /// Advance one render-loop tick.
    ///
    /// * `event` — the live event's clip, or `None` when no window is open.
    ///   If it goes away or changes mid-play, the play ends.
    /// * `mode`, `menu_active` — what the ordinary screen is. Setup, startup,
    ///   an update in progress and the league menu are never covered, and one
    ///   appearing mid-play ends the play at once.
    /// * `dismissed` — a button was pressed while [`Tick::captures_input`]
    ///   was set.
    pub fn tick(
        &mut self,
        now: WallMs,
        event: Option<ClipTiming>,
        mode: Mode,
        menu_active: bool,
        dismissed: bool,
    ) -> Tick {
        let interruptible =
            !menu_active && !matches!(mode, Mode::Startup | Mode::Setup | Mode::Updating);

        match self.phase {
            Phase::Waiting => {
                let Some(timing) = event else {
                    self.armed_ms = None;
                    return screen(false, u8::MAX);
                };
                let armed = *self.armed_ms.get_or_insert(now.0);
                let due = match self.finished_ms {
                    None => armed + CADENCE.first_ms,
                    Some(finished) if is_live(mode) => finished + CADENCE.live_interval_ms,
                    Some(finished) => finished + CADENCE.interval_ms,
                };
                if now.0 < due || !interruptible {
                    return screen(false, u8::MAX);
                }
                self.phase = Phase::Leaving { tick: 0, timing };
                self.tick(now, event, mode, menu_active, false)
            }
            Phase::Leaving { tick, timing } => {
                if !interruptible || event != Some(timing) {
                    return self.finish(now, false);
                }
                if dismissed {
                    // Mirror the fade so the brightness carries on from where
                    // it is: `ease(1 - x) == 1 - ease(x)`.
                    return self.enter_returning(now, RETURN_TICKS.saturating_sub(tick), false);
                }
                let fade = u8::MAX - ease(tick, LEAVE_TICKS);
                self.phase = if tick + 1 >= LEAVE_TICKS {
                    Phase::Playing { tick: 0, timing }
                } else {
                    Phase::Leaving {
                        tick: tick + 1,
                        timing,
                    }
                };
                Tick {
                    draw: Draw::Screen { redraw: false },
                    fade,
                    captures_input: true,
                }
            }
            Phase::Playing { tick, timing } => {
                if !interruptible || event != Some(timing) {
                    return self.finish(now, true);
                }
                if dismissed {
                    return self.enter_returning(now, 0, true);
                }
                let per_frame = timing.ticks_per_frame as u32;
                let draw = if tick % per_frame == 0 {
                    Draw::ClipFrame((tick / per_frame) as u16)
                } else {
                    Draw::Hold
                };
                let remaining = timing.ticks() - 1 - tick;
                let fade = ease(tick, CLIP_FADE_IN_TICKS).min(ease(remaining, CLIP_FADE_OUT_TICKS));
                self.phase = if tick + 1 >= timing.ticks() {
                    Phase::Returning { tick: 0 }
                } else {
                    Phase::Playing {
                        tick: tick + 1,
                        timing,
                    }
                };
                Tick {
                    draw,
                    fade,
                    captures_input: true,
                }
            }
            Phase::Returning { tick } => {
                if !interruptible {
                    return self.finish(now, tick == 0);
                }
                self.enter_returning(now, tick, tick == 0)
            }
        }
    }

    /// End the current play now — the decoder failed, or the app swapped the
    /// clip under it. The next tick draws the ordinary screen, fading back in.
    ///
    /// A no-op unless the screen is going or the clip is up, so the app can
    /// call it on any clip change without dimming a screen that never left.
    pub fn abort(&mut self, now: WallMs) {
        if matches!(self.phase, Phase::Leaving { .. } | Phase::Playing { .. }) {
            self.phase = Phase::Returning { tick: 0 };
            // A clip that fails once fails every time, and the cadence is what
            // stops that becoming a flicker every few minutes rather than never.
            self.finished_ms = Some(now.0);
        }
    }

    fn enter_returning(&mut self, now: WallMs, tick: u32, redraw: bool) -> Tick {
        if tick >= RETURN_TICKS {
            return self.finish(now, redraw);
        }
        self.phase = Phase::Returning { tick: tick + 1 };
        Tick {
            draw: Draw::Screen { redraw },
            fade: ease(tick, RETURN_TICKS),
            captures_input: false,
        }
    }

    fn finish(&mut self, now: WallMs, redraw: bool) -> Tick {
        self.phase = Phase::Waiting;
        self.finished_ms = Some(now.0);
        screen(redraw, u8::MAX)
    }
}

impl Default for EventPlayer {
    fn default() -> Self {
        Self::new()
    }
}

const fn screen(redraw: bool, fade: u8) -> Tick {
    Tick {
        draw: Draw::Screen { redraw },
        fade,
        captures_input: false,
    }
}

const fn is_live(mode: Mode) -> bool {
    matches!(
        mode,
        Mode::MlbLive | Mode::SoccerLive | Mode::NbaLive | Mode::FootballLive
    )
}

/// Smoothstep of `tick / span`, as `0..=255`. Integer-only, like [`crate::pulse`].
const fn ease(tick: u32, span: u32) -> u8 {
    if tick >= span {
        return u8::MAX;
    }
    let (x, n) = (tick as u64, span as u64);
    // 255 · (3x²n − 2x³) / n³
    (255 * (3 * x * x * n - 2 * x * x * x) / (n * n * n)) as u8
}
