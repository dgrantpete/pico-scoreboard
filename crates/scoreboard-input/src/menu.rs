//! The league-select menu: a session, a cursor, and one dispatch point for
//! both buttons.
//!
//! Port of `scoreboard/menu.py`. The controller owns the **whole** session —
//! the working checkbox flags, the cursor, the scroll window and the inactivity
//! timeout — and everything core 1 draws is computed here, so
//! `scoreboard_render::menu` stays a pure reader of the published view.
//!
//! # The checked set is the configuration
//!
//! The rows are the registered leagues ([`LEAGUES`], all nine, in rotation
//! order), and a checked row is a league that is on in the device
//! configuration — the same `sports` section the settings page's switches
//! edit. Opening the menu reads that configuration as a mask
//! (`DeviceConfig::enabled_leagues`); closing it hands a new mask back
//! ([`Action::LeaguesChosen`]), and the caller writes it into the
//! configuration, saves it to flash **once** — the same single write a
//! settings save makes — and rebuilds the poll sources. Nothing here touches
//! the `Slate`, the configuration or flash: the controller decides what was
//! chosen, and the poller does the rest in the order its own docs give.
//!
//! `menu.py` applied a *session filter* instead — a mask over the configured
//! sources, forgotten at reboot, the configuration left alone. That gave the
//! board two notions of "which leagues": one in flash and one in RAM, free to
//! disagree, with the web's switches needing a reboot and the panel's
//! checkboxes not surviving one. One state, edited from both ends, is simpler
//! to explain and cannot drift. It is also why `menu.py`'s "no configured
//! leagues" refusal is gone: the nine rows always exist, and opening the menu
//! with nothing checked is how a league gets switched on from the panel.
//!
//! # Three semantics are user-locked, and two of them are surprising
//!
//! * **Every exit applies.** DONE, a long press on B, and the 10 s timeout all
//!   commit the working flags. There is deliberately no cancel path — the menu
//!   is a setting you are adjusting, not a form you are filling in, and a
//!   cancel would mean the highlighted state on screen was not the state in
//!   effect. An exit that lands back on the set it opened with is
//!   [`Action::Handled`], so an untouched menu costs no flash write.
//! * **The last checked league cannot be unchecked**, silently. An empty
//!   selection is a blank scoreboard, and refusing is better than explaining.
//!   There is no toast to explain with, either: the menu view preempts the
//!   renderer's mode dispatch entirely, so nothing else is on screen while it
//!   is open.
//! * **It does not open over an OTA progress screen.** A reboot is imminent
//!   and the bar has to stay visible.
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
use scoreboard_model::feed::LEAGUES;
use scoreboard_model::store::MenuRowInput;
use scoreboard_model::{Mode, Store};
use scoreboard_render::menu::thumb;

pub use crate::button::Press;

/// Inactivity before the menu applies and closes itself. `menu.py`'s
/// `_TIMEOUT_MS`.
pub const TIMEOUT_MS: u64 = 10_000;

/// Rows in the menu: one per registered league, in registry order, so the
/// mask the checkboxes make is `DeviceConfig::enabled_leagues`'s bit for bit.
pub const ROWS: usize = LEAGUES.len();

const _: () = assert!(ROWS <= u16::BITS as usize);

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
    /// The menu closed on a different set of leagues than it opened with. One
    /// bit per row of [`LEAGUES`] — the shape `DeviceConfig::set_enabled_leagues`
    /// takes — and from here the work is the caller's: write it, save it,
    /// rebuild the sources, and tick now, so the board moves off a league that
    /// was just switched off within a tick rather than a poll interval
    /// (`poller.py:353` woke the loop at this point for the same reason).
    LeaguesChosen(u16),
}

/// Core 0's owner of the league menu session.
#[derive(Debug, Clone)]
pub struct MenuController {
    active: bool,
    /// The mask the session opened with. An exit that lands back on it is
    /// [`Action::Handled`], not a write: toggling a league off and on again, or
    /// walking away from an untouched menu, must not cost a flash write.
    opened_with: u16,
    /// Working checkbox flags, parallel to [`LEAGUES`].
    checked: [bool; ROWS],
    /// `0..ROWS` are items; `ROWS` is the DONE footer.
    cursor: usize,
    /// First visible item.
    scroll: usize,
    last_input_ms: u64,
}

impl MenuController {
    pub const fn new() -> MenuController {
        MenuController {
            active: false,
            opened_with: 0,
            checked: [false; ROWS],
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
    ///
    /// `enabled` is the configuration's league mask *now*. It is read only by
    /// the press that opens the menu, to seed the checkboxes; from then on the
    /// session works on its own copy, so a settings save landing mid-session
    /// cannot renumber the boxes under the cursor. Every other press ignores
    /// it, which is why the caller can pass it unconditionally.
    pub fn press(
        &mut self,
        button: Button,
        press: Press,
        enabled: u16,
        store: &mut Store,
        now_ms: u64,
    ) -> Action {
        match (button, press, self.active) {
            (Button::A, Press::Short, true) => {
                self.touch(now_ms);
                self.advance(store, now_ms);
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
                self.select(store, now_ms)
            }
            (Button::B, Press::Short, false) => Action::ToggleLock,
            (Button::B, Press::Long, true) => self.apply_and_close(store),
            (Button::B, Press::Long, false) => {
                self.open(enabled, store, now_ms);
                Action::Handled
            }
        }
    }

    /// Apply and close if nothing has happened for [`TIMEOUT_MS`].
    pub fn check_timeout(&mut self, store: &mut Store, now_ms: u64) -> Action {
        if !self.active || now_ms.saturating_sub(self.last_input_ms) < TIMEOUT_MS {
            return Action::Handled;
        }
        self.apply_and_close(store)
    }

    /// Open the session, seeding the checkboxes from the configuration.
    ///
    /// Refuses during an OTA: the progress screen must stay visible because a
    /// reboot is imminent. `menu.py` also refused with no configured sources;
    /// the rows are the registry now, so there is always something to show.
    fn open(&mut self, enabled: u16, store: &mut Store, now_ms: u64) {
        if store.mode() == Mode::Updating {
            return;
        }
        self.opened_with = enabled;
        for (index, checked) in self.checked.iter_mut().enumerate() {
            *checked = enabled & (1 << index) != 0;
        }
        self.active = true;
        self.cursor = 0;
        self.scroll = 0;
        self.touch(now_ms);
        self.stage(store, now_ms);
    }

    /// Move the cursor one row down, wrapping through DONE.
    fn advance(&mut self, store: &mut Store, now_ms: u64) {
        self.cursor = (self.cursor + 1) % (ROWS + 1);
        // Keep the cursor inside the window. DONE is drawn in its own footer
        // and is always visible, so it needs no scroll of its own.
        if self.cursor == 0 {
            self.scroll = 0;
        } else if self.cursor < ROWS && self.cursor >= self.scroll + thumb::VISIBLE_ROWS {
            self.scroll = self.cursor - thumb::VISIBLE_ROWS + 1;
        }
        self.stage(store, now_ms);
    }

    /// Toggle the highlighted checkbox, or activate DONE.
    fn select(&mut self, store: &mut Store, now_ms: u64) -> Action {
        if self.cursor == ROWS {
            return self.apply_and_close(store);
        }
        if self.checked[self.cursor] && self.checked_count() == 1 {
            // Silent: there is nowhere to say it. See the module docs.
            return Action::Handled;
        }
        self.checked[self.cursor] = !self.checked[self.cursor];
        self.stage(store, now_ms);
        Action::Handled
    }

    /// Take the menu off the panel and report what was chosen, if it differs
    /// from what the session opened with.
    fn apply_and_close(&mut self, store: &mut Store) -> Action {
        let chosen = self.chosen();
        self.active = false;
        store.clear_menu();
        if chosen == self.opened_with {
            Action::Handled
        } else {
            Action::LeaguesChosen(chosen)
        }
    }

    /// The working flags as the configuration's mask.
    fn chosen(&self) -> u16 {
        self.checked
            .iter()
            .enumerate()
            .filter(|(_, checked)| **checked)
            .fold(0, |mask, (index, _)| mask | (1 << index))
    }

    fn checked_count(&self) -> usize {
        self.checked.iter().filter(|checked| **checked).count()
    }

    fn touch(&mut self, now_ms: u64) {
        self.last_input_ms = now_ms;
    }

    /// Build the visible window and STAGE it into the store.
    ///
    /// Staged, not published: this writes core 0's authoritative `Store`, and
    /// nothing reaches core 1 until the caller pushes the store across the
    /// snapshot channel. The app's poller does that on every `Action` this
    /// controller returns — a `Handled` that skipped the push left the menu
    /// running invisibly, which is the bug that earned this method its name.
    ///
    /// `menu.py` built fresh lists every time because core 1 may still be
    /// holding the previously published ones; here `Store::set_menu` copies
    /// into the snapshot it owns, so the wholesale-replacement contract is the
    /// snapshot channel's and this just passes a slice of borrowed labels.
    fn stage(&self, store: &mut Store, now_ms: u64) {
        let mut rows: Vec<MenuRowInput<'_>, { thumb::VISIBLE_ROWS }> = Vec::new();
        let end = (self.scroll + thumb::VISIBLE_ROWS).min(ROWS);
        for (index, league) in LEAGUES.iter().enumerate().take(end).skip(self.scroll) {
            let _ = rows.push(MenuRowInput {
                label: league.display_name,
                checked: self.checked[index],
                source: index as u8,
            });
        }
        // -1 is the DONE footer, which is where the cursor is when it is past
        // the last item.
        let highlight = if self.cursor < ROWS {
            (self.cursor - self.scroll) as i8
        } else {
            -1
        };
        let (thumb_y, thumb_h) = thumb::compute(ROWS, self.scroll);
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

    /// Bits of the configuration mask, by registry position.
    const MLB: u16 = 1 << 0;
    const NBA: u16 = 1 << 1;
    const NHL: u16 = 1 << 2;
    const WORLD_CUP: u16 = 1 << 8;
    /// Every registered league on.
    const ALL: u16 = (1 << ROWS) - 1;
    /// What the caller passes on a press that is not the opening one: the
    /// controller does not read it, and these tests prove that by passing
    /// something that would be wrong if it did.
    const IGNORED: u16 = 0;

    fn open(enabled: u16, store: &mut Store) -> MenuController {
        let mut menu = MenuController::new();
        assert_eq!(
            menu.press(Button::B, Press::Long, enabled, store, 0),
            Action::Handled
        );
        assert!(menu.active(), "a long press on B opens the menu");
        menu
    }

    /// `steps` short presses on A.
    fn down(menu: &mut MenuController, store: &mut Store, steps: usize) {
        for _ in 0..steps {
            menu.press(Button::A, Press::Short, IGNORED, store, 0);
        }
    }

    /// A short press on B: toggle the highlighted row, or activate DONE.
    fn select(menu: &mut MenuController, store: &mut Store) -> Action {
        menu.press(Button::B, Press::Short, IGNORED, store, 0)
    }

    /// A long press on B inside the menu: apply and close.
    fn close(menu: &mut MenuController, store: &mut Store) -> Action {
        menu.press(Button::B, Press::Long, IGNORED, store, 0)
    }

    #[test]
    fn a_closed_menu_routes_every_press_to_the_poller() {
        let mut menu = MenuController::new();
        let mut store = Store::new();
        for (button, press, expected) in [
            (Button::A, Press::Short, Action::Skip),
            (Button::A, Press::Long, Action::SkipLeague),
            (Button::B, Press::Short, Action::ToggleLock),
        ] {
            assert_eq!(menu.press(button, press, ALL, &mut store, 0), expected);
            assert!(!menu.active(), "none of these open the menu");
        }
    }

    #[test]
    fn opening_shows_the_registry_in_rotation_order_seeded_from_the_configuration() {
        let mut store = Store::new();
        // The shipped defaults: MLB and the NHL on.
        let _menu = open(MLB | NHL, &mut store);
        let view = &store.snapshot().menu;
        assert!(view.active);
        let labels: Vec<&str, { thumb::VISIBLE_ROWS }> =
            view.rows.iter().map(|row| row.label.as_str()).collect();
        assert_eq!(labels, ["MLB", "NBA", "NHL", "NFL", "NCAA FOOTBALL"]);
        let checked: Vec<bool, { thumb::VISIBLE_ROWS }> =
            view.rows.iter().map(|row| row.checked).collect();
        assert_eq!(checked, [true, false, true, false, false]);
        assert_eq!(view.highlight, 0, "the cursor starts on the first item");
        assert!(view.thumb_y >= 0, "nine rows need a scrollbar");
    }

    #[test]
    fn the_ninth_league_is_the_ninth_bit_and_reopens_as_the_configuration_says() {
        // One past what a byte-wide mask could hold.
        let mut store = Store::new();
        let mut menu = open(ALL, &mut store);
        down(&mut menu, &mut store, 8);
        select(&mut menu, &mut store);
        assert_eq!(
            close(&mut menu, &mut store),
            Action::LeaguesChosen(ALL & !WORLD_CUP)
        );

        // The caller wrote that mask; the next session opens on it.
        menu.press(Button::B, Press::Long, ALL & !WORLD_CUP, &mut store, 1_000);
        down(&mut menu, &mut store, 8);
        let view = &store.snapshot().menu;
        let row = &view.rows[view.highlight as usize];
        assert_eq!(row.source, 8);
        assert_eq!(row.label.as_str(), "WORLD CUP");
        assert!(!row.checked);
        select(&mut menu, &mut store);
        assert_eq!(close(&mut menu, &mut store), Action::LeaguesChosen(ALL));
    }

    #[test]
    fn opening_with_nothing_on_is_how_a_league_gets_switched_on() {
        // `menu.py` refused to open with no configured sources. The rows are
        // the registry now, so a board with every sport off can turn one on
        // from the panel.
        let mut store = Store::new();
        let mut menu = open(0, &mut store);
        assert!(store.snapshot().menu.rows.iter().all(|row| !row.checked));
        assert_eq!(select(&mut menu, &mut store), Action::Handled);
        assert!(store.snapshot().menu.rows[0].checked);
        assert_eq!(close(&mut menu, &mut store), Action::LeaguesChosen(MLB));
    }

    #[test]
    fn the_menu_refuses_to_open_over_an_ota_progress_screen() {
        let mut store = Store::new();
        store.set_mode(Mode::Updating);
        let mut menu = MenuController::new();
        menu.press(Button::B, Press::Long, ALL, &mut store, 0);
        assert!(!menu.active(), "a reboot is imminent; the progress bar stays");
        assert!(!store.snapshot().menu.active);
    }

    #[test]
    fn a_short_wraps_through_done_and_back_to_the_top() {
        let mut store = Store::new();
        let mut menu = open(ALL, &mut store);
        down(&mut menu, &mut store, 1);
        assert_eq!(store.snapshot().menu.highlight, 1);
        // The rest of the items, then DONE, then wrap.
        down(&mut menu, &mut store, ROWS - 1);
        assert_eq!(store.snapshot().menu.highlight, -1, "DONE");
        down(&mut menu, &mut store, 1);
        assert_eq!(store.snapshot().menu.highlight, 0, "wrapped to the top");
    }

    #[test]
    fn the_window_scrolls_to_keep_the_cursor_visible_and_the_thumb_follows() {
        let mut store = Store::new();
        let mut menu = open(ALL, &mut store);
        let (top_y, top_h) = (
            store.snapshot().menu.thumb_y,
            store.snapshot().menu.thumb_h,
        );
        assert!(top_y >= 0 && top_h > 0, "nine items need a scrollbar");

        // Four presses put the cursor on row 4, the last visible one.
        down(&mut menu, &mut store, 4);
        assert_eq!(store.snapshot().menu.highlight, 4);
        assert_eq!(store.snapshot().menu.thumb_y, top_y, "no scroll yet");

        // The fifth scrolls by one, and the highlight stays pinned to the
        // bottom row.
        down(&mut menu, &mut store, 1);
        assert_eq!(store.snapshot().menu.highlight, 4);
        assert!(store.snapshot().menu.thumb_y > top_y, "the thumb moved down");
        assert_eq!(
            store.snapshot().menu.rows[0].source,
            1,
            "the window starts at the second league"
        );

        // Wrapping back to the top resets the window in one step, rather than
        // scrolling back up through it.
        down(&mut menu, &mut store, ROWS - 5);
        assert_eq!(store.snapshot().menu.highlight, -1, "DONE");
        down(&mut menu, &mut store, 1);
        assert_eq!(store.snapshot().menu.highlight, 0);
        assert_eq!(store.snapshot().menu.thumb_y, top_y);
        assert_eq!(store.snapshot().menu.rows[0].source, 0);
    }

    #[test]
    fn b_short_toggles_the_highlighted_checkbox() {
        let mut store = Store::new();
        let mut menu = open(MLB | NBA, &mut store);
        select(&mut menu, &mut store);
        assert!(!store.snapshot().menu.rows[0].checked);
        select(&mut menu, &mut store);
        assert!(store.snapshot().menu.rows[0].checked);
    }

    #[test]
    fn the_last_checked_league_cannot_be_unchecked() {
        let mut store = Store::new();
        let mut menu = open(MLB | NBA, &mut store);
        // Uncheck the first, move to the second, try to uncheck it too.
        select(&mut menu, &mut store);
        down(&mut menu, &mut store, 1);
        assert_eq!(select(&mut menu, &mut store), Action::Handled);
        assert!(
            store.snapshot().menu.rows[1].checked,
            "an empty selection is a blank scoreboard; the refusal is silent"
        );
        assert_eq!(
            close(&mut menu, &mut store),
            Action::LeaguesChosen(NBA),
            "and what applies is the set with the refusal in it"
        );
    }

    #[test]
    fn b_long_applies_and_closes_with_the_chosen_mask() {
        let mut store = Store::new();
        let mut menu = open(MLB | NBA, &mut store);
        select(&mut menu, &mut store);
        assert_eq!(close(&mut menu, &mut store), Action::LeaguesChosen(NBA));
        assert!(!menu.active());
        assert!(!store.snapshot().menu.active, "the menu left the panel");
    }

    #[test]
    fn done_applies_and_closes_the_same_way_b_long_does() {
        let mut store = Store::new();
        let mut menu = open(MLB | NBA, &mut store);
        select(&mut menu, &mut store);
        // Cursor to DONE, then activate it.
        down(&mut menu, &mut store, ROWS);
        assert_eq!(store.snapshot().menu.highlight, -1);
        assert_eq!(select(&mut menu, &mut store), Action::LeaguesChosen(NBA));
        assert!(!menu.active());
        assert!(!store.snapshot().menu.active);
    }

    #[test]
    fn the_inactivity_timeout_applies_it_does_not_cancel() {
        let mut store = Store::new();
        let mut menu = open(MLB | NBA, &mut store);
        select(&mut menu, &mut store);

        assert_eq!(menu.deadline_ms(), Some(TIMEOUT_MS));
        assert_eq!(
            menu.check_timeout(&mut store, TIMEOUT_MS - 1),
            Action::Handled
        );
        assert!(menu.active(), "not yet");
        assert_eq!(
            menu.check_timeout(&mut store, TIMEOUT_MS),
            Action::LeaguesChosen(NBA),
            "there is no cancel path: walking away commits what is on screen"
        );
        assert!(!menu.active());
        assert!(!store.snapshot().menu.active);
        assert_eq!(menu.deadline_ms(), None);
    }

    #[test]
    fn every_press_including_the_no_op_hold_pushes_the_timeout_out() {
        let mut store = Store::new();
        let mut menu = open(MLB | NBA, &mut store);
        // A long press on A does nothing inside the menu, but the user is
        // clearly still there.
        assert_eq!(
            menu.press(Button::A, Press::Long, IGNORED, &mut store, 5_000),
            Action::Handled
        );
        assert_eq!(menu.deadline_ms(), Some(5_000 + TIMEOUT_MS));
        assert_eq!(
            menu.check_timeout(&mut store, TIMEOUT_MS + 1),
            Action::Handled
        );
        assert!(menu.active());
    }

    #[test]
    fn an_exit_that_lands_where_it_opened_is_handled_not_a_write() {
        let mut store = Store::new();
        // Off and on again is no change.
        let mut menu = open(MLB | NBA, &mut store);
        select(&mut menu, &mut store);
        select(&mut menu, &mut store);
        assert_eq!(
            close(&mut menu, &mut store),
            Action::Handled,
            "the configuration already says this, so there is nothing to save"
        );
        assert!(!store.snapshot().menu.active, "but the menu still closed");

        // And an untouched menu timing out is the same.
        menu.press(Button::B, Press::Long, ALL, &mut store, 20_000);
        assert_eq!(
            menu.check_timeout(&mut store, 20_000 + TIMEOUT_MS),
            Action::Handled
        );
        assert!(!menu.active());
        assert!(!store.snapshot().menu.active);
    }

    #[test]
    fn reopening_reads_the_configuration_not_the_last_session() {
        let mut store = Store::new();
        let mut menu = open(MLB | NBA, &mut store);
        select(&mut menu, &mut store);
        assert_eq!(close(&mut menu, &mut store), Action::LeaguesChosen(NBA));

        // The caller wrote NBA alone, and that is what the next session shows.
        menu.press(Button::B, Press::Long, NBA, &mut store, 20_000);
        let rows = &store.snapshot().menu.rows;
        assert!(!rows[0].checked);
        assert!(rows[1].checked);
        close(&mut menu, &mut store);

        // Had the write not happened — or a settings save undone it — the
        // menu would show that instead. It keeps nothing of its own between
        // sessions; the configuration is the only memory.
        menu.press(Button::B, Press::Long, MLB | NBA, &mut store, 40_000);
        assert!(store.snapshot().menu.rows[0].checked);
    }

    #[test]
    fn a_press_that_arrives_while_the_menu_is_open_never_reaches_the_poller() {
        let mut store = Store::new();
        let mut menu = open(MLB | NBA, &mut store);
        for (button, press) in [
            (Button::A, Press::Short),
            (Button::A, Press::Long),
            (Button::B, Press::Short),
        ] {
            let action = menu.press(button, press, IGNORED, &mut store, 0);
            assert!(
                matches!(action, Action::Handled | Action::LeaguesChosen(_)),
                "the menu owns both buttons while it is open, got {action:?}"
            );
        }
    }
}
