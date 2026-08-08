//! [`ScoreboardSnapshot`] — the core-0 → core-1 handoff (SPEC §4).
//!
//! # What lives here and what does not
//!
//! The snapshot carries **semantics**: strings that came off the wire or that
//! only core 0 can build (a local kickoff time needs the clock offset; a
//! line-score row needs the equal-column convention), numbers, colors, and the
//! *inputs* to geometry. It carries no pixels: no 1-bit text strips, no
//! projected field coordinates, no RGB565, no framebuffers.
//!
//! `state.py` did carry those, because in MicroPython the alternative was
//! per-frame work on core 1 — glyph-looping three line-score rows measured
//! ~41 ms of a 50 ms frame. That constraint is real and survives the port, but
//! it is a constraint on *when* the work happens, not on *which crate* holds
//! it: `scoreboard-render` builds a prepared view once per commit (keyed on
//! [`ScoreboardSnapshot::commit_seq`]) and the render path stays a pure
//! reader. Keeping fonts and layout out of this crate is what lets the whole
//! state machine be tested without a panel.
//!
//! Concretely, the renderer derives: text strips, the play flash's display
//! window, the pregame info cycle's per-phase dwell, the football field's
//! yardline→pixel map and perspective endpoints, the possession arrow's x, the
//! setup QR, menu row strips, and every RGB565 conversion.

use crate::color::{Rgb888, UiColors};
use crate::text::Text;

/// Re-exported rather than redefined: `scoreboard-wire`'s enums are plain
/// `Copy` data with no borrow, and both crates are in every consumer's tree
/// already. A parallel set here would be conversion boilerplate that can drift.
pub use scoreboard_wire::Side;
pub use scoreboard_wire::mlb::InningHalf;

/// Milliseconds from an arbitrary boot-relative origin — `embassy_time`'s
/// monotonic clock, as `time.ticks_ms()` was. Absolute stamps only; elapsed
/// time is always a difference taken by the reader.
pub type Millis = u64;

// -- String bounds ---------------------------------------------------------
//
// Sized from the corpus maxima measured by `backend/src/wire_corpus.rs`
// (`corpus_string_maxima`, and the `BUDGET` table beside it) plus margin, with
// the wire format's `u8` length prefix as the hard ceiling. `tests::bounds`
// re-measures the corpus against these numbers so a fixture that outgrows one
// fails here rather than silently truncating on a device.

/// ESPN event ids run to 9 digits today.
pub const GAME_ID: usize = 20;
/// Team abbreviation ("BOS", "MNUFC").
pub const ABBR: usize = 8;
/// Display clock ("10:08", "53.0", "90'+3'").
pub const CLOCK: usize = 12;
/// Play / commentary change-detection key.
pub const PLAY_ID: usize = 24;
/// Play or commentary line. Deliberately the wire's own cap: at 8 px per
/// `unscii_16` glyph, 255 glyphs is 2,040 px, which is the play strip pool's
/// 2,048 px capacity. No legal payload can overflow either, so the renderer's
/// per-glyph fallback stays structurally unreachable.
pub const PLAY_TEXT: usize = scoreboard_wire::MAX_STRING_BYTES;
/// Athlete short name ("G. Marquez", "R. Lukaku").
pub const PLAYER: usize = 40;
/// The pregame info line — stadium or league display name.
pub const INFO: usize = 56;
/// "72F PARTLY CLOUDY", or a bare stadium name on the sports with no weather.
pub const WEATHER: usize = 48;
/// Pre-formatted goal-scorer list.
pub const SCORERS: usize = 160;
/// The per-team pregame line: probable pitcher, rank line, or abbreviation.
pub const TEAM_LINE: usize = 40;
/// One line-score row: 3 chars per period, 32 periods.
pub const LINESCORE: usize = 96;
/// A pre-built panel line. 25 glyphs is the display cap; the byte bound is
/// wider because a glyph is up to 4 bytes and SSIDs are not our text.
pub const LINE: usize = 64;
/// Screen title ("API ERROR", "WRONG PASS") — 12 glyphs on the panel.
pub const TITLE: usize = 16;
/// Short chip text: "Q3", "1ST HALF", "F/10", "PENALTIES".
pub const SHORT: usize = 12;
/// Toast body ("LOCKED", "SKIPPING").
pub const TOAST: usize = 32;
/// League display name in the menu ("PREMIER LEAGUE", "NCAA FOOTBALL").
pub const MENU_LABEL: usize = 32;
/// Network identifiers shown on the setup screen.
pub const SSID: usize = 40;
pub const IP: usize = 16;

/// Every view's `new()` is a `const fn` so the whole snapshot can initialise a
/// `static`. `Default` derives from it rather than the other way round.
macro_rules! default_from_new {
    ($($view:ty),+ $(,)?) => {$(
        impl Default for $view {
            fn default() -> Self {
                Self::new()
            }
        }
    )+};
}

default_from_new!(
    StartupView,
    SetupView,
    ErrorView,
    UpdatingView,
    PlayView,
    MlbLiveView,
    PregameSide,
    PregameView,
    FinalView,
    SoccerLiveView,
    SoccerFinalView,
    NbaLiveView,
    FootballLiveView,
    ToastView,
    MenuRow,
    MenuView,
    ScoreboardSnapshot,
);

/// Rows the league menu shows at once (`menu._VISIBLE_ROWS`).
pub const MENU_ROWS: usize = 5;
/// Detail lines on the error screen.
pub const ERROR_LINES: usize = 4;

/// Which screen the renderer draws. `Final` is shared by MLB, NBA and
/// football — one line-score screen distinguished by [`FinalView::sport`];
/// soccer's full-time screen is its own shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Idle,
    Startup,
    NoGames,
    Setup,
    Error,
    Updating,
    MlbLive,
    Pregame,
    Final,
    SoccerLive,
    SoccerFinal,
    NbaLive,
    FootballLive,
}

impl Mode {
    /// Screens that only change on a commit, so the render loop can skip a
    /// frame outright (`display._STATIC_MODES`).
    pub const fn is_static(self) -> bool {
        matches!(
            self,
            Mode::Idle | Mode::NoGames | Mode::Error | Mode::Startup | Mode::Updating
        )
    }
}

/// Which sport a shared screen (pregame, line-score final) is showing —
/// the renderer's variant-table and column-header selector. Replaces
/// `state.py`'s `variant_key` strings and its `total_label` field, both of
/// which were this enum spelled out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sport {
    #[default]
    Mlb,
    Nba,
    Football,
    Soccer,
}

/// A 24×24 crest in the app's logo pool. The snapshot carries the handle, not
/// the 1,152 bytes: the pool outlives any one commit, and copying two crests
/// per publish would double the handoff cost for data that rarely changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogoRef(pub u8);

/// Boot progress. Every string is pre-built by the setter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupView {
    pub step: u8,
    pub total_steps: u8,
    pub operation: Text<LINE>,
    pub detail: Text<LINE>,
    /// Wi-Fi attempt in progress, and the retry budget. Zero hides the dots.
    pub attempt: u8,
    pub attempts_total: u8,
}

impl StartupView {
    pub const fn new() -> Self {
        Self {
            step: 1,
            total_steps: 5,
            operation: Text::new(),
            detail: Text::new(),
            attempt: 0,
            attempts_total: 0,
        }
    }
}

/// Why the device is in AP mode. Drives the setup screen's wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SetupReason {
    #[default]
    NoConfig,
    ConnectionFailed,
    BadAuth,
}

/// AP-mode setup screen. The QR is not here: it is a bitmap, and the renderer
/// builds it from [`SetupView::ap_ssid`] once per view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupView {
    pub reason: SetupReason,
    pub ap_ssid: Text<SSID>,
    pub ap_ip: Text<IP>,
    /// The station SSID that failed, for error context.
    pub wifi_ssid: Text<SSID>,
    pub title: Text<TITLE>,
    /// Pre-built body lines, named for their y position on the panel.
    pub line_18: Text<LINE>,
    pub line_28: Text<LINE>,
    pub line_44: Text<LINE>,
    pub line_54: Text<LINE>,
}

impl SetupView {
    pub const fn new() -> Self {
        Self {
            reason: SetupReason::NoConfig,
            ap_ssid: Text::new(),
            ap_ip: Text::new(),
            wifi_ssid: Text::new(),
            title: Text::new(),
            line_18: Text::new(),
            line_28: Text::new(),
            line_44: Text::new(),
            line_54: Text::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorView {
    pub title: Text<TITLE>,
    pub lines: heapless::Vec<Text<LINE>, ERROR_LINES, u8>,
}

impl ErrorView {
    pub const fn new() -> Self {
        Self {
            title: Text::new(),
            lines: heapless::Vec::new(),
        }
    }
}

/// OTA progress screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatingView {
    /// Bar fill, 0..=100.
    pub progress: u8,
    /// Percentage beside the bar; empty hides it.
    pub percent_text: Text<SHORT>,
    pub phase: Text<LINE>,
    pub detail: Text<LINE>,
}

impl UpdatingView {
    pub const fn new() -> Self {
        Self {
            progress: 0,
            percent_text: Text::new(),
            phase: Text::new(),
            detail: Text::new(),
        }
    }
}

/// The cross-sport play/commentary flash: written by every live commit (MLB
/// play, NBA play, football play, soccer commentary), read by every live
/// screen. Top level because it belongs to no one sport.
///
/// The renderer derives the visibility window from the text's measured width;
/// `updated_ms` is the anchor it counts from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayView {
    /// ESPN play id — the poller compares it to detect a new line.
    pub id: Text<PLAY_ID>,
    pub text: Text<PLAY_TEXT>,
    /// When `id` last changed.
    pub updated_ms: Millis,
}

impl PlayView {
    pub const fn new() -> Self {
        Self {
            id: Text::new(),
            text: Text::new(),
            updated_ms: 0,
        }
    }
}

/// Occupied bases (`mlb.Bases`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Bases {
    pub first: bool,
    pub second: bool,
    pub third: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MlbLiveView {
    pub half: InningHalf,
    /// The inning ordinal, pre-formatted: "7th".
    pub inning_text: Text<SHORT>,
    pub away_score: u16,
    pub home_score: u16,
    pub balls: u8,
    pub strikes: u8,
    pub outs: u8,
    pub bases: Bases,
    /// Half-resolved brightened team colors; `None` between halves, where the
    /// renderer draws the count and base markers dim. `bat_color` is also what
    /// the base markers and the critical-count pulse take their hue from —
    /// `state.py` kept a second, *unbrightened* copy for that and re-applied
    /// the brightening in the renderer.
    pub pitch_color: Option<Rgb888>,
    pub bat_color: Option<Rgb888>,
    pub pitcher: Text<PLAYER>,
    pub batter: Text<PLAYER>,
    /// False between innings and before an at-bat starts, when the two name
    /// slots are empty.
    pub has_at_bat: bool,
}

impl MlbLiveView {
    pub const fn new() -> Self {
        Self {
            half: InningHalf::Top,
            inning_text: Text::new(),
            away_score: 0,
            home_score: 0,
            balls: 0,
            strikes: 0,
            outs: 0,
            bases: Bases {
                first: false,
                second: false,
                third: false,
            },
            pitch_color: None,
            bat_color: None,
            pitcher: Text::new(),
            batter: Text::new(),
            has_at_bat: false,
        }
    }
}

/// One side of the pregame screen.
///
/// `line` is the per-team text slot under the divider. Each sport fills it
/// with what it has: MLB the probable starter, college football the rank line
/// ("#3 OHIO STATE"), soccer the team abbreviation, NBA nothing. The
/// MicroPython models rode this on a field literally named `pitcher`; the slot
/// is named for what it is instead, so nothing has to duck-type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PregameSide {
    pub record: Option<Record>,
    pub line: Text<TEAM_LINE>,
    pub color: Rgb888,
}

impl PregameSide {
    pub const fn new() -> Self {
        Self {
            record: None,
            line: Text::new(),
            color: Rgb888::WHITE,
        }
    }
}

/// Wins and losses. `None` on the view means the feed did not advertise a
/// record — the screen leaves the slot blank rather than render a fake 0-0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    pub wins: u16,
    pub losses: u16,
}

/// The upcoming-game screen: a big first-pitch time over one cycling info
/// line.
///
/// `info_primary` / `info_secondary` are the two phases of that cycle. MLB
/// fills them with stadium and weather; football and soccer with league name
/// and stadium; NBA leaves the second empty. In `state.py` those were fields
/// called `venue_text` and `weather_text` that three of the four sports lied
/// into — same bytes on the panel, honest names here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PregameView {
    pub sport: Sport,
    pub away: PregameSide,
    pub home: PregameSide,
    pub info_primary: Text<INFO>,
    pub info_secondary: Text<WEATHER>,
    /// "7:05 PM". Empty when the device has no UTC offset — a wrong-timezone
    /// time is worse than none.
    pub time_text: Text<SHORT>,
    /// "WED JUL 16", shown alternating with the time, and only when the
    /// game's local day is not today's.
    pub date_text: Text<SHORT>,
}

impl PregameView {
    pub const fn new() -> Self {
        Self {
            sport: Sport::Mlb,
            away: PregameSide::new(),
            home: PregameSide::new(),
            info_primary: Text::new(),
            info_secondary: Text::new(),
            time_text: Text::new(),
            date_text: Text::new(),
        }
    }
}

/// The line-score final screen, shared by MLB, NBA and football.
///
/// The three rows are equal-length strings, 3 chars per period, so they
/// measure identically in the fixed-width font and scroll in lockstep with no
/// extra mechanism. A team with fewer entries than `periods` gets `" X "` for
/// the missing trailing columns (walk-off convention).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalView {
    pub sport: Sport,
    pub away_score: u16,
    pub home_score: u16,
    /// "FINAL", "F/10" (extra innings), "F/OT", "F/2OT".
    pub final_text: Text<SHORT>,
    pub header_row: Text<LINESCORE>,
    pub away_row: Text<LINESCORE>,
    pub home_row: Text<LINESCORE>,
    pub home_won: bool,
    pub away_color: Rgb888,
    pub home_color: Rgb888,
}

impl FinalView {
    pub const fn new() -> Self {
        Self {
            sport: Sport::Mlb,
            away_score: 0,
            home_score: 0,
            final_text: Text::new(),
            header_row: Text::new(),
            away_row: Text::new(),
            home_row: Text::new(),
            home_won: false,
            away_color: Rgb888::WHITE,
            home_color: Rgb888::WHITE,
        }
    }
}

/// Live soccer, including the match-clock anchor.
///
/// The clock is not a string: core 0 stores the elapsed match seconds and the
/// millisecond it read them, and the renderer extrapolates the displayed
/// minute per frame. The clock therefore ticks between polls with no core-0
/// involvement, and an event-loop stall can neither freeze nor jump it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoccerLiveView {
    pub away_score: u16,
    pub home_score: u16,
    /// Elapsed match seconds at the anchor, and when it was taken.
    pub clock_anchor_s: u16,
    pub clock_anchor_ms: Millis,
    /// False during breaks, once a shootout starts, and when the upstream
    /// clock stopped advancing between polls (stale-feed guard).
    pub clock_running: bool,
    /// The current period's stoppage threshold in minutes: 45, 90, 105, 120.
    pub base_min: u8,
    pub on_break: bool,
    /// "1ST" / "2ND" / "ET" / "PENS"; empty during a break, where the clock
    /// slot announces the state instead.
    pub phase_text: Text<SHORT>,
    /// The spelled-out form for the wider variant: "1ST HALF".
    pub phase_long: Text<SHORT>,
    /// "GOAL 90'+3'" over the scorer's name, in the scoring side's color.
    pub event_top: Text<SHORT>,
    pub event_name: Text<PLAYER>,
    pub event_color: Rgb888,
    pub has_event: bool,
}

impl SoccerLiveView {
    pub const fn new() -> Self {
        Self {
            away_score: 0,
            home_score: 0,
            clock_anchor_s: 0,
            clock_anchor_ms: 0,
            clock_running: false,
            base_min: 45,
            on_break: false,
            phase_text: Text::new(),
            phase_long: Text::new(),
            event_top: Text::new(),
            event_name: Text::new(),
            event_color: Rgb888::WHITE,
            has_event: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoccerFinalView {
    pub away_score: u16,
    pub home_score: u16,
    pub home_won: bool,
    /// Soccer draws are real: a level score colors both sides, no dim loser.
    pub draw: bool,
    pub away_color: Rgb888,
    pub home_color: Rgb888,
    /// "FULL TIME" / "AET" / "PENALTIES".
    pub ft_text: Text<SHORT>,
    pub scorers_away: Text<SCORERS>,
    pub scorers_home: Text<SCORERS>,
}

impl SoccerFinalView {
    pub const fn new() -> Self {
        Self {
            away_score: 0,
            home_score: 0,
            home_won: false,
            draw: false,
            away_color: Rgb888::WHITE,
            home_color: Rgb888::WHITE,
            ft_text: Text::new(),
            scorers_away: Text::new(),
            scorers_home: Text::new(),
        }
    }
}

/// Live NBA. No clock anchor: a stop-clock has no run signal to extrapolate
/// from, so the poll-time string is redrawn until the next poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NbaLiveView {
    pub away_score: u16,
    pub home_score: u16,
    /// "Q3" / "OT" / "2OT"; empty at halftime.
    pub phase_text: Text<SHORT>,
    /// "4:37" / "53.0", or "HT" / "END" during a break.
    pub clock_text: Text<CLOCK>,
    /// Break state: draw the clock in the accent color.
    pub clock_accent: bool,
    /// Sub-minute in-play clock: draw it in the warning color.
    pub clock_low: bool,
}

impl NbaLiveView {
    pub const fn new() -> Self {
        Self {
            away_score: 0,
            home_score: 0,
            phase_text: Text::new(),
            clock_text: Text::new(),
            clock_accent: false,
            clock_low: false,
        }
    }
}

/// The drive situation, as semantics. The renderer turns this into the field
/// strip's pixels: the yardline→x map, both perspective line endpoints, and
/// the possession arrow's position beside the down-and-distance text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldSituation {
    pub down: u8,
    pub distance: u8,
    /// Possession-relative, 0..=100, as ESPN reports it.
    pub yard_line: u8,
    pub possession: Side,
    pub red_zone: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FootballLiveView {
    pub away_score: u16,
    pub home_score: u16,
    /// "Q3" / "OT"; empty at halftime.
    pub phase_text: Text<SHORT>,
    /// "10:42", or "HT" / "END" during a break.
    pub clock_text: Text<CLOCK>,
    pub clock_accent: bool,
    /// Sub-minute clock in a period that can end a half (Q2, Q4, OT).
    pub clock_low: bool,
    /// "3RD & 7", "1ST & GOAL"; empty when there is no situation.
    pub situation_text: Text<SHORT>,
    pub situation: Option<FieldSituation>,
    /// `None` when the feed did not advertise timeouts, so the bars stay
    /// undrawn rather than showing a fake three.
    pub away_timeouts: Option<u8>,
    pub home_timeouts: Option<u8>,
    pub away_color: Rgb888,
    pub home_color: Rgb888,
}

impl FootballLiveView {
    pub const fn new() -> Self {
        Self {
            away_score: 0,
            home_score: 0,
            phase_text: Text::new(),
            clock_text: Text::new(),
            clock_accent: false,
            clock_low: false,
            situation_text: Text::new(),
            situation: None,
            away_timeouts: None,
            home_timeouts: None,
            away_color: Rgb888::WHITE,
            home_color: Rgb888::WHITE,
        }
    }
}

/// How a toast presents itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToastKind {
    /// Text in the bottom strip.
    #[default]
    Text,
    /// Centered icon overlays; text is ignored.
    Lock,
    Unlock,
    Spinner,
}

/// How long a transient toast stays up.
pub const TOAST_DISPLAY_MS: Millis = 1_500;

/// Belt against a bug stranding a sticky toast on screen. Requests hard-cap at
/// 15 s, so 20 s is only reachable through a logic error.
pub const TOAST_STICKY_MAX_MS: Millis = 20_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToastView {
    pub text: Text<TOAST>,
    pub kind: ToastKind,
    /// When the toast was set; 0 = never.
    pub updated_ms: Millis,
    /// Persists past [`TOAST_DISPLAY_MS`] until explicitly cleared — an
    /// in-flight skip owns its spinner for exactly the work it announces.
    pub sticky: bool,
    /// Start of a one-shot "rejected press" dim; 0 = not pulsing.
    pub pulse_ms: Millis,
}

impl ToastView {
    pub const fn new() -> Self {
        Self {
            text: Text::new(),
            kind: ToastKind::Text,
            updated_ms: 0,
            sticky: false,
            pulse_ms: 0,
        }
    }
}

/// One visible row of the league menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuRow {
    pub label: Text<MENU_LABEL>,
    pub checked: bool,
    /// Index of the league this row stands for, stable across scrolls — the
    /// marquee restamps when the highlighted *item* changes, not when the
    /// cursor lands on the same item at a different row.
    pub source: u8,
}

impl MenuRow {
    pub const fn new() -> Self {
        Self {
            label: Text::new(),
            checked: false,
            source: 0,
        }
    }
}

/// The league-select overlay. When active it *is* the frame — the renderer's
/// mode dispatch is preempted entirely, so toasts and poll commits continue
/// underneath, invisible, with no special-casing.
///
/// Only the visible window is published. Scroll geometry is computed by the
/// controller so the renderer draws two rects verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuView {
    pub active: bool,
    /// Marquee anchor, restamped only on open and on a highlight change, so a
    /// checkbox toggle never jerks an in-progress scroll.
    pub updated_ms: Millis,
    pub rows: heapless::Vec<MenuRow, MENU_ROWS, u8>,
    /// Visible-row index of the cursor; -1 is the DONE footer.
    pub highlight: i8,
    /// Scrollbar thumb; `thumb_y` of -1 means no scrollbar.
    pub thumb_y: i8,
    pub thumb_h: u8,
}

impl MenuView {
    pub const fn new() -> Self {
        Self {
            active: false,
            updated_ms: 0,
            rows: heapless::Vec::new(),
            highlight: -1,
            thumb_y: -1,
            thumb_h: 0,
        }
    }
}

/// The complete display state for one frame.
///
/// Core 0 owns the authoritative copy inside a [`crate::Store`] and publishes
/// clones of it through [`crate::SnapshotChannel`]; core 1 latches one at the
/// top of each frame and renders from it for the whole frame. Every field is
/// owned and bounded — nothing borrows the receive buffer, which is what makes
/// the type `Send`/`Sync` and the handoff a plain index swap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoreboardSnapshot {
    pub mode: Mode,
    /// Bumped by every commit. The render loop's static-screen skip compares
    /// it; the renderer's prepared view keys its cached strips on it.
    pub commit_seq: u32,
    /// Restarts every continuous animation. Stamped only when the displayed
    /// view identity — `(mode, game_id)` — changes, so a standing re-poll of
    /// the same game keeps the scroll where it is.
    pub animation_start_ms: Millis,
    /// The game on screen. Hoisted out of the per-sport views (where
    /// `state.py` kept seven copies of it) so the view-identity rule has one
    /// implementation; the renderer reads it only to tell "no game yet" from
    /// "a game".
    pub game_id: Text<GAME_ID>,
    pub away_abbr: Text<ABBR>,
    pub home_abbr: Text<ABBR>,
    pub away_logo: Option<LogoRef>,
    pub home_logo: Option<LogoRef>,
    pub ui_colors: UiColors,
    pub startup: StartupView,
    pub setup: SetupView,
    pub error: ErrorView,
    pub updating: UpdatingView,
    pub play: PlayView,
    pub mlb_live: MlbLiveView,
    pub pregame: PregameView,
    pub linescore_final: FinalView,
    pub soccer_live: SoccerLiveView,
    pub soccer_final: SoccerFinalView,
    pub nba_live: NbaLiveView,
    pub football_live: FootballLiveView,
    pub toast: ToastView,
    pub menu: MenuView,
}

impl ScoreboardSnapshot {
    pub const fn new() -> Self {
        Self {
            mode: Mode::Idle,
            commit_seq: 0,
            animation_start_ms: 0,
            game_id: Text::new(),
            away_abbr: Text::new(),
            home_abbr: Text::new(),
            away_logo: None,
            home_logo: None,
            ui_colors: UiColors::new(),
            startup: StartupView::new(),
            setup: SetupView::new(),
            error: ErrorView::new(),
            updating: UpdatingView::new(),
            play: PlayView::new(),
            mlb_live: MlbLiveView::new(),
            pregame: PregameView::new(),
            linescore_final: FinalView::new(),
            soccer_live: SoccerLiveView::new(),
            soccer_final: SoccerFinalView::new(),
            nba_live: NbaLiveView::new(),
            football_live: FootballLiveView::new(),
            toast: ToastView::new(),
            menu: MenuView::new(),
        }
    }

    /// Bytes one snapshot occupies. Identical on the host and on
    /// `thumbv8m.main-none-eabihf`: every field is a fixed-width integer, an
    /// enum, or a [`Text`] whose length prefix is a `u16`.
    pub const SIZE: usize = core::mem::size_of::<Self>();
}

// SPEC §4 requires the handoff type to be `Sync`, and `SnapshotChannel`'s
// `unsafe impl Sync` rests on it: core 1 reads a slot core 0 wrote. Both hold
// today because every field is plain owned data, but that is a property of the
// fields, not a declaration — one `Cell` or one raw pointer added later would
// take it away silently. This fails the build instead.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ScoreboardSnapshot>();
};
