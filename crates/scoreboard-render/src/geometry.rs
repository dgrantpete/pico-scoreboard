//! Where every text slot and drawn primitive goes on the panel — the port of
//! `screen_geometry.py`.
//!
//! # The convention split
//!
//! Coordinates registered to sprite *art* live in the Aseprite file and reach
//! this crate through [`crate::generated::layout`]. Coordinates registered to
//! *text slots and drawn primitives* — score windows, dividers, count-dot rows
//! — live here, as code. A live screen therefore draws its sprites from the
//! art's own positions and places every string from a table below; the pregame
//! and final screens carry no sprites at all.
//!
//! # One design per screen
//!
//! Each screen has exactly one table, a `const` struct below. Through
//! 2026-11-08 the final, soccer-live and hockey-live screens carried two or
//! three selectable designs behind a `display.variants` config key; the
//! gallery-review defaults (the "line-score forward" final, soccer's "phase
//! ledger", hockey's rink) were the only ones anyone ran, so they are now the
//! only ones there are, and the selector went with the alternatives.

use crate::blit::Slice;
use crate::font::Scroll;
use scoreboard_model::Mode;

/// Panel width in pixels.
pub const WIDTH: i32 = 128;
/// Panel height in pixels.
pub const HEIGHT: i32 = 64;

// These are pinned against `hub75::geometry` by a compile-time assertion in
// `tests/geometry.rs`. It lives there so the shipping graph of this crate does
// not pull in a crate that touches a PAC (SPEC §2's crate boundary rule), while
// the build still fails the moment the two disagree.

/// The panel's outermost ring of pixels is unreliable — garbage-colored LEDs
/// have surfaced there — so nothing draws in row 0, row 63, column 0 or
/// column 127 (the owner's edge rule, BACKLOG 56). Rules that would span the
/// panel stop this far short of each edge; the slot tables below respect it.
///
/// A glyph *cell* may still overhang onto the ring when its ink provably does
/// not: every digit leaves its last column blank (the eighth of an `unscii_16`
/// cell, the fifth of a `spleen_5x8` one), so a three-digit total centered in
/// a two-digit slot can put its last cell on column 127 and light nothing
/// there.
pub const EDGE_INSET: i32 = 1;

// =============================================================================
// Scroll speeds
// =============================================================================

/// Frames per second the render loop runs at, from [`crate::time::FPS`] — the
/// one constant every frame-coupled number in this crate is derived from.
pub const FPS: i32 = crate::time::FPS as i32;

/// Speeds the config accepts for the game-description scrollers (the play
/// flash and the soccer event/scorer lines).
///
/// **Every speed must evenly divide the frame rate, in one direction or the
/// other.** The scroll offset is derived from elapsed time, so a speed of `S`
/// px/s advances `S / FPS` px per rendered frame. Neither a non-integer
/// px/frame nor a non-integer frames/px is smooth: at 20 FPS, 30 px/s is
/// 1.5 px per frame, realised by floor math as alternating 1 px and 2 px steps,
/// so every third pixel column is never displayed. The mirror case is 40 px/s
/// at 60 FPS — ⅔ px per frame, which shows every column but dwells on them for
/// alternately one and two frames. Both read as a rhythmic stutter on the
/// panel rather than as motion.
///
/// The legal values are therefore the divisors of [`FPS`] together with its
/// multiples. At 60 that is a generous ladder — {1, 2, 3, 4, 5, 6, 10, 12, 15,
/// 20, 30, 60} and up — and the set below is the slice of it worth offering in
/// a dropdown: the three the parity release shipped that are still legal, the
/// 30 px/s the whole 60 FPS change exists to make expressible, and 60 as the
/// fast end. [`is_smooth`] is the test, and the const block below applies it to
/// every member.
///
/// **40 px/s is no longer in the set.** It was uniform-but-coarse at 20 FPS
/// (2 px per frame) and is the stutter case at 60, so it degrades like any
/// other illegal value — see [`DEFAULT_SCROLL_SPEED`], which is chosen so that
/// the degrade lands somewhere a device configured for 40 would want to be.
pub const SCROLL_SPEEDS: [i32; 5] = [5, 10, 20, 30, 60];

/// What an out-of-set configured speed degrades to.
///
/// 30 px/s: one pixel every two frames. It is the nearest legal speed to the
/// 40 px/s the parity release accepted and stored, which makes it what a device
/// carrying that value falls back to — a quarter slower rather than half — and
/// it is the speed the panel was upgraded to 60 FPS to be able to draw.
pub const DEFAULT_SCROLL_SPEED: i32 = 30;

/// Whether `speed` px/s yields evenly spaced pixel steps at [`FPS`].
pub const fn is_smooth(speed: i32) -> bool {
    speed > 0 && (speed % FPS == 0 || FPS % speed == 0)
}

const _: () = {
    let mut index = 0;
    while index < SCROLL_SPEEDS.len() {
        assert!(
            is_smooth(SCROLL_SPEEDS[index]),
            "a configurable scroll speed that does not divide the frame rate \
             drops pixel columns — see SCROLL_SPEEDS"
        );
        index += 1;
    }
    assert!(is_smooth(DEFAULT_SCROLL_SPEED));
};

/// Apply `config.display.scroll_speed_px_per_sec`, degrading anything outside
/// [`SCROLL_SPEEDS`] to [`DEFAULT_SCROLL_SPEED`].
pub const fn scroll_speed(requested: i32) -> i32 {
    let mut index = 0;
    while index < SCROLL_SPEEDS.len() {
        if SCROLL_SPEEDS[index] == requested {
            return requested;
        }
        index += 1;
    }
    DEFAULT_SCROLL_SPEED
}

// =============================================================================
// Tunables
// =============================================================================

/// Minimum dwell for one cycling pregame info phase. A phase whose text scrolls
/// stays up for at least one full scroll cycle, but never less than this, so
/// short lines do not flash by.
pub const PREGAME_INFO_DWELL_MS: u64 = 4000;

/// Scroll feel for the pregame info line (venue ↔ weather). 20 px/s is one
/// pixel every three frames.
pub const PREGAME_SCROLL: Scroll = Scroll {
    pause_ms: 1000,
    pixels_per_second: 20,
};

/// Final line-score horizontal scroll: slow, long dwell — the score is the
/// point and the scroll is a reveal of later innings. 10 px/s is exactly six
/// frames per pixel; 12 was tried at 20 FPS and showed every pixel with uneven
/// dwell, which is the same objection [`SCROLL_SPEEDS`] documents (12 is legal
/// at 60 FPS, but the feel was chosen and 10 is the feel).
pub const FINAL_LINESCORE_SCROLL: Scroll = Scroll {
    pause_ms: 1800,
    pixels_per_second: 10,
};

/// The two fixed scroll feels above and [`Scroll::DEFAULT`] are not
/// configurable, so nothing degrades them and nothing would catch them drifting
/// out of the legal set when the frame rate moves. This does.
const _: () = {
    assert!(
        is_smooth(PREGAME_SCROLL.pixels_per_second),
        "the pregame info line scrolls at a speed that stutters at this frame rate"
    );
    assert!(
        is_smooth(FINAL_LINESCORE_SCROLL.pixels_per_second),
        "the final line score scrolls at a speed that stutters at this frame rate"
    );
    assert!(
        is_smooth(Scroll::DEFAULT.pixels_per_second),
        "the default text scroll stutters at this frame rate"
    );
};

/// Pause feel for the soccer last-event line and full-time scorer lists. The
/// speed is the user-configurable one.
pub const SOCCER_SCROLL_PAUSE_MS: u64 = 1500;

/// Dwell at each end of the play flash's scroll.
pub const PLAY_SCROLL_PAUSE_MS: u64 = 1000;

/// Pause feel for hockey's last-goal lines; the speed is the user-configurable
/// one, as for soccer's event line.
pub const HOCKEY_SCROLL_PAUSE_MS: u64 = 1500;

/// The three-stars final's names and series line: a fixed feel, like the
/// pregame lines, slow enough to read a name off.
pub const HOCKEY_STARS_SCROLL: Scroll = Scroll {
    pause_ms: 1500,
    pixels_per_second: 20,
};

const _: () = assert!(
    is_smooth(HOCKEY_STARS_SCROLL.pixels_per_second),
    "the three stars scroll at a speed that stutters at this frame rate"
);

/// The bottom strip of the column-frame live screens — MLB, NBA and soccer:
/// the data column under its rule, from the column's text margin to the last
/// legal column. The play flash draws here.
///
/// It is not every live screen's strip. The bottom strip is whatever the screen
/// gives up while the flash shows, so it is a per-screen slot (`strip` on each
/// live table): the column screens keep their identity column drawn and give
/// up only this, while football and hockey have no column and give up the full
/// width. [`bottom_strip`] selects one by mode.
pub const COLUMN_STRIP: Slice = Slice {
    x: 51,
    y: 43,
    width: 76,
    height: 16,
};

// =============================================================================
// Football field mapping
// =============================================================================

/// Away goal line (yard 0). The field spans 100 yards at 1 px/yard between the
/// goal lines; the sprite's 11 px endzone blocks sit outside that span.
pub const FOOTBALL_FIELD_YARD0_X: i32 = 14;
/// Clamp for the 2 px-wide perspective lines.
pub const FOOTBALL_FIELD_LOS_MAX_X: i32 = 113;
/// The vanishing point's column.
pub const FOOTBALL_VP_X: i32 = 63;
/// Perspective lean: 10 field rows of a 63-row run to the vanishing point.
pub const FOOTBALL_PERSP_NUM: i32 = 10;
pub const FOOTBALL_PERSP_DEN: i32 = 63;

/// Where a vertical field line whose bottom endpoint is at `x` meets the
/// field's top row.
///
/// Rounds half away from zero, which is what `state._football_top_x` does and
/// what the field sprite's own perspective was drawn to. Truncating instead
/// puts the line — and the ball and possession arrow that hang off its top
/// endpoint — one pixel short of the lean on 49 of the 100 yard positions,
/// which is how the parity harness found this.
pub const fn football_top_x(x: i32) -> i32 {
    let run = (FOOTBALL_VP_X - x) * FOOTBALL_PERSP_NUM;
    let half = FOOTBALL_PERSP_DEN / 2;
    if run >= 0 {
        x + (run + half) / FOOTBALL_PERSP_DEN
    } else {
        x - (-run + half) / FOOTBALL_PERSP_DEN
    }
}

// =============================================================================
// Pregame
// =============================================================================

/// The pregame screen, one design for every sport ("Big time", locked
/// 2026-07-15): kickoff time always visible top-right, one cycling info line,
/// per-team lines in team colors below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PregameGeometry {
    pub logo_away: Slice,
    pub logo_home: Slice,
    pub record_away_wins: Slice,
    pub record_away_losses: Slice,
    pub record_home_wins: Slice,
    pub record_home_losses: Slice,
    /// Hockey's W-L-OTL stack (NHL.md §3.9), wins down to overtime losses:
    /// three `spleen_5x8` rows filling the crest's 24 px exactly, where the
    /// two-row stack above is centered on it.
    pub hockey_record_away: [Slice; 3],
    pub hockey_record_home: [Slice; 3],
    pub divider_x: i32,
    /// First-pitch time, alternating with the date. 80 px is exactly ten
    /// `unscii_16` glyphs — the width of "WED JUL 16".
    ///
    /// Every right-column slot is 80 px wide and starts after a 1 px gutter
    /// beside the divider, so the column ends at 126 rather than on the
    /// panel's edge ring. Keeping a 2 px gutter would mean a 79 px column,
    /// which scrolls every 16-character `spleen_5x8` line — "crypto.com
    /// Arena" is one — by a single pixel.
    pub info_time: Slice,
    /// The cycling venue ↔ weather line.
    pub info_cycle: Slice,
    pub separator_y: i32,
    /// The per-team line under the divider (`PITCHER_AWAY` in the Python
    /// table): probable starter, rank line, or abbreviation, per sport.
    pub team_line_away: Slice,
    pub team_line_home: Slice,
}

pub const PREGAME: PregameGeometry = PregameGeometry {
    logo_away: rect(1, 4, 24, 24),
    logo_home: rect(1, 36, 24, 24),
    record_away_wins: rect(26, 8, 19, 8),
    record_away_losses: rect(26, 17, 19, 8),
    record_home_wins: rect(26, 40, 19, 8),
    record_home_losses: rect(26, 49, 19, 8),
    hockey_record_away: [rect(26, 4, 19, 8), rect(26, 12, 19, 8), rect(26, 20, 19, 8)],
    hockey_record_home: [
        rect(26, 36, 19, 8),
        rect(26, 44, 19, 8),
        rect(26, 52, 19, 8),
    ],
    divider_x: 45,
    info_time: rect(47, 2, 80, 16),
    info_cycle: rect(47, 24, 80, 8),
    separator_y: 41,
    team_line_away: rect(47, 45, 80, 8),
    team_line_home: rect(47, 54, 80, 8),
};

// =============================================================================
// Final (line score — MLB, NBA, football)
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FinalGeometry {
    pub logo_away: Slice,
    pub logo_home: Slice,
    pub final_label: Slice,
    pub linescore_header: Slice,
    pub linescore_away: Slice,
    pub linescore_home: Slice,
    /// Rule separating the line score from the pinned R column.
    pub divider_x: i32,
    /// The pinned totals column, which carries the headline score in
    /// `unscii_16`. Sized for two digits; a basketball total overhangs it
    /// evenly, so the table leaves the room a three-digit total needs between
    /// the rule and column 126 — that is what places the rule. Three
    /// `unscii_16` digits ink 23 px.
    pub total_header: Slice,
    pub total_away: Slice,
    pub total_home: Slice,
}

/// "Line-score forward": the line score is the hero, rows aligned to the
/// stacked logos, totals in `unscii_16` pinned right. Chosen at the 2026-07-07
/// gallery review over a marquee and a stacked-ledger design, and the only
/// final since 2026-11-08.
///
/// Horizontally this is packed to the pixel: crest 1..=24, a 2 px gutter, the
/// 75 px line-score window (five periods, trailing space included) at
/// 27..=101, the rule at 102, a 1 px gutter, and a three-digit total inked at
/// 104..=126. The window may meet the rule because a row that scrolls keeps
/// its trailing space (only a row that fits is drawn without it), so its ink
/// passes column 95 only while the scroll is in progress.
pub const FINAL: FinalGeometry = FinalGeometry {
    logo_away: rect(1, 2, 24, 24),
    logo_home: rect(1, 36, 24, 24),
    final_label: rect(27, 30, 75, 8),
    linescore_header: rect(27, 2, 75, 8),
    linescore_away: rect(27, 14, 75, 8),
    linescore_home: rect(27, 48, 75, 8),
    divider_x: 102,
    total_header: rect(105, 2, 22, 8),
    total_away: rect(105, 10, 22, 16),
    total_home: rect(105, 44, 22, 16),
};

// =============================================================================
// Soccer live
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SoccerLiveGeometry {
    pub logo_away: Slice,
    pub logo_home: Slice,
    pub score_away: Slice,
    pub score_home: Slice,
    /// Short period chip ("1ST") in the identity column.
    pub phase: Slice,
    pub divider_x: i32,
    pub separator_y: i32,
    pub clock: Slice,
    pub event_top: Slice,
    pub event_name: Slice,
    /// Where the "no events yet" line goes — one line, vertically centered
    /// between the two event rows.
    pub event_empty: Slice,
    /// The bottom strip the play flash takes over: the area
    /// the two event rows give up.
    pub strip: Slice,
}

/// "Phase ledger": the exact MLB-live silhouette, with the big clock alone in
/// the data column. The only soccer live design since 2026-11-08 — a
/// clock-over-spelled-out-period design and a broadcast-corners one were
/// offered beside it and never picked (BACKLOG 32, closed by deleting them).
pub const SOCCER_LIVE: SoccerLiveGeometry = SoccerLiveGeometry {
    logo_away: rect(1, 1, 24, 24),
    score_away: rect(24, 7, 22, 16),
    phase: rect(2, 29, 42, 8),
    logo_home: rect(1, 39, 24, 24),
    score_home: rect(24, 47, 22, 16),
    divider_x: 45,
    separator_y: 36,
    clock: rect(46, 10, 81, 16),
    event_top: rect(51, 41, 76, 8),
    event_name: rect(51, 53, 76, 8),
    event_empty: rect(51, 47, 76, 8),
    strip: COLUMN_STRIP,
};

// =============================================================================
// Single-design screens
// =============================================================================

/// MLB live, "field + count ledger" — the original live screen, and the
/// silhouette every other live and pregame screen mirrors.
///
/// The count-dot rows carry their pixel widths; the renderer derives the dot
/// count as `(width + 1) / (dot_width + 1)`. The label heights are 7 px against
/// an 8 px `unscii_8`; that clip is part of the current art.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MlbLiveGeometry {
    pub logo_away: Slice,
    pub logo_home: Slice,
    pub score_away: Slice,
    pub score_home: Slice,
    pub inning: Slice,
    pub divider_x: i32,
    pub separator_y: i32,
    pub ball_label: Slice,
    pub ball_dots: Slice,
    pub strike_label: Slice,
    pub strike_dots: Slice,
    pub out_label: Slice,
    pub out_dots: Slice,
    pub pitcher_label: Slice,
    pub pitcher_name: Slice,
    pub batter_label: Slice,
    pub batter_name: Slice,
    /// The bottom strip the play flash takes over: the
    /// pitcher/batter rows.
    pub strip: Slice,
}

pub const MLB_LIVE: MlbLiveGeometry = MlbLiveGeometry {
    logo_away: rect(1, 1, 24, 24),
    score_away: rect(24, 7, 22, 11),
    inning: rect(11, 30, 32, 7),
    logo_home: rect(1, 39, 24, 24),
    score_home: rect(24, 47, 22, 11),
    divider_x: 45,
    separator_y: 36,
    ball_label: rect(51, 5, 8, 7),
    ball_dots: rect(61, 7, 19, 4),
    strike_label: rect(51, 15, 8, 7),
    strike_dots: rect(61, 17, 14, 4),
    out_label: rect(51, 25, 8, 7),
    out_dots: rect(61, 27, 14, 4),
    pitcher_label: rect(51, 41, 24, 7),
    pitcher_name: rect(77, 41, 50, 8),
    batter_label: rect(51, 54, 24, 7),
    batter_name: rect(77, 54, 50, 8),
    strip: COLUMN_STRIP,
};

/// NBA live, "quarter + clock ledger" — the soccer-live silhouette, with the
/// identity column 4 px wider because NBA scores reach three digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NbaLiveGeometry {
    pub logo_away: Slice,
    pub logo_home: Slice,
    pub score_away: Slice,
    pub score_home: Slice,
    pub phase: Slice,
    pub divider_x: i32,
    pub separator_y: i32,
    pub clock: Slice,
    /// The bottom strip the play flash takes over. NBA has no
    /// persistent content there, so between flashes it is empty.
    pub strip: Slice,
}

pub const NBA_LIVE: NbaLiveGeometry = NbaLiveGeometry {
    logo_away: rect(1, 1, 24, 24),
    score_away: rect(24, 7, 25, 16),
    phase: rect(2, 29, 46, 8),
    logo_home: rect(1, 39, 24, 24),
    score_home: rect(24, 47, 25, 16),
    divider_x: 49,
    separator_y: 36,
    clock: rect(50, 10, 77, 16),
    strip: COLUMN_STRIP,
};

/// Soccer full time, "FT + scorers" — the line-score final's silhouette with
/// the line score replaced by what soccer actually has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SoccerFinalGeometry {
    pub logo_away: Slice,
    pub logo_home: Slice,
    pub score_away: Slice,
    pub score_home: Slice,
    pub divider_x: i32,
    pub scorers_away: Slice,
    pub scorers_home: Slice,
    pub full_time_label: Slice,
}

/// The scorer windows end at column 126; they scroll on almost every real
/// match, and a scrolling line carries ink through every column of its window.
pub const SOCCER_FINAL: SoccerFinalGeometry = SoccerFinalGeometry {
    logo_away: rect(1, 2, 24, 24),
    score_away: rect(26, 6, 20, 16),
    logo_home: rect(1, 36, 24, 24),
    score_home: rect(26, 40, 20, 16),
    divider_x: 48,
    scorers_away: rect(51, 10, 76, 8),
    full_time_label: rect(51, 28, 76, 8),
    scorers_home: rect(51, 44, 76, 8),
};

/// Football live, "broadcast corners + field strip". The first game screen born
/// under the 1 px edge rule: nothing here touches row 0/63 or column 0/127.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FootballLiveGeometry {
    pub logo_away: Slice,
    pub logo_home: Slice,
    pub score_away: Slice,
    pub score_home: Slice,
    /// Timeout bars: three per team, 6×1 px, 1 px gaps.
    pub timeout_y: i32,
    pub timeout_away_x: i32,
    pub timeout_home_x: i32,
    pub phase: Slice,
    pub clock: Slice,
    /// "3RD & 7", centered; the possession arrow sits beside it.
    pub situation: Slice,
    /// The bottom strip the play flash takes over. The field strip (and the
    /// ball riding it) is skipped while the flash shows, and there
    /// is no identity column, so it is every legal column of the band under
    /// the score digits' ink (rows 41..=62), with the 16 px line centered in
    /// it.
    pub strip: Slice,
}

pub const FOOTBALL_LIVE: FootballLiveGeometry = FootballLiveGeometry {
    logo_away: rect(1, 1, 24, 24),
    logo_home: rect(103, 1, 24, 24),
    timeout_y: 26,
    timeout_away_x: 3,
    timeout_home_x: 105,
    score_away: rect(1, 28, 24, 16),
    score_home: rect(103, 28, 24, 16),
    phase: rect(26, 3, 26, 8),
    clock: rect(52, 2, 50, 16),
    situation: rect(26, 30, 77, 8),
    strip: rect(1, 44, 126, 16),
};

// =============================================================================
// Hockey (NHL.md §6.5)
// =============================================================================
//
// Ported from the decided mockups (data/nhl-design, D1/D2), whose geometry
// was drawn by this crate over real ESPN captures.

/// Hockey live, "Rink" (D1): football's broadcast corners with a top-down
/// rink where the field was. Skater dots under each crest carry the strength;
/// one line over the shots ledger carries the banner or the last goal; the
/// rink carries every goal where it was scored. The only live design since
/// 2026-11-08 — a "period ledger" on the MLB/NBA silhouette shipped beside it
/// as the `hockey_live` variant and went with the variant system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HockeyLiveGeometry {
    pub logo_away: Slice,
    pub logo_home: Slice,
    /// The skater dots' row, under the crests: the away row runs right from
    /// `skaters_away_x`, the home row left from `skaters_home_end` (its last
    /// column), so an extra attacker's sixth dot grows inward on both sides.
    pub skaters_y: i32,
    pub skaters_away_x: i32,
    pub skaters_home_end: i32,
    pub score_away: Slice,
    pub score_home: Slice,
    pub period: Slice,
    pub clock: Slice,
    /// The banner or the last goal; during the shootout, its label.
    pub situation: Slice,
    pub shots: Slice,
    pub shots_bar_y: i32,
    /// The shootout's attempt rows across the middle, each level with its
    /// side's score. The tally takes the clock's slot (§3.2).
    pub shootout_away: Slice,
    pub shootout_home: Slice,
    /// The shootout's rink, under the rows: the art squashed to nine rows by
    /// dropping six from its middle, which its rounded corners survive.
    pub shootout_rink_y: i32,
    /// The bottom strip the play flash takes over: every legal column under
    /// the shots bar. `unscii_16` inks rows 1-12 of its cell, so at y 47 the
    /// text sits four dark rows below the bar and the scores' ink (both end
    /// at row 43) and four above the panel's bottom, the dark edge row
    /// included (owner, 2026-10-12).
    pub strip: Slice,
}

pub const HOCKEY_LIVE: HockeyLiveGeometry = HockeyLiveGeometry {
    logo_away: rect(1, 1, 24, 24),
    logo_home: rect(103, 1, 24, 24),
    skaters_y: 26,
    skaters_away_x: 1,
    skaters_home_end: 126,
    score_away: rect(1, 31, 24, 16),
    score_home: rect(103, 31, 24, 16),
    period: rect(26, 3, 26, 8),
    clock: rect(52, 2, 50, 16),
    situation: rect(28, 21, 73, 8),
    shots: rect(28, 33, 73, 8),
    shots_bar_y: 42,
    shootout_away: rect(34, 31, SHOOTOUT_ROW_WIDTH, SHOOTOUT_MARK),
    shootout_home: rect(34, 41, SHOOTOUT_ROW_WIDTH, SHOOTOUT_MARK),
    shootout_rink_y: 53,
    strip: rect(1, 47, 126, 16),
};

/// A shootout attempt marker's edge, and the row pitch: 7 px marks, 3 px gaps.
pub const SHOOTOUT_MARK: i32 = 7;
pub const SHOOTOUT_PITCH: i32 = 10;
/// A full row: [`scoreboard_model::snapshot::SHOOTOUT_SLOTS`] marks.
pub const SHOOTOUT_ROW_WIDTH: i32 =
    (scoreboard_model::snapshot::SHOOTOUT_SLOTS as i32 - 1) * SHOOTOUT_PITCH + SHOOTOUT_MARK;

/// Hockey's final, "Three stars" (D2): soccer's full-time silhouette, with the
/// stars where soccer lists its scorers, and a rule over the series and shots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HockeyFinalGeometry {
    pub logo_away: Slice,
    pub logo_home: Slice,
    pub score_away: Slice,
    pub score_home: Slice,
    pub divider_x: i32,
    pub final_label: Slice,
    /// First, second and third star: the rank's star icons from `star_x` at
    /// `star_pitch`, a row down from the row's top, then the name.
    pub star_rows: [Slice; 3],
    pub star_x: i32,
    pub star_pitch: i32,
    /// While ESPN has not named the stars, their band carries the line score
    /// instead: header, away, home.
    pub linescore_rows: [Slice; 3],
    /// The rule under the band, from the divider to the last legal column.
    pub rule_y: i32,
    /// The series over the shots when both are known; whichever one is,
    /// alone, in `single_line`.
    pub series: Slice,
    pub shots: Slice,
    pub single_line: Slice,
}

pub const HOCKEY_FINAL: HockeyFinalGeometry = HockeyFinalGeometry {
    logo_away: rect(1, 2, 24, 24),
    score_away: rect(26, 6, 20, 16),
    logo_home: rect(1, 38, 24, 24),
    score_home: rect(26, 42, 20, 16),
    divider_x: 48,
    final_label: rect(50, 2, 77, 8),
    star_rows: [
        rect(71, 14, 56, 8),
        rect(71, 24, 56, 8),
        rect(71, 34, 56, 8),
    ],
    star_x: 52,
    star_pitch: 6,
    linescore_rows: [
        rect(50, 14, 77, 8),
        rect(50, 24, 77, 8),
        rect(50, 34, 77, 8),
    ],
    rule_y: 43,
    series: rect(50, 46, 77, 8),
    shots: rect(50, 54, 77, 8),
    single_line: rect(50, 51, 77, 8),
};

// =============================================================================
// Settings
// =============================================================================

/// What the renderer reads out of the configuration: the game-description
/// scroll speed.
///
/// In MicroPython this was a module global that `set_scroll_speed` mutated,
/// read back by renderers on every frame. Nothing in this crate mutates a
/// `static`, so it is a value core 0 owns and hands to the render loop — one
/// field, and a type rather than a bare integer because it crosses that seam
/// as a message and is borrowed by every [`crate::game::Scene`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderSettings {
    /// The game-description scroll speed, already degraded to the legal set.
    pub scroll_px_per_second: i32,
}

impl RenderSettings {
    pub const fn new() -> Self {
        RenderSettings {
            scroll_px_per_second: DEFAULT_SCROLL_SPEED,
        }
    }

    /// Apply `config.display.scroll_speed_px_per_sec`, degrading an illegal
    /// value to [`DEFAULT_SCROLL_SPEED`].
    pub const fn set_scroll_speed(&mut self, requested: i32) -> i32 {
        self.scroll_px_per_second = scroll_speed(requested);
        self.scroll_px_per_second
    }

    /// Scroll feel for the game-description lines.
    pub const fn game_scroll(&self, pause_ms: u64) -> Scroll {
        Scroll {
            pause_ms,
            pixels_per_second: self.scroll_px_per_second,
        }
    }
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self::new()
    }
}

/// The bottom strip the play flash draws into under `mode`: the live screen's
/// own `strip`. The other screens draw no flash; they get [`COLUMN_STRIP`] so
/// the prepared view always has a width to size by.
///
/// One selector so the renderer and the prepared view's flash window cannot
/// disagree about how wide the strip is.
pub const fn bottom_strip(mode: Mode) -> Slice {
    match mode {
        Mode::MlbLive => MLB_LIVE.strip,
        Mode::NbaLive => NBA_LIVE.strip,
        Mode::FootballLive => FOOTBALL_LIVE.strip,
        Mode::SoccerLive => SOCCER_LIVE.strip,
        Mode::HockeyLive => HOCKEY_LIVE.strip,
        _ => COLUMN_STRIP,
    }
}

const fn rect(x: i32, y: i32, width: i32, height: i32) -> Slice {
    Slice {
        x,
        y,
        width,
        height,
    }
}
