//! The league-select menu: a session, a cursor, and one dispatch point for
//! both buttons.
//!
//! Port of `scoreboard/menu.py`. The controller owns the **whole** session —
//! the working checkbox flags, the cursor, the scroll window and the inactivity
//! timeout — and everything core 1 draws is computed here, so
//! `scoreboard_render::menu` stays a pure reader of the published view.
//!
//! # The semantics are user-locked, and three of them are surprising
//!
//! * **The checked set is a *session* filter.** It restricts the rotation to
//!   some of the configured league sources and resets to all-checked on reboot;
//!   the persisted configuration still owns which leagues are polled at all.
//!   Nothing here writes flash.
//! * **Every exit applies.** DONE, a long press on B, and the 10 s timeout all
//!   commit the working flags. There is deliberately no cancel path — the menu
//!   is a filter you are adjusting, not a form you are filling in, and a cancel
//!   would mean the highlighted state on screen was not the state in effect.
//! * **The last checked league cannot be unchecked**, silently. An empty filter
//!   is a blank scoreboard, and refusing is better than explaining. There is no
//!   toast to explain with, either: the menu view preempts the renderer's mode
//!   dispatch entirely, so nothing else is on screen while it is open.
//!
//! # Why this is the dispatch point for presses the menu does not want
//!
//! `menu.py`'s controller is bound to *both* buttons' trackers and falls
//! through to the poller's actions whenever the menu is closed. Keeping that
//! shape means the open/closed question is asked in exactly one place; the
//! alternative — the input loop deciding, and the controller handling only what
//! it was given — puts the same condition in two files that can disagree about
//! it mid-press. So [`MenuController::press`] takes every press and returns an
//! [`Action`] for the ones that are not the menu's.

use heapless::Vec;
use scoreboard_model::feed::LEAGUE_KEY;
use scoreboard_model::slate::MAX_SOURCES;
use scoreboard_model::store::MenuRowInput;
use scoreboard_model::{Mode, Slate, Store, Text};
use scoreboard_render::menu::thumb;

pub use crate::button::Press;

/// Inactivity before the menu applies and closes itself. `menu.py`'s
/// `_TIMEOUT_MS`.
pub const TIMEOUT_MS: u64 = 10_000;

/// Which physical button a press came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    /// GPIO 10. Skip, and the cursor while the menu is open.
    A,
    /// GPIO 22. Lock, and the menu itself.
    B,
}

/// What the caller must do with a press the menu did not consume.
///
/// The poller owns the rotation, so these are requests rather than effects —
/// the arm/reject decision for a skip is about poller state and is made there,
/// exactly where `poller.py` made it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// The menu handled it, or refused it. Nothing else to do.
    Handled,
    /// Advance to the next game.
    Skip,
    /// Advance to the next league's games.
    SkipLeague,
    /// Toggle the rotation lock.
    ToggleLock,
    /// The menu closed and the filter it applied actually changed the
    /// rotation. `poller.py:353` woke the poll loop here so the board moves off
    /// a filtered-out game within a tick instead of waiting out the interval.
    FilterApplied,
}

/// Core 0's owner of the league menu session.
#[derive(Debug, Clone)]
pub struct MenuController {
    active: bool,
    /// Configured sources at the moment the session opened. Held so a
    /// mid-session change to the slate cannot renumber the working flags —
    /// which cannot happen today (sources are read once at poller start) and is
    /// exactly the kind of thing that stops being true quietly.
    count: usize,
    /// Working checkbox flags, parallel to [`Slate::sources`].
    checked: [bool; MAX_SOURCES],
    /// `0..count` are items; `count` is the DONE footer.
    cursor: usize,
    /// First visible item.
    scroll: usize,
    last_input_ms: u64,
}

impl MenuController {
    pub const fn new() -> MenuController {
        MenuController {
            active: false,
            count: 0,
            checked: [true; MAX_SOURCES],
            cursor: 0,
            scroll: 0,
            last_input_ms: 0,
        }
    }

    pub const fn active(&self) -> bool {
        self.active
    }

    /// When the inactivity timeout expires, or `None` while the menu is closed.
    ///
    /// `menu.py` checked the timeout from the 50 ms button poll, which was a
    /// task that ran whatever else was happening. Here the controller lives
    /// with the poller — the only owner of the `Store` — and the poller spends
    /// most of its life asleep on the poll interval, which at the default 30 s
    /// is three times the timeout. So the deadline is *published* and the
    /// poller's sleep is capped by it. Without that the menu would sit open on
    /// the panel for up to a poll interval after the user walked away.
    pub const fn deadline_ms(&self) -> Option<u64> {
        if self.active {
            Some(self.last_input_ms + TIMEOUT_MS)
        } else {
            None
        }
    }

    /// The single dispatch point for both buttons. See the module docs.
    pub fn press(
        &mut self,
        button: Button,
        press: Press,
        slate: &mut Slate,
        store: &mut Store,
        now_ms: u64,
    ) -> Action {
        match (button, press, self.active) {
            (Button::A, Press::Short, true) => {
                self.touch(now_ms);
                self.advance(slate, store, now_ms);
                Action::Handled
            }
            (Button::A, Press::Short, false) => Action::Skip,
            // A deliberate no-op that still counts as input: holding A inside
            // the menu means nothing, but it means the user is still there.
            (Button::A, Press::Long, true) => {
                self.touch(now_ms);
                Action::Handled
            }
            (Button::A, Press::Long, false) => Action::SkipLeague,
            (Button::B, Press::Short, true) => {
                self.touch(now_ms);
                self.select(slate, store, now_ms)
            }
            (Button::B, Press::Short, false) => Action::ToggleLock,
            (Button::B, Press::Long, true) => self.apply_and_close(slate, store),
            (Button::B, Press::Long, false) => {
                self.open(slate, store, now_ms);
                Action::Handled
            }
        }
    }

    /// Apply and close if nothing has happened for [`TIMEOUT_MS`].
    pub fn check_timeout(&mut self, slate: &mut Slate, store: &mut Store, now_ms: u64) -> Action {
        if !self.active || now_ms.saturating_sub(self.last_input_ms) < TIMEOUT_MS {
            return Action::Handled;
        }
        self.apply_and_close(slate, store)
    }

    /// Open the session, seeding the working flags from the live filter.
    ///
    /// Refuses in two cases, both `menu.py`'s: with no configured sources there
    /// is nothing to show, and during an OTA the progress screen must stay
    /// visible because a reboot is imminent.
    fn open(&mut self, slate: &Slate, store: &mut Store, now_ms: u64) {
        let count = slate.sources().len();
        if count == 0 || store.mode() == Mode::Updating {
            return;
        }
        let filter = slate.filter();
        self.count = count;
        for (index, checked) in self.checked.iter_mut().enumerate() {
            *checked = filter.is_none_or(|mask| mask & (1 << index) != 0);
        }
        self.active = true;
        self.cursor = 0;
        self.scroll = 0;
        self.touch(now_ms);
        self.publish(slate, store, now_ms);
    }

    /// Move the cursor one row down, wrapping through DONE.
    fn advance(&mut self, slate: &Slate, store: &mut Store, now_ms: u64) {
        self.cursor = (self.cursor + 1) % (self.count + 1);
        // Keep the cursor inside the window. DONE is drawn in its own footer
        // and is always visible, so it needs no scroll of its own.
        if self.cursor == 0 {
            self.scroll = 0;
        } else if self.cursor < self.count && self.cursor >= self.scroll + thumb::VISIBLE_ROWS {
            self.scroll = self.cursor - thumb::VISIBLE_ROWS + 1;
        }
        self.publish(slate, store, now_ms);
    }

    /// Toggle the highlighted checkbox, or activate DONE.
    fn select(&mut self, slate: &mut Slate, store: &mut Store, now_ms: u64) -> Action {
        if self.cursor == self.count {
            return self.apply_and_close(slate, store);
        }
        if self.checked[self.cursor] && self.checked_count() == 1 {
            // Silent: there is nowhere to say it. See the module docs.
            return Action::Handled;
        }
        self.checked[self.cursor] = !self.checked[self.cursor];
        self.publish(slate, store, now_ms);
        Action::Handled
    }

    /// Commit the working flags and take the menu off the panel.
    fn apply_and_close(&mut self, slate: &mut Slate, store: &mut Store) -> Action {
        // The keys are copied out rather than borrowed: `set_filter` takes
        // `&mut Slate` and the sources live inside it, so a slice of borrowed
        // `&str` would still be alive at the call. Eight 32-byte keys on the
        // stack is the cheapest way to say "these came from the slate and the
        // slate may now change".
        let mut owned: Vec<Text<LEAGUE_KEY>, MAX_SOURCES> = Vec::new();
        for (index, source) in slate.sources().iter().enumerate().take(self.count) {
            if self.checked[index] {
                let _ = owned.push(source.key.clone());
            }
        }
        let keys: Vec<&str, MAX_SOURCES> = owned.iter().map(Text::as_str).collect();
        self.active = false;
        let changed = slate.set_filter(&keys);
        store.clear_menu();
        if changed {
            Action::FilterApplied
        } else {
            Action::Handled
        }
    }

    fn checked_count(&self) -> usize {
        self.checked[..self.count]
            .iter()
            .filter(|checked| **checked)
            .count()
    }

    fn touch(&mut self, now_ms: u64) {
        self.last_input_ms = now_ms;
    }

    /// Build the visible window and publish it.
    ///
    /// `menu.py` built fresh lists every time because core 1 may still be
    /// holding the previously published ones; here `Store::set_menu` copies
    /// into the snapshot it owns, so the wholesale-replacement contract is the
    /// snapshot channel's and this just passes a slice of borrowed labels.
    fn publish(&self, slate: &Slate, store: &mut Store, now_ms: u64) {
        let mut rows: Vec<MenuRowInput<'_>, { thumb::VISIBLE_ROWS }> = Vec::new();
        let sources = slate.sources();
        for index in self.scroll..(self.scroll + thumb::VISIBLE_ROWS).min(self.count) {
            let Some(source) = sources.get(index) else {
                break;
            };
            let _ = rows.push(MenuRowInput {
                label: source.display_name.as_str(),
                checked: self.checked[index],
                source: index as u8,
            });
        }
        // -1 is the DONE footer, which is where the cursor is when it is past
        // the last item.
        let highlight = if self.cursor < self.count {
            (self.cursor - self.scroll) as i8
        } else {
            -1
        };
        let (thumb_y, thumb_h) = thumb::compute(self.count, self.scroll);
        store.set_menu(&rows, highlight, thumb_y, thumb_h, now_ms);
    }
}

impl Default for MenuController {
    fn default() -> MenuController {
        MenuController::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scoreboard_model::{LeagueId, Sport};

    fn slate_with(leagues: &[(Sport, &str)]) -> Slate {
        let mut slate = Slate::new();
        let sources: Vec<LeagueId, MAX_SOURCES> = leagues
            .iter()
            .map(|(sport, slug)| LeagueId::from_slug(*sport, slug))
            .collect();
        slate.set_sources(&sources);
        slate
    }

    /// Two leagues: enough to exercise the last-checked rule, too few to
    /// scroll.
    fn small() -> Slate {
        slate_with(&[(Sport::Mlb, "mlb"), (Sport::Nba, "nba")])
    }

    /// Seven leagues, so the five-row window has to scroll.
    fn large() -> Slate {
        slate_with(&[
            (Sport::Mlb, "mlb"),
            (Sport::Nba, "nba"),
            (Sport::Football, "nfl"),
            (Sport::Football, "college-football"),
            (Sport::Soccer, "usa.1"),
            (Sport::Soccer, "eng.1"),
            (Sport::Soccer, "esp.1"),
        ])
    }

    fn open(slate: &mut Slate, store: &mut Store) -> MenuController {
        let mut menu = MenuController::new();
        assert_eq!(
            menu.press(Button::B, Press::Long, slate, store, 0),
            Action::Handled
        );
        assert!(menu.active(), "a long press on B opens the menu");
        menu
    }

    #[test]
    fn a_closed_menu_routes_every_press_to_the_poller() {
        let mut menu = MenuController::new();
        let mut slate = small();
        let mut store = Store::new();
        for (button, press, expected) in [
            (Button::A, Press::Short, Action::Skip),
            (Button::A, Press::Long, Action::SkipLeague),
            (Button::B, Press::Short, Action::ToggleLock),
        ] {
            assert_eq!(
                menu.press(button, press, &mut slate, &mut store, 0),
                expected
            );
            assert!(!menu.active(), "none of these open the menu");
        }
    }

    #[test]
    fn opening_publishes_every_league_checked() {
        let mut slate = small();
        let mut store = Store::new();
        let _menu = open(&mut slate, &mut store);
        let view = &store.snapshot().menu;
        assert!(view.active);
        assert_eq!(view.rows.len(), 2);
        assert!(view.rows.iter().all(|row| row.checked));
        assert_eq!(view.highlight, 0, "the cursor starts on the first item");
        assert_eq!(view.thumb_y, -1, "two rows need no scrollbar");
    }

    #[test]
    fn the_menu_refuses_to_open_over_an_ota_progress_screen() {
        let mut slate = small();
        let mut store = Store::new();
        store.set_mode(Mode::Updating);
        let mut menu = MenuController::new();
        menu.press(Button::B, Press::Long, &mut slate, &mut store, 0);
        assert!(!menu.active(), "a reboot is imminent; the progress bar stays");
        assert!(!store.snapshot().menu.active);
    }

    #[test]
    fn the_menu_refuses_to_open_with_no_configured_leagues() {
        let mut slate = Slate::new();
        let mut store = Store::new();
        let mut menu = MenuController::new();
        menu.press(Button::B, Press::Long, &mut slate, &mut store, 0);
        assert!(!menu.active());
    }

    #[test]
    fn a_short_wraps_through_done_and_back_to_the_top() {
        let mut slate = small();
        let mut store = Store::new();
        let mut menu = open(&mut slate, &mut store);
        // Items, then DONE, then wrap.
        menu.press(Button::A, Press::Short, &mut slate, &mut store, 0);
        assert_eq!(store.snapshot().menu.highlight, 1);
        menu.press(Button::A, Press::Short, &mut slate, &mut store, 0);
        assert_eq!(store.snapshot().menu.highlight, -1, "DONE");
        menu.press(Button::A, Press::Short, &mut slate, &mut store, 0);
        assert_eq!(store.snapshot().menu.highlight, 0, "wrapped to the top");
    }

    #[test]
    fn the_window_scrolls_to_keep_the_cursor_visible_and_the_thumb_follows() {
        let mut slate = large();
        let mut store = Store::new();
        let mut menu = open(&mut slate, &mut store);
        let (top_y, top_h) = (
            store.snapshot().menu.thumb_y,
            store.snapshot().menu.thumb_h,
        );
        assert!(top_y >= 0 && top_h > 0, "seven items need a scrollbar");

        // Four presses put the cursor on row 4, the last visible one.
        for _ in 0..4 {
            menu.press(Button::A, Press::Short, &mut slate, &mut store, 0);
        }
        assert_eq!(store.snapshot().menu.highlight, 4);
        assert_eq!(store.snapshot().menu.thumb_y, top_y, "no scroll yet");

        // The fifth scrolls by one, and the highlight stays pinned to the
        // bottom row.
        menu.press(Button::A, Press::Short, &mut slate, &mut store, 0);
        assert_eq!(store.snapshot().menu.highlight, 4);
        assert!(store.snapshot().menu.thumb_y > top_y, "the thumb moved down");
        assert_eq!(
            store.snapshot().menu.rows[0].source,
            1,
            "the window starts at the second league"
        );

        // Wrapping back to the top resets the window in one step, rather than
        // scrolling back up through it.
        for _ in 0..3 {
            menu.press(Button::A, Press::Short, &mut slate, &mut store, 0);
        }
        assert_eq!(store.snapshot().menu.highlight, 0);
        assert_eq!(store.snapshot().menu.thumb_y, top_y);
        assert_eq!(store.snapshot().menu.rows[0].source, 0);
    }

    #[test]
    fn b_short_toggles_the_highlighted_checkbox() {
        let mut slate = small();
        let mut store = Store::new();
        let mut menu = open(&mut slate, &mut store);
        menu.press(Button::B, Press::Short, &mut slate, &mut store, 0);
        assert!(!store.snapshot().menu.rows[0].checked);
        menu.press(Button::B, Press::Short, &mut slate, &mut store, 0);
        assert!(store.snapshot().menu.rows[0].checked);
    }

    #[test]
    fn the_last_checked_league_cannot_be_unchecked() {
        let mut slate = small();
        let mut store = Store::new();
        let mut menu = open(&mut slate, &mut store);
        // Uncheck the first, move to the second, try to uncheck it too.
        menu.press(Button::B, Press::Short, &mut slate, &mut store, 0);
        menu.press(Button::A, Press::Short, &mut slate, &mut store, 0);
        assert_eq!(
            menu.press(Button::B, Press::Short, &mut slate, &mut store, 0),
            Action::Handled
        );
        assert!(
            store.snapshot().menu.rows[1].checked,
            "an empty filter is a blank scoreboard; the refusal is silent"
        );
    }

    #[test]
    fn b_long_applies_and_closes() {
        let mut slate = small();
        let mut store = Store::new();
        let mut menu = open(&mut slate, &mut store);
        menu.press(Button::B, Press::Short, &mut slate, &mut store, 0);
        assert_eq!(
            menu.press(Button::B, Press::Long, &mut slate, &mut store, 0),
            Action::FilterApplied
        );
        assert!(!menu.active());
        assert!(!store.snapshot().menu.active, "the menu left the panel");
        assert_eq!(slate.filter(), Some(0b10), "only the second league rotates");
    }

    #[test]
    fn done_applies_and_closes_the_same_way_b_long_does() {
        let mut slate = small();
        let mut store = Store::new();
        let mut menu = open(&mut slate, &mut store);
        menu.press(Button::B, Press::Short, &mut slate, &mut store, 0);
        // Cursor to DONE, then activate it.
        menu.press(Button::A, Press::Short, &mut slate, &mut store, 0);
        menu.press(Button::A, Press::Short, &mut slate, &mut store, 0);
        assert_eq!(
            menu.press(Button::B, Press::Short, &mut slate, &mut store, 0),
            Action::FilterApplied
        );
        assert!(!menu.active());
        assert_eq!(slate.filter(), Some(0b10));
    }

    #[test]
    fn the_inactivity_timeout_applies_it_does_not_cancel() {
        let mut slate = small();
        let mut store = Store::new();
        let mut menu = open(&mut slate, &mut store);
        menu.press(Button::B, Press::Short, &mut slate, &mut store, 0);

        assert_eq!(menu.deadline_ms(), Some(TIMEOUT_MS));
        assert_eq!(
            menu.check_timeout(&mut slate, &mut store, TIMEOUT_MS - 1),
            Action::Handled
        );
        assert!(menu.active(), "not yet");
        assert_eq!(
            menu.check_timeout(&mut slate, &mut store, TIMEOUT_MS),
            Action::FilterApplied
        );
        assert!(!menu.active());
        assert_eq!(
            slate.filter(),
            Some(0b10),
            "there is no cancel path: walking away commits what is on screen"
        );
    }

    #[test]
    fn every_press_including_the_no_op_hold_pushes_the_timeout_out() {
        let mut slate = small();
        let mut store = Store::new();
        let mut menu = open(&mut slate, &mut store);
        // A long press on A does nothing inside the menu, but the user is
        // clearly still there.
        assert_eq!(
            menu.press(Button::A, Press::Long, &mut slate, &mut store, 5_000),
            Action::Handled
        );
        assert_eq!(menu.deadline_ms(), Some(5_000 + TIMEOUT_MS));
        assert_eq!(
            menu.check_timeout(&mut slate, &mut store, TIMEOUT_MS + 1),
            Action::Handled
        );
        assert!(menu.active());
    }

    #[test]
    fn closing_with_everything_checked_clears_the_filter_rather_than_storing_a_no_op() {
        let mut slate = small();
        let mut store = Store::new();
        let mut menu = open(&mut slate, &mut store);
        assert_eq!(
            menu.press(Button::B, Press::Long, &mut slate, &mut store, 0),
            Action::Handled,
            "nothing changed, so the poll loop is not woken"
        );
        assert_eq!(slate.filter(), None);
    }

    #[test]
    fn reopening_seeds_the_boxes_from_the_filter_that_is_actually_in_effect() {
        let mut slate = small();
        let mut store = Store::new();
        let mut menu = open(&mut slate, &mut store);
        menu.press(Button::B, Press::Short, &mut slate, &mut store, 0);
        menu.press(Button::B, Press::Long, &mut slate, &mut store, 0);

        menu.press(Button::B, Press::Long, &mut slate, &mut store, 20_000);
        let rows = &store.snapshot().menu.rows;
        assert!(!rows[0].checked, "the session reopens where it left off");
        assert!(rows[1].checked);
    }

    #[test]
    fn a_press_that_arrives_while_the_menu_is_open_never_reaches_the_poller() {
        let mut slate = small();
        let mut store = Store::new();
        let mut menu = open(&mut slate, &mut store);
        for (button, press) in [
            (Button::A, Press::Short),
            (Button::A, Press::Long),
            (Button::B, Press::Short),
        ] {
            let action = menu.press(button, press, &mut slate, &mut store, 0);
            assert!(
                matches!(action, Action::Handled | Action::FilterApplied),
                "the menu owns both buttons while it is open, got {action:?}"
            );
        }
    }
}
