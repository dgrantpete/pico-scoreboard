//! `scoreboard_render::event`: the clip decoder against the clips that ship,
//! the decoder against malformed input, the date window, and the player's
//! cadence, fades and interruptions.

use scoreboard_model::Mode;
use scoreboard_render::event::{
    CADENCE, Clip, ClipError, ClipTiming, Draw, EventPlayer, Window, days_from_civil, local_day,
};
use scoreboard_render::time::{FPS, WallMs};

const SURFACE: usize = 128 * 64 * 2;

// ------------------------------------------------------------------ shipped

/// Every clip the firmware embeds, with the per-frame hashes its encoder wrote.
/// A clip added to `firmware-rs/app/src/event.rs` belongs here too.
const SHIPPED: &[(&str, &[u8], &str)] = &[(
    "colin-birthday-2026",
    include_bytes!("../../../firmware-rs/app/assets/events/colin-birthday-2026.sbev"),
    include_str!("../../../firmware-rs/app/assets/events/colin-birthday-2026.sbev.frames"),
)];

fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xCBF2_9CE4_8422_2325, |hash, &byte| {
        (hash ^ byte as u64).wrapping_mul(0x0100_0000_01B3)
    })
}

#[test]
fn shipped_clips_decode_to_exactly_what_the_encoder_verified() {
    for (name, bytes, frames) in SHIPPED {
        let clip = Clip::parse(bytes).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let expected: Vec<u64> = frames
            .lines()
            .map(|line| u64::from_str_radix(line, 16).unwrap())
            .collect();
        assert_eq!(
            expected.len(),
            clip.timing().frames as usize,
            "{name}: frame count"
        );

        // Start from garbage: frame 0 is a keyframe and must not depend on it.
        let mut surface = vec![0xA5u8; SURFACE];
        for (frame, want) in expected.iter().enumerate() {
            clip.decode_into(frame as u16, &mut surface)
                .unwrap_or_else(|e| panic!("{name} frame {frame}: {e:?}"));
            assert_eq!(fnv1a64(&surface), *want, "{name} frame {frame}");
        }
    }
}

#[test]
fn shipped_clips_last_whole_ticks() {
    for (name, bytes, _) in SHIPPED {
        let timing = Clip::parse(bytes).unwrap().timing();
        assert_eq!(FPS % (FPS / timing.ticks_per_frame as u32), 0, "{name}");
        // Sanity on length, so a clip that is accidentally minutes long fails here.
        let seconds = timing.ticks() / FPS;
        assert!((5..=30).contains(&seconds), "{name}: {seconds} s");
    }
}

// ---------------------------------------------------------------- malformed

/// A two-frame, two-colour clip: frame 0 fills black then white halves,
/// frame 1 skips the first half and fills it red.
fn tiny(ops0: &[u8], ops1: &[u8]) -> Vec<u8> {
    let mut clip = b"SBEV".to_vec();
    clip.extend([1, 30]);
    clip.extend(2u16.to_le_bytes()); // frames
    clip.extend(3u16.to_le_bytes()); // palette
    clip.extend([128, 64]);
    for colour in [0x0000u16, 0xFFFF, 0xF800] {
        clip.extend(colour.to_le_bytes());
    }
    for offset in [0, ops0.len(), ops0.len() + ops1.len()] {
        clip.extend((offset as u32).to_le_bytes());
    }
    clip.extend(ops0);
    clip.extend(ops1);
    clip
}

/// `n` pixels of one palette index as fill ops.
fn fill(mut n: usize, index: u8) -> Vec<u8> {
    let mut ops = vec![];
    while n > 0 {
        let run = n.min(64);
        ops.extend([0x40 | (run as u8 - 1), index]);
        n -= run;
    }
    ops
}

fn skip(mut n: usize) -> Vec<u8> {
    let mut ops = vec![];
    while n > 0 {
        let run = n.min(64);
        ops.push(run as u8 - 1);
        n -= run;
    }
    ops
}

const HALF: usize = 128 * 32;

fn good() -> Vec<u8> {
    let frame0 = [fill(HALF, 0), fill(HALF, 1)].concat();
    let frame1 = [skip(HALF), fill(HALF, 2)].concat();
    tiny(&frame0, &frame1)
}

#[test]
fn a_well_formed_clip_decodes_deltas_onto_the_previous_frame() {
    let bytes = good();
    let clip = Clip::parse(&bytes).unwrap();
    assert_eq!(
        clip.timing(),
        ClipTiming {
            frames: 2,
            ticks_per_frame: 2
        }
    );
    let mut surface = vec![0x55u8; SURFACE];
    clip.decode_into(0, &mut surface).unwrap();
    assert!(surface[..HALF * 2].iter().all(|&b| b == 0x00));
    assert!(surface[HALF * 2..].iter().all(|&b| b == 0xFF));
    clip.decode_into(1, &mut surface).unwrap();
    assert!(
        surface[..HALF * 2].iter().all(|&b| b == 0x00),
        "skipped half kept"
    );
    assert_eq!(&surface[HALF * 2..HALF * 2 + 2], &0xF800u16.to_le_bytes());
}

#[test]
fn malformed_headers_are_refused() {
    let bytes = good();
    let mut bad = bytes.clone();
    bad[0] = b'X';
    assert_eq!(Clip::parse(&bad).unwrap_err(), ClipError::BadMagic);
    let mut bad = bytes.clone();
    bad[4] = 2;
    assert_eq!(
        Clip::parse(&bad).unwrap_err(),
        ClipError::UnsupportedVersion(2)
    );
    let mut bad = bytes.clone();
    bad[5] = 25; // does not divide 60: frames would last 2 and 3 ticks
    assert_eq!(Clip::parse(&bad).unwrap_err(), ClipError::BadFps(25));
    let mut bad = bytes.clone();
    bad[10] = 64;
    assert_eq!(Clip::parse(&bad).unwrap_err(), ClipError::BadGeometry);
    let mut bad = bytes.clone();
    bad.pop();
    assert_eq!(Clip::parse(&bad).unwrap_err(), ClipError::BadOffsets);
    for cut in 0..40 {
        assert!(Clip::parse(&bytes[..cut]).is_err(), "cut at {cut}");
    }
}

#[test]
fn malformed_frames_fail_without_panicking() {
    let keyframe_skip = tiny(&[skip(HALF), fill(HALF, 1)].concat(), &skip(HALF * 2));
    let out_of_palette = tiny(&[fill(HALF, 0), fill(HALF, 3)].concat(), &skip(HALF * 2));
    let short = tiny(&fill(HALF, 0), &skip(HALF * 2));
    let long = tiny(
        &[fill(HALF * 2, 0), vec![0x40, 0]].concat(),
        &skip(HALF * 2),
    );
    let reserved = tiny(
        &[vec![0xC0], fill(HALF * 2 - 1, 0)].concat(),
        &skip(HALF * 2),
    );
    let dangling = tiny(
        &[fill(HALF * 2 - 1, 0), vec![0x40]].concat(),
        &skip(HALF * 2),
    );
    for bytes in [
        keyframe_skip,
        out_of_palette,
        short,
        long,
        reserved,
        dangling,
    ] {
        let clip = Clip::parse(&bytes).unwrap();
        let mut surface = vec![0u8; SURFACE];
        assert_eq!(
            clip.decode_into(0, &mut surface),
            Err(ClipError::Corrupt(0))
        );
    }
    let bytes = good();
    let clip = Clip::parse(&bytes).unwrap();
    assert_eq!(
        clip.decode_into(2, &mut vec![0u8; SURFACE]),
        Err(ClipError::NoSuchFrame(2))
    );
    assert_eq!(
        clip.decode_into(0, &mut [0u8; 10]),
        Err(ClipError::BadSurface)
    );
}

#[test]
fn arbitrary_bytes_never_panic() {
    // A cheap deterministic fuzz over the shipped clip: flip bytes all over it
    // and decode every frame. Errors are fine; panics are not.
    let (_, original, _) = SHIPPED[0];
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    for _ in 0..64 {
        let mut bytes = original.to_vec();
        for _ in 0..8 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let at = (state as usize) % bytes.len();
            bytes[at] = (state >> 32) as u8;
        }
        if let Ok(clip) = Clip::parse(&bytes) {
            let mut surface = vec![0u8; SURFACE];
            for frame in 0..clip.timing().frames {
                let _ = clip.decode_into(frame, &mut surface);
            }
        }
    }
}

// ------------------------------------------------------------------- window

#[test]
fn days_from_civil_matches_known_dates() {
    assert_eq!(days_from_civil(1970, 1, 1), 0);
    assert_eq!(days_from_civil(2000, 3, 1), 11_017);
    assert_eq!(days_from_civil(2024, 2, 29), 19_782);
    assert_eq!(days_from_civil(2026, 9, 30), 20_726);
    assert_eq!(days_from_civil(2026, 10, 2), 20_728);
    assert_eq!(days_from_civil(1969, 12, 31), -1);
}

#[test]
fn local_day_honours_the_offset_across_midnight() {
    // 2026-10-01T02:00:00Z — still Sept 30 in New York (UTC-4).
    let epoch = 1_790_820_000;
    assert_eq!(local_day(epoch, Some(0)), days_from_civil(2026, 10, 1));
    assert_eq!(
        local_day(epoch, Some(-4 * 3600)),
        days_from_civil(2026, 9, 30)
    );
    assert_eq!(
        local_day(epoch, None),
        days_from_civil(2026, 10, 1),
        "unknown reads as UTC"
    );
}

#[test]
fn a_window_is_inclusive_at_both_ends() {
    let window = Window::new((2026, 9, 30), (2026, 10, 2));
    assert!(!window.contains(days_from_civil(2026, 9, 29)));
    assert!(window.contains(days_from_civil(2026, 9, 30)));
    assert!(window.contains(days_from_civil(2026, 10, 2)));
    assert!(!window.contains(days_from_civil(2026, 10, 3)));
}

// ------------------------------------------------------------------- player

const TIMING: ClipTiming = ClipTiming {
    frames: 390,
    ticks_per_frame: 2,
};
const TICK_MS: u64 = 1000 / FPS as u64;

/// Drives a player one tick at a time, the way the render loop does.
struct Rig {
    player: EventPlayer,
    now: u64,
    mode: Mode,
    menu: bool,
}

impl Rig {
    fn new() -> Rig {
        Rig {
            player: EventPlayer::new(),
            now: 1_000,
            mode: Mode::Idle,
            menu: false,
        }
    }

    fn step(
        &mut self,
        event: Option<ClipTiming>,
        dismissed: bool,
    ) -> scoreboard_render::event::Tick {
        let tick = self
            .player
            .tick(WallMs(self.now), event, self.mode, self.menu, dismissed);
        self.now += TICK_MS;
        tick
    }

    /// Ticks until the first clip frame, or panic after `limit_ms`.
    fn until_clip(&mut self, limit_ms: u64) -> u64 {
        let start = self.now;
        while self.now - start < limit_ms {
            if let Draw::ClipFrame(0) = self.step(Some(TIMING), false).draw {
                return self.now - start;
            }
        }
        panic!("no clip within {limit_ms} ms");
    }

    /// Ticks until the player is back to plain screen at full brightness.
    fn until_done(&mut self) {
        for _ in 0..10_000 {
            let tick = self.step(Some(TIMING), false);
            if tick.fade == u8::MAX
                && !tick.captures_input
                && matches!(tick.draw, Draw::Screen { redraw: false })
            {
                return;
            }
        }
        panic!("never finished");
    }
}

#[test]
fn nothing_happens_without_a_live_event() {
    let mut rig = Rig::new();
    for _ in 0..(20 * 60 * FPS) {
        let tick = rig.step(None, false);
        assert_eq!(tick.draw, Draw::Screen { redraw: false });
        assert_eq!(tick.fade, u8::MAX);
    }
}

#[test]
fn first_play_comes_soon_after_the_event_goes_live() {
    let mut rig = Rig::new();
    let waited = rig.until_clip(60_000);
    // The first-play delay plus the 0.3 s fade out of the screen.
    assert!(
        (CADENCE.first_ms..=CADENCE.first_ms + 400).contains(&waited),
        "{waited}"
    );
}

#[test]
fn the_clip_plays_every_frame_once_in_order_and_fades_both_ends() {
    let mut rig = Rig::new();
    rig.until_clip(60_000);
    let mut next = 1u16;
    let mut fades = vec![0u8];
    loop {
        let tick = rig.step(Some(TIMING), false);
        match tick.draw {
            Draw::ClipFrame(frame) => {
                assert_eq!(frame, next);
                next += 1;
            }
            Draw::Hold => {}
            Draw::Screen { redraw } => {
                assert!(redraw, "the first screen tick after a clip repaints");
                break;
            }
        }
        assert!(tick.captures_input);
        fades.push(tick.fade);
    }
    assert_eq!(next, TIMING.frames, "every frame shown");
    assert_eq!(
        fades.len() as u32,
        TIMING.ticks(),
        "each frame held its ticks"
    );
    assert_eq!(fades[0], 0, "fades in from dark");
    assert_eq!(*fades.last().unwrap(), 0, "fades out to dark");
    assert_eq!(fades[fades.len() / 2], u8::MAX, "full brightness mid-clip");
}

#[test]
fn it_repeats_every_three_minutes_measured_from_the_end() {
    let mut rig = Rig::new();
    rig.until_clip(60_000);
    rig.until_done();
    let gap = rig.until_clip(10 * 60_000);
    assert!(
        (CADENCE.interval_ms - 1_000..=CADENCE.interval_ms + 1_000).contains(&gap),
        "{gap}"
    );
}

#[test]
fn live_games_stretch_the_interval_to_ten_minutes() {
    let mut rig = Rig::new();
    rig.until_clip(60_000);
    rig.until_done();
    rig.mode = Mode::MlbLive;
    let gap = rig.until_clip(20 * 60_000);
    assert!(
        (CADENCE.live_interval_ms - 1_000..=CADENCE.live_interval_ms + 1_000).contains(&gap),
        "{gap}"
    );
}

#[test]
fn protected_screens_are_never_covered_and_end_a_play_at_once() {
    for protected in [Mode::Setup, Mode::Startup, Mode::Updating] {
        let mut rig = Rig::new();
        rig.mode = protected;
        for _ in 0..(60 * FPS) {
            let tick = rig.step(Some(TIMING), false);
            assert_eq!(tick.draw, Draw::Screen { redraw: false }, "{protected:?}");
            assert_eq!(tick.fade, u8::MAX);
        }
        // Mid-play: an update starting takes the screen back immediately.
        rig.mode = Mode::Idle;
        rig.until_clip(60_000);
        rig.mode = protected;
        let tick = rig.step(Some(TIMING), false);
        assert_eq!(tick.draw, Draw::Screen { redraw: true }, "{protected:?}");
        assert_eq!(tick.fade, u8::MAX);
    }
}

#[test]
fn the_menu_is_never_covered() {
    let mut rig = Rig::new();
    rig.menu = true;
    for _ in 0..(60 * FPS) {
        assert_eq!(
            rig.step(Some(TIMING), false).draw,
            Draw::Screen { redraw: false }
        );
    }
    rig.menu = false;
    rig.until_clip(1_000);
}

#[test]
fn a_press_dismisses_and_fades_the_screen_back() {
    let mut rig = Rig::new();
    rig.until_clip(60_000);
    for _ in 0..30 {
        rig.step(Some(TIMING), false);
    }
    let tick = rig.step(Some(TIMING), true);
    assert_eq!(tick.draw, Draw::Screen { redraw: true });
    assert_eq!(tick.fade, 0);
    assert!(!tick.captures_input, "the next press is the screen's again");
    let mut last = 0;
    loop {
        let tick = rig.step(Some(TIMING), false);
        assert!(!matches!(tick.draw, Draw::ClipFrame(_) | Draw::Hold));
        assert!(tick.fade >= last, "brightness only rises");
        last = tick.fade;
        if tick.fade == u8::MAX {
            break;
        }
    }
    // And the cadence restarts from the dismissal.
    let gap = rig.until_clip(10 * 60_000);
    assert!(gap >= CADENCE.interval_ms - 1_000, "{gap}");
}

#[test]
fn a_press_while_leaving_reverses_without_a_jump() {
    let mut rig = Rig::new();
    let last = loop {
        let tick = rig.step(Some(TIMING), false);
        if tick.captures_input && tick.fade < 200 {
            break tick.fade;
        }
    };
    let tick = rig.step(Some(TIMING), true);
    assert_eq!(
        tick.draw,
        Draw::Screen { redraw: false },
        "the clip never reached the surface"
    );
    assert!(tick.fade.abs_diff(last) < 40, "{} → {}", last, tick.fade);
}

#[test]
fn the_window_closing_mid_play_ends_it() {
    let mut rig = Rig::new();
    rig.until_clip(60_000);
    let tick = rig.step(None, false);
    assert_eq!(tick.draw, Draw::Screen { redraw: true });
    assert_eq!(tick.fade, u8::MAX);
    for _ in 0..(20 * 60 * FPS) {
        assert_eq!(rig.step(None, false).draw, Draw::Screen { redraw: false });
    }
}

#[test]
fn abort_ends_a_play_and_does_nothing_otherwise() {
    let mut rig = Rig::new();
    rig.player.abort(WallMs(rig.now));
    assert_eq!(
        rig.step(Some(TIMING), false).fade,
        u8::MAX,
        "no dim when idle"
    );
    rig.until_clip(60_000);
    rig.player.abort(WallMs(rig.now));
    let tick = rig.step(Some(TIMING), false);
    assert_eq!(tick.draw, Draw::Screen { redraw: true });
}
