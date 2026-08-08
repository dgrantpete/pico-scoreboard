//! The poller-facing update API: one authoritative snapshot, one place that
//! decides when an animation restarts.

use crate::color::UiColors;
use crate::snapshot::{
    ERROR_LINES, LogoRef, MenuRow, Millis, Mode, ScoreboardSnapshot, SetupReason, TOAST_DISPLAY_MS,
    ToastKind,
};
use crate::text::{Text, set_capped, set_folded, set_line, set_plain, write_text};

/// Both crest handles for the game being committed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Logos {
    pub away: Option<LogoRef>,
    pub home: Option<LogoRef>,
}

/// One visible menu row, as the controller publishes it.
#[derive(Debug, Clone, Copy)]
pub struct MenuRowInput<'a> {
    pub label: &'a str,
    pub checked: bool,
    /// Index of the league this row stands for.
    pub source: u8,
}

/// Where [`Store::finish_startup`] hands off. The single transition out of the
/// startup phase, so no caller can half-leave it.
#[derive(Debug, Clone, Copy)]
pub enum StartupExit<'a> {
    /// `Mode::Idle` in practice; the poller takes it from there.
    Mode(Mode),
    Setup {
        reason: SetupReason,
        ap_ssid: &'a str,
        ap_ip: &'a str,
        wifi_ssid: &'a str,
    },
    Error {
        title: &'a str,
        lines: &'a [&'a str],
    },
}

/// Owns the authoritative display state and every rule for changing it.
///
/// Core 0 mutates this; [`crate::SnapshotChannel`] publishes clones of
/// [`Store::snapshot`] to core 1. Nothing here is `async` or hardware-aware,
/// so the whole state machine is exercised by host tests.
#[derive(Debug, Clone)]
pub struct Store {
    snapshot: ScoreboardSnapshot,
    /// True until [`Store::finish_startup`]; gates the boot-progress screen.
    startup_phase: bool,
    /// The last committed soccer live game and its upstream clock, for the
    /// stale-feed guard. `poller.py` carried this beside the poll loop and
    /// passed it back in; it belongs with the state it guards.
    prev_soccer_clock: Option<(Text<{ crate::snapshot::GAME_ID }>, u16)>,
}

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}

impl Store {
    pub const fn new() -> Self {
        Self {
            snapshot: ScoreboardSnapshot::new(),
            startup_phase: true,
            prev_soccer_clock: None,
        }
    }

    pub fn snapshot(&self) -> &ScoreboardSnapshot {
        &self.snapshot
    }

    pub fn commit_seq(&self) -> u32 {
        self.snapshot.commit_seq
    }

    pub fn mode(&self) -> Mode {
        self.snapshot.mode
    }

    /// Publish the accumulated mutations. Every public setter ends with this;
    /// it exists separately only so the setters read like the MicroPython ones.
    fn commit(&mut self) {
        self.snapshot.commit_seq = self.snapshot.commit_seq.wrapping_add(1);
    }

    /// **The view-identity rule**, in the one place it exists.
    ///
    /// A game screen restarts its animations only when the displayed identity
    /// — `(mode, game_id)` — changes. A standing re-poll of the same game
    /// keeps the scroll, the pregame info cycle and the line-score crawl
    /// exactly where they are; everything else is rebuilt on every commit so
    /// late data corrections still land.
    ///
    /// `state.py` spelled this out at seven setters, one of which could have
    /// drifted from the others without anything noticing.
    fn enter_game_view(&mut self, mode: Mode, game_id: &str, logos: Logos, now_ms: Millis) {
        let changed = self.snapshot.mode != mode || self.snapshot.game_id != game_id;
        self.snapshot.mode = mode;
        if changed {
            self.snapshot.animation_start_ms = now_ms;
        }
        set_folded(&mut self.snapshot.game_id, game_id);
        self.snapshot.away_logo = logos.away;
        self.snapshot.home_logo = logos.home;
    }

    /// Copy both team abbreviations into the snapshot's shared slots.
    fn set_teams(&mut self, away: &str, home: &str) {
        set_folded(&mut self.snapshot.away_abbr, away);
        set_folded(&mut self.snapshot.home_abbr, home);
    }

    // -- Non-game screens --------------------------------------------------

    pub fn set_mode(&mut self, mode: Mode) {
        self.snapshot.mode = mode;
        self.commit();
    }

    /// Update boot progress. A no-op once startup has finished.
    ///
    /// The visible step is monotonic: the Wi-Fi retry loop re-enters earlier
    /// steps, and the bar must not walk backwards. Retries read as new text
    /// plus attempt dots (`attempt` / `attempts_total`; zero hides them).
    pub fn set_startup_step(
        &mut self,
        step: u8,
        total: u8,
        operation: &str,
        detail: &str,
        attempt: u8,
        attempts_total: u8,
    ) {
        if !self.startup_phase {
            return;
        }
        self.snapshot.mode = Mode::Startup;
        let startup = &mut self.snapshot.startup;
        startup.step = step.max(startup.step);
        startup.total_steps = total;
        set_line(&mut startup.operation, operation);
        set_line(&mut startup.detail, detail);
        startup.attempt = attempt;
        startup.attempts_total = attempts_total;
        self.commit();
    }

    /// Boot progress as a fraction, for the renderer's step counter ("2/5").
    /// Formatting a two-digit pair is not worth a snapshot field.
    pub fn startup_step(&self) -> (u8, u8) {
        (
            self.snapshot.startup.step,
            self.snapshot.startup.total_steps,
        )
    }

    /// End the startup phase and enter the runtime screen. After this,
    /// [`Store::set_startup_step`] does nothing.
    pub fn finish_startup(&mut self, exit: StartupExit<'_>) {
        self.startup_phase = false;
        self.snapshot.startup = crate::snapshot::StartupView::new();
        match exit {
            StartupExit::Mode(mode) => self.set_mode(mode),
            StartupExit::Setup {
                reason,
                ap_ssid,
                ap_ip,
                wifi_ssid,
            } => self.set_setup_mode(reason, ap_ssid, ap_ip, wifi_ssid),
            StartupExit::Error { title, lines } => self.set_error(title, lines),
        }
    }

    /// AP-mode setup screen, with the wording the failure reason calls for.
    pub fn set_setup_mode(
        &mut self,
        reason: SetupReason,
        ap_ssid: &str,
        ap_ip: &str,
        wifi_ssid: &str,
    ) {
        self.snapshot.mode = Mode::Setup;
        let setup = &mut self.snapshot.setup;
        setup.reason = reason;
        set_folded(&mut setup.ap_ssid, ap_ssid);
        set_plain(&mut setup.ap_ip, ap_ip);
        set_folded(&mut setup.wifi_ssid, wifi_ssid);

        let shown_ssid = if ap_ssid.is_empty() {
            "scoreboard"
        } else {
            ap_ssid
        };
        let shown_ip = if ap_ip.is_empty() {
            "192.168.4.1"
        } else {
            ap_ip
        };
        match reason {
            SetupReason::BadAuth => {
                set_plain(&mut setup.title, "WRONG PASS");
                write_text!(&mut setup.line_18, "for \"{wifi_ssid}\"");
                write_text!(&mut setup.line_28, "Scan/join \"{shown_ssid}\"");
                write_text!(&mut setup.line_44, "Then go to {shown_ip}");
                set_plain(&mut setup.line_54, "to fix password");
            }
            SetupReason::ConnectionFailed => {
                set_plain(&mut setup.title, "WIFI FAIL");
                write_text!(&mut setup.line_18, "\"{wifi_ssid}\"");
                write_text!(&mut setup.line_28, "Scan/join \"{shown_ssid}\"");
                write_text!(&mut setup.line_44, "Then go to {shown_ip}");
                set_plain(&mut setup.line_54, "to reconfigure");
            }
            SetupReason::NoConfig => {
                set_plain(&mut setup.title, "SETUP");
                set_plain(&mut setup.line_18, "Scan QR or join");
                write_text!(&mut setup.line_28, "\"{shown_ssid}\" WiFi");
                set_plain(&mut setup.line_44, "Then go to");
                set_plain(&mut setup.line_54, shown_ip);
            }
        }
        self.commit();
    }

    /// Error screen. The title caps at 12 glyphs and each line at 25; lines
    /// past the fourth are dropped.
    pub fn set_error(&mut self, title: &str, lines: &[&str]) {
        self.snapshot.mode = Mode::Error;
        let error = &mut self.snapshot.error;
        if title.is_empty() {
            set_plain(&mut error.title, "ERROR");
        } else {
            set_capped(&mut error.title, title, 12);
        }
        error.lines.clear();
        for line in lines.iter().take(ERROR_LINES) {
            let mut text = Text::new();
            set_line(&mut text, line);
            let _ = error.lines.push(text);
        }
        self.commit();
    }

    /// OTA download progress. Called once per percent change.
    pub fn set_updating_progress(&mut self, percent: u8, version_short: &str) {
        self.snapshot.mode = Mode::Updating;
        let updating = &mut self.snapshot.updating;
        updating.progress = percent;
        write_text!(&mut updating.percent_text, "{percent}%");
        set_plain(&mut updating.phase, "Downloading");
        let mut detail = Text::<{ crate::snapshot::LINE }>::new();
        write_text!(&mut detail, "v {version_short}");
        set_line(&mut updating.detail, detail.as_str());
        self.commit();
    }

    /// The pre-restart countdown, bar full. The version detail carries over
    /// from the download phase.
    pub fn set_updating_countdown(&mut self, seconds: u8) {
        self.snapshot.mode = Mode::Updating;
        let updating = &mut self.snapshot.updating;
        updating.progress = 100;
        updating.percent_text.clear();
        write_text!(&mut updating.phase, "Restarting in {seconds}");
        self.commit();
    }

    pub fn set_ui_colors(&mut self, colors: UiColors) {
        self.snapshot.ui_colors = colors;
        self.commit();
    }

    // -- Toasts ------------------------------------------------------------

    /// Show a transient overlay: bottom-strip text, or a centered icon.
    ///
    /// A sticky toast — the in-flight skip spinner — persists until
    /// [`Store::clear_toast_if_sticky`]; everything else expires on its own
    /// after [`TOAST_DISPLAY_MS`].
    pub fn set_toast(&mut self, text: &str, kind: ToastKind, sticky: bool, now_ms: Millis) {
        let toast = &mut self.snapshot.toast;
        set_folded(&mut toast.text, text);
        toast.kind = kind;
        toast.updated_ms = now_ms;
        toast.sticky = sticky;
        toast.pulse_ms = 0;
        self.commit();
    }

    /// Tear down a sticky toast; a no-op on any other kind.
    ///
    /// The skip tick's teardown runs on every exit path, so this must not
    /// clobber a LOCKED toast an unrelated press fired mid-skip. The stamp is
    /// rewound to "just expired" rather than zeroed, and the kind is kept, so
    /// the renderer's overlay fade eases out instead of snapping.
    pub fn clear_toast_if_sticky(&mut self, now_ms: Millis) {
        if !self.snapshot.toast.sticky {
            return;
        }
        let toast = &mut self.snapshot.toast;
        toast.text.clear();
        toast.updated_ms = now_ms.saturating_sub(TOAST_DISPLAY_MS);
        toast.sticky = false;
        toast.pulse_ms = 0;
        self.commit();
    }

    /// Dim the visible toast one cycle: a button press that landed while a
    /// skip was already in flight is rejected, not queued, and this is the
    /// feedback. Restamps per press, so hammering the button dims per press.
    pub fn pulse_toast(&mut self, now_ms: Millis) {
        self.snapshot.toast.pulse_ms = now_ms;
        self.commit();
    }

    // -- League menu -------------------------------------------------------

    /// Publish the menu's visible window, opening it if it was closed.
    ///
    /// The marquee restamps only when the highlighted *item* changes — a
    /// different row, a different league under the same row (the window
    /// scrolled), or a fresh open — so toggling a checkbox never restarts an
    /// in-progress scroll.
    pub fn set_menu(
        &mut self,
        rows: &[MenuRowInput<'_>],
        highlight: i8,
        thumb_y: i8,
        thumb_h: u8,
        now_ms: Millis,
    ) {
        let menu = &mut self.snapshot.menu;
        let previous = if menu.active {
            usize::try_from(menu.highlight)
                .ok()
                .and_then(|index| menu.rows.get(index))
                .map(|row| row.source)
        } else {
            None
        };
        let current = usize::try_from(highlight)
            .ok()
            .and_then(|index| rows.get(index))
            .map(|row| row.source);
        if !menu.active || highlight != menu.highlight || current != previous {
            menu.updated_ms = now_ms;
        }
        menu.active = true;
        menu.rows.clear();
        for row in rows.iter().take(crate::snapshot::MENU_ROWS) {
            let mut published = MenuRow::new();
            set_folded(&mut published.label, row.label);
            published.checked = row.checked;
            published.source = row.source;
            let _ = menu.rows.push(published);
        }
        menu.highlight = highlight;
        menu.thumb_y = thumb_y;
        menu.thumb_h = thumb_h;
        self.commit();
    }

    /// Close the menu. The mode underneath — which kept committing beneath the
    /// take-over — shows again on the next frame.
    pub fn clear_menu(&mut self) {
        if !self.snapshot.menu.active {
            return;
        }
        self.snapshot.menu = crate::snapshot::MenuView::new();
        self.commit();
    }

    // -- The shared play flash --------------------------------------------

    /// Stage the cross-sport flash slot when `id` names a line we have not
    /// shown. Returns whether anything changed.
    ///
    /// One mechanism for MLB plays, NBA plays, football plays and soccer
    /// commentary. The previous id lives in the snapshot, so change detection
    /// needs no state beside the poller — and rotating to a different game
    /// legitimately trips it, which is what lets a viewer catch up on the new
    /// game's latest line.
    pub fn flash_play(&mut self, id: &str, text: &str, now_ms: Millis) -> bool {
        if id.is_empty() || self.snapshot.play.id == id {
            return false;
        }
        let play = &mut self.snapshot.play;
        set_folded(&mut play.id, id);
        set_folded(&mut play.text, text);
        play.updated_ms = now_ms;
        self.commit();
        true
    }

    // -- Shared internals for the per-sport commits (see `sports`) ---------

    pub(crate) fn snapshot_mut(&mut self) -> &mut ScoreboardSnapshot {
        &mut self.snapshot
    }

    pub(crate) fn begin_game(
        &mut self,
        mode: Mode,
        game_id: &str,
        away: &str,
        home: &str,
        logos: Logos,
        now_ms: Millis,
    ) {
        self.enter_game_view(mode, game_id, logos, now_ms);
        self.set_teams(away, home);
    }

    pub(crate) fn finish_game(&mut self) {
        self.commit();
    }

    /// The soccer stale-clock guard: the previous poll's clock for this game,
    /// or `None` when the last soccer commit was a different game.
    pub(crate) fn take_prev_soccer_clock(&mut self, game_id: &str, clock_s: u16) -> Option<u16> {
        let previous = self
            .prev_soccer_clock
            .as_ref()
            .filter(|(id, _)| id == game_id)
            .map(|(_, clock)| *clock);
        let mut id = Text::new();
        set_plain(&mut id, game_id);
        self.prev_soccer_clock = Some((id, clock_s));
        previous
    }
}
