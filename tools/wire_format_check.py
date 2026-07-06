#!/usr/bin/env python3
"""Cross-implementation golden test for the binary wire format.

Decodes golden fixture bytes with the ACTUAL firmware parser
(`firmware/src/scoreboard/mlb.py`, which is plain Python and runs under
CPython) and asserts every field. The same fixture hex is asserted against
the Rust encoder in `backend/src/wire.rs` tests — if both suites pass, the
encoder and parser agree byte-for-byte. The normative spec is the doc
comment in `backend/src/wire.rs`.

Run:  python tools/wire_format_check.py
"""

import importlib
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

# Golden fixtures — keep in lockstep with backend/src/wire.rs tests.
GOLDEN_FULL = bytes.fromhex(
    "010107020302020503000500562c0c005c5c00003930bd0040230c000934303135"
    "37303732390353454103424f530b472e20576869746c6f636b0d4a2e20526f6472"
    "c3ad6775657a0d34303135373037323930303731294a756c696f20526f6472c3ad"
    "6775657a2073696e676c657320746f2063656e746572206669656c642e"
)
GOLDEN_MINIMAL = bytes.fromhex(
    "010001000000000000000000332211006655440099887700ccbbaa000934303135"
    "3730303031034e595903544f5202703100"
)
GOLDEN_IDS = bytes.fromhex("01020934303135373037323909343031353730303031")


def check_full() -> None:
    game = mlb.LiveGame.from_struct(memoryview(GOLDEN_FULL))
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


def check_minimal() -> None:
    game = mlb.LiveGame.from_struct(memoryview(GOLDEN_MINIMAL))
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


def check_game_ids() -> None:
    ids = mlb.parse_game_ids(memoryview(GOLDEN_IDS))
    assert ids == ["401570729", "401570001"]


def check_rejections() -> None:
    """Malformed payloads must fail loudly with DeserializeError."""

    def expect_error(payload: bytes, why: str) -> None:
        try:
            mlb.LiveGame.from_struct(memoryview(payload))
        except mlb.DeserializeError:
            return
        raise AssertionError(f"accepted invalid payload: {why}")

    expect_error(b"", "empty payload")
    expect_error(b"{" + GOLDEN_FULL[1:], "JSON body masquerading (bad version)")
    expect_error(bytes([2]) + GOLDEN_FULL[1:], "future version")
    expect_error(GOLDEN_FULL[:20], "truncated fixed section")
    expect_error(GOLDEN_FULL[:-5], "truncated inside final string")
    expect_error(GOLDEN_FULL + b"\x00", "trailing bytes")
    bad_half = bytearray(GOLDEN_FULL)
    bad_half[3] = 9
    expect_error(bytes(bad_half), "invalid inning half code")

    try:
        mlb.parse_game_ids(memoryview(GOLDEN_IDS[:-3]))
    except mlb.DeserializeError:
        pass
    else:
        raise AssertionError("accepted truncated id list")


def main() -> int:
    check_full()
    check_minimal()
    check_game_ids()
    check_rejections()
    print("wire_format_check: all cross-implementation golden checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
