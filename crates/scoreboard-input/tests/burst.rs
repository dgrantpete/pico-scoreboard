//! A burst of presses advances the rotation exactly once.
//!
//! This is the one promise that spans every layer of the input path, which is
//! why it is an integration test rather than a unit one: the PIO's debounce
//! decides which physical bounces become edges, [`PressTracker`] decides which
//! edges become presses, [`MenuController`] decides where a press goes, and
//! [`SkipMachine`] decides whether the rotation actually moves. Each of those is
//! tested on its own; a scoreboard that advances four games because somebody
//! tapped the button four times in a second is a failure of the *composition*.
//!
//! `poller.py`'s `skip()` docstring states the rule and its mechanism: a press
//! that lands while a skip is already armed or in flight is **rejected, not
//! queued**, and dims the visible toast instead. So the count that matters is
//! not "how many presses" but "how many ticks consumed an armed skip".

use scoreboard_input::button::{ButtonEvent, Press, PressTracker};
use scoreboard_input::menu::{Action, Button, MenuController};
use scoreboard_model::feed::LeagueId;
use scoreboard_model::poll::{SkipKind, SkipMachine, SkipVerdict};
use scoreboard_model::{ListSink, Slate, Sport, Store};
use scoreboard_wire::GameState;

/// A slate with one league and four games, so an advance is observable.
fn slate() -> Slate {
    let mut slate = Slate::new();
    let sources = [LeagueId::from_slug(Sport::Mlb, "mlb")];
    slate.set_sources(&sources);
    {
        let mut update = slate.update_source(0);
        for id in ["401000001", "401000002", "401000003", "401000004"] {
            update.entry(GameState::Final, id);
        }
    }
    slate.rebuild();
    slate
}

fn current(slate: &Slate) -> String {
    slate
        .current()
        .map(|(_, id)| id.to_string())
        .expect("a non-empty rotation")
}

/// One press of button A: a press edge and a release edge, both under the long
/// threshold, exactly as the PIO would deliver them.
fn tap(tracker: &mut PressTracker, at_ms: u64) -> Option<Press> {
    tracker.event(ButtonEvent {
        pressed: true,
        at_ms,
    });
    tracker.event(ButtonEvent {
        pressed: false,
        at_ms: at_ms + 40,
    })
}

#[test]
fn a_burst_of_presses_advances_the_rotation_exactly_once() {
    let mut slate = slate();
    let mut store = Store::new();
    let mut menu = MenuController::new();
    let mut tracker = PressTracker::new(false);
    let mut skips = SkipMachine::new();

    let started = current(&slate);

    // Eight taps in 800 ms — faster than anybody actually manages, and well
    // inside one poll interval, so no tick runs between them.
    let mut armed = 0;
    let mut rejected = 0;
    for index in 0..8u64 {
        let press = tap(&mut tracker, 1_000 + index * 100).expect("each tap is a short press");
        assert_eq!(
            menu.press(Button::A, press, &mut slate, &mut store, 1_000 + index * 100),
            Action::Skip,
            "with the menu closed, A short is always a skip"
        );
        match skips.request(SkipKind::Game) {
            SkipVerdict::Armed => armed += 1,
            SkipVerdict::Rejected => rejected += 1,
        }
    }

    assert_eq!(armed, 1, "only the first press arms a skip");
    assert_eq!(rejected, 7, "the rest are rejected, not queued");
    assert_eq!(
        current(&slate),
        started,
        "nothing moves until a tick consumes the armed skip"
    );

    // The tick: consume, advance, finish.
    let consumed = skips.consume();
    assert_eq!(consumed, Some(SkipKind::Game));
    slate.advance();
    assert!(skips.finish(), "the tick tears down the spinner it armed");

    assert_eq!(current(&slate), "401000002", "exactly one game forward");
}

#[test]
fn a_press_that_lands_while_a_skip_is_in_flight_is_still_rejected() {
    // The window `poller.py` cared about most: the request is out, the skip is
    // no longer "requested" but has not finished either. A machine that only
    // checked `requested` would arm a second skip here and the board would jump
    // two games for two presses that felt like one.
    let mut skips = SkipMachine::new();
    assert_eq!(skips.request(SkipKind::Game), SkipVerdict::Armed);
    assert_eq!(skips.consume(), Some(SkipKind::Game));
    assert_eq!(
        skips.request(SkipKind::Game),
        SkipVerdict::Rejected,
        "in flight is as good as armed"
    );
    assert!(skips.finish());
    assert_eq!(
        skips.request(SkipKind::Game),
        SkipVerdict::Armed,
        "and the next press after the tick is accepted again"
    );
}

#[test]
fn a_burst_of_long_presses_advances_one_league_not_several() {
    // The same rule covers the league skip, and the two share one machine — so
    // a long press landing during a short press's in-flight skip is rejected
    // too, rather than the two kinds queueing independently.
    let mut skips = SkipMachine::new();
    assert_eq!(skips.request(SkipKind::League), SkipVerdict::Armed);
    for _ in 0..5 {
        assert_eq!(skips.request(SkipKind::Game), SkipVerdict::Rejected);
        assert_eq!(skips.request(SkipKind::League), SkipVerdict::Rejected);
    }
    assert_eq!(skips.consume(), Some(SkipKind::League));
}

#[test]
fn holding_the_button_through_a_burst_fires_one_long_press_and_no_shorts() {
    // The other half of "exactly once": a user who holds the button down gets
    // the league skip once, mid-hold, and the release does not then also fire a
    // game skip. `_PressTracker` clears the timestamp when it consumes the long
    // press, and that is the whole mechanism.
    let mut tracker = PressTracker::new(false);
    tracker.event(ButtonEvent {
        pressed: true,
        at_ms: 0,
    });

    let mut longs = 0;
    // Polled at the production 50 ms cadence for three seconds.
    for tick in 1..=60u64 {
        if tracker.poll(tick * 50) == Some(Press::Long) {
            longs += 1;
        }
    }
    assert_eq!(longs, 1, "a held button fires once, not once per poll");
    assert_eq!(
        tracker.event(ButtonEvent {
            pressed: false,
            at_ms: 3_000
        }),
        None,
        "and the release adds nothing"
    );
}
