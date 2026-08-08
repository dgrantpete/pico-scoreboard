"""
Sport-agnostic wire-format primitives shared by every sport parser.

The backend serves game state as fixed-layout packed structs (negotiated via
`Accept: application/x-scoreboard-struct`). The NORMATIVE spec lives in
`backend/src/wire.rs`; the per-sport modules (scoreboard/mlb.py, soccer.py,
nba.py) parse their own payload layouts, while everything the layouts share
lives here: the 2-byte header (byte0 = version, byte1 = state), the u8
game-state codes, the length-prefixed string reader, the sport-agnostic
game-list parser, and the value types every sport reuses (TeamColors,
TeamState, PregameTeam, LastPlay).

Version mismatches fail loudly (a stray JSON body starts with '{' or '['
and fails the version check immediately). Model classes are plain-attribute
value types: instances are treated as immutable after construction and are
read by the display thread at 20 FPS, so attribute access stays on
MicroPython's fast path — no property descriptors.
"""

from .textfold import fold_text

# Must match crates/scoreboard-wire (the normative wire definition).
WIRE_VERSION = 2
STRUCT_CONTENT_TYPE = "application/x-scoreboard-struct"

# Common detail header: byte0 = version, byte1 = state. State codes are also
# the per-entry tags in the game list and the ETag tokens.
HDR_SIZE = 2
GAME_STATE_PRE = 0
GAME_STATE_IN = 1
GAME_STATE_POST = 2


class DeserializeError(Exception):
    """
    Raised when a payload doesn't match the expected wire format.

    Attributes:
        path: byte-offset context of the failure (e.g. "@29").
        message: Human-readable description of the mismatch.
    """

    def __init__(self, path: str, message: str) -> None:
        self.path: str = path
        self.message: str = message
        super().__init__(f"{path}: {message}")


def check_version(buf, end: int) -> None:
    if end < 1:
        raise DeserializeError("@0", "empty payload")
    version = buf[0]
    if version != WIRE_VERSION:
        raise DeserializeError(
            "@0", f"unsupported wire version {version} (expected {WIRE_VERSION})"
        )


def read_str(buf, offset: int, end: int, what: str) -> tuple[str, int]:
    """Read one u8-length-prefixed UTF-8 string. Returns (text, next_offset).

    This is the single point where every wire string enters the firmware, so
    it also normalizes to the display fonts' ASCII + Latin-1 repertoire
    (textfold.fold_text): a decoded UTF-8 string is pure ASCII exactly when
    its char count equals its byte count, so the common case skips the fold
    in O(1). Game ids are folded too — harmless while ESPN ids stay numeric,
    and every internal comparison sees the same folded form.
    """
    if offset >= end:
        raise DeserializeError(f"@{offset}", f"truncated before {what} length")
    n = buf[offset]
    offset += 1
    if offset + n > end:
        raise DeserializeError(
            f"@{offset}", f"truncated inside {what}: need {n} bytes, have {end - offset}"
        )
    s = str(buf[offset:offset + n], "utf-8")
    if len(s) != n:
        s = fold_text(s)
    return s, offset + n


class TeamColors:
    """Primary / alternate team colors as packed RGB integers."""

    def __init__(self, primary: int, alternate: int) -> None:
        self.primary = primary
        self.alternate = alternate


class TeamState:
    """One team's snapshot: abbreviation, current score, and colors."""

    def __init__(self, abbreviation: str, score: int, colors: TeamColors) -> None:
        self.abbreviation = abbreviation
        self.score = score
        self.colors = colors


class PregameTeam:
    """One team's pregame snapshot: abbreviation, colors, record, probable.

    `wins`/`losses` are None when the backend didn't advertise a record for
    this side (record flag off); `pitcher` is None when no probable was
    advertised. Absent numeric fields arrive on the wire as 0 and are surfaced
    as None here so the display never renders a fake 0-0 record.
    """

    def __init__(
        self,
        abbreviation: str,
        colors: TeamColors,
        wins: int | None,
        losses: int | None,
        pitcher: str | None,
    ) -> None:
        self.abbreviation = abbreviation
        self.colors = colors
        self.wins = wins
        self.losses = losses
        self.pitcher = pitcher


class LastPlay:
    """Most recent play's ESPN id and human-readable description."""

    def __init__(self, id: str, text: str) -> None:
        self.id = id
        self.text = text


def dispatch_detail(buf, live_cls, pregame_cls, final_cls, league=None):
    """Parse a v2 detail payload, dispatching on the state header byte.

    Each sport's `parse_game_detail` is a thin wrapper passing its three
    `from_struct`-bearing state classes. Multi-league sports also pass
    `league`, the polled league's display name, which is threaded into the
    pregame model only (live/final don't carry it); single-league sports
    leave it None.
    """
    end = len(buf)
    check_version(buf, end)
    if end < HDR_SIZE:
        raise DeserializeError("@1", "truncated before state byte")
    state = buf[1]
    if state == GAME_STATE_IN:
        return live_cls.from_struct(buf)
    if state == GAME_STATE_PRE:
        if league is not None:
            return pregame_cls.from_struct(buf, league)
        return pregame_cls.from_struct(buf)
    if state == GAME_STATE_POST:
        return final_cls.from_struct(buf)
    raise DeserializeError("@1", f"unknown game state {state}")


def parse_game_list(buf) -> list[tuple[int, str]]:
    """Parse a v2 game-list payload (see backend/src/wire.rs for the spec).

    Returns (state, id) pairs in backend (chronological) order. `state` is one
    of GAME_STATE_PRE / GAME_STATE_IN / GAME_STATE_POST.
    """
    end = len(buf)
    check_version(buf, end)
    if end < 2:
        raise DeserializeError("@1", "truncated before game count")
    count = buf[1]

    games: list[tuple[int, str]] = []
    o = 2
    for _ in range(count):
        if o >= end:
            raise DeserializeError(f"@{o}", "truncated before game state")
        state = buf[o]
        o += 1
        if state > GAME_STATE_POST:
            raise DeserializeError(f"@{o - 1}", f"invalid game state {state}")
        game_id, o = read_str(buf, o, end, "game id")
        games.append((state, game_id))

    if o != end:
        raise DeserializeError(f"@{o}", f"{end - o} unexpected trailing bytes")
    return games
