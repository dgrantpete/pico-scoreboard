#!/usr/bin/env python3
"""Cross-implementation golden test for the binary wire format (v2).

Encodes golden fixtures with a tiny pure-Python encoder that follows the
NORMATIVE spec (the doc comment in `backend/src/wire.rs`), then decodes them
with the ACTUAL firmware parser (`firmware/src/scoreboard/mlb.py`, which is
plain Python and runs under CPython) and asserts every field round-trips.

Because the encoder here and the firmware parser are independent
implementations of the same spec, a passing run proves they agree
byte-for-byte. The same fixture bytes are additionally pinned against the Rust
encoder's goldens in `backend/src/wire.rs`: each golden is printed as hex on
success so the two can be diffed by eye. To swap in Rust-derived bytes as the
source of truth later, replace a single `GOLDEN_* = encode_*(...)` line with
`GOLDEN_* = bytes.fromhex("...")`; the field assertions reference the spec
constants independently, so they still verify the parser.

Run:  python tools/wire_format_check.py
"""

import importlib
import struct
import sys
import types
from pathlib import Path

FIRMWARE_SRC = Path(__file__).resolve().parent.parent / "firmware" / "src"

# Import scoreboard.mlb WITHOUT executing scoreboard/__init__.py (which pulls
# in MicroPython-only modules like hub75): pre-seed sys.modules with a
# synthetic package whose __path__ points at the real directory, so the
# import machinery resolves submodules but never runs the package init.
_pkg = types.ModuleType("scoreboard")
_pkg.__path__ = [str(FIRMWARE_SRC / "scoreboard")]
sys.modules["scoreboard"] = _pkg
mlb = importlib.import_module("scoreboard.mlb")
nba = importlib.import_module("scoreboard.nba")
soccer = importlib.import_module("scoreboard.soccer")
inning_half = importlib.import_module("scoreboard.inning_half")


# --- Pure-Python encoder (normative spec, little-endian) --------------------

def _str(s: str) -> bytes:
    b = s.encode("utf-8")
    if len(b) > 255:
        raise ValueError(f"string too long to length-prefix: {len(b)} bytes")
    return bytes([len(b)]) + b


def encode_list(entries: list[tuple[int, str]]) -> bytes:
    out = bytearray([mlb.WIRE_VERSION, len(entries)])
    for state, game_id in entries:
        out.append(state)
        out += _str(game_id)
    return bytes(out)


def encode_live(
    *, flags, inning_number, half, balls, strikes, outs, bases,
    away_score, home_score, away_pri, away_alt, home_pri, home_alt,
    game_id, away_abbr, home_abbr, pitcher, batter, last_play_id, last_play_text,
) -> bytes:
    out = bytearray([mlb.WIRE_VERSION, mlb.GAME_STATE_IN])
    out += struct.pack(
        "<BBBBBBBHHIIII",
        flags, inning_number, half, balls, strikes, outs, bases,
        away_score, home_score, away_pri, away_alt, home_pri, home_alt,
    )
    out += _str(game_id) + _str(away_abbr) + _str(home_abbr)
    if flags & 0x01:
        out += _str(pitcher) + _str(batter)
    out += _str(last_play_id) + _str(last_play_text)
    return bytes(out)


def encode_pregame(
    *, flags, temperature, away_wins, away_losses, home_wins, home_losses,
    start_time, away_pri, away_alt, home_pri, home_alt,
    game_id, away_abbr, home_abbr, venue,
    condition, away_probable, home_probable,
) -> bytes:
    out = bytearray([mlb.WIRE_VERSION, mlb.GAME_STATE_PRE])
    out += struct.pack(
        "<BBHHHHIIIII",
        flags, temperature, away_wins, away_losses, home_wins, home_losses,
        start_time, away_pri, away_alt, home_pri, home_alt,
    )
    out += _str(game_id) + _str(away_abbr) + _str(home_abbr) + _str(venue)
    if flags & 0x01:
        out += _str(condition)
    if flags & 0x08:
        out += _str(away_probable)
    if flags & 0x10:
        out += _str(home_probable)
    return bytes(out)


def encode_final(
    *, innings_played, away_line, home_line, away_score, home_score,
    away_pri, away_alt, home_pri, home_alt,
    game_id, away_abbr, home_abbr,
) -> bytes:
    out = bytearray([mlb.WIRE_VERSION, mlb.GAME_STATE_POST])
    out += struct.pack(
        "<BBBHHIIII",
        innings_played, len(away_line), len(home_line), away_score, home_score,
        away_pri, away_alt, home_pri, home_alt,
    )
    out += bytes(away_line) + bytes(home_line)
    out += _str(game_id) + _str(away_abbr) + _str(home_abbr)
    return bytes(out)


def encode_soccer_live(
    *, flags, half, clock_seconds, away_score, home_score,
    away_pri, away_alt, home_pri, home_alt,
    game_id, away_abbr, home_abbr, event_clock, event_name,
    comment_id="", comment_text="",
) -> bytes:
    out = bytearray([mlb.WIRE_VERSION, mlb.GAME_STATE_IN])
    out += struct.pack(
        "<BBHHHIIII",
        flags, half, clock_seconds, away_score, home_score,
        away_pri, away_alt, home_pri, home_alt,
    )
    out += _str(game_id) + _str(away_abbr) + _str(home_abbr)
    if flags & 0x02:
        out += _str(event_clock) + _str(event_name)
    if flags & 0x20:
        out += _str(comment_id) + _str(comment_text)
    return bytes(out)


def encode_soccer_pregame(
    *, start_time, away_pri, away_alt, home_pri, home_alt,
    game_id, away_abbr, home_abbr,
) -> bytes:
    out = bytearray([mlb.WIRE_VERSION, mlb.GAME_STATE_PRE])
    out += struct.pack("<IIIII", start_time, away_pri, away_alt, home_pri, home_alt)
    out += _str(game_id) + _str(away_abbr) + _str(home_abbr)
    return bytes(out)


def encode_nba_live(
    *, flags, period, phase, away_score, home_score,
    away_pri, away_alt, home_pri, home_alt,
    game_id, away_abbr, home_abbr, clock, last_play_id, last_play_text,
) -> bytes:
    out = bytearray([mlb.WIRE_VERSION, mlb.GAME_STATE_IN])
    out += struct.pack(
        "<BBBHHIIII",
        flags, period, phase, away_score, home_score,
        away_pri, away_alt, home_pri, home_alt,
    )
    out += _str(game_id) + _str(away_abbr) + _str(home_abbr) + _str(clock)
    if flags & 0x01:
        out += _str(last_play_id) + _str(last_play_text)
    return bytes(out)


def encode_nba_pregame(
    *, flags, away_wins, away_losses, home_wins, home_losses,
    start_time, away_pri, away_alt, home_pri, home_alt,
    game_id, away_abbr, home_abbr, venue,
) -> bytes:
    out = bytearray([mlb.WIRE_VERSION, mlb.GAME_STATE_PRE])
    out += struct.pack(
        "<BHHHHIIIII",
        flags, away_wins, away_losses, home_wins, home_losses,
        start_time, away_pri, away_alt, home_pri, home_alt,
    )
    out += _str(game_id) + _str(away_abbr) + _str(home_abbr) + _str(venue)
    return bytes(out)


def encode_nba_final(
    *, periods_played, away_line, home_line, away_score, home_score,
    away_pri, away_alt, home_pri, home_alt,
    game_id, away_abbr, home_abbr,
) -> bytes:
    out = bytearray([mlb.WIRE_VERSION, mlb.GAME_STATE_POST])
    out += struct.pack(
        "<BBBHHIIII",
        periods_played, len(away_line), len(home_line), away_score, home_score,
        away_pri, away_alt, home_pri, home_alt,
    )
    out += bytes(away_line) + bytes(home_line)
    out += _str(game_id) + _str(away_abbr) + _str(home_abbr)
    return bytes(out)


def encode_soccer_final(
    *, away_score, home_score, away_pri, away_alt, home_pri, home_alt,
    game_id, away_abbr, home_abbr, away_scorers, home_scorers,
) -> bytes:
    out = bytearray([mlb.WIRE_VERSION, mlb.GAME_STATE_POST])
    out += struct.pack("<HHIIII", away_score, home_score, away_pri, away_alt, home_pri, home_alt)
    out += _str(game_id) + _str(away_abbr) + _str(home_abbr)
    out += _str(away_scorers) + _str(home_scorers)
    return bytes(out)


# --- Fixture specs ----------------------------------------------------------
#
# The old v1 live golden, kept only to pin the plan's "v2 live == 02 01 +
# v1[1:]" claim (v1 header was a single version byte; v2 is version+state).
_V1_FULL = bytes.fromhex(
    "010107020302020503000500562c0c005c5c00003930bd0040230c000934303135"
    "37303732390353454103424f530b472e20576869746c6f636b0d4a2e20526f6472"
    "c3ad6775657a0d34303135373037323930303731294a756c696f20526f6472c3ad"
    "6775657a2073696e676c657320746f2063656e746572206669656c642e"
)

LIST_ENTRIES = [
    (mlb.GAME_STATE_POST, "401570729"),
    (mlb.GAME_STATE_PRE, "401570001"),
    (mlb.GAME_STATE_IN, "401570500"),
]

LIVE_FULL = dict(
    flags=0x01, inning_number=7, half=2, balls=3, strikes=2, outs=2, bases=0x05,
    away_score=3, home_score=5,
    away_pri=0x0C2C56, away_alt=0x005C5C, home_pri=0xBD3039, home_alt=0x0C2340,
    game_id="401570729", away_abbr="SEA", home_abbr="BOS",
    pitcher="G. Whitlock", batter="J. Rodríguez",
    last_play_id="4015707290071",
    last_play_text="Julio Rodríguez singles to center field.",
)

LIVE_MINIMAL = dict(
    flags=0x00, inning_number=1, half=0, balls=0, strikes=0, outs=0, bases=0x00,
    away_score=0, home_score=0,
    away_pri=0x112233, away_alt=0x445566, home_pri=0x778899, home_alt=0xAABBCC,
    game_id="401570001", away_abbr="NYY", home_abbr="TOR",
    pitcher="", batter="",
    last_play_id="p1", last_play_text="",
)

PREGAME_ALL = dict(
    flags=0x1F, temperature=72,
    away_wins=41, away_losses=28, home_wins=47, home_losses=42,
    start_time=1720368300,  # 2024-07-07 15:05:00Z
    away_pri=0x0C2C56, away_alt=0x005C5C, home_pri=0xBD3039, home_alt=0x0C2340,
    game_id="401570729", away_abbr="SEA", home_abbr="BOS", venue="Fenway Park",
    condition="Partly Cloudy", away_probable="G. Marquez", home_probable="T. Houck",
)

PREGAME_NONE = dict(
    flags=0x00, temperature=0,
    away_wins=0, away_losses=0, home_wins=0, home_losses=0,
    start_time=1720368300,
    away_pri=0x112233, away_alt=0x445566, home_pri=0x778899, home_alt=0xAABBCC,
    game_id="401570001", away_abbr="NYY", home_abbr="TOR", venue="Yankee Stadium",
    condition="", away_probable="", home_probable="",
)

FINAL_EVEN = dict(
    innings_played=9,
    away_line=[0, 1, 0, 0, 2, 0, 0, 0, 0],
    home_line=[0, 0, 0, 1, 0, 0, 1, 0, 0],
    away_score=3, home_score=2,
    away_pri=0x0C2C56, away_alt=0x005C5C, home_pri=0xBD3039, home_alt=0x0C2340,
    game_id="401570729", away_abbr="SEA", home_abbr="BOS",
)

# Walk-off: home wins in the bottom of the 9th, so its half-inning is short
# (nH == 8) while the away line is full (nA == 9).
FINAL_WALKOFF = dict(
    innings_played=9,
    away_line=[1, 0, 0, 0, 0, 0, 0, 0, 0],
    home_line=[0, 0, 0, 0, 0, 0, 0, 2],
    away_score=1, home_score=2,
    away_pri=0x112233, away_alt=0x445566, home_pri=0x778899, home_alt=0xAABBCC,
    game_id="401570002", away_abbr="LAD", home_abbr="SFG",
)

FINAL_EXTRAS = dict(
    innings_played=11,
    away_line=[0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2],
    home_line=[0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    away_score=3, home_score=1,
    away_pri=0x0C2C56, away_alt=0x005C5C, home_pri=0xBD3039, home_alt=0x0C2340,
    game_id="401570003", away_abbr="HOU", home_abbr="TEX",
)


NBA_LIVE_FULL = dict(
    flags=0x01, period=3, phase=nba.PHASE_IN_PROGRESS,
    away_score=75, home_score=77,
    away_pri=0x007AC1, away_alt=0xEF3B24, home_pri=0x0E2240, home_alt=0xFEC524,
    game_id="401811037", away_abbr="OKC", home_abbr="DEN", clock="4:37",
    last_play_id="401811037411",
    last_play_text="Zeke Nnaji out of bounds bad pass turnover",
)

NBA_LIVE_HALFTIME = dict(
    flags=0x00, period=2, phase=nba.PHASE_HALFTIME,
    away_score=52, home_score=74,
    away_pri=0x5D76A9, away_alt=0xF5B112, home_pri=0x4E008E, home_alt=0xF9A01B,
    game_id="401811036", away_abbr="MEM", home_abbr="UTAH", clock="0.0",
    last_play_id="", last_play_text="",
)

NBA_PREGAME_ALL = dict(
    flags=0x03,
    away_wins=40, away_losses=42, home_wins=50, home_losses=32,
    start_time=1775874600,  # 2026-04-11T02:30Z
    away_pri=0x29127A, away_alt=0xE56020, home_pri=0x552583, home_alt=0xFDB927,
    game_id="401811040", away_abbr="PHX", home_abbr="LAL",
    venue="crypto.com Arena",
)

NBA_PREGAME_NONE = dict(
    flags=0x00,
    away_wins=0, away_losses=0, home_wins=0, home_losses=0,
    start_time=1775874600,
    away_pri=0x29127A, away_alt=0xE56020, home_pri=0x552583, home_alt=0xFDB927,
    game_id="401811040", away_abbr="PHX", home_abbr="LAL",
    venue="crypto.com Arena",
)

NBA_FINAL = dict(
    periods_played=4,
    away_line=[30, 28, 30, 30],
    home_line=[25, 25, 25, 25],
    away_score=118, home_score=100,
    away_pri=0x1D428A, away_alt=0xC8102E, home_pri=0x008CA8, home_alt=0x1D1160,
    game_id="401811026", away_abbr="DET", home_abbr="CHA",
)

# Overtime: five periods, five line-score entries per side.
NBA_FINAL_OT = dict(
    periods_played=5,
    away_line=[30, 28, 30, 30, 12],
    home_line=[25, 25, 25, 25, 10],
    away_score=130, home_score=110,
    away_pri=0x1D428A, away_alt=0xC8102E, home_pri=0x008CA8, home_alt=0x1D1160,
    game_id="401811027", away_abbr="DET", home_abbr="CHA",
)

SOCCER_LIVE_FULL = dict(
    # flags: event present (0x02) + event away (0x08)
    flags=0x0A, half=1, clock_seconds=51 * 60,
    away_score=2, home_score=1,
    away_pri=0xE30613, away_alt=0xFDDA25, home_pri=0x002868, home_alt=0xBF0A30,
    game_id="401800100", away_abbr="BEL", home_abbr="USA",
    event_clock="45'+1'", event_name="R. Lukaku",
)

SOCCER_LIVE_COMMENTARY = dict(
    # flags: event (0x02) + event away (0x08) + commentary (0x20)
    flags=0x2A, half=1, clock_seconds=51 * 60,
    away_score=2, home_score=1,
    away_pri=0xE30613, away_alt=0xFDDA25, home_pri=0x002868, home_alt=0xBF0A30,
    game_id="401800100", away_abbr="BEL", home_abbr="USA",
    event_clock="45'+1'", event_name="R. Lukaku",
    comment_id="87",
    comment_text="Goal!  Belgium 2, USA 1. Romelu Lukaku right footed shot to the bottom left corner.",
)

SOCCER_LIVE_HALFTIME = dict(
    # flags: halftime (0x01) + event present (0x02) + event home (0x10)
    flags=0x13, half=1, clock_seconds=51 * 60,
    away_score=2, home_score=2,
    away_pri=0xE30613, away_alt=0xFDDA25, home_pri=0x002868, home_alt=0xBF0A30,
    game_id="401800100", away_abbr="BEL", home_abbr="USA",
    event_clock="45'+1'", event_name="C. Pulisic",
)

SOCCER_LIVE_QUIET = dict(
    flags=0x00, half=2, clock_seconds=87 * 60,
    away_score=0, home_score=0,
    away_pri=0x004812, away_alt=0xEAE827, home_pri=0x5D9741, home_alt=0x005595,
    game_id="401800101", away_abbr="POR", home_abbr="SEA",
    event_clock="", event_name="",
)

SOCCER_PREGAME = dict(
    start_time=1783647600,
    away_pri=0x004812, away_alt=0xEAE827, home_pri=0x5D9741, home_alt=0x005595,
    game_id="401800102", away_abbr="POR", home_abbr="SEA",
)

SOCCER_FINAL = dict(
    away_score=1, home_score=0,
    away_pri=0xFF0000, away_alt=0xFFC400, home_pri=0x004812, home_alt=0xEAE827,
    game_id="401800103", away_abbr="ESP", home_abbr="POR",
    away_scorers="M. Merino 90'+1'", home_scorers="",
)


# --- Goldens (swap a line to bytes.fromhex(...) to pin Rust bytes) ----------

GOLDEN_LIST = encode_list(LIST_ENTRIES)
GOLDEN_LIVE_FULL = encode_live(**LIVE_FULL)
GOLDEN_LIVE_MINIMAL = encode_live(**LIVE_MINIMAL)
GOLDEN_PREGAME_ALL = encode_pregame(**PREGAME_ALL)
GOLDEN_PREGAME_NONE = encode_pregame(**PREGAME_NONE)
GOLDEN_FINAL_EVEN = encode_final(**FINAL_EVEN)
GOLDEN_FINAL_WALKOFF = encode_final(**FINAL_WALKOFF)
GOLDEN_FINAL_EXTRAS = encode_final(**FINAL_EXTRAS)
GOLDEN_NBA_LIVE_FULL = encode_nba_live(**NBA_LIVE_FULL)
GOLDEN_NBA_LIVE_HALFTIME = encode_nba_live(**NBA_LIVE_HALFTIME)
GOLDEN_NBA_PREGAME_ALL = encode_nba_pregame(**NBA_PREGAME_ALL)
GOLDEN_NBA_PREGAME_NONE = encode_nba_pregame(**NBA_PREGAME_NONE)
GOLDEN_NBA_FINAL = encode_nba_final(**NBA_FINAL)
GOLDEN_NBA_FINAL_OT = encode_nba_final(**NBA_FINAL_OT)
GOLDEN_SOCCER_LIVE_FULL = encode_soccer_live(**SOCCER_LIVE_FULL)
GOLDEN_SOCCER_LIVE_COMMENTARY = encode_soccer_live(**SOCCER_LIVE_COMMENTARY)
GOLDEN_SOCCER_LIVE_HALFTIME = encode_soccer_live(**SOCCER_LIVE_HALFTIME)
GOLDEN_SOCCER_LIVE_QUIET = encode_soccer_live(**SOCCER_LIVE_QUIET)
GOLDEN_SOCCER_PREGAME = encode_soccer_pregame(**SOCCER_PREGAME)
GOLDEN_SOCCER_FINAL = encode_soccer_final(**SOCCER_FINAL)

_GOLDENS = [
    ("list", GOLDEN_LIST),
    ("live_full", GOLDEN_LIVE_FULL),
    ("live_minimal", GOLDEN_LIVE_MINIMAL),
    ("pregame_all", GOLDEN_PREGAME_ALL),
    ("pregame_none", GOLDEN_PREGAME_NONE),
    ("final_even", GOLDEN_FINAL_EVEN),
    ("final_walkoff", GOLDEN_FINAL_WALKOFF),
    ("final_extras", GOLDEN_FINAL_EXTRAS),
    ("nba_live_full", GOLDEN_NBA_LIVE_FULL),
    ("nba_live_ht", GOLDEN_NBA_LIVE_HALFTIME),
    ("nba_pregame_all", GOLDEN_NBA_PREGAME_ALL),
    ("nba_pregame_none", GOLDEN_NBA_PREGAME_NONE),
    ("nba_final", GOLDEN_NBA_FINAL),
    ("nba_final_ot", GOLDEN_NBA_FINAL_OT),
    ("soccer_live_full", GOLDEN_SOCCER_LIVE_FULL),
    ("soccer_live_comm", GOLDEN_SOCCER_LIVE_COMMENTARY),
    ("soccer_live_ht", GOLDEN_SOCCER_LIVE_HALFTIME),
    ("soccer_live_quiet", GOLDEN_SOCCER_LIVE_QUIET),
    ("soccer_pregame", GOLDEN_SOCCER_PREGAME),
    ("soccer_final", GOLDEN_SOCCER_FINAL),
]


# --- Rust encoder pins ------------------------------------------------------
#
# Byte-for-byte output of the Rust encoder (copied from the GOLDEN_* constants
# in backend/src/wire.rs tests). Re-encoding the same field values with the
# pure-Python encoder above must reproduce them exactly — this pins the two
# independent encoders (and, via the round-trip checks, the firmware parser)
# to each other, not just to a shared reading of the spec.

RUST_PIN_LIVE = bytes.fromhex(
    "02010107020302020503000500562c0c005c5c00003930bd0040230c0009343031"
    "3537303732390353454103424f530b472e20576869746c6f636b0d4a2e20526f64"
    "72c3ad6775657a0d34303135373037323930303731294a756c696f20526f6472c3"
    "ad6775657a2073696e676c657320746f2063656e746572206669656c642e"
)
RUST_PIN_PRE = bytes.fromhex(
    "02001f482c002e0032002800704d506a873000002c00e4001d242f0025c4ff0009"
    "343031353730303031034e59590253440a506574636f205061726b0c4d6f73746c"
    "792073756e6e790a472e204d61727175657a0a592e2044617276697368"
)
RUST_PIN_PRE_NO_WEATHER = bytes.fromhex(
    "020000000000000000000000704d506a873000002c00e4001d242f0025c4ff0009"
    "343031353730303031034e59590253440a506574636f205061726b"
)
RUST_PIN_FINAL = bytes.fromhex(
    "020209090804000500562c0c005c5c00003930bd0040230c000100000200000100"
    "000001000002000002093430313537303732390353454103424f53"
)
RUST_PIN_LIST = bytes.fromhex(
    "0203000934303135373037323901093430313537303030310209343031353730303032"
)

# NBA goldens (backend/src/wire.rs GOLDEN_NBA_* test constants).
RUST_PIN_NBA_LIVE = bytes.fromhex(
    "02010103004b004d00c17a0000243bef0040220e0024c5fe000934303138313130"
    "3337034f4b430344454e04343a33370c3430313831313033373431312a5a656b65"
    "204e6e616a69206f7574206f6620626f756e6473206261642070617373207475726e6f766572"
)
RUST_PIN_NBA_HALFTIME = bytes.fromhex(
    "020100020134004a00a9765d0012b1f5008e004e001ba0f90009343031383131303336"
    "034d454d045554414803302e30"
)
RUST_PIN_NBA_PRE = bytes.fromhex(
    "02000328002a003200200028b2d9697a1229002060e5008325550027b9fd0009343031"
    "38313130343003504858034c414c1063727970746f2e636f6d204172656e61"
)
RUST_PIN_NBA_PRE_NO_RECORDS = bytes.fromhex(
    "020000000000000000000028b2d9697a1229002060e5008325550027b9fd0009343031"
    "38313130343003504858034c414c1063727970746f2e636f6d204172656e61"
)
RUST_PIN_NBA_FINAL = bytes.fromhex(
    "0202040404760064008a421d002e10c800a88c000060111d001e1c1e1e19191919"
    "093430313831313032360344455403434841"
)

# Soccer goldens (backend/src/wire.rs GOLDEN_SOCCER_* test constants).
RUST_PIN_SOCCER_LIVE = bytes.fromhex(
    "02010a01f40b020001001306e30025dafd0068280000300abf000934303138303031"
    "30300342454c03555341063435272b312709522e204c756b616b75"
)
RUST_PIN_SOCCER_COMMENTARY = bytes.fromhex(
    "02012a01f40b020001001306e30025dafd0068280000300abf000934303138303031"
    "30300342454c03555341063435272b312709522e204c756b616b7502383753476f61"
    "6c21202042656c6769756d20322c2055534120312e20526f6d656c75204c756b616b"
    "7520726967687420666f6f7465642073686f7420746f2074686520626f74746f6d20"
    "6c65667420636f726e65722e"
)
RUST_PIN_SOCCER_HALFTIME = bytes.fromhex(
    "02011301f40b020002001306e30025dafd0068280000300abf000934303138303031"
    "30300342454c03555341063435272b31270a432e2050756c69736963"
)
RUST_PIN_SOCCER_QUIET = bytes.fromhex(
    "020100026414000000001248000027e8ea0041975d0095550000093430313830303130"
    "3103504f5203534541"
)
RUST_PIN_SOCCER_PRE = bytes.fromhex(
    "0200704d506a1248000027e8ea0041975d0095550000093430313830303130"
    "3203504f5203534541"
)
RUST_PIN_SOCCER_FINAL = bytes.fromhex(
    "0202010000000000ff0000c4ff001248000027e8ea0009343031383030313033034553"
    "5003504f52104d2e204d6572696e6f203930272b312700"
)


def check_rust_pins() -> None:
    assert encode_live(**LIVE_FULL) == RUST_PIN_LIVE

    rust_pre = dict(
        flags=0x1F, temperature=72,
        away_wins=44, away_losses=46, home_wins=50, home_losses=40,
        start_time=1783647600,  # 2026-07-10T01:40Z
        away_pri=0x003087, away_alt=0xE4002C, home_pri=0x2F241D, home_alt=0xFFC425,
        game_id="401570001", away_abbr="NYY", home_abbr="SD", venue="Petco Park",
        condition="Mostly sunny", away_probable="G. Marquez", home_probable="Y. Darvish",
    )
    assert encode_pregame(**rust_pre) == RUST_PIN_PRE
    assert encode_pregame(**{
        **rust_pre,
        "flags": 0x00, "temperature": 0,
        "away_wins": 0, "away_losses": 0, "home_wins": 0, "home_losses": 0,
        "condition": "", "away_probable": "", "home_probable": "",
    }) == RUST_PIN_PRE_NO_WEATHER

    assert encode_final(
        innings_played=9,
        away_line=[1, 0, 0, 2, 0, 0, 1, 0, 0],
        home_line=[0, 1, 0, 0, 2, 0, 0, 2],
        away_score=4, home_score=5,
        away_pri=0x0C2C56, away_alt=0x005C5C, home_pri=0xBD3039, home_alt=0x0C2340,
        game_id="401570729", away_abbr="SEA", home_abbr="BOS",
    ) == RUST_PIN_FINAL

    assert encode_list([
        (mlb.GAME_STATE_PRE, "401570729"),
        (mlb.GAME_STATE_IN, "401570001"),
        (mlb.GAME_STATE_POST, "401570002"),
    ]) == RUST_PIN_LIST

    # And the firmware parser accepts the Rust bytes directly.
    game = mlb.parse_game_detail(memoryview(RUST_PIN_PRE))
    assert game.start_epoch == 1783647600
    assert game.home.pitcher == "Y. Darvish"
    final = mlb.parse_game_detail(memoryview(RUST_PIN_FINAL))
    assert final.home.line == bytes([0, 1, 0, 0, 2, 0, 0, 2])

    # NBA: the pure-Python encodings must reproduce the Rust bytes, and the
    # firmware NBA parser must accept the Rust bytes directly.
    assert encode_nba_live(**NBA_LIVE_FULL) == RUST_PIN_NBA_LIVE
    assert encode_nba_live(**NBA_LIVE_HALFTIME) == RUST_PIN_NBA_HALFTIME
    assert encode_nba_pregame(**NBA_PREGAME_ALL) == RUST_PIN_NBA_PRE
    assert encode_nba_pregame(**NBA_PREGAME_NONE) == RUST_PIN_NBA_PRE_NO_RECORDS
    assert encode_nba_final(**NBA_FINAL) == RUST_PIN_NBA_FINAL

    nba_live = nba.parse_game_detail(memoryview(RUST_PIN_NBA_LIVE))
    assert nba_live.clock == "4:37"
    assert nba_live.last_play.id == "401811037411"
    nba_final_game = nba.parse_game_detail(memoryview(RUST_PIN_NBA_FINAL))
    assert nba_final_game.home.line == bytes([25, 25, 25, 25])

    # Soccer: the pure-Python encodings must reproduce the Rust bytes, and
    # the firmware soccer parser must accept the Rust bytes directly.
    assert encode_soccer_live(**SOCCER_LIVE_FULL) == RUST_PIN_SOCCER_LIVE
    assert encode_soccer_live(**SOCCER_LIVE_COMMENTARY) == RUST_PIN_SOCCER_COMMENTARY
    assert encode_soccer_live(**SOCCER_LIVE_HALFTIME) == RUST_PIN_SOCCER_HALFTIME
    assert encode_soccer_live(**SOCCER_LIVE_QUIET) == RUST_PIN_SOCCER_QUIET
    assert encode_soccer_pregame(**SOCCER_PREGAME) == RUST_PIN_SOCCER_PRE
    assert encode_soccer_final(**SOCCER_FINAL) == RUST_PIN_SOCCER_FINAL

    live = soccer.parse_game_detail(memoryview(RUST_PIN_SOCCER_LIVE), "MLS")
    assert live.last_event.name == "R. Lukaku"
    ft = soccer.parse_game_detail(memoryview(RUST_PIN_SOCCER_FINAL), "MLS")
    assert ft.away.scorers == "M. Merino 90'+1'"


# --- Round-trip checks ------------------------------------------------------

def check_list() -> None:
    games = mlb.parse_game_list(memoryview(GOLDEN_LIST))
    assert games == LIST_ENTRIES, games


def check_live_full() -> None:
    # Pins the plan invariant: a v2 live payload is the v1 body with its single
    # version byte replaced by the 2-byte version+state header.
    assert GOLDEN_LIVE_FULL == bytes([mlb.WIRE_VERSION, mlb.GAME_STATE_IN]) + _V1_FULL[1:]

    game = mlb.parse_game_detail(memoryview(GOLDEN_LIVE_FULL))
    assert isinstance(game, mlb.LiveGame)
    assert game.game_id == "401570729"
    assert game.inning.number == 7
    assert game.inning.half is inning_half.BOTTOM
    assert game.count.balls == 3
    assert game.count.strikes == 2
    assert game.count.outs == 2
    assert game.bases.first is True
    assert game.bases.second is False
    assert game.bases.third is True
    assert game.away.abbreviation == "SEA"
    assert game.away.score == 3
    assert game.away.colors.primary == 0x0C2C56
    assert game.away.colors.alternate == 0x005C5C
    assert game.home.abbreviation == "BOS"
    assert game.home.score == 5
    assert game.home.colors.primary == 0xBD3039
    assert game.home.colors.alternate == 0x0C2340
    assert game.at_bat is not None
    assert game.at_bat.pitcher == "G. Whitlock"
    assert game.at_bat.batter == "J. Rodríguez"  # multi-byte UTF-8 coverage
    assert game.last_play.id == "4015707290071"
    assert game.last_play.text == "Julio Rodríguez singles to center field."


def check_live_minimal() -> None:
    game = mlb.parse_game_detail(memoryview(GOLDEN_LIVE_MINIMAL))
    assert isinstance(game, mlb.LiveGame)
    assert game.game_id == "401570001"
    assert game.inning.number == 1
    assert game.inning.half is inning_half.TOP
    assert game.at_bat is None
    assert game.away.abbreviation == "NYY"
    assert game.home.abbreviation == "TOR"
    assert game.away.score == 0 and game.home.score == 0
    assert game.away.colors.primary == 0x112233
    assert game.away.colors.alternate == 0x445566
    assert game.home.colors.primary == 0x778899
    assert game.home.colors.alternate == 0xAABBCC
    assert game.last_play.id == "p1"
    assert game.last_play.text == ""


def check_pregame_all() -> None:
    game = mlb.parse_game_detail(memoryview(GOLDEN_PREGAME_ALL))
    assert isinstance(game, mlb.PregameGame)
    assert game.game_id == "401570729"
    assert game.start_epoch == 1720368300
    assert game.venue == "Fenway Park"
    assert game.weather_temp == 72
    assert game.weather_condition == "Partly Cloudy"
    assert game.away.abbreviation == "SEA"
    assert game.away.colors.primary == 0x0C2C56
    assert game.away.colors.alternate == 0x005C5C
    assert game.away.wins == 41 and game.away.losses == 28
    assert game.away.pitcher == "G. Marquez"
    assert game.home.abbreviation == "BOS"
    assert game.home.colors.primary == 0xBD3039
    assert game.home.colors.alternate == 0x0C2340
    assert game.home.wins == 47 and game.home.losses == 42
    assert game.home.pitcher == "T. Houck"


def check_pregame_none() -> None:
    game = mlb.parse_game_detail(memoryview(GOLDEN_PREGAME_NONE))
    assert isinstance(game, mlb.PregameGame)
    assert game.game_id == "401570001"
    assert game.start_epoch == 1720368300
    assert game.venue == "Yankee Stadium"
    # Unset flags surface as None even though the wire carries 0/empty.
    assert game.weather_temp is None
    assert game.weather_condition is None
    assert game.away.wins is None and game.away.losses is None
    assert game.away.pitcher is None
    assert game.home.wins is None and game.home.losses is None
    assert game.home.pitcher is None
    assert game.away.abbreviation == "NYY"
    assert game.home.abbreviation == "TOR"
    assert game.away.colors.primary == 0x112233
    assert game.home.colors.alternate == 0xAABBCC


def check_final_even() -> None:
    game = mlb.parse_game_detail(memoryview(GOLDEN_FINAL_EVEN))
    assert isinstance(game, mlb.FinalGame)
    assert game.game_id == "401570729"
    assert game.innings_played == 9
    assert game.away.abbreviation == "SEA"
    assert game.away.score == 3
    assert game.away.line == bytes([0, 1, 0, 0, 2, 0, 0, 0, 0])
    assert game.away.colors.primary == 0x0C2C56
    assert game.home.abbreviation == "BOS"
    assert game.home.score == 2
    assert game.home.line == bytes([0, 0, 0, 1, 0, 0, 1, 0, 0])
    assert game.home.colors.primary == 0xBD3039
    # Line vec must be a copy, not a view aliasing the source buffer.
    assert isinstance(game.away.line, bytes)


def check_final_walkoff() -> None:
    game = mlb.parse_game_detail(memoryview(GOLDEN_FINAL_WALKOFF))
    assert isinstance(game, mlb.FinalGame)
    assert game.innings_played == 9
    assert len(game.away.line) == 9
    assert len(game.home.line) == 8  # home didn't bat in the bottom 9th
    assert game.away.line == bytes([1, 0, 0, 0, 0, 0, 0, 0, 0])
    assert game.home.line == bytes([0, 0, 0, 0, 0, 0, 0, 2])
    assert game.away.score == 1 and game.home.score == 2


def check_final_extras() -> None:
    game = mlb.parse_game_detail(memoryview(GOLDEN_FINAL_EXTRAS))
    assert isinstance(game, mlb.FinalGame)
    assert game.innings_played == 11
    assert len(game.away.line) == 11 and len(game.home.line) == 11
    assert game.away.line == bytes([0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2])
    assert game.home.line == bytes([0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0])
    assert game.away.score == 3 and game.home.score == 1


def check_final_copies_out() -> None:
    """The line vec must not alias the source buffer after it's reused."""
    buf = bytearray(GOLDEN_FINAL_EVEN)
    game = mlb.parse_game_detail(memoryview(buf))
    original = bytes(game.away.line)
    # Simulate the client overwriting its shared response buffer in place.
    for i in range(len(buf)):
        buf[i] = 0xEE
    assert game.away.line == original, "line vec aliased the reusable buffer"


def check_nba_live_full() -> None:
    game = nba.parse_game_detail(memoryview(GOLDEN_NBA_LIVE_FULL))
    assert isinstance(game, nba.LiveGame)
    assert game.game_id == "401811037"
    assert game.period == 3
    assert game.phase == nba.PHASE_IN_PROGRESS
    assert game.clock == "4:37"
    assert game.away.abbreviation == "OKC"
    assert game.away.score == 75
    assert game.away.colors.primary == 0x007AC1
    assert game.away.colors.alternate == 0xEF3B24
    assert game.home.abbreviation == "DEN"
    assert game.home.score == 77
    assert game.home.colors.primary == 0x0E2240
    play = game.last_play
    assert play is not None
    assert play.id == "401811037411"
    assert play.text == "Zeke Nnaji out of bounds bad pass turnover"


def check_nba_live_halftime() -> None:
    game = nba.parse_game_detail(memoryview(GOLDEN_NBA_LIVE_HALFTIME))
    assert isinstance(game, nba.LiveGame)
    assert game.phase == nba.PHASE_HALFTIME
    assert game.period == 2
    assert game.clock == "0.0"  # break clock is meaningless; phase renders
    assert game.last_play is None
    assert game.away.abbreviation == "MEM" and game.away.score == 52
    assert game.home.abbreviation == "UTAH" and game.home.score == 74


def check_nba_pregame() -> None:
    game = nba.parse_game_detail(memoryview(GOLDEN_NBA_PREGAME_ALL))
    assert isinstance(game, nba.PregameGame)
    assert game.game_id == "401811040"
    assert game.start_epoch == 1775874600
    assert game.venue == "crypto.com Arena"
    assert game.away.abbreviation == "PHX"
    assert game.away.wins == 40 and game.away.losses == 42
    assert game.home.abbreviation == "LAL"
    assert game.home.wins == 50 and game.home.losses == 32
    # NBA ducks the MLB pregame contract: weather/probables permanently absent.
    assert game.weather_temp is None and game.weather_condition is None
    assert game.away.pitcher is None and game.home.pitcher is None

    none = nba.parse_game_detail(memoryview(GOLDEN_NBA_PREGAME_NONE))
    # Unset record flags surface as None even though the wire carries zeros.
    assert none.away.wins is None and none.away.losses is None
    assert none.home.wins is None and none.home.losses is None


def check_nba_final() -> None:
    game = nba.parse_game_detail(memoryview(GOLDEN_NBA_FINAL))
    assert isinstance(game, nba.FinalGame)
    assert game.game_id == "401811026"
    assert game.periods_played == 4
    assert game.away.abbreviation == "DET" and game.away.score == 118
    assert game.away.line == bytes([30, 28, 30, 30])
    assert game.home.abbreviation == "CHA" and game.home.score == 100
    assert game.home.line == bytes([25, 25, 25, 25])

    ot = nba.parse_game_detail(memoryview(GOLDEN_NBA_FINAL_OT))
    assert ot.periods_played == 5
    assert len(ot.away.line) == 5 and len(ot.home.line) == 5
    assert ot.away.line[4] == 12


def check_nba_rejections() -> None:
    def expect_error(payload: bytes, why: str) -> None:
        try:
            nba.parse_game_detail(memoryview(payload))
        except mlb.DeserializeError:
            return
        raise AssertionError(f"accepted invalid NBA payload: {why}")

    expect_error(b"", "empty payload")
    expect_error(bytes([3]) + GOLDEN_NBA_LIVE_FULL[1:], "future version")
    expect_error(bytes([mlb.WIRE_VERSION, 9]), "unknown state")
    expect_error(GOLDEN_NBA_LIVE_FULL[:20], "truncated NBA live fixed section")
    expect_error(GOLDEN_NBA_LIVE_FULL[:-3], "truncated inside last play text")
    expect_error(GOLDEN_NBA_LIVE_FULL + b"\x00", "trailing bytes after live")
    expect_error(GOLDEN_NBA_PREGAME_ALL[:20], "truncated NBA pregame fixed section")
    expect_error(GOLDEN_NBA_PREGAME_ALL + b"\x00", "trailing bytes after pregame")
    expect_error(GOLDEN_NBA_FINAL[:15], "truncated NBA final fixed section")
    expect_error(GOLDEN_NBA_FINAL + b"\x00", "trailing bytes after final")

    bad_phase = bytearray(GOLDEN_NBA_LIVE_FULL)
    bad_phase[4] = 9  # phase code (offset 2 + 2)
    expect_error(bytes(bad_phase), "invalid live phase code")

    bad_final = bytearray(GOLDEN_NBA_FINAL)
    bad_final[3] = 200  # away linescore len
    expect_error(bytes(bad_final), "linescore length overruns body")


def check_soccer_live_full() -> None:
    game = soccer.parse_game_detail(memoryview(GOLDEN_SOCCER_LIVE_FULL), "WORLD CUP")
    assert isinstance(game, soccer.LiveGame)
    assert game.game_id == "401800100"
    assert game.clock_seconds == 51 * 60
    assert game.half == 1
    assert game.halftime is False
    assert game.away.abbreviation == "BEL"
    assert game.away.score == 2
    assert game.away.colors.primary == 0xE30613
    assert game.away.colors.alternate == 0xFDDA25
    assert game.home.abbreviation == "USA"
    assert game.home.score == 1
    assert game.home.colors.primary == 0x002868
    ev = game.last_event
    assert ev is not None
    assert ev.kind == soccer.EVENT_GOAL
    assert ev.side == soccer.SIDE_AWAY
    assert ev.clock_text == "45'+1'"
    assert ev.name == "R. Lukaku"


def check_soccer_live_commentary() -> None:
    game = soccer.parse_game_detail(memoryview(GOLDEN_SOCCER_LIVE_COMMENTARY), "WORLD CUP")
    assert isinstance(game, soccer.LiveGame)
    assert game.comment_id == "87"
    assert game.comment_text.startswith("Goal!  Belgium 2, USA 1.")
    # Event fields still parse alongside commentary.
    assert game.last_event is not None
    assert game.last_event.name == "R. Lukaku"
    # No-commentary payloads surface empty strings, not None.
    quiet = soccer.parse_game_detail(memoryview(GOLDEN_SOCCER_LIVE_QUIET), "MLS")
    assert quiet.comment_id == "" and quiet.comment_text == ""


def check_soccer_live_halftime() -> None:
    game = soccer.parse_game_detail(memoryview(GOLDEN_SOCCER_LIVE_HALFTIME), "WORLD CUP")
    assert isinstance(game, soccer.LiveGame)
    assert game.halftime is True
    assert game.last_event is not None
    assert game.last_event.side == soccer.SIDE_HOME
    assert game.last_event.name == "C. Pulisic"


def check_soccer_live_quiet() -> None:
    game = soccer.parse_game_detail(memoryview(GOLDEN_SOCCER_LIVE_QUIET), "MLS")
    assert isinstance(game, soccer.LiveGame)
    assert game.half == 2
    assert game.clock_seconds == 87 * 60
    assert game.away.score == 0 and game.home.score == 0
    assert game.last_event is None


def check_soccer_pregame() -> None:
    game = soccer.parse_game_detail(memoryview(GOLDEN_SOCCER_PREGAME), "MLS")
    assert isinstance(game, soccer.PregameGame)
    assert game.game_id == "401800102"
    assert game.start_epoch == 1783647600
    # League display name threads into the venue slot (see soccer.PregameGame).
    assert game.venue == "MLS"
    # Soccer pregame ducks the MLB pregame contract: records/weather absent,
    # the abbreviation rides the probable-pitcher slot.
    assert game.away.wins is None and game.away.losses is None
    assert game.away.pitcher == "POR"
    assert game.home.pitcher == "SEA"
    assert game.weather_temp is None and game.weather_condition is None
    assert game.away.colors.primary == 0x004812
    assert game.home.colors.primary == 0x5D9741


def check_soccer_final() -> None:
    game = soccer.parse_game_detail(memoryview(GOLDEN_SOCCER_FINAL), "LA LIGA")
    assert isinstance(game, soccer.FinalGame)
    assert game.game_id == "401800103"
    assert game.away.abbreviation == "ESP"
    assert game.away.score == 1
    assert game.away.scorers == "M. Merino 90'+1'"
    assert game.home.abbreviation == "POR"
    assert game.home.score == 0
    assert game.home.scorers == ""


def check_soccer_rejections() -> None:
    def expect_error(payload: bytes, why: str) -> None:
        try:
            soccer.parse_game_detail(memoryview(payload), "MLS")
        except mlb.DeserializeError:
            return
        raise AssertionError(f"accepted invalid soccer payload: {why}")

    expect_error(b"", "empty payload")
    expect_error(bytes([3]) + GOLDEN_SOCCER_LIVE_FULL[1:], "future version")
    expect_error(bytes([mlb.WIRE_VERSION, 9]), "unknown state")
    expect_error(GOLDEN_SOCCER_LIVE_FULL[:20], "truncated soccer live fixed section")
    expect_error(GOLDEN_SOCCER_LIVE_FULL[:-3], "truncated inside event athlete")
    expect_error(GOLDEN_SOCCER_LIVE_FULL + b"\x00", "trailing bytes after live")
    expect_error(GOLDEN_SOCCER_PREGAME[:15], "truncated soccer pregame fixed section")
    expect_error(GOLDEN_SOCCER_PREGAME + b"\x00", "trailing bytes after pregame")
    expect_error(GOLDEN_SOCCER_FINAL[:-1], "truncated inside scorers")
    expect_error(GOLDEN_SOCCER_FINAL + b"\x00", "trailing bytes after final")


def check_rejections() -> None:
    """Malformed payloads must fail loudly with DeserializeError."""

    def expect_error(payload: bytes, why: str) -> None:
        try:
            mlb.parse_game_detail(memoryview(payload))
        except mlb.DeserializeError:
            return
        raise AssertionError(f"accepted invalid payload: {why}")

    expect_error(b"", "empty payload")
    expect_error(b"{" + GOLDEN_LIVE_FULL[1:], "JSON body masquerading (bad version)")
    expect_error(bytes([3]) + GOLDEN_LIVE_FULL[1:], "future version")
    expect_error(bytes([mlb.WIRE_VERSION, 9]) + GOLDEN_LIVE_FULL[2:], "unknown state")
    expect_error(GOLDEN_LIVE_FULL[:20], "truncated live fixed section")
    expect_error(GOLDEN_LIVE_FULL[:-5], "truncated inside final live string")
    expect_error(GOLDEN_LIVE_FULL + b"\x00", "trailing bytes after live")
    expect_error(GOLDEN_PREGAME_ALL[:20], "truncated pregame fixed section")
    expect_error(GOLDEN_PREGAME_ALL + b"\x00", "trailing bytes after pregame")
    expect_error(GOLDEN_FINAL_EVEN[:15], "truncated final fixed section")
    expect_error(GOLDEN_FINAL_EXTRAS + b"\x00", "trailing bytes after final")

    # A final payload claiming more linescore bytes than the body carries.
    bad_final = bytearray(GOLDEN_FINAL_EVEN)
    bad_final[3] = 200  # away linescore len
    expect_error(bytes(bad_final), "linescore length overruns body")

    bad_half = bytearray(GOLDEN_LIVE_FULL)
    bad_half[4] = 9  # inning half code (offset 2 + 2)
    expect_error(bytes(bad_half), "invalid inning half code")

    def expect_list_error(payload: bytes, why: str) -> None:
        try:
            mlb.parse_game_list(memoryview(payload))
        except mlb.DeserializeError:
            return
        raise AssertionError(f"accepted invalid list: {why}")

    expect_list_error(GOLDEN_LIST[:-3], "truncated id list")
    bad_state = bytearray(GOLDEN_LIST)
    bad_state[2] = 9  # first entry's state byte
    expect_list_error(bytes(bad_state), "invalid list entry state")


def main() -> int:
    check_list()
    check_live_full()
    check_live_minimal()
    check_pregame_all()
    check_pregame_none()
    check_final_even()
    check_final_walkoff()
    check_final_extras()
    check_final_copies_out()
    check_nba_live_full()
    check_nba_live_halftime()
    check_nba_pregame()
    check_nba_final()
    check_nba_rejections()
    check_soccer_live_full()
    check_soccer_live_commentary()
    check_soccer_live_halftime()
    check_soccer_live_quiet()
    check_soccer_pregame()
    check_soccer_final()
    check_soccer_rejections()
    check_rejections()
    check_rust_pins()

    print("wire_format_check: all cross-implementation golden checks passed")
    print("goldens (hex — cross-pin against backend/src/wire.rs):")
    for name, golden in _GOLDENS:
        print(f"  {name:>14} = {golden.hex()}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
