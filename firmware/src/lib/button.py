import machine
import micropython
import rp2
import time
from collections import namedtuple
from micropython import const
from pio_types import *


_DEFAULT_PIO_INDEX = const(0)

# SM cycles per FIFO duration tick. One stable-loop iteration in _button_pio
# is padded to exactly half this (16 cycles, via [3] on each of the 4
# instructions on every loop path); the FIFO word drops the iteration
# counter's LSB, so one reported tick = 2 iterations = 32 cycles. The class
# derives the SM clock from this and tick_period_ms. Bump in lockstep if you
# change the loop padding.
_PIO_CYCLES_PER_TICK = const(32)

# FIFO word format pushed by _button_pio on every accepted transition.
# The PIO is polarity-agnostic: it just reports the raw pin level. The
# wrapper class XORs with `active_low` to produce a "pressed" boolean.
_FIFO_STATE_BIT = const(0x80000000)      # MSB: 1=pin HIGH, 0=pin LOW
_FIFO_DURATION_MASK = const(0x7FFFFFFF)  # bits 30..0: ticks of previous state

# The single public data type: one debounced edge.
#   pressed:  the debounced state AFTER this edge (`active_low` already applied)
#   ticks_ms: when that state began, as a point on the device-global
#             time.ticks_ms() timeline. Compare only via time.ticks_diff /
#             time.ticks_add (the counter wraps; pairs are valid < ~6.2 days).
ButtonEvent = namedtuple("ButtonEvent", ("pressed", "ticks_ms"))

# Shared idle result: read() returns this exact object whenever no events are
# pending, so the steady-state polling path allocates nothing.
_NO_EVENTS = ()


@rp2.asm_pio(
        in_shiftdir=rp2.PIO.SHIFT_LEFT
    )
def _button_pio():
    """Debounced button reader with duration counting.

    Contract with the `Button` wrapper class:

    * Pin mapping: `in_base` and `jmp_pin` are both bound to the button's GPIO.
      This program is polarity-agnostic -- it reports raw pin level (HIGH/LOW).
      The wrapper class applies `active_low` to convert to a "pressed" boolean.

    * Timing: every path through a stable loop is exactly 4 instructions, each
      carrying [3] delay -> 16 cycles per iteration, one x decrement per
      iteration. The FIFO duration drops the counter LSB, so one reported tick
      = 2 iterations = `_PIO_CYCLES_PER_TICK` (32) SM cycles. The two loop
      paths (debounce counting vs. saturated) are equalized by routing the
      saturated case through a nop shim -- see `saturating_decrement`. The pin
      is sampled once per iteration, i.e. every half tick.

    * Debounce: the reload value is seeded by the CPU into the OSR via the
      initial blocking pull (`Button.__init__` does `sm.put()`). Units are
      loop iterations = HALF ticks; the class doubles its ms-derived value.
      An edge is accepted only if the *previous* state was held for the full
      debounce window (y counted down to 0 = "armed"), so an accepted edge
      fires on the very sample it is seen -- zero added latency -- and the
      post-edge bounce tail is rejected because every rejected crossing
      reloads y. Consequences (accepted trade-offs, see class docstring):
      a press or release shorter than the debounce window is swallowed and
      surfaces as a same-state event on the next accepted edge.

    * FIFO output: on every accepted transition, push one 32-bit word:
        - MSB (bit 31) = new pin level (1 = HIGH, 0 = LOW) -- raw, no inversion
        - bits 30..0   = duration of the *previous* state in counter ticks
          (floor(iterations/2); x is complemented before packing so this is
          an up-count). Durations span push-to-push, so bounce excursions are
          included in the previous state's duration. Each transition's
          transit path is unmeasured: error < 1 tick per event.

    * Initial state: no event is pushed at startup. The dispatch lands in the
      low loop (y resets to 0), and if the pin is actually high the first
      sample routes into the high loop without pushing -- so the PIO's notion
      of current state converges to the real pin level within ~2 iterations,
      matching what the CPU sampled in `__init__`.

    * Counter overflow: after 2^32 iterations (~24.8 days at 1ms ticks) of an
      unbroken state, x wraps and a same-state "saturation" event is pushed
      whose duration field decodes to 0 (indistinguishable from a fresh
      count -- accepted quirk at this scale). The cycle then restarts cleanly.
    """

    anonymous_label_counter = 0

    def get_anonymous_label() -> str:
        nonlocal anonymous_label_counter
        label = f"_anonymous_{anonymous_label_counter}"
        anonymous_label_counter += 1
        return label

    # Constant-time saturating decrement: register = max(register - 1, 0).
    # The zero case lands on a nop shim that falls through to the same join
    # point, so BOTH paths are exactly two instructions of (1 + delay) cycles.
    # (Without the shim the zero path is one instruction shorter, and because
    # the first jmp executes in both paths no delay assignment can ever
    # equalize them -- that skew was the original timing bug.)
    def saturating_decrement(register: PIORegister, delay: int = 0) -> None:
        zero_label = get_anonymous_label()
        join_label = get_anonymous_label()
        jmp(not_x if register == x else not_y, zero_label)  [delay]
        # Register is known nonzero here, so this always branches.
        jmp(x_dec if register == x else y_dec, join_label)  [delay]
        label(zero_label)
        nop()                                               [delay]
        label(join_label)

    # "edge" means a transition from high to low or vice versa, "stable" means the pin's state is unchanged

    # Debounce length is stored in the OSR immediately since its seeded by CPU
    pull(block)
    wrap_target()
    # Initial entry and post-push routing share this instruction: at startup
    # x is 0 and needs initialization; after a push x is intentionally reset.
    mov(x, invert(null))
    # y is still all 1s or 0s, we go back to the previous loop with the debounce count reset
    # We don't need to worry about accidentally recurring the not_y jmps because y will never be 0 after this decrement
    jmp(y_dec, "high_edge")

    label("low_edge")
    jmp(not_y, "transition_low")
    mov(y, osr)

    label("low_stable")
    jmp(pin, "high_edge")           [3]
    saturating_decrement(y, 3)
    jmp(x_dec, "low_stable")        [3]
    # If we fall through to this, the pin has stayed the same for a full time cycle
    # We push an event to the FIFO to report a full cycle of stable time
    jmp("transition_low")

    label("high_edge")
    jmp(not_y, "transition_high")
    mov(y, osr)

    label("high_stable")
    jmp(pin, "no_low_edge")         [3]
    jmp("low_edge")
    label("no_low_edge")
    saturating_decrement(y, 3)
    jmp(x_dec, "high_stable")       [3]
    # If we fall through to this, the pin has stayed the same for a full time cycle.
    # Fall through into transition_high to push the saturation event (saves a jmp).
    label("transition_high")
    # y guaranteed to be 0 here in all paths
    mov(y, invert(y))
    label("transition_low")
    # x down-counted from all 1s, so complementing turns it into elapsed iterations
    mov(x, invert(x))
    # We want to ignore the single LSB so saturation plays nicely and we don't have wrapping discontinuity
    mov(x, reverse(x))
    in_(x, 31)
    # y will be all 1s in the high case, and all 0s in the low case
    in_(y, 1)
    # Unreverse the count bits
    mov(isr, reverse(isr))
    push()
    wrap()


class Button:
    """PIO-backed debounced button: an events-only, lowest-level primitive.

    A PIO state machine watches the GPIO continuously and pushes a FIFO word
    on every debounced edge. The CPU never polls the pin directly, and this
    class makes no claims about "now" -- it asserts only past facts:

    * `initial` -- ButtonEvent captured at construction: the boundary
      condition (state + time seed) that grounds the event stream.
    * `read()`  -- drains the hardware FIFO and returns the new events as a
      tuple of ButtonEvent (the shared empty tuple when idle: zero-alloc).

    Everything else is a fold, and belongs to the consumer/composition layer:

        state = btn.initial
        ...
        for ev in btn.read():
            handle_edge(state, ev)          # duration = ticks_diff(ev.ticks_ms, state.ticks_ms)
            state = ev                      # current pressed = state.pressed

    Contract details:

    * Events are as fresh as the last `read()` -- an edge that happened but
      was not yet read does not exist for the consumer.
    * Same-state events are real and meaningful: a press or release shorter
      than `debounce_ms` is "swallowed" (its pairing edge was rejected), and
      surfaces as two consecutive events with the same `pressed` value. Do
      not assume alternation. (The PIO's ~24.8-day counter-rollover marker is
      the one same-state artifact that is pure implementation detail; `read()`
      filters it out internally.)
    * `ticks_ms` values obey the time.ticks_ms algebra ONLY: ticks_diff /
      ticks_add, pairs valid within ~6.2 days.
    * The hardware FIFO holds 4 events (2 full press+release cycles). When
      full the PIO blocks -- events are never dropped -- but time spent
      blocked is not counted, skewing subsequent timestamps. Under sustained
      input, call `read()` at least every ~4x debounce_ms.
    * `tick_period_ms` (what one FIFO duration tick means in real time) must
      stay in [1, 16]: the SM clock is 32 kHz / tick_period_ms and the RP2040
      divider bottoms out near 1907 Hz.
    """

    @micropython.native
    def __init__(
        self,
        *,
        pin: machine.Pin,
        tick_period_ms: int = 1,
        debounce_ms: int = 20,
        pio: rp2.PIO | None = None,
        sm_offset: int = 0,
        active_low: bool = True,
    ):
        # `pin` is taken as-is; the caller is responsible for configuring it
        # as an input with appropriate pull (internal via Pin.PULL_UP/DOWN, or
        # an external resistor on the board).
        self._pin = pin
        self._tick_period_ms = tick_period_ms
        self._active_low = active_low

        if tick_period_ms < 1:
            raise ValueError("tick_period_ms must be >= 1")
        # The PIO's debounce counter decrements once per loop iteration, and
        # there are 2 iterations per tick (the FIFO drops the counter LSB).
        # reload >= 2 (i.e. debounce >= one tick) is required: below that a
        # real event could decode to duration 0 and be mistaken for the
        # rollover marker read() filters on.
        debounce_reload = (2 * debounce_ms) // tick_period_ms
        if not 2 <= debounce_reload < (1 << 30):
            raise ValueError("debounce_ms out of range for this tick period")
        self._debounce_reload = debounce_reload

        # Resolve PIO block (defaults to PIO0). The block's own factory
        # constructs the SM relative to it, so we never need to recover a
        # block index from the object.
        self._pio = pio if pio is not None else rp2.PIO(_DEFAULT_PIO_INDEX)

        # SM clock derived from the loop-cycles-per-tick contract. With the
        # defaults (32 cycles/tick, 1ms tick), this is 32 kHz -- well above
        # the RP2040 SM minimum (~1907 Hz) and exactly representable by the
        # divider (125 MHz / 32 kHz = 3906.25, hits cleanly with FRAC=64).
        sm_freq_hz = 1000 * _PIO_CYCLES_PER_TICK // tick_period_ms

        self._state_machine = self._pio.state_machine(
            sm_offset,
            _button_pio,
            freq=sm_freq_hz,
            in_base=pin,
            jmp_pin=pin
        )

        # Seed the debounce reload value the program's initial blocking pull
        # is waiting for. Must happen before (or at any point after) the SM is
        # activated -- the TX FIFO holds it either way.
        self._state_machine.put(debounce_reload)

        # The boundary condition that grounds the event stream: the fold seed
        # for both state (sampled pin level, matching what the PIO converges
        # to on startup) and time. Immutable; consumers fold from here.
        self.initial = ButtonEvent(bool(pin.value()) ^ active_low, time.ticks_ms())

        # Private fold state mirroring the consumer's: the anchor advances by
        # FIFO durations to place events on the ticks_ms timeline, and the
        # last emitted state identifies rollover markers.
        self._anchor_ms = self.initial.ticks_ms
        self._last_pressed = self.initial.pressed

        self._state_machine.active(1)

    @micropython.native
    def read(self) -> 'tuple[ButtonEvent, ...]':
        """Drain the hardware FIFO; return the new debounced edges, oldest first.

        Returns the shared empty tuple when nothing is pending (identity-equal
        across calls -- the idle path allocates nothing).
        """
        sm = self._state_machine
        if not sm.rx_fifo():
            return _NO_EVENTS

        events = []
        anchor = self._anchor_ms
        last_pressed = self._last_pressed
        tick_ms = self._tick_period_ms
        active_low = self._active_low
        while sm.rx_fifo():
            word = sm.get()
            # PIO reports raw pin level in the MSB; XOR with active_low yields "pressed".
            pressed = bool(word & _FIFO_STATE_BIT) ^ active_low
            duration_ticks = word & _FIFO_DURATION_MASK
            anchor = time.ticks_add(anchor, duration_ticks * tick_ms)
            if duration_ticks == 0 and pressed == last_pressed:
                # Counter-rollover marker (~24.8 days of one unbroken state).
                # Unique signature: real events always span >= one debounce
                # window (reload >= 2 enforced in __init__). Implementation
                # detail -- consumers never see it.
                continue
            last_pressed = pressed
            events.append(ButtonEvent(pressed, anchor))
        self._anchor_ms = anchor
        self._last_pressed = last_pressed
        return tuple(events) if events else _NO_EVENTS

    @micropython.native
    def deinit(self) -> None:
        self._state_machine.active(0)
        # Pass the specific program reference so we don't disturb other
        # programs on this PIO block. NOTE: this removes _button_pio from the
        # block's instruction memory -- if another Button shares this block,
        # deinit it last (or not at all).
        self._pio.remove_program(_button_pio)
