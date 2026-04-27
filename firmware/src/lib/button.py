import machine
import micropython
import re
import rp2
import time
from micropython import const
from pio_types import *


_DEFAULT_PIO_INDEX = const(0)
_STATE_MACHINE_OFFSET = const(0)

# SM cycles per counter increment in _button_pio. The class derives the SM
# clock from this and the requested tick_period_ms, so it MUST match the
# inner-loop length of the PIO program (use [delay] padding to hit it exactly).
# Bump in lockstep if you redesign the asm.
_PIO_CYCLES_PER_TICK = const(32)

# FIFO word format pushed by _button_pio on every accepted transition.
# The PIO is polarity-agnostic: it just reports the raw pin level. The
# wrapper class XORs with `active_low` to produce a "pressed" boolean.
_FIFO_STATE_BIT = const(0x80000000)      # MSB: 1=pin HIGH, 0=pin LOW
_FIFO_DURATION_MASK = const(0x7FFFFFFF)  # bits 30..0: ticks of previous state

_PIO_INDEX_EXPRESSION = re.compile(r'PIO\((\d)\)')


@rp2.asm_pio(
        in_shiftdir=rp2.PIO.SHIFT_LEFT
    )
def _button_pio():
    """Debounced button reader with duration counting (SKELETON -- fill me in).

    Contract with the `Button` wrapper class:

    * Pin mapping: `in_base` and `jmp_pin` are both bound to the button's GPIO.
      This program is polarity-agnostic -- it reports raw pin level (HIGH/LOW).
      The wrapper class applies `active_low` to convert to a "pressed" boolean.

    * Inner-loop cycle count: the counter-increment loop body MUST execute in
      exactly `_PIO_CYCLES_PER_TICK` SM cycles. Pad with `[delay]` on
      instructions, or use chained `nop()[N]`, to hit the target exactly. If
      you change the loop length, update `_PIO_CYCLES_PER_TICK` in lockstep --
      the class derives the SM clock from it.

    * Debounce: hardcode via `set(y, n)` where n in [0, 31] is the number of
      stable-input ticks required to accept a transition. With the default
      `tick_period_ms=1`, this is 0-31ms, which covers nearly every realistic
      button. For longer debounce, raise `tick_period_ms` (e.g. 5 -> up to
      155ms hardcoded debounce).

    * FIFO output: on every accepted transition, push one 32-bit word:
        - MSB (bit 31) = new pin level (1 = HIGH, 0 = LOW) -- raw, no inversion
        - bits 30..0   = duration of the *previous* state in counter ticks

    * Initial state: the PIO must sample the pin at startup so its internal
      "current state" matches what the CPU sampled at `__init__`.

    * Counter overflow: open contract decision. Current CPU code assumes the
      31-bit duration field never overflows. With 1ms ticks that's ~24 days
      of idle, so functionally a non-issue at this scale. If you want
      saturation behavior, implement and document here.
    """

    anonymous_label_counter = 0

    def get_anonymous_label() -> str:
        nonlocal anonymous_label_counter
        label = f"_anonymous_{anonymous_label_counter}"
        anonymous_label_counter += 1
        return label

    # Helper to indiscriminately decrement a register
    def saturating_decrement(register: PIORegister, jmp_target: str | None = None) -> "tuple[PIODelayableInstruction, PIODelayableInstruction]":
        target_label = jmp_target or get_anonymous_label()
        first_jmp = jmp(not_x if register == x else not_y, target_label)
        second_jmp = jmp(x_dec if register == x else y_dec, target_label)

        if jmp_target is None:
            label(target_label)

        # Returning instructions so caller can delay or sideset as needed
        return (first_jmp, second_jmp)

    # "edge" means a transition from high to low or vice versa, "stable" means the pin's state is unchanged

    # Debounce length is stored in the OSR immediately since its seeded by CPU
    pull(block)
    wrap_target()
    label("transition_high")
    # y guaranteed to be 0 here in all paths
    mov(y, invert(y))
    label("transition_low")
    # We want to ignore the single LSB so saturation plays nicely and we don't have wrapping discontinuity
    mov(x, reverse(x))
    in_(x, 31)
    # y will be all 1s in the high case, and all 0s in the low case
    in_(y, 1)
    # Unreverse the count bits
    mov(isr, reverse(isr))
    push()
    mov(x, invert(null))
    # y is still all 1s or 0s, we go back to the previous loop with the debounce count reset
    # We don't need to worry about accidentally recurring the not_y jmps because the pin will never be 0 after this decrement
    jmp(y_dec, "high_edge")

    label("low_edge")
    jmp(not_y, "transition_low")
    mov(y, osr)

    label("low_stable")
    jmp(pin, "high_edge")
    saturating_decrement(y)
    jmp(x_dec, "low_stable")
    # If we fall through to this, the pin has stayed the same for a full time cycle
    # We push an event to the FIFO to report a full cycle of stable time
    jmp("transition_low")

    label("high_edge")
    jmp(not_y, "transition_high")
    mov(y, osr)

    label("high_stable")
    jmp(pin, "no_low_edge")
    jmp("low_edge")
    label("no_low_edge")
    saturating_decrement(y)
    jmp(x_dec, "high_stable")
    # If we fall through to this, the pin has stayed the same for a full time cycle
    # We push an event to the FIFO to report a full cycle of stable time
    wrap()


class Button:
    """PIO-backed debounced button reader.

    A PIO state machine watches the GPIO continuously and pushes a single FIFO
    word on every debounced edge, encoding (new state, duration of previous
    state in ticks). The CPU never polls the pin directly.

    `current_state()`, `is_pressed`, and `poll_event()` lazily drain the FIFO
    and reconstruct the live state, so the main loop pays nothing for button
    handling between calls. No IRQs in this iteration.

    All durations exposed by the public API are in milliseconds. `tick_period_ms`
    declares what one PIO counter increment represents in real time; the class
    uses it for both deriving the SM clock (via `_PIO_CYCLES_PER_TICK`) and
    converting FIFO duration ticks back to ms.
    """

    @staticmethod
    @micropython.native
    def _get_pio_index(pio: rp2.PIO) -> int:
        match = _PIO_INDEX_EXPRESSION.match(repr(pio))
        if not match:
            raise ValueError(f"Could not determine PIO index: '{pio!r}'")
        return int(match.group(1))

    @staticmethod
    @micropython.native
    def _get_absolute_state_machine_index(pio_block_index: int, state_machine_offset: int) -> int:
        return pio_block_index * 4 + state_machine_offset

    @micropython.native
    def __init__(
        self,
        *,
        pin: machine.Pin,
        tick_period_ms: int = 1,
        pio: rp2.PIO | None = None,
        active_low: bool = True,
    ):
        # `pin` is taken as-is; the caller is responsible for configuring it
        # as an input with appropriate pull (internal via Pin.PULL_UP/DOWN, or
        # an external resistor on the board).
        self._pin = pin
        self._tick_period_ms = tick_period_ms
        self._active_low = active_low

        # Resolve PIO block (defaults to PIO0)
        self._pio = pio if pio is not None else rp2.PIO(_DEFAULT_PIO_INDEX)
        pio_block_index = self.__class__._get_pio_index(self._pio)

        # SM clock derived from the loop-cycles-per-tick contract. With the
        # defaults (32 cycles/tick, 1ms tick), this is 32 kHz -- well above
        # the RP2040 SM minimum (~1907 Hz) and exactly representable by the
        # divider (125 MHz / 32 kHz = 3906.25, hits cleanly with FRAC=64).
        sm_freq_hz = 1000 * _PIO_CYCLES_PER_TICK // tick_period_ms

        absolute_sm_index = self.__class__._get_absolute_state_machine_index(
            pio_block_index, _STATE_MACHINE_OFFSET
        )
        self._state_machine = rp2.StateMachine(
            absolute_sm_index,
            _button_pio,
            freq=sm_freq_hz,
            in_base=pin,
            jmp_pin=pin
        )

        # Sample the pin once at init so the CPU's notion of "current state"
        # matches what the PIO program will see when it starts running. The
        # PIO reports raw pin level; we XOR with active_low to get "pressed".
        self._is_pressed = bool(pin.value()) ^ active_low

        # Anchor: ticks_ms timestamp at which the current state began.
        # Advanced forward by FIFO-reported durations as transitions are consumed.
        self._anchor_ms = time.ticks_ms()

        self._state_machine.active(1)

    @micropython.native
    def _consume_one(self) -> 'tuple[bool, int] | None':
        sm = self._state_machine
        if not sm.rx_fifo():
            return None
        word = sm.get()
        # PIO reports raw pin level in the MSB; XOR with active_low yields "pressed".
        new_pressed = bool(word & _FIFO_STATE_BIT) ^ self._active_low
        duration_ticks = word & _FIFO_DURATION_MASK
        duration_ms = duration_ticks * self._tick_period_ms
        self._anchor_ms = time.ticks_add(self._anchor_ms, duration_ms)
        self._is_pressed = new_pressed
        return new_pressed, duration_ms

    @micropython.native
    def _drain_fifo(self) -> None:
        while self._consume_one() is not None:
            pass

    @property
    @micropython.native
    def is_pressed(self) -> bool:
        self._drain_fifo()
        return self._is_pressed

    @micropython.native
    def current_state(self) -> 'tuple[bool, int]':
        # Drain first, then timestamp. If we timestamped first and an edge
        # landed between timestamp and drain, the new aggregate could exceed
        # `now` and yield a negative held-for value.
        self._drain_fifo()
        now_ms = time.ticks_ms()
        return self._is_pressed, time.ticks_diff(now_ms, self._anchor_ms)

    @micropython.native
    def poll_event(self) -> 'tuple[bool, int] | None':
        return self._consume_one()

    @micropython.native
    def deinit(self) -> None:
        self._state_machine.active(0)
        # Pass the specific program reference so we don't disturb other drivers
        # sharing this PIO block.
        self._pio.remove_program(_button_pio)
