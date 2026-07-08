"""Golden smoke test: the live-critical-count frame is byte-stable.

Renders `live-critical-count` at a fixed virtual instant (500 ms in, the peak
of the brightness pulse) and hashes the raw RGB565 buffer. A change to the shim,
the fonts, the sprites, or the renderer that alters even one pixel trips this.

If the generated layout/font artifacts are missing the test skips with the
build instruction rather than failing. On a hash mismatch it prints the actual
hash and how to update the constant -- update it only after eyeballing the
gallery output and confirming the change is intended.

    python -m pytest tools/preview/tests/test_golden.py -q
    python tools/preview/tests/test_golden.py
"""

import hashlib
import os
import sys

_REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
if _REPO_ROOT not in sys.path:
    sys.path.insert(0, _REPO_ROOT)

_SCENARIO = "live-critical-count"
_ELAPSED_MS = 500

# sha256 of the raw RGB565 buffer. Update ONLY after confirming an intended
# visual change in the gallery (the test prints the new hash on mismatch).
#
# Updated for the C8 critical-dot red tint: at 500ms (pulse peak) the count
# dots now pack pack_hsv_to_rgb565(0, s>0, v) instead of grayscale (s=0), so
# the dot pixels legitimately changed bytes. Verified against the
# critical-red-tint gallery frames.
_EXPECTED = "b12be533b43b3dfebd081358350f486dce3ab48aff4546d7b9f5b79e7f84a0e1"


class _ArtifactsMissing(Exception):
    pass


def _render_hash() -> str:
    from tools.preview.firmware_env import load_firmware
    from tools.preview.shims.time_shim import VirtualClock
    from tools.preview import render
    from tools.preview.scenarios import ScenarioContext
    from tools.preview.logos import LogoProvider

    clock = VirtualClock()
    try:
        env = load_firmware(clock)
    except SystemExit as exc:
        raise _ArtifactsMissing(str(exc)) from exc

    display, writer, regions = render.build_render_targets(env)
    ctx = ScenarioContext(env, LogoProvider())
    buf = render.render_golden_frame(ctx, display, writer, regions, _SCENARIO, _ELAPSED_MS)
    return hashlib.sha256(buf).hexdigest()


def test_golden_live_critical_count():
    try:
        actual = _render_hash()
    except _ArtifactsMissing as exc:
        try:
            import pytest
            pytest.skip(f"generated artifacts missing: {exc}")
        except ImportError:
            print(f"SKIP: generated artifacts missing: {exc}")
            return
    assert actual == _EXPECTED, (
        f"\ngolden mismatch for {_SCENARIO} @ {_ELAPSED_MS}ms\n"
        f"  actual:   {actual}\n"
        f"  expected: {_EXPECTED}\n"
        f"If this change is intentional, set _EXPECTED to the actual hash."
    )


if __name__ == "__main__":
    try:
        got = _render_hash()
    except _ArtifactsMissing as exc:
        print(f"SKIP: generated artifacts missing: {exc}")
        raise SystemExit(0)
    if got == _EXPECTED:
        print(f"PASS  golden {_SCENARIO} @ {_ELAPSED_MS}ms == {got}")
        raise SystemExit(0)
    print(f"FAIL  golden {_SCENARIO} @ {_ELAPSED_MS}ms\n  actual:   {got}\n  expected: {_EXPECTED}")
    raise SystemExit(1)
