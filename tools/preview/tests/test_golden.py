"""Golden smoke tests: one byte-stable frame per sport's live screen.

Each golden renders a scenario at a fixed virtual instant and hashes the raw
RGB565 buffer. A change to the shim, the fonts, the sprites, or the renderer
that alters even one pixel trips it.

- `live-critical-count` @ 500 ms — MLB live, the peak of the count-dot pulse.
- `soccer-live-stoppage` @ 500 ms — soccer live (variant A), stoppage clock
  in the warning color plus the last-event strip.

If the generated layout/font artifacts are missing the tests skip with the
build instruction rather than failing. On a hash mismatch the actual hash is
printed -- update the constant only after eyeballing the gallery output and
confirming the change is intended.

    python -m pytest tools/preview/tests/test_golden.py -q
    python tools/preview/tests/test_golden.py
"""

import hashlib
import os
import sys

_REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
if _REPO_ROOT not in sys.path:
    sys.path.insert(0, _REPO_ROOT)

# sha256 of the raw RGB565 buffer per (scenario, elapsed_ms). Update ONLY
# after confirming an intended visual change in the gallery (a mismatch
# prints the new hash).
#
# live-critical-count history: updated for the C8 critical-dot red tint —
# at 500ms (pulse peak) the count dots pack pack_hsv_to_rgb565(0, s>0, v)
# instead of grayscale; verified against the critical-red-tint gallery.
_GOLDENS = [
    ("live-critical-count", 500,
     "a7b1fa409e3e45081fd4e835db034eba7621b92a6f08b3c89a925a8edd97f31e"),
    ("soccer-live-stoppage", 500,
     "5cd32164d34ff0f5a549150151987eb6d67e5e71a65675a95645ceed20032ac6"),
]


class _ArtifactsMissing(Exception):
    pass


_ENV_CACHE = {}


def _render_hash(scenario: str, elapsed_ms: int) -> str:
    from tools.preview.firmware_env import load_firmware
    from tools.preview.shims.time_shim import VirtualClock
    from tools.preview import render
    from tools.preview.scenarios import ScenarioContext
    from tools.preview.logos import LogoProvider

    if not _ENV_CACHE:
        clock = VirtualClock()
        try:
            env = load_firmware(clock)
        except SystemExit as exc:
            raise _ArtifactsMissing(str(exc)) from exc
        display, writer, regions = render.build_render_targets(env)
        _ENV_CACHE["ctx"] = ScenarioContext(env, LogoProvider())
        _ENV_CACHE["targets"] = (display, writer, regions)

    ctx = _ENV_CACHE["ctx"]
    display, writer, regions = _ENV_CACHE["targets"]
    from tools.preview import render
    buf = render.render_golden_frame(ctx, display, writer, regions, scenario, elapsed_ms)
    return hashlib.sha256(buf).hexdigest()


def _check(scenario: str, elapsed_ms: int, expected: str) -> str | None:
    """Returns None on pass, an error message on mismatch."""
    actual = _render_hash(scenario, elapsed_ms)
    if actual == expected:
        return None
    return (
        f"golden mismatch for {scenario} @ {elapsed_ms}ms\n"
        f"  actual:   {actual}\n"
        f"  expected: {expected}\n"
        f"If this change is intentional, update _GOLDENS with the actual hash."
    )


def test_goldens():
    try:
        failures = [msg for s, ms, exp in _GOLDENS if (msg := _check(s, ms, exp))]
    except _ArtifactsMissing as exc:
        try:
            import pytest
            pytest.skip(f"generated artifacts missing: {exc}")
        except ImportError:
            print(f"SKIP: generated artifacts missing: {exc}")
            return
    assert not failures, "\n\n".join(failures)


if __name__ == "__main__":
    try:
        results = [(s, ms, _check(s, ms, exp)) for s, ms, exp in _GOLDENS]
    except _ArtifactsMissing as exc:
        print(f"SKIP: generated artifacts missing: {exc}")
        raise SystemExit(0)
    status = 0
    for scenario, elapsed_ms, msg in results:
        if msg is None:
            print(f"PASS  golden {scenario} @ {elapsed_ms}ms")
        else:
            print(f"FAIL  {msg}")
            status = 1
    raise SystemExit(status)
