//! The two time rails, and the rule that separates them.

use scoreboard_model::ScoreboardSnapshot;
use scoreboard_render::time::{FRAME_MS, FrameElapsed, FrameRail, WallMs};

#[test]
fn the_frame_rail_advances_one_frame_per_tick_regardless_of_wall_time() {
    let mut snapshot = ScoreboardSnapshot::new();
    snapshot.animation_start_ms = 1_000;
    let mut rail = FrameRail::new();

    // The frame that first shows a view sits at elapsed zero; every frame after
    // it is one period further along.
    for tick in 1..=10u64 {
        rail.advance_and_latch(&snapshot);
        assert_eq!(rail.view_elapsed(), FrameElapsed((tick - 1) * FRAME_MS));
    }
}

#[test]
fn a_stall_stretches_motion_but_consumes_waiting() {
    // The headline rule. Ten frames render while 2 s of wall time passes — a
    // 1.5 s stall somewhere in the middle. Motion has advanced ten frames' worth
    // and holds position; the toast that started at t=0 has spent 2 s of its
    // life.
    let mut snapshot = ScoreboardSnapshot::new();
    snapshot.animation_start_ms = 0;
    let mut rail = FrameRail::new();
    for _ in 0..10 {
        rail.advance_and_latch(&snapshot);
    }
    let wall = WallMs(2_000);

    assert_eq!(
        rail.view_elapsed(),
        FrameElapsed(450),
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
    assert_eq!(rail.view_elapsed(), FrameElapsed(4 * FRAME_MS));

    // A new view: the epoch stamp changes, so the rail restarts from this frame
    // rather than being compared against a stamp from the wall domain.
    snapshot.animation_start_ms = 9_999;
    rail.advance_and_latch(&snapshot);
    assert_eq!(rail.view_elapsed(), FrameElapsed(0));
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
    let before = rail.view_elapsed();

    snapshot.commit_seq += 1;
    rail.advance_and_latch(&snapshot);
    assert_eq!(rail.view_elapsed(), FrameElapsed(before.0 + FRAME_MS));
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
    assert_eq!(rail.view_elapsed(), FrameElapsed(4 * FRAME_MS));
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
    for tick in 1..=40u64 {
        rail.advance_and_latch(&snapshot);
        let wall = WallMs(tick * FRAME_MS);
        assert_eq!(rail.view_elapsed().0 + FRAME_MS, wall.since(0).0);
    }
}
