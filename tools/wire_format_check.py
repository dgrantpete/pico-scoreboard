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


# --- Goldens (swap a line to bytes.fromhex(...) to pin Rust bytes) ----------

GOLDEN_LIST = encode_list(LIST_ENTRIES)
GOLDEN_LIVE_FULL = encode_live(**LIVE_FULL)
GOLDEN_LIVE_MINIMAL = encode_live(**LIVE_MINIMAL)
GOLDEN_PREGAME_ALL = encode_pregame(**PREGAME_ALL)
GOLDEN_PREGAME_NONE = encode_pregame(**PREGAME_NONE)
GOLDEN_FINAL_EVEN = encode_final(**FINAL_EVEN)
GOLDEN_FINAL_WALKOFF = encode_final(**FINAL_WALKOFF)
GOLDEN_FINAL_EXTRAS = encode_final(**FINAL_EXTRAS)

_GOLDENS = [
    ("list", GOLDEN_LIST),
    ("live_full", GOLDEN_LIVE_FULL),
    ("live_minimal", GOLDEN_LIVE_MINIMAL),
    ("pregame_all", GOLDEN_PREGAME_ALL),
    ("pregame_none", GOLDEN_PREGAME_NONE),
    ("final_even", GOLDEN_FINAL_EVEN),
    ("final_walkoff", GOLDEN_FINAL_WALKOFF),
    ("final_extras", GOLDEN_FINAL_EXTRAS),
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
    check_rejections()
    check_rust_pins()

    print("wire_format_check: all cross-implementation golden checks passed")
    print("goldens (hex — cross-pin against backend/src/wire.rs):")
    for name, golden in _GOLDENS:
        print(f"  {name:>14} = {golden.hex()}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
