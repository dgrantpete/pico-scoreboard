//! The two time rails, and the rule that separates them.

use scoreboard_model::ScoreboardSnapshot;
use scoreboard_render::geometry::FPS;
use scoreboard_render::time::{FrameElapsed, FrameRail, WallMs, frame_ms, frame_us};

#[test]
fn the_frame_rail_advances_one_frame_per_tick_regardless_of_wall_time() {
    let mut snapshot = ScoreboardSnapshot::new();
    snapshot.animation_start_ms = 1_000;
    let mut rail = FrameRail::new();

    // The frame that first shows a view sits at elapsed zero; every frame after
    // it is one period further along.
    for tick in 1..=10u64 {
        rail.advance_and_latch(&snapshot);
        assert_eq!(rail.view_elapsed(), FrameElapsed(frame_ms(tick - 1)));
    }
}

#[test]
fn the_quantiser_tracks_true_time_and_never_lags_it() {
    // Both properties matter and they are different. Tracking: the rail must
    // not drift, which is what an accumulated per-frame constant would do —
    // and the frame counts below reach a full day of uptime. Never lagging: the
    // scroll math floors, so a rail a hair *behind* true time puts pixel steps
    // one frame late, which is the judder — see `frame_ms`'s docs.
    //
    // The comparison is cross-multiplied because true time is a rational and
    // any integer form of it would be doing the rounding under test.
    let rate = FPS as u64;
    assert_eq!((frame_ms(0), frame_us(0)), (0, 0), "the rail starts at zero");
    for frames in [1u64, 2, 3, 59, 60, 61, 1_000, 86_400 * 60] {
        let exact = frames * 1_000_000;
        let ms = frame_ms(frames);
        assert!(ms * 1_000 * rate >= exact, "lags true time at frame {frames}");
        assert!(
            (ms - 1) * 1_000 * rate < exact,
            "over a millisecond ahead at frame {frames}"
        );

        // The microsecond form is the same quantiser, a thousand times finer.
        let us = frame_us(frames);
        assert!(us * rate >= exact, "us lags true time at frame {frames}");
        assert!(
            (us - 1) * rate < exact,
            "over a microsecond ahead at frame {frames}"
        );
    }
}

#[test]
fn the_parity_harness_offsets_are_positions_the_rail_actually_visits() {
    // `tests/gen_parity.py` pins its frames at 0, 1500, 4500 and 11000 ms on the
    // frame rail, and both stacks render at those numbers rather than at a frame
    // index. That is only an honest comparison if a loop running at this rate
    // passes through each of them exactly — an offset that fell between two
    // frames would be a picture the panel never shows.
    for offset in [0u64, 1_500, 4_500, 11_000] {
        let frames = offset * FPS as u64 / 1_000;
        assert_eq!(
            frame_ms(frames),
            offset,
            "{offset} ms is not a frame boundary at {FPS} FPS"
        );
    }
}

#[test]
fn a_stall_stretches_motion_but_consumes_waiting() {
    // The headline rule. Ten frames render while 2 s of wall time passes — a
    // stall somewhere in the middle. Motion has advanced ten frames' worth and
    // holds position; the toast that started at t=0 has spent 2 s of its life.
    let mut snapshot = ScoreboardSnapshot::new();
    snapshot.animation_start_ms = 0;
    let mut rail = FrameRail::new();
    for _ in 0..10 {
        rail.advance_and_latch(&snapshot);
    }
    let wall = WallMs(2_000);

    assert_eq!(
        rail.view_elapsed(),
        FrameElapsed(frame_ms(9)),
        "ten frames of motion"
    );
    assert_eq!(wall.since(0).0, 2_000, "two seconds of waiting");
}

#[test]
fn a_new_animation_epoch_zeroes_the_rail() {
    let mut snapshot = ScoreboardSnapshot::new();
    snapshot.animation_start_ms = 1_000;
    let mut rail = FrameRail::new();
    for _ in 0..5 {
        rail.advance_and_latch(&snapshot);
    }
    assert_eq!(rail.view_elapsed(), FrameElapsed(frame_ms(4)));

    // A new view: the epoch stamp changes, so the rail restarts from this frame
    // rather than being compared against a stamp from the wall domain.
    snapshot.animation_start_ms = 9_999;
    rail.advance_and_latch(&snapshot);
    assert_eq!(rail.view_elapsed(), FrameElapsed(0));
}

#[test]
fn elapsed_time_does_not_depend_on_which_frame_the_epoch_landed_on() {
    // The reason both epochs are frame counts. `frame_ms(a) - frame_ms(b)` is a
    // millisecond off `frame_ms(a - b)` for two thirds of the phases at 60 FPS,
    // which would make a screen's scroll rhythm depend on when it appeared.
    for phase in 0..2 * FPS as u64 {
        let mut snapshot = ScoreboardSnapshot::new();
        snapshot.animation_start_ms = 1_000;
        let mut rail = FrameRail::new();
        for _ in 0..phase {
            rail.advance_and_latch(&snapshot);
        }
        snapshot.animation_start_ms = 2_000;
        for age in 0..6u64 {
            rail.advance_and_latch(&snapshot);
            assert_eq!(
                rail.view_elapsed(),
                FrameElapsed(frame_ms(age)),
                "a view that appeared at frame {phase}, {age} frames on"
            );
        }
    }
}

#[test]
fn a_re_commit_of_the_same_view_keeps_the_scroll_where_it_is() {
    // Every poll tick re-commits the current game, including unchanged ones.
    // Core 0 only restamps `animation_start_ms` when the view identity changes,
    // and the rail latches on *change*, so a standing re-poll must not restart
    // any motion.
    let mut snapshot = ScoreboardSnapshot::new();
    snapshot.animation_start_ms = 1_000;
    let mut rail = FrameRail::new();
    for _ in 0..20 {
        rail.advance_and_latch(&snapshot);
    }
    assert_eq!(rail.view_elapsed(), FrameElapsed(frame_ms(19)));

    snapshot.commit_seq += 1;
    rail.advance_and_latch(&snapshot);
    assert_eq!(rail.view_elapsed(), FrameElapsed(frame_ms(20)));
}

#[test]
fn the_play_rail_runs_independently_of_the_view_rail() {
    let mut snapshot = ScoreboardSnapshot::new();
    snapshot.animation_start_ms = 1_000;
    snapshot.play.updated_ms = 1_000;
    let mut rail = FrameRail::new();
    for _ in 0..4 {
        rail.advance_and_latch(&snapshot);
    }
    assert_eq!(rail.view_elapsed(), rail.play_elapsed());

    // A new play line restarts only the play flash.
    snapshot.play.updated_ms = 4_000;
    rail.advance_and_latch(&snapshot);
    assert_eq!(rail.play_elapsed(), FrameElapsed(0));
    assert_eq!(rail.view_elapsed(), FrameElapsed(frame_ms(4)));
}

#[test]
fn the_first_frame_latches_both_epochs() {
    // A rail that has never latched must not treat epoch 0 as "already seen" —
    // an animation stamped at boot would otherwise start mid-cycle.
    let snapshot = ScoreboardSnapshot::new();
    assert_eq!(snapshot.animation_start_ms, 0);
    let mut rail = FrameRail::new();
    rail.advance_and_latch(&snapshot);
    assert_eq!(rail.view_elapsed(), FrameElapsed(0));
    assert_eq!(rail.play_elapsed(), FrameElapsed(0));
}

#[test]
fn a_wall_stamp_in_the_future_reads_as_zero_elapsed() {
    // MicroPython's `ticks_diff` wrapped a 30-bit counter and could return a
    // negative, which every caller guarded against. A 64-bit monotonic cannot go
    // backwards, so the only way to see a future stamp is one that was never
    // set.
    assert_eq!(WallMs(100).since(500).0, 0);
    assert_eq!(WallMs(500).since(100).0, 400);
    assert_eq!(WallMs(0).since(0).0, 0);
}

#[test]
fn the_rails_advance_in_step_under_perfect_pacing() {
    // Which is exactly why mixing them up survives every test that does not
    // stall a frame: with frames arriving exactly one period apart, the two
    // rails differ only by the phase between the epoch stamp and the first frame
    // that showed it.
    let mut snapshot = ScoreboardSnapshot::new();
    snapshot.animation_start_ms = 0;
    let mut rail = FrameRail::new();
    let period = frame_ms(1);
    for tick in 1..=120u64 {
        rail.advance_and_latch(&snapshot);
        let wall = WallMs(frame_ms(tick));
        let lead = wall.since(0).0 - rail.view_elapsed().0;
        assert!(
            period.abs_diff(lead) <= 1,
            "at frame {tick} the wall rail led the frame rail by {lead} ms, not one              period ({period} ms) give or take the quantiser"
        );
    }
}
