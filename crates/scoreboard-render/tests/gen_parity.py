"""Golden-frame generator for the MicroPython <-> Rust pixel-parity harness.

Every frame this writes comes out of the **shipping MicroPython firmware**,
run on CPython under `tools/preview`'s shims. Nothing is transcribed and
nothing is reimplemented: the wire bytes go through
`scoreboard.{mlb,nba,football,soccer}.parse_game_detail`, the parsed model goes
through the real `poller._commit_*_live` / `state.set_pregame` /
`state.set_*_final` setters, and the published `StateBuffer` goes through the
real `display.render_frame`. The RGB565 buffer that comes back is the parity
baseline the Rust stack has to reproduce byte for byte.

    py crates/scoreboard-render/tests/gen_parity.py

Needs: Pillow + freetype (via `tools/preview`), and the generated layout/font
modules (`py tools/compile_layout.py && py tools/compile_fonts.py`).

# What "the same frame" means, precisely

Three things have to be pinned or the two stacks cannot be compared at all:

* **The wall clock.** Every setter stamps `time.ticks_ms()` into the state it
  publishes (`animation_start_ms`, `play.updated_ms`, the soccer clock anchor).
  The virtual clock is parked at `COMMIT_MS` for the whole commit, so those
  stamps are a constant the Rust side passes as `now_ms`.
* **The two rails.** A frame is rendered at wall `COMMIT_MS + t` with both
  frame rails equal to `t` — the values `LoopState.advance_and_latch` produces
  under ideal pacing, `t` ms after a commit that changed the view identity.
  Pinning them directly rather than stepping a loop is what makes a single
  frame reproducible from `t` alone on both sides.
* **Today's date.** `state.set_pregame` compares the game's local day against
  `time.time()`'s, so an unpinned run would emit a different date line
  tomorrow. `time.time()` is pinned to `NOW_EPOCH_S`, which the Rust side
  receives as `LocalClock::now_epoch_s`.

Everything else that could differ between the two stacks is *emitted into the
manifest* rather than agreed on by hand: the UI palette comes from
`config.Config()`'s defaults, the screen variants and scroll speed from
`screen_geometry`'s module state, and the crest pixels from `LogoProvider`'s
placeholder builder. A change to any of those shows up on both sides at once.

# Output

    tests/parity/manifest.txt      the case list + every pinned input
    tests/parity/logos.rgb565      the crest pool, 1,152 B per slot
    tests/parity/frames/<case>__t<ms>.bin   raw 128x64 RGB565, 16,384 B
"""

import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
OUT_DIR = Path(__file__).resolve().parent / "parity"
FIXTURES = REPO / "backend" / "testdata" / "wire"

sys.path.insert(0, str(REPO))

from tools.preview import render as preview_render  # noqa: E402
from tools.preview.firmware_env import load_firmware  # noqa: E402
from tools.preview.logos import LogoProvider  # noqa: E402
from tools.preview.shims.time_shim import CLOCK_START_MS, VirtualClock  # noqa: E402

# The instant every commit is stamped with. `tools/preview`'s clock starts here
# so that a zero timestamp still reads as the "never" sentinel.
COMMIT_MS = CLOCK_START_MS

# Pinned wall clock for the pregame date line: 2026-03-01 00:00:00 UTC. Chosen
# to be a day no fixture kicks off on, so every pregame card renders its date
# phase — the branch that would otherwise never be exercised.
NOW_EPOCH_S = 1_772_323_200
# US Eastern, standard time. Not None: `set_pregame` omits the first-pitch line
# entirely without an offset, which would skip the time formatting too.
UTC_OFFSET_S = -5 * 3600

# Frame-rail offsets rendered per fixture. Each lands in a different phase of
# the animations the screens run, so a scroll or cycle that is wrong anywhere
# but t=0 still fails:
#   0      every animation at its start; the opening scroll pause
#   1500   past the 1,000 ms play/pregame pause, mid-scroll; soccer clock +1 min
#   4500   past the 4,000 ms pregame dwell floor (phase 2) and the 1,800 ms
#          line-score pause (mid-crawl)
#   11000  deep into every scroll, and past the shorter play windows, so the
#          bottom strip falls back from the flash to the sport's own content
TIME_POINTS = (0, 1500, 4500, 11000)

# Sport -> the `LogoPool` cache namespace the poller builds crest paths from.
# Only reaches the placeholder builder's cache key here, but keeping the real
# shape means a fetched-logo run caches under the same names the firmware uses.
LEAGUE_PATHS = {
    "mlb": "baseball/mlb",
    "nba": "basketball/nba",
    "football": "football/{slug}",
    "soccer": "soccer/{slug}",
}


class Case:
    """One fixture, resolved to everything both stacks need to reproduce it."""

    def __init__(self, name, sport, slug, fixture):
        self.name = name
        self.sport = sport
        self.slug = slug
        self.fixture = fixture  # repo-relative, forward slashes
        self.away_slot = 0
        self.home_slot = 0


def discover() -> "list[Case]":
    """Every committed wire fixture, with the sport and league slug its path
    encodes — the same two facts the poller knows from the endpoint it polled,
    never sniffed from the bytes."""
    cases = []
    for path in sorted(FIXTURES.rglob("*.bin")):
        parts = path.relative_to(FIXTURES).with_suffix("").parts
        sport = parts[0]
        if sport in ("mlb", "nba"):
            slug, stem = sport, parts[1]
        elif sport in ("football", "soccer"):
            slug, stem = parts[1], parts[2]
        else:
            raise SystemExit(f"unknown sport directory: {path}")
        name = "__".join((sport, slug, stem)) if slug != sport else f"{sport}__{stem}"
        cases.append(
            Case(name, sport, slug, path.relative_to(REPO).as_posix())
        )
    return cases


class LogoPool:
    """Deduplicated crest slots, addressed the way `LogoRef` addresses them.

    Two teams whose placeholder tiles are byte-identical share a slot, which is
    exactly what the firmware's pool would do with two identical crests — and
    what keeps the pool small enough for `LogoRef`'s u8.
    """

    def __init__(self, provider) -> None:
        self._provider = provider
        self._slots: list[bytes] = []
        self._index: dict[bytes, int] = {}

    def slot(self, team, league_path: str):
        """Returns `(slot_index, framebuffer)` — the handle the Rust snapshot
        carries and the crest the Python setter is handed."""
        buffer = self._provider.get(
            team.abbreviation, team.colors.primary, team.colors.alternate, league_path
        )
        # The framebuf shim exposes no public accessor for its backing bytes,
        # and this script does not get to change `tools/preview`.
        data = bytes(buffer._buf)
        existing = self._index.get(data)
        if existing is not None:
            return existing, buffer
        index = len(self._slots)
        if index > 255:
            raise SystemExit("crest pool overflowed LogoRef's u8")
        self._slots.append(data)
        self._index[data] = index
        return index, buffer

    def blob(self) -> bytes:
        return b"".join(self._slots)

    def count(self) -> int:
        return len(self._slots)


class _PollerStub:
    """Stands in for the `GamePoller` the commit functions take first.

    Only one of them reads anything off it: `_commit_soccer_live` wants the
    previous poll's clock for the same game. A fresh stub per case leaves that
    None, matching a `Store` that has never seen this game — the stale-clock
    guard is off on a first commit on both sides.
    """

    def __init__(self) -> None:
        self._prev_soccer_clock = None


def commit(env, poller, case, detail, away_logo, home_logo) -> None:
    """Publish one parsed detail exactly as `poller._poll_current` would.

    The dispatch is on the model's own `wire_state`, and every arm calls the
    firmware's real commit path — including the shared play-flash staging,
    which lives in `poller`, not in `state`.
    """
    from scoreboard.wire import GAME_STATE_IN, GAME_STATE_PRE, GAME_STATE_POST

    commit_live = {
        "mlb": poller._commit_mlb_live,
        "nba": poller._commit_nba_live,
        "football": poller._commit_football_live,
        "soccer": poller._commit_soccer_live,
    }[case.sport]
    commit_final = {
        "mlb": env.state.set_mlb_final,
        "nba": env.state.set_nba_final,
        "football": env.state.set_football_final,
        "soccer": env.state.set_soccer_final,
    }[case.sport]

    state = detail.wire_state
    if state == GAME_STATE_IN:
        commit_live(_PollerStub(), detail.game_id, detail, home_logo, away_logo)
    elif state == GAME_STATE_PRE:
        env.state.set_pregame(detail, home_logo, away_logo, UTC_OFFSET_S, case.sport)
    elif state == GAME_STATE_POST:
        commit_final(detail, home_logo, away_logo)
    else:
        raise SystemExit(f"{case.name}: unhandled wire state {state}")


def parse(env, case, payload):
    """The sport's parser, with the league display name the endpoint implies."""
    if case.sport == "mlb":
        return env.mlb.parse_game_detail(payload)
    if case.sport == "nba":
        return env.nba.parse_game_detail(payload)
    if case.sport == "football":
        name = env.football.LEAGUE_NAMES.get(case.slug, case.slug.upper())
        return env.football.parse_game_detail(payload, name)
    name = env.soccer.LEAGUE_NAMES.get(case.slug, case.slug.upper())
    return env.soccer.parse_game_detail(payload, name)


def reset_state(env) -> None:
    """A clean mailbox at the commit instant, with config-default UI colors.

    A fresh mailbox per case is the point: the Rust side builds a fresh `Store`,
    so a field a setter forgets to write would read as a leftover here and as a
    default there — which is a diff worth seeing, not one worth hiding.
    """
    env.state._state_mailbox = env.state.TripleBufferedState()
    env.clock.set(COMMIT_MS)
    env.state.update_ui_colors(env.config.Config())


def commit_fixture(env, poller, pool, case) -> None:
    """Parse one fixture and publish it, recording the crest slots it used."""
    payload = (REPO / case.fixture).read_bytes()
    detail = parse(env, case, payload)
    league_path = LEAGUE_PATHS[case.sport].format(slug=case.slug)
    case.away_slot, away_logo = pool.slot(detail.away, league_path)
    case.home_slot, home_logo = pool.slot(detail.home, league_path)
    commit(env, poller, case, detail, away_logo, home_logo)


def render_at_time_points(env, targets) -> "list[bytes]":
    """Render whatever is currently published, once per time point.

    Regions are rebuilt here so a screen that narrows them (setup's QR) cannot
    leak the narrowing into the next case.
    """
    display, writer, _regions = targets
    regions = env.display.Regions(display)
    frames = []
    for offset in TIME_POINTS:
        now = COMMIT_MS + offset
        env.clock.set(now)
        state, _seq = env.state.acquire_display_state()
        preview_render.poison_scratch(env.display, writer)
        env.display.render_frame(
            display, writer, regions, state, state.ui_colors, now, offset, offset
        )
        frames.append(bytes(display.buffer))
    return frames


def render_case(env, poller, pool, case, targets) -> "list[bytes]":
    """Drive one fixture through the whole Python stack, once per time point."""
    reset_state(env)
    commit_fixture(env, poller, pool, case)
    return render_at_time_points(env, targets)


# -- Static screens -----------------------------------------------------------
#
# The screens no wire payload reaches: they are published by hand, so the
# arguments below ARE the fixture. `parity_frames.rs` carries the same table
# against the Rust setters, and the manifest names every case, so a screen added
# to one side and not the other fails the test rather than going unnoticed.
#
# `set_setup_mode` is called without an AP SSID on purpose. With one, the
# MicroPython path generates its QR through `miqro`, which ships as a
# precompiled `.mpy` that CPython cannot import — the preview has always
# rendered that screen QR-less. Rather than compare a screen neither stack draws
# the way the device does, the QR is left out of the parity corpus entirely; the
# Rust encoder is already pinned against the independent `qrcode` package in
# `tests/qr.rs`.

# The highlighted row's label is wider than the 112 px label region on purpose:
# the menu marquee only scrolls a label that does not fit, and a row that fits
# would leave the marquee — the one thing on this screen that moves — untested.
# An over-long name is what `LeagueId::from_slug`'s unknown-slug fallback
# produces, so it is a shape the device can actually reach.
MENU_LABELS = ("MLB", "NBA", "NFL", "ENG.CHAMPIONSHIP", "LIGA MX")
MENU_CHECKED = (True, True, False, True, False)
MENU_HIGHLIGHT = 3
MENU_THUMB_Y = 1
MENU_THUMB_H = 25

# The live screen the icon toasts dim and the text toast displaces. `render_idle`
# draws no toast at all (toast drawing lives inside the mode renderers), so an
# overlay staged over idle would render an idle screen on both stacks and prove
# nothing. Over a live MLB game the dim covers a busy frame and the text toast
# takes the bottom strip away from the play flash — the priority rule itself.
TOAST_BASE = "mlb__live_inning"


def _menu_strips(env, labels):
    """Render one 1-bit strip per label, the way `MenuController._open` does.

    Three lines of `menu.py` rather than a `MenuController`: the controller
    needs a live poller and a source list to reach `_open`, and none of that
    changes a pixel. The cap arithmetic is copied because it is load-bearing —
    a MONO_HLSB strip's rows are byte-padded, so `cap` must be a multiple of 8.
    """
    fonts = env.fonts
    strips = []
    for label in labels:
        cap = ((fonts.measure_text(label, fonts.unscii_8) + 7) // 8) * 8
        strips.append(fonts.render_strip(bytearray(cap), cap, label, fonts.unscii_8))
    return strips


def static_screens(env):
    """`(name, base, publish)` per hand-published screen, in manifest order.

    `base` names the wire fixture published underneath (None for a bare screen);
    the overlays need one because they draw *over* a mode, not instead of it.
    """
    state = env.state

    def idle(_):
        state.set_mode("idle")

    def no_games(_):
        state.set_mode("no_games")

    def startup(_):
        state.set_startup_step(2, 5, "Connecting WiFi", "HOME-NET-5G", 2, 4)

    def error(_):
        state.set_error("NO WIFI", ["Check credentials", "in the web UI"])

    def updating_progress(_):
        state.set_updating_progress(42, "1.4.2")

    def updating_countdown(_):
        state.set_updating_countdown(3)

    def setup_no_config(_):
        state.set_setup_mode("no_config", "", "", "")

    def setup_bad_auth(_):
        state.set_setup_mode("bad_auth", "", "", "HOME-NET-5G")

    def toast_text(_):
        state.set_toast("ROTATION LOCKED", sticky=False, kind=state.TOAST_TEXT)

    def toast_lock(_):
        state.set_toast(sticky=True, kind=state.TOAST_LOCK)

    def toast_spinner(_):
        state.set_toast(sticky=True, kind=state.TOAST_SPINNER)

    def menu(env_):
        state.set_mode("idle")
        state.set_menu(
            _menu_strips(env_, MENU_LABELS), list(MENU_CHECKED),
            MENU_HIGHLIGHT, MENU_THUMB_Y, MENU_THUMB_H,
        )

    return [
        ("idle", None, idle),
        ("no_games", None, no_games),
        ("startup", None, startup),
        ("error", None, error),
        ("updating_progress", None, updating_progress),
        ("updating_countdown", None, updating_countdown),
        ("setup_no_config", None, setup_no_config),
        ("setup_bad_auth", None, setup_bad_auth),
        ("toast_text", TOAST_BASE, toast_text),
        ("toast_lock", TOAST_BASE, toast_lock),
        ("toast_spinner", TOAST_BASE, toast_spinner),
        ("menu", None, menu),
    ]


def render_screen(env, poller, pool, base_case, publish, targets) -> "list[bytes]":
    """Publish one hand-built screen (over `base_case`, if any) and render it."""
    reset_state(env)
    if base_case is not None:
        commit_fixture(env, poller, pool, base_case)
    publish(env)
    return render_at_time_points(env, targets)


def manifest_lines(env, cases, screens, pool) -> "list[str]":
    """The pinned inputs, read out of the firmware rather than restated.

    The palette comes from `config.Config()`'s defaults and the layout
    selections from `screen_geometry`'s module state, so if either moves, both
    stacks move with it on the next regeneration instead of silently disagreeing.
    """
    geometry = sys.modules["scoreboard.screen_geometry"]
    config = env.config.Config()

    lines = [
        "# GENERATED by tests/gen_parity.py -- do not edit by hand.",
        "# Whitespace-separated records; see gen_parity.py for what each pins.",
        "version 1",
        f"commit_ms {COMMIT_MS}",
        f"now_epoch_s {NOW_EPOCH_S}",
        f"utc_offset_s {UTC_OFFSET_S}",
        f"time_points {' '.join(str(t) for t in TIME_POINTS)}",
    ]
    for name in ("primary", "secondary", "accent", "clock_normal", "clock_warning"):
        color = config.get_color(name)
        lines.append(f"color {name} {color['r']} {color['g']} {color['b']}")
    for key in sorted(geometry._ACTIVE):
        lines.append(f"variant {key} {geometry._ACTIVE[key]}")
    lines.append(f"show_dividers {1 if geometry.SHOW_DIVIDERS else 0}")
    lines.append(f"scroll_px_per_sec {geometry.GAME_SCROLL_PX_PER_SEC}")
    lines.append(f"logo_slots {pool.count()}")
    for case in cases:
        lines.append(
            f"game {case.name} {case.sport} {case.slug} {case.fixture} "
            f"{case.away_slot} {case.home_slot}"
        )
    for name, base, _publish in screens:
        lines.append(f"screen {name} {base or '-'}")
    return lines


def main() -> None:
    frames_dir = OUT_DIR / "frames"
    frames_dir.mkdir(parents=True, exist_ok=True)
    for stale in frames_dir.glob("*.bin"):
        stale.unlink()

    clock = VirtualClock()
    env = load_firmware(clock)
    # Pin "today" for `set_pregame`'s date-phase comparison. The shim delegates
    # unknown attributes to the real `time`, so without this the date line would
    # change from one day to the next.
    sys.modules["time"].time = lambda: NOW_EPOCH_S

    import scoreboard.poller as poller

    targets = preview_render.build_render_targets(env)
    pool = LogoPool(LogoProvider())

    cases = discover()
    for case in cases:
        for offset, frame in zip(TIME_POINTS, render_case(env, poller, pool, case, targets)):
            (frames_dir / f"{case.name}__t{offset}.bin").write_bytes(frame)

    by_name = {case.name: case for case in cases}
    screens = static_screens(env)
    for name, base, publish in screens:
        base_case = by_name[base] if base else None
        if base and base_case is None:
            raise SystemExit(f"{name}: base fixture {base!r} is not in the corpus")
        frames = render_screen(env, poller, pool, base_case, publish, targets)
        for offset, frame in zip(TIME_POINTS, frames):
            (frames_dir / f"{name}__t{offset}.bin").write_bytes(frame)

    (OUT_DIR / "logos.rgb565").write_bytes(pool.blob())
    (OUT_DIR / "manifest.txt").write_text(
        "\n".join(manifest_lines(env, cases, screens, pool)) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    total = (len(cases) + len(screens)) * len(TIME_POINTS)
    print(
        f"wrote {total} frames ({len(cases)} fixtures + {len(screens)} static screens "
        f"x {len(TIME_POINTS)} time points), {pool.count()} crest slots "
        f"-> {OUT_DIR.relative_to(REPO).as_posix()}"
    )


if __name__ == "__main__":
    main()
