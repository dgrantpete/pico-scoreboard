"""
Hand-written screen geometry for the text-only PREGAME and FINAL screens.

Convention split: `mlb_layout.aseprite` (round-tripped through
`tools/build.py` into `scoreboard/layout/`) remains the source of truth for
every *sprite-bearing* screen (the live game view). The pregame and final
screens carry no sprites -- only text, logos, and dividers -- so an aseprite
round-trip would buy nothing. Their rectangles live here as plain code, the
same way the startup / idle / error regions are code-defined in
`display.Regions.__init__`.

Each screen offers three design variants (A/B/C). `PREGAME_VARIANT` and
`FINAL_VARIANT` select the active table independently -- the user may mix,
e.g. pregame "B" with final "A". The desktop preview flips these module
constants per variant column (see tools/preview/variants.py); the firmware
ships whichever letter the user picks.

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

# Active variant selectors (independent). Overridden per-column by the preview.
PREGAME_VARIANT = "A"
FINAL_VARIANT = "A"

# --- Tuning constants (preview-tunable; user picks final values) -------------

# Minimum dwell for one cycling pregame info phase. A phase whose text scrolls
# stays up for at least one full scroll cycle (computed at commit time), but
# never less than this so short lines don't flash by.
PREGAME_INFO_DWELL_MS = 4000

# Scroll feel for pregame info lines (venue / weather). Kept here (not in
# display.py) so state.set_pregame can compute per-phase dwell without
# importing display -- the renderer passes these same values to writer.draw,
# so the pre-computed dwell and the live scroll stay in lockstep.
PREGAME_SCROLL_PAUSE_MS = 1000
PREGAME_SCROLL_PX_PER_SEC = 30

# Final line-score horizontal scroll. Slow, long dwell: the score is the point,
# the scroll is a reveal of later innings.
FINAL_LS_PAUSE_MS = 1800
FINAL_LS_PX_PER_SEC = 12


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


def pregame_geometry() -> dict:
    """The active PREGAME variant's slot table."""
    return _PREGAME[PREGAME_VARIANT]


def final_geometry() -> dict:
    """The active FINAL variant's slot table."""
    return _FINAL[FINAL_VARIANT]


def pregame_value_width() -> int:
    """Width (px) of the cycling info value region for the active variant.

    state.set_pregame sizes each info phase's scroll dwell against this, so the
    pre-computed per-phase dwell matches the region the renderer actually
    scrolls the text in. B has no cycling value; its venue row width is a
    reasonable stand-in (its info lines are pre-built the same way).
    """
    g = _PREGAME[PREGAME_VARIANT]
    if "INFO_VALUE" in g:
        return g["INFO_VALUE"][2]
    if "INFO_CYCLE" in g:
        return g["INFO_CYCLE"][2]
    return g["INFO_VENUE"][2]
