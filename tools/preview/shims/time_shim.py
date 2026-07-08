"""Virtual clock + a `time` module stand-in for the firmware.

The render code reads the clock through `time.ticks_ms()` / `time.ticks_diff`.
Driving those from a `VirtualClock` lets the preview advance time in exact
50 ms steps (the real display tick) so animations are deterministic and a
golden frame can be captured at a fixed virtual instant.

The clock starts at 100_000 ms, comfortably above the `updated_ms == 0`
"never" sentinel the toast/animation code uses, so a freshly-built state with
a zero timestamp still reads as "not yet set".

Anything the shim doesn't define (e.g. `time.time()` used by the firmware
logger, or `strftime` used by Pillow-adjacent code) delegates to the real
`time` module via the module `__getattr__`, so unrelated callers keep working.
"""

import time as _real_time
from types import ModuleType

CLOCK_START_MS = 100_000


class VirtualClock:
    """Monotonic millisecond clock the preview advances by hand."""

    def __init__(self, start_ms: int = CLOCK_START_MS) -> None:
        self.now = start_ms

    def set(self, now_ms: int) -> None:
        self.now = now_ms

    def advance(self, delta_ms: int) -> None:
        self.now += delta_ms


def make_time_module(clock: VirtualClock) -> ModuleType:
    """Build a `time` module backed by `clock`, delegating the rest to real time."""
    mod = ModuleType("time")

    def ticks_ms() -> int:
        return clock.now

    def ticks_diff(a: int, b: int) -> int:
        return a - b

    def ticks_add(t: int, delta: int) -> int:
        return t + delta

    def sleep_ms(_ms: int) -> None:
        # The preview never really sleeps; time only moves via the clock.
        return None

    def sleep(_s) -> None:
        return None

    mod.ticks_ms = ticks_ms
    mod.ticks_diff = ticks_diff
    mod.ticks_add = ticks_add
    mod.ticks_us = lambda: clock.now * 1000
    mod.sleep_ms = sleep_ms
    mod.sleep = sleep
    mod._clock = clock

    def __getattr__(name):
        # Fall back to the real time module for time(), gmtime(), strftime(), ...
        return getattr(_real_time, name)

    mod.__getattr__ = __getattr__
    return mod
