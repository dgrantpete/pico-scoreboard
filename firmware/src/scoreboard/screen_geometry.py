"""
Hand-written screen geometry for the text-only PREGAME and FINAL screens.

Convention split: `mlb_layout.aseprite` (round-tripped through
`tools/build.py` into `scoreboard/layout/`) remains the source of truth for
every *sprite-bearing* screen (the live game view). The pregame and final
screens carry no sprites -- only text, logos, and dividers -- so an aseprite
round-trip would buy nothing. Their rectangles live here as plain code, the
same way the startup / idle / error regions are code-defined in
`display.Regions.__init__`.

Variant selection is scoped PER SPORT x SCREEN (config `display.variants`
keys like `mlb_pregame`, `soccer_live`): each key picks a letter from its
table set independently, so any sport's screen can diverge without touching
the others. Table sets start as shared references (the sports currently
ship identical designs); forking a sport's look = write a new table dict
and point that key's entry at it. Keys with a single design (soccer_final,
nba_live) live in the same registry for uniform access but are not exposed
in config until a second design exists. The desktop preview overrides the
`_ACTIVE` selection dict per variant column (see tools/preview/variants.py).

Geometry tables map a slot NAME to either:
  * a 4-tuple ``(X, Y, W, H)`` -- built into a `Region` by `display.Regions`
    and drawn into by the renderer, or
  * an ``int`` scalar (e.g. ``DIVIDER_X``, ``SEPARATOR_Y``) -- read directly
    by the renderer for `vline`/`hline` dividers, never a Region.

All numbers are in display pixels. Font metrics used to size these tables
(every font is fixed-width): spleen_5x8 = 5px/char x 8px tall,
unscii_8 = 8px/char x 8px tall, unscii_16 = 8px/char x 16px tall. Logos are
24x24; the panel is 128x64.
"""


# Draw the DIM_GRAY vline/hline dividers on every game-facing screen (live,
# pregame, final). A style-wide switch: the screens must read consistently, so
# this is all-or-nothing rather than per-variant.
SHOW_DIVIDERS = True

# Live-screen divider geometry, aligned with pregame variant C's column split
# (identity column | data column, bottom strip below a rule) so rotation
# between screens keeps one visual frame. The hline sits centered in the
# 8-row gap between the field/count block (last content row 32) and the
# PIT/BAT strip (top row 41) — at 41 it sat directly on the text's top row.
LIVE_DIVIDER_X = 45
LIVE_SEPARATOR_Y = 36

# --- Tuning constants (preview-tunable; user picks final values) -------------

# Minimum dwell for one cycling pregame info phase. A phase whose text scrolls
# stays up for at least one full scroll cycle (computed at commit time), but
# never less than this so short lines don't flash by.
PREGAME_INFO_DWELL_MS = 4000

# Scroll speeds MUST evenly divide the display's 20 FPS refresh (see
# run_display_thread's 50 ms tick): the scroll offset is derived from wall
# time, so a speed of S px/s advances S/20 px per rendered frame. A
# non-integer px/frame (e.g. 30 -> 1.5) is realized by floor math as
# alternating 1 px and 2 px steps -- every third pixel column of the scroll
# is simply never displayed, which reads as a rhythmic stutter on the panel.
# Legal smooth values: 20 (1 px/frame), 10 (1 px every 2nd frame),
# 5, 4, 2, 1; 40 (2 px/frame) is uniform but coarse.

# Scroll feel for pregame info lines (venue / weather). Kept here (not in
# display.py) so state.set_pregame can compute per-phase dwell without
# importing display -- the renderer passes these same values to writer.draw,
# so the pre-computed dwell and the live scroll stay in lockstep.
PREGAME_SCROLL_PAUSE_MS = 1000
PREGAME_SCROLL_PX_PER_SEC = 20

# Final line-score horizontal scroll. Slow, long dwell: the score is the point,
# the scroll is a reveal of later innings. (12 showed every pixel but with
# uneven 1-vs-2-frame dwell; 10 is perfectly uniform at 2 frames per pixel.)
FINAL_LS_PAUSE_MS = 1800
FINAL_LS_PX_PER_SEC = 10

# One user-configurable speed (config `display.scroll_speed_px_per_sec`) for
# the game-description scrollers: the MLB play-by-play flash and the soccer
# live-event / full-time scorer lines. Pregame (PREGAME_SCROLL_*) and the
# final line score (FINAL_LS_*) keep their own feel. Restricted to the legal
# smooth set above.
GAME_SCROLL_PX_PER_SEC = 20
_SCROLL_SPEEDS = (5, 10, 20, 40)


def set_scroll_speed(px_per_sec) -> int:
    """Apply config display.scroll_speed_px_per_sec; anything outside the
    smooth set falls back to the default 20. Renderers (and Core 0's
    play-window math) re-read the global on every use, so this applies live.
    A play flash already on screen keeps its old display window while
    scrolling at the new rate — self-corrects on the next play."""
    global GAME_SCROLL_PX_PER_SEC
    GAME_SCROLL_PX_PER_SEC = px_per_sec if px_per_sec in _SCROLL_SPEEDS else 20
    return GAME_SCROLL_PX_PER_SEC


def set_show_dividers(enabled) -> bool:
    """Apply config display.show_dividers. Renderers re-read the flag every
    frame, so this applies live with no Regions rebuild."""
    global SHOW_DIVIDERS
    SHOW_DIVIDERS = bool(enabled)
    return SHOW_DIVIDERS


# =============================================================================
# PREGAME variant tables (away above home, matching the live game screen)
# =============================================================================

_PREGAME = {
    # A "Cycling ledger": logos + stacked W/L on the left, one cycling
    # info line (venue / first pitch / weather) over static pitchers on the
    # right, split by a full-height vline.
    "A": {
        "LOGO_AWAY": (0, 4, 24, 24),
        "LOGO_HOME": (0, 36, 24, 24),
        "REC_AWAY_WINS": (26, 8, 19, 8),
        "REC_AWAY_LOSSES": (26, 17, 19, 8),
        "REC_HOME_WINS": (26, 40, 19, 8),
        "REC_HOME_LOSSES": (26, 49, 19, 8),
        "DIVIDER_X": 45,               # vline, full height
        "INFO_LABEL": (48, 4, 80, 8),  # spleen, dim
        "INFO_VALUE": (48, 15, 80, 16),  # unscii_16 (time) or spleen (scroll)
        "SEPARATOR_Y": 41,             # hline under the info block, x>=48
        "PITCHER_AWAY": (48, 45, 80, 8),
        "PITCHER_HOME": (48, 54, 80, 8),
    },
    # B "All at once": horizontal records beside the logos; venue / time /
    # weather all visible stacked on the right; one pitcher line alternating
    # away<->home every few seconds.
    "B": {
        "LOGO_AWAY": (0, 4, 24, 24),
        "LOGO_HOME": (0, 36, 24, 24),
        "REC_AWAY": (26, 12, 30, 8),
        "REC_HOME": (26, 44, 30, 8),
        "DIVIDER_X": 57,               # vline, full height
        "INFO_VENUE": (59, 3, 69, 8),
        "INFO_TIME": (59, 18, 69, 8),
        "INFO_WEATHER": (59, 33, 69, 8),
        "PITCHER_LINE": (59, 50, 69, 8),  # alternates away/home
    },
    # C "Big time": first-pitch time always visible in unscii_16 top-right;
    # one cycling line venue<->weather; pitchers as A.
    "C": {
        "LOGO_AWAY": (0, 4, 24, 24),
        "LOGO_HOME": (0, 36, 24, 24),
        "REC_AWAY_WINS": (26, 8, 19, 8),
        "REC_AWAY_LOSSES": (26, 17, 19, 8),
        "REC_HOME_WINS": (26, 40, 19, 8),
        "REC_HOME_LOSSES": (26, 49, 19, 8),
        "DIVIDER_X": 45,
        "INFO_TIME": (48, 2, 80, 16),   # unscii_16 centered, always shown
        "INFO_CYCLE": (48, 24, 80, 8),  # spleen, venue<->weather
        "SEPARATOR_Y": 41,
        "PITCHER_AWAY": (48, 45, 80, 8),
        "PITCHER_HOME": (48, 54, 80, 8),
    },
}


# =============================================================================
# FINAL variant tables
# =============================================================================

_FINAL = {
    # A "Marquee + boxscore": logos top corners, scores inboard, FINAL label
    # centered; full-width bottom band with three lockstep-scrolling line-score
    # rows (header / away / home) and a pinned R total column.
    "A": {
        "LOGO_AWAY": (0, 2, 24, 24),
        "LOGO_HOME": (104, 2, 24, 24),
        "SCORE_AWAY": (26, 4, 34, 16),
        "SCORE_HOME": (68, 4, 34, 16),
        "FINAL_LABEL": (44, 20, 40, 8),
        "LS_HEADER": (2, 32, 108, 8),
        "LS_AWAY": (2, 42, 108, 8),
        "LS_HOME": (2, 52, 108, 8),
        "DIVIDER_X": 112,          # vline separating line score from R column
        "SEPARATOR_Y": 30,         # hline under the top band, full width
        "R_HEADER": (115, 32, 13, 8),
        "R_AWAY": (115, 42, 13, 8),
        "R_HOME": (115, 52, 13, 8),
    },
    # B "Stacked ledger": live-game silhouette -- logos stacked left, big
    # scores beside -- with the line score as three aligned rows on the right
    # (narrow window, more scrolling) and a pinned R column.
    "B": {
        "LOGO_AWAY": (0, 0, 24, 24),
        "LOGO_HOME": (0, 40, 24, 24),
        "SCORE_AWAY": (26, 4, 30, 16),
        "SCORE_HOME": (26, 44, 30, 16),
        "FINAL_LABEL": (2, 26, 54, 8),  # wide enough for unscii_8 "FINAL" (40px)
        "LS_HEADER": (58, 0, 54, 8),
        "LS_AWAY": (58, 10, 54, 8),
        "LS_HOME": (58, 50, 54, 8),
        "DIVIDER_X": 112,
        "R_HEADER": (115, 0, 13, 8),
        "R_AWAY": (115, 10, 13, 8),
        "R_HOME": (115, 50, 13, 8),
    },
    # C "Line-score forward": line score is the hero -- rows aligned to the
    # stacked logos, totals in unscii_16 pinned right, FINAL label between the
    # away and home rows.
    "C": {
        "LOGO_AWAY": (0, 2, 24, 24),
        "LOGO_HOME": (0, 36, 24, 24),
        "LS_HEADER": (28, 2, 75, 8),
        "LS_AWAY": (28, 14, 75, 8),
        "LS_HOME": (28, 48, 75, 8),
        "FINAL_LABEL": (28, 30, 75, 8),
        "DIVIDER_X": 105,
        "R_HEADER": (108, 2, 20, 8),
        "R_AWAY": (108, 10, 20, 16),   # unscii_16 totals
        "R_HOME": (108, 44, 20, 16),
    },
}


# =============================================================================
# SOCCER LIVE variant tables
# =============================================================================
# Same visual frame as the MLB live screen (identity column left, data column
# right of the x=45 divider, bottom strip below the separator) so slate
# rotation between sports keeps one silhouette. What changes is the data
# column's content: soccer is a clock sport, so the running match clock takes
# the space the field/count block owns for baseball, the period ("1ST"/"2ND")
# takes the inning ordinal's slot, and the bottom strip carries the last goal
# or red card instead of pitcher/batter.

# Pause feel for the last-event scorer line and full-time scorer lists; the
# speed is the shared GAME_SCROLL_PX_PER_SEC (user-configurable, see above).
SOCCER_SCROLL_PAUSE_MS = 1500

_SOCCER_LIVE = {
    # A "Phase ledger": exact MLB-live silhouette — logos + scores stacked
    # left with the period short-form where the inning ordinal sat; the big
    # clock centered alone in the data column; last event in the bottom strip.
    "A": {
        "LOGO_AWAY": (0, 0, 24, 24),
        "SCORE_AWAY": (24, 7, 22, 16),
        "PHASE": (2, 29, 42, 8),
        "LOGO_HOME": (0, 40, 24, 24),
        "SCORE_HOME": (24, 47, 22, 16),
        "DIVIDER_X": 45,
        "SEPARATOR_Y": 36,
        "CLOCK": (46, 10, 82, 16),
        "EVENT_TOP": (51, 41, 76, 8),
        "EVENT_NAME": (51, 53, 76, 8),
        "EVENT_EMPTY": (51, 47, 76, 8),
    },
    # B "Clock + phase stacked": identity column without the phase; the data
    # column carries the clock over the spelled-out period.
    "B": {
        "LOGO_AWAY": (0, 0, 24, 24),
        "SCORE_AWAY": (24, 7, 22, 16),
        "LOGO_HOME": (0, 40, 24, 24),
        "SCORE_HOME": (24, 47, 22, 16),
        "DIVIDER_X": 45,
        "SEPARATOR_Y": 36,
        "CLOCK": (46, 5, 82, 16),
        "PHASE_LONG": (46, 25, 82, 8),
        "EVENT_TOP": (51, 41, 76, 8),
        "EVENT_NAME": (51, 53, 76, 8),
        "EVENT_EMPTY": (51, 47, 76, 8),
    },
    # C "Broadcast corners": logos in the top corners with scores inboard
    # (the final-A silhouette), period chip between the scores, full-width
    # clock beneath, event strip along the bottom. No vline — this variant
    # deliberately tests breaking the column frame.
    "C": {
        "LOGO_AWAY": (0, 0, 24, 24),
        "SCORE_AWAY": (26, 4, 22, 16),
        "PHASE": (48, 8, 32, 8),
        "SCORE_HOME": (80, 4, 22, 16),
        "LOGO_HOME": (104, 0, 24, 24),
        "CLOCK": (0, 26, 128, 16),
        "SEPARATOR_Y": 44,
        "EVENT_TOP": (2, 47, 124, 8),
        "EVENT_NAME": (2, 56, 124, 8),
        "EVENT_EMPTY": (2, 51, 124, 8),
    },
}


# =============================================================================
# NBA LIVE table (single design: "quarter + clock ledger")
# =============================================================================
# The soccer-A silhouette adapted for basketball: identity column left
# (logos + scores stacked, the period chip where soccer's half sat), the
# clock string centered in the data column, and the shared play-flash strip
# along the bottom (regions.play_text — NBA has no persistent event ticker;
# the last play flashes once on change, like MLB). NBA scores reach three
# digits, so the identity column is 4px wider than soccer's: divider at 49,
# 25px score slots (3 unscii_16 digits = 24px).

_NBA_LIVE = {
    "LOGO_AWAY": (0, 0, 24, 24),
    "SCORE_AWAY": (24, 7, 25, 16),
    "PHASE": (2, 29, 46, 8),
    "LOGO_HOME": (0, 40, 24, 24),
    "SCORE_HOME": (24, 47, 25, 16),
    "DIVIDER_X": 49,
    "SEPARATOR_Y": 36,
    "CLOCK": (50, 10, 78, 16),
}


# =============================================================================
# SOCCER FINAL table (single design: "FT + scorers")
# =============================================================================
# The final-C silhouette with the line score replaced by what soccer actually
# has: goal scorers. Identity column (logos + unscii_16 scores) left of the
# divider, scorer lists aligned to their team's row on the right, FULL TIME
# centered between them. Winner emphasis by color; a draw colors both.

_SOCCER_FINAL = {
    "LOGO_AWAY": (0, 2, 24, 24),
    "SCORE_AWAY": (26, 6, 20, 16),
    "LOGO_HOME": (0, 36, 24, 24),
    "SCORE_HOME": (26, 40, 20, 16),
    "DIVIDER_X": 48,
    "SCORERS_AWAY": (52, 10, 76, 8),
    "FT_LABEL": (52, 28, 76, 8),
    "SCORERS_HOME": (52, 44, 76, 8),
}


# Variant registry: config key -> {letter: slot table}. Shared references
# on purpose (see module docstring); _CONFIGURABLE lists the keys the
# settings UI exposes (>1 design exists). Defaults per the 2026-07-07
# gallery review: pregame "Big time" (C), final "Line-score forward" (C).
_TABLES = {
    "mlb_pregame": _PREGAME,
    "nba_pregame": _PREGAME,
    "soccer_pregame": _PREGAME,
    "mlb_final": _FINAL,
    "nba_final": _FINAL,
    "soccer_live": _SOCCER_LIVE,
    "soccer_final": {"A": _SOCCER_FINAL},
    "nba_live": {"A": _NBA_LIVE},
}

_ACTIVE = {
    "mlb_pregame": "C",
    "nba_pregame": "C",
    "soccer_pregame": "C",
    "mlb_final": "C",
    "nba_final": "C",
    "soccer_live": "A",
    "soccer_final": "A",
    "nba_live": "A",
}


def variant_keys() -> tuple:
    """Every registered sport x screen key (regions are built for all)."""
    return tuple(_TABLES.keys())


def set_variants(variants: dict) -> dict:
    """Apply configured layout variants (config `display.variants`).

    Unknown keys and unknown letters are ignored (the current selection
    stays), so a hand-edited or pre-rename config can't select a
    nonexistent table. Returns the active selection dict for logging.
    Callers that flip variants at runtime must rebuild the display Regions
    afterwards (state.update_screen_variants owns that sequencing).
    """
    for key, letter in variants.items():
        tables = _TABLES.get(key)
        if tables is not None and letter in tables:
            _ACTIVE[key] = letter
    return dict(_ACTIVE)


def active_variant(key: str) -> str:
    """The active design letter for one sport x screen key."""
    return _ACTIVE[key]


def geometry_for(key: str) -> dict:
    """The active slot table for one sport x screen key."""
    return _TABLES[key][_ACTIVE[key]]


def pregame_value_width(key: str) -> int:
    """Width (px) of the cycling info value region for `key`'s active table.

    state.set_pregame sizes each info phase's scroll dwell against this, so the
    pre-computed per-phase dwell matches the region the renderer actually
    scrolls the text in. B has no cycling value; its venue row width is a
    reasonable stand-in (its info lines are pre-built the same way).
    """
    g = geometry_for(key)
    if "INFO_VALUE" in g:
        return g["INFO_VALUE"][2]
    if "INFO_CYCLE" in g:
        return g["INFO_CYCLE"][2]
    return g["INFO_VENUE"][2]
