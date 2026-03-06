from typing import Final, Iterable, Literal, overload, Set, TypeVar

# ----- PIO TypeVars -----
T = TypeVar("T")

# ----- PIO Core Instruction Types -----
class PIODelayableInstruction:
    """A PIO instruction that can have delay cycles appended.

    All PIO instructions support a delay via the Delay/side-set field (bits 12..8).
    The number of idle cycles inserted between this instruction and the next is
    encoded in up to 5 LSBs of this field (the exact split between side-set bits
    and delay bits is configured per state machine by ``PINCTRL_SIDESET_COUNT``).

    Usage::

        instr[n]   # Insert n delay cycles after this instruction (0..31 max,
                   # actual max depends on side-set configuration)

    Delay cycles always take effect whether the instruction's own condition is
    true or false (e.g. JMP delay happens regardless of branch outcome). For
    stalling instructions (WAIT, PUSH block, PULL block), delay cycles begin
    *after* the stall condition is resolved.

    All PIO instructions execute in one clock cycle (plus any delay cycles).
    """
    def __getitem__(self, delay: int):
        """Append ``delay`` idle cycles after this instruction.

        Args:
            delay: Number of idle cycles to insert (0 to max, where max depends
                on how many bits are allocated to side-set vs delay in the
                Delay/side-set field). With no side-set pins configured, up to
                5 bits = 31 delay cycles. Each side-set pin used reduces the
                max delay by a factor of 2.
        """
        ...

class PIOInstruction(PIODelayableInstruction):
    """A PIO instruction that supports both side-set and delay.

    Side-set optionally asserts a constant value onto some GPIOs *concurrently*
    with the main instruction execution logic. The side-set value is encoded in
    the MSBs of the Delay/side-set field (bits 12..8).

    Side-set pin mapping is configured independently from OUT/SET/IN pin mappings
    via ``PINCTRL_SIDESET_BASE`` and ``PINCTRL_SIDESET_COUNT``.
    """
    def side(self, value: int) -> PIODelayableInstruction:
        """Apply a side-set operation concurrently with this instruction.

        Asserts ``value`` onto the side-set pins at the same time as the main
        instruction executes. The number of pins driven is determined by
        ``PINCTRL_SIDESET_COUNT``. The base pin is ``PINCTRL_SIDESET_BASE``.

        Args:
            value: The value to drive onto the side-set pins. The number of
                valid bits depends on ``PINCTRL_SIDESET_COUNT``.

        Returns:
            A ``PIODelayableInstruction`` -- you can still chain ``.side(v)[delay]``
            to apply both side-set and delay to the same instruction.
        """
        ...

# ----- PIO Type Categories -----
class PIOMoveOperable:
    """A source that can be used with MOV and can also be passed through
    ``invert()`` or ``reverse()`` operations.

    MOV sources in this category: ``pins``, ``x``, ``y``, ``null``, ``status``,
    ``isr``, ``osr``.
    """
    ...

class PIOMoveOperated:
    """The result of applying ``invert()`` or ``reverse()`` to a ``PIOMoveOperable``.

    Can be used as a MOV source but cannot be further operated on.
    """
    ...

class PIOMoveTarget:
    """A valid destination for the MOV instruction.

    MOV destinations: ``pins``, ``x``, ``y``, ``exec``, ``pc``, ``isr``, ``osr``.
    """
    ...

class PIOInSource:
    """A valid source for the IN instruction.

    IN sources: ``pins`` (000), ``x`` (001), ``y`` (010), ``null`` (011),
    ``isr`` (110), ``osr`` (111).
    """
    ...

class PIOOutTarget:
    """A valid destination for the OUT instruction.

    OUT destinations: ``pins`` (000), ``x`` (001), ``y`` (010), ``null`` (011),
    ``pindirs`` (100), ``pc`` (101), ``isr`` (110), ``exec`` (111).
    """
    ...

class PIOSetTarget:
    """A valid destination for the SET instruction.

    SET destinations: ``pins`` (000), ``x`` (001), ``y`` (010), ``pindirs`` (100).
    """
    ...

class PIOJumpCondition:
    """A condition code for the JMP instruction.

    JMP evaluates the condition and branches to the target address if true.
    If no condition is specified, the branch is always taken (condition 000).

    Available conditions: ``not_x``, ``x_dec``, ``not_y``, ``y_dec``,
    ``x_not_y``, ``pin``, ``not_osre``.
    """
    ...

class PIOWaitSource:
    """A source type for the WAIT instruction, specifying what to wait on.

    WAIT sources: ``gpio`` (00), ``pin`` (01), ``irq`` (10).
    """
    ...

class PIOPushPullModifier:
    """A modifier for PUSH or PULL instructions.

    Modifiers control conditional execution and blocking behavior:

    - ``iffull`` / ``ifempty``: Make the operation conditional on shift count threshold.
    - ``block`` / ``noblock``: Control whether to stall on a full/empty FIFO.
    """
    ...

class PIOIRQModifier:
    """A modifier for the IRQ instruction.

    Available modifier: ``clear`` -- clears the IRQ flag instead of raising it.
    """
    ...

PIOMoveSource = PIOMoveOperable | PIOMoveOperated
"""A valid source for MOV: either a raw operable or the result of invert()/reverse()."""

PIOJumpTarget = int | str
"""A JMP target: either an integer instruction address or a string program label."""

# ----- PIO Register/Pin Classes -----
class PIORegister(PIOMoveOperable, PIOMoveTarget, PIOOutTarget, PIOInSource, PIOSetTarget):
    """A PIO scratch register (``x`` or ``y``).

    The RP2040 PIO has two 32-bit scratch registers per state machine:

    - **x** (scratch register X): General-purpose. Can be used as a loop counter,
      temporary storage, or data source/destination. Encoded as source/dest 001.
    - **y** (scratch register Y): General-purpose. Same capabilities as X.
      Encoded as source/dest 010.

    Valid for: MOV (source & dest), IN (source), OUT (dest), SET (dest, 5 LSBs
    set to value, all others cleared to 0).

    When used with SET, only the 5 LSBs are written (values 0-31); all other
    bits are cleared to 0.
    """
    ...

class PIOPins(PIOMoveOperable, PIOMoveTarget, PIOOutTarget, PIOInSource, PIOSetTarget):
    """PIO pins (encoded as 000 in source/destination fields).

    Pin mapping is configured independently for different instruction types:

    - **IN**: Reads from pins starting at ``PINCTRL_IN_BASE``. Each successive
      bit comes from a higher-numbered pin, wrapping after 31.
    - **OUT**: Writes to pins using the OUT pin mapping (``PINCTRL_OUT_BASE``,
      ``PINCTRL_OUT_COUNT``).
    - **SET**: Writes to pins using the SET pin mapping (``PINCTRL_SET_BASE``,
      ``PINCTRL_SET_COUNT``). SET and OUT may map to distinct or overlapping
      pin ranges.
    - **MOV dst, pins**: Reads using the IN pin mapping and writes the full
      32-bit value without masking.
    - **MOV pins, src**: Writes using the OUT pin mapping (same as OUT).

    Valid for: MOV (source & dest), IN (source), OUT (dest), SET (dest).
    """
    ...

class PIOPinDirs(PIOOutTarget, PIOSetTarget):
    """PIO pin direction control (encoded as 100 in destination fields).

    Controls whether pins are configured as inputs (0) or outputs (1).
    Uses the same pin mapping as the instruction type:

    - **OUT pindirs**: Uses the OUT pin mapping.
    - **SET pindirs**: Uses the SET pin mapping.

    Valid for: OUT (dest), SET (dest).
    """
    ...

class PIOProgramCounter(PIOMoveTarget, PIOOutTarget):
    """PIO program counter (encoded as 101 in destination fields).

    - **MOV pc, src**: Causes an unconditional jump to the address in Source.
    - **OUT pc, count**: Behaves as an unconditional jump to an address shifted
      out from the OSR.

    Valid for: MOV (dest), OUT (dest). Not a valid source.
    """
    ...

class PIOInputShiftRegister(PIOOutTarget, PIOMoveOperable, PIOInSource, PIOMoveTarget):
    """PIO Input Shift Register (ISR), encoded as 110.

    The ISR accumulates input data via the IN instruction. Shift direction is
    configured per state machine by ``SHIFTCTRL_IN_SHIFTDIR``.

    - **IN isr, count**: Shifts ISR's own contents (useful for extending bit patterns).
    - **OUT isr, count**: Also sets the ISR shift counter to Bit count.
    - **MOV isr, src**: Copies source to ISR; input shift counter is reset to 0
      (i.e. ISR becomes empty).
    - **MOV dest, isr**: Copies ISR contents to destination.
    - **PUSH**: Pushes ISR contents to RX FIFO and clears ISR to all-zeroes.

    Valid for: MOV (source & dest), IN (source), OUT (dest).
    """
    ...

class PIOOutputShiftRegister(PIOMoveOperable, PIOInSource, PIOMoveTarget):
    """PIO Output Shift Register (OSR), encoded as 111.

    The OSR holds data to be shifted out via the OUT instruction. Shift direction
    is configured by ``SHIFTCTRL_OUT_SHIFTDIR``.

    - **IN osr, count**: Shifts OSR contents into ISR (useful for loopback/inspection).
    - **MOV osr, src**: Copies source to OSR; output shift counter is reset to 0
      (i.e. OSR becomes full).
    - **MOV dest, osr**: Copies OSR contents to destination.
    - **PULL**: Loads a 32-bit word from TX FIFO into OSR.

    When autopull is enabled, any PULL is a no-op when the OSR is full, so PULL
    behaves as a barrier. Use ``OUT null, 32`` to explicitly discard OSR contents.

    Valid for: MOV (source & dest), IN (source).
    """
    ...

class PIONull(PIOMoveOperable, PIOMoveTarget, PIOInSource, PIOOutTarget):
    """PIO null source/destination (encoded as 011).

    - **As source (IN, MOV)**: Produces all zeroes. Useful for shifting zero bits
      into the ISR for alignment. For example, after 8 ``IN pins, 1`` instructions,
      an ``IN null, 24`` will shift in 24 zero bits to align data at ISR bits 7..0.
    - **As destination (OUT, MOV)**: Discards data. ``OUT null, count`` can be used
      to discard bits from the OSR.
    - **MOV dest, null**: The value is all-ones or all-zeroes depending on STATUS
      configuration... (Note: MOV dest 3 is 'Reserved' in hardware, but MicroPython
      allows it.)

    Valid for: MOV (source & dest), IN (source), OUT (dest).
    """
    ...

class PIOStatus(PIOMoveOperable):
    """PIO STATUS source (encoded as source 101 for MOV).

    The STATUS source has a value of all-ones or all-zeroes, depending on some
    state machine status such as FIFO full/empty, configured by
    ``EXECCTRL_STATUS_SEL``.

    This is useful for conditional branching based on FIFO state: MOV the STATUS
    into a scratch register, then JMP based on that register being zero/non-zero.

    Valid for: MOV (source only). Not a valid destination.
    """
    ...

class PIOExec(PIOMoveTarget, PIOOutTarget):
    """PIO EXEC destination (encoded as destination 100 for MOV, 111 for OUT).

    Allows register contents or shifted-out data to be executed as an instruction:

    - **MOV exec, src**: The MOV itself executes in 1 cycle, and the instruction
      from Source is executed on the *next* cycle. Delay cycles on MOV EXEC are
      ignored, but the executee may insert delay cycles as normal.
    - **OUT exec, count**: The OUT itself executes on one cycle, and the
      instruction from the OSR is executed on the next cycle. There are no
      restrictions on the types of instructions which can be executed. Delay
      cycles on the initial OUT are ignored, but the executee may insert delay
      cycles as normal.

    Valid for: MOV (dest), OUT (dest). Not a valid source.
    """
    ...

class PIOGpio(PIOWaitSource):
    """WAIT source: system GPIO (encoded as source 00 for WAIT).

    ``WAIT polarity gpio index`` waits on the system GPIO input selected by
    ``index``. This is an *absolute* GPIO index and is **not** affected by the
    state machine's input IO mapping.

    Valid for: WAIT (source only).
    """
    ...

class PIOPin(PIOJumpCondition, PIOWaitSource):
    """PIO input pin, used as a JMP condition and WAIT source.

    **As JMP condition (condition 110)**:
    Branches on the GPIO selected by ``EXECCTRL_JMP_PIN``, a configuration field
    which selects one out of the maximum of 32 GPIO inputs visible to a state
    machine, independently of the state machine's other input mapping. The branch
    is taken if the GPIO is high.

    **As WAIT source (source 01)**:
    ``WAIT polarity pin index`` waits on the input pin selected by ``index``.
    The state machine's input IO mapping is applied first, and then ``index``
    selects which of the mapped bits to wait on. In other words, the pin is
    selected by adding ``index`` to the ``PINCTRL_IN_BASE`` configuration,
    modulo 32.

    Valid for: JMP (condition), WAIT (source).
    """
    ...

class PIOIRQ(PIOWaitSource):
    """PIO IRQ flag, used as a WAIT source and as a callable instruction.

    **As WAIT source (source 10)**:
    ``WAIT polarity irq index`` waits on the PIO IRQ flag selected by ``index``.

    WAIT x IRQ behaves slightly differently from other WAIT sources:
    - If Polarity is 1, the selected IRQ flag is *cleared* by the state machine
      upon the wait condition being met.
    - The flag index is decoded the same way as the IRQ index field: if the MSB
      is set (i.e. ``rel()`` is used), the state machine ID (0..3) is added to
      the IRQ index via modulo-4 addition on the two LSBs.

    CAUTION: ``WAIT 1 IRQ x`` should not be used with IRQ flags presented to the
    interrupt controller, to avoid a race condition with a system interrupt handler.

    **As instruction** (``irq(...)``):
    Sets or clears the IRQ flag selected by the index argument. See the ``irq``
    instruction function for full details.

    Valid for: WAIT (source), IRQ instruction.
    """
    @overload
    def __call__(self, index: int) -> PIOInstruction:
        """Set (raise) the IRQ flag -- no wait (encoding: Clr=0, Wait=0).

        Equivalent to ``irq set`` / ``irq nowait`` in PIO assembler syntax.
        Sets the IRQ flag selected by ``index`` and continues immediately to
        the next instruction without waiting for the flag to be acknowledged.

        Args:
            index: IRQ flag index (0-7). Use ``rel(n)`` for relative addressing:
                if the MSB is set, the state machine ID (0..3) is added to the
                IRQ index via modulo-4 addition on the two LSBs. This allows
                multiple state machines running the same program to use distinct
                IRQ flags.

        Example::

            irq(0)          # Set IRQ flag 0
            irq(rel(0))     # Set this state machine's own relative IRQ flag
        """
        ...
    @overload
    def __call__(self, modifier: PIOIRQModifier, index: int) -> PIOInstruction:
        """Clear an IRQ flag (encoding: Clr=1).

        The only valid ``PIOIRQModifier`` is ``clear``. When Clear is set, the
        flag selected by ``index`` is cleared instead of raised, and the Wait
        bit has no effect.

        Args:
            modifier: Must be ``clear``.
            index: IRQ flag index (0-7), or ``rel(n)`` for relative addressing.

        Example::

            irq(clear, 0)       # Clear IRQ flag 0
            irq(clear, rel(0))  # Clear this SM's own relative IRQ flag
        """
        ...
    @overload
    def __call__(self, modifier: PIOPushPullModifier, index: int) -> PIOInstruction:
        """Set (raise) an IRQ flag with explicit wait/no-wait control.

        Valid modifiers for IRQ are ``block`` and ``noblock``:

        - ``irq(block, index)``: **Set and wait** (Clr=0, Wait=1). Sets the IRQ
          flag and then stalls until the raised flag is lowered again, e.g. by a
          system interrupt handler acknowledging it. Delay cycles do not begin
          until after the wait period elapses. Equivalent to ``irq wait`` in
          PIO assembler syntax.
        - ``irq(noblock, index)``: **Set, no wait** (Clr=0, Wait=0). Sets the
          IRQ flag and continues immediately. Equivalent to ``irq(index)`` /
          ``irq nowait`` in PIO assembler syntax.

        Note: ``iffull`` and ``ifempty`` are not valid here -- only ``block``
        and ``noblock`` apply to the IRQ instruction.

        Args:
            modifier: ``block`` to set-and-wait, or ``noblock`` to set-and-continue.
            index: IRQ flag index (0-7), or ``rel(n)`` for relative addressing.

        Example::

            irq(block, 4)        # Set IRQ 4 and wait for acknowledgement
            irq(noblock, rel(0)) # Set relative IRQ, don't wait
            irq(block, rel(0))   # Set relative IRQ and wait (synchronize SMs)
        """
        ...

# ----- PIO Register/Pin Instances -----
x: Final[PIORegister]
"""Scratch register X (encoded as 001).

A 32-bit general-purpose register. Common uses:

- **Loop counter**: ``set(x, n)`` to initialize, then ``jmp(x_dec, label)``
  to loop. JMP X-- always decrements; branch is taken if X was non-zero
  *before* the decrement.
- **Temporary storage**: ``mov(x, pins)`` to snapshot pin state.
- **Data manipulation**: ``in_(x, count)`` to shift X into ISR,
  ``out(x, count)`` to load from OSR into X.

Valid in: JMP conditions (``not_x``, ``x_dec``, ``x_not_y``), MOV (source & dest),
IN (source), OUT (dest), SET (dest, only 5 LSBs written, rest cleared).
"""

y: Final[PIORegister]
"""Scratch register Y (encoded as 010).

A 32-bit general-purpose register. Identical capabilities to X.

- **Loop counter**: ``set(y, n)`` to initialize, then ``jmp(y_dec, label)``.
  JMP Y-- always decrements; branch is taken if Y was non-zero *before*
  the decrement.
- **Temporary storage / comparison**: ``jmp(x_not_y, label)`` branches if X != Y.

Valid in: JMP conditions (``not_y``, ``y_dec``, ``x_not_y``), MOV (source & dest),
IN (source), OUT (dest), SET (dest, only 5 LSBs written, rest cleared).
"""

pins: Final[PIOPins]
"""PIO pins (encoded as 000).

Pin mapping varies by instruction type -- IN, OUT, SET, and MOV each have
independently configured base pins and pin counts via PINCTRL registers.
See ``PIOPins`` class docstring for full details on per-instruction mapping.
"""

pindirs: Final[PIOPinDirs]
"""PIO pin direction control (encoded as destination 100).

Sets pin direction: 0 = input, 1 = output. Uses the OUT or SET pin mapping
depending on the instruction. For example::

    set(pindirs, 1)    # Set pin to output (using SET pin mapping)
    out(pindirs, 4)    # Set 4 pin directions from OSR (using OUT pin mapping)
"""

pc: Final[PIOProgramCounter]
"""PIO program counter (encoded as destination 101).

Writing to PC causes an unconditional jump::

    mov(pc, x)         # Jump to address stored in scratch register X
    out(pc, 5)         # Jump to address shifted out of OSR (5 bits)

OUT PC behaves as an unconditional jump to an address shifted out from the OSR.
"""

isr: Final[PIOInputShiftRegister]
"""PIO Input Shift Register (encoded as 110).

Accumulates input data via the IN instruction. Shift direction is configured
per state machine by ``SHIFTCTRL_IN_SHIFTDIR``.

- IN always uses the least significant ``bit_count`` bits of the source data.
- The ISR is shifted left or right to make room, then input data is copied
  into the gap. Bit order of input data is independent of shift direction.
- PUSH transfers ISR contents to RX FIFO and clears ISR to all-zeroes.
- If autopush is enabled, IN will also push when the push threshold
  (``SHIFTCTRL_PUSH_THRESH``) is reached. The state machine stalls if the
  RX FIFO is full during an automatic push.

When used as MOV destination, the input shift counter is reset to 0 (empty).
When used as OUT destination, the ISR shift counter is set to Bit count.
"""

osr: Final[PIOOutputShiftRegister]
"""PIO Output Shift Register (encoded as 111).

Holds data to be shifted out via OUT. Loaded from TX FIFO via PULL.

- OUT shifts ``bit_count`` bits out of the OSR to the destination. The lower
  bits come from the OSR and the remainder are zeroes (or most significant,
  depending on ``SHIFTCTRL_OUT_SHIFTDIR``).
- If autopull is enabled, the OSR is automatically refilled from TX FIFO when
  the pull threshold (``SHIFTCTRL_PULL_THRESH``) is reached. The output shift
  count is cleared to 0. The OUT will stall if the TX FIFO is empty.
- A nonblocking PULL on an empty FIFO has the same effect as ``MOV osr, x``.

When used as MOV destination, the output shift counter is reset to 0 (full).
"""

null: Final[PIONull]
"""PIO null (encoded as 011).

- **As source**: All zeroes. Useful for padding/alignment in ISR.
  Example: ``in_(null, 24)`` shifts 24 zero bits into ISR.
- **As destination**: Discards data. ``out(null, count)`` drops bits from OSR
  without writing them anywhere (useful with autopull to trigger a refill).
"""

status: Final[PIOStatus]
"""PIO STATUS (MOV source 101).

Reads as all-ones or all-zeroes depending on state machine status (e.g. FIFO
full/empty), configured by ``EXECCTRL_STATUS_SEL``. Only valid as a MOV source.

Useful pattern for FIFO-level branching::

    mov(x, status)          # X = all-1s or all-0s based on FIFO state
    jmp(not_x, "fifo_ok")   # Branch if status was all-zeroes
"""

exec: Final[PIOExec]
"""PIO EXEC destination (MOV dest 100, OUT dest 111).

Allows register contents or OSR shift data to be executed as a PIO instruction.
The MOV/OUT executes in one cycle; the injected instruction runs on the *next*
cycle. Delay cycles on the initial MOV/OUT are ignored, but the executee may
have its own delay. There are no restrictions on instruction types that can be
executed this way.
"""

gpio: Final[PIOGpio]
"""WAIT source: absolute system GPIO (WAIT source 00).

``wait(polarity, gpio, index)`` waits on the raw system GPIO number ``index``.
This is an absolute GPIO index (0-31) and is **not** affected by the state
machine's input pin mapping. Use this when you need to wait on a specific
physical GPIO regardless of how the state machine's pins are mapped.
"""

pin: Final[PIOPin]
"""PIO input pin -- used as JMP condition and WAIT source.

**JMP condition (110)**: Branches on the GPIO selected by ``EXECCTRL_JMP_PIN``.
This selects one of the 32 GPIO inputs independently of other input mappings.
The branch is taken if the selected GPIO is **high**.

**WAIT source (01)**: ``wait(polarity, pin, index)`` waits on the input pin
selected by adding ``index`` to ``PINCTRL_IN_BASE`` (modulo 32). This *does*
use the state machine's input pin mapping.
"""

irq: Final[PIOIRQ]
"""PIO IRQ -- used as WAIT source and as an instruction via ``irq(...)``.

**As WAIT source (10)**: ``wait(polarity, irq, index)`` waits on the PIO IRQ
flag selected by ``index``. If polarity is 1, the flag is cleared upon the
wait condition being met. Supports ``rel()`` for relative addressing.

**As instruction**: ``irq(index)`` or with modifiers -- sets or clears IRQ flags.
See the ``irq()`` function for details.

IRQ flags 4-7 are visible only to state machines. IRQ flags 0-3 can be routed
to system-level interrupts via ``IRQ0_INTE`` and ``IRQ1_INTE``.
"""

# ----- PIO Jump Conditions -----
x_dec: Final[PIOJumpCondition]
"""JMP condition 010: ``X--`` -- scratch X non-zero, prior to decrement.

JMP X-- **always** decrements scratch register X. The decrement is *not*
conditional on the current value. The branch is conditioned on the **initial**
value of X (i.e. before the decrement took place): if X was initially non-zero,
the branch is taken.

This is the standard PIO loop pattern::

    set(x, 4)              # Loop 5 times (4 down to 0)
    label("loop")
    # ... loop body ...
    jmp(x_dec, "loop")     # Decrement X; branch if X was non-zero before decrement
"""

y_dec: Final[PIOJumpCondition]
"""JMP condition 100: ``Y--`` -- scratch Y non-zero, prior to decrement.

JMP Y-- **always** decrements scratch register Y. The branch is conditioned
on the **initial** value of Y (before the decrement): if Y was initially
non-zero, the branch is taken.

Same semantics as ``x_dec`` but for scratch register Y.
"""

not_x: Final[PIOJumpCondition]
"""JMP condition 001: ``!X`` -- branch if scratch register X is zero.

The branch is taken if X == 0. X is not modified.
"""

not_y: Final[PIOJumpCondition]
"""JMP condition 011: ``!Y`` -- branch if scratch register Y is zero.

The branch is taken if Y == 0. Y is not modified.
"""

x_not_y: Final[PIOJumpCondition]
"""JMP condition 101: ``X!=Y`` -- branch if scratch X does not equal scratch Y.

The branch is taken if X != Y. Neither register is modified.
"""

not_osre: Final[PIOJumpCondition]
"""JMP condition 111: ``!OSRE`` -- branch if output shift register is not empty.

Compares the bits shifted out since the last PULL with the shift count threshold
configured by ``SHIFTCTRL_PULL_THRESH``. This is the same threshold used by
autopull.

Useful for shifting out variable-length data::

    pull()
    label("loop")
    out(pins, 1)
    jmp(not_osre, "loop")  # Keep shifting until OSR is empty
"""

# ----- PIO Push/Pull Modifiers -----
iffull: Final[PIOPushPullModifier]
"""PUSH modifier: ``IfFull`` (bit 6 = 1).

When used with PUSH: do nothing unless the total input shift count has reached
its threshold (``SHIFTCTRL_PUSH_THRESH``), the same threshold as for autopush.

``push(iffull)`` helps make programs more compact, like autopush. It is useful
when IN would stall at an inappropriate time if autopush were enabled (e.g. if
the state machine is asserting some external control signal at that point).

If ``iffull`` is not specified, the default is IfFull == 0 (always push).
"""

ifempty: Final[PIOPushPullModifier]
"""PULL modifier: ``IfEmpty`` (bit 6 = 1).

When used with PULL: do nothing unless the total output shift count has reached
its threshold (``SHIFTCTRL_PULL_THRESH``), the same threshold as for autopull.

``pull(ifempty)`` is useful if an OUT with autopull would stall in an
inappropriate location when the TX FIFO is empty. For example, a UART
transmitter should not stall immediately after asserting the start bit.
IfEmpty permits the same program simplifications as autopull, but the stall
occurs at a controlled point in the program.

If ``ifempty`` is not specified, the default is IfEmpty == 0 (always pull).
"""

block: Final[PIOPushPullModifier]
"""PUSH/PULL modifier: ``Block`` (bit 5 = 1). **This is the default.**

- **PUSH block**: Stall execution if RX FIFO is full. If the Block bit is not
  set, a PUSH to a full RX FIFO continues immediately to the next instruction
  (FIFO state and contents unchanged, ISR still cleared, FDEBUG_RXSTALL set).
- **PULL block**: Stall if TX FIFO is empty. If pulling from an empty FIFO
  with block, the state machine halts until data arrives.

Block is the default if neither ``block`` nor ``noblock`` are specified.
"""

noblock: Final[PIOPushPullModifier]
"""PUSH/PULL modifier: ``NoBlock`` (bit 5 = 0).

- **PUSH noblock**: If RX FIFO is full, do not stall. The ISR is still cleared
  to all-zeroes and ``FDEBUG_RXSTALL`` is set to indicate data was lost.
- **PULL noblock**: If TX FIFO is empty, do not stall. Instead, copies scratch
  register X to OSR (same effect as ``mov(osr, x)``). The program can preload
  X with a suitable default, or execute ``mov(x, osr)`` after each
  ``pull(noblock)`` so the last valid FIFO word is recycled until new data
  arrives.

Some peripherals (UART, SPI...) should halt when no data is available (use
``block``); others (I2S) should clock continuously and output placeholder/
repeated data (use ``noblock``).
"""

# ----- PIO IRQ Modifiers -----
clear: Final[PIOIRQModifier]
"""IRQ modifier: ``Clear`` (Clr bit = 1).

When used with the IRQ instruction, clears the flag selected by index instead
of raising it. If Clear is set, the Wait bit has no effect.

``irq(clear, index)`` clears IRQ flag ``index``.
"""

# ----- PIO MOV Operations -----
def invert(to_invert: PIOMoveOperable) -> PIOMoveOperated:
    """MOV operation 01: Bitwise complement (``!`` / ``~``).

    Sets each bit in the MOV destination to the logical NOT of the corresponding
    bit in the source. I.e. 1-bits become 0-bits and vice versa. This is always
    a bitwise NOT, not a logical NOT.

    Usage::

        mov(x, invert(y))      # x = ~y (bitwise complement)
        mov(pins, invert(x))   # Drive inverted X value to pins

    Args:
        to_invert: A MOV-operable source (``x``, ``y``, ``pins``, ``null``,
            ``status``, ``isr``, ``osr``).

    Returns:
        An operated source suitable for use in ``mov()``.
    """
    ...

def reverse(to_reverse: PIOMoveOperable) -> PIOMoveOperated:
    """MOV operation 10: Bit-reverse.

    Sets each bit *n* in the destination to bit *31 - n* in the source,
    assuming bits are numbered 0 to 31. Effectively mirrors the entire 32-bit
    word.

    Usage::

        mov(x, reverse(y))    # x = bit-reversed y
        mov(osr, reverse(isr))

    Args:
        to_reverse: A MOV-operable source (``x``, ``y``, ``pins``, ``null``,
            ``status``, ``isr``, ``osr``).

    Returns:
        An operated source suitable for use in ``mov()``.
    """
    ...

# ----- PIO IRQ Modifier Function -----
def rel(irq_index: int) -> int:
    """Make an IRQ or WAIT index relative to the current state machine.

    When ``rel()`` is used, the actual IRQ number is calculated by replacing the
    low two bits of ``irq_index`` with the low two bits of the sum
    ``(irq_index + sm_number)`` where ``sm_number`` is the state machine number
    (0-3). Bit 2 (the third LSB) is unaffected.

    This allows multiple state machines running the same program to synchronize
    with each other using distinct IRQ flags. For example, state machine 2 with
    ``rel(0)`` will use flag 2, and ``rel(1)`` will use flag 3.

    Args:
        irq_index: Base IRQ flag index (0-7).

    Returns:
        An encoded IRQ index with the relative addressing bit set.

    Example::

        irq(rel(0))              # Raise this SM's own IRQ flag
        wait(1, irq, rel(0))     # Wait for this SM's own IRQ flag
    """
    ...

# ----- PIO Instructions -----
def wrap() -> None:
    """Mark the end of the PIO program wrap point.

    When the program counter reaches the instruction *before* ``wrap()``, it
    wraps back to the instruction after ``wrap_target()`` instead of continuing
    to the next sequential instruction. This wrapping is free -- it does not
    consume an instruction cycle or a JMP instruction.

    ``wrap()`` and ``wrap_target()`` together define the main loop of a PIO
    program. If not specified, ``wrap_target()`` defaults to the start of the
    program and ``wrap()`` defaults to the end.

    Must be placed *after* the last instruction in the wrap region (i.e. between
    two instructions or at the end of the program).
    """
    ...

def wrap_target() -> None:
    """Mark the beginning of the PIO program wrap point.

    After executing the instruction just before ``wrap()``, the program counter
    wraps back to the instruction immediately after ``wrap_target()`` at no cost
    (no extra cycle, no JMP needed).

    Must be placed *before* the first instruction in the wrap region.

    Example::

        wrap_target()
        set(pins, 1)    [1]    # Drive pin high, delay 1 cycle
        set(pins, 0)           # Drive pin low
        wrap()                 # Wrap back to set(pins, 1) -- free, no cycle cost
    """
    ...

def nop() -> PIOInstruction:
    """No operation. Encoded as ``MOV y, y`` (copies Y to itself).

    Executes in one clock cycle, doing nothing. Primarily useful for inserting
    delay cycles via ``.side()`` or ``[delay]``::

        nop()          .side(1) [3]   # Assert side-set and delay 3 cycles
        nop()          [7]            # Delay 7 cycles

    Returns:
        A ``PIOInstruction`` that supports ``.side()`` and ``[delay]``.
    """
    ...

def mov(destination: PIOMoveTarget, source: PIOMoveSource) -> PIOInstruction:
    """MOV -- Copy data from source to destination (encoding: 101).

    Copies a full 32-bit value from ``source`` to ``destination``. The data can
    optionally be manipulated in transit using ``invert()`` (bitwise NOT) or
    ``reverse()`` (bit-reverse).

    **Destinations** (bits 7..5):
    - ``pins`` (000): Uses same pin mapping as OUT.
    - ``x`` (001): Scratch register X.
    - ``y`` (010): Scratch register Y.
    - ``exec`` (100): Execute data as instruction (runs on next cycle).
    - ``pc`` (101): Unconditional jump.
    - ``isr`` (110): Input shift counter reset to 0 (empty).
    - ``osr`` (111): Output shift counter reset to 0 (full).

    **Sources** (bits 2..0):
    - ``pins`` (000): Uses same pin mapping as IN.
    - ``x`` (001): Scratch register X.
    - ``y`` (010): Scratch register Y.
    - ``null`` (011): All zeroes (or use with invert for all ones).
    - ``status`` (101): All-ones or all-zeroes based on FIFO status.
    - ``isr`` (110): Input Shift Register.
    - ``osr`` (111): Output Shift Register.

    **Operations** (bits 4..3, applied via ``invert()``/``reverse()``):
    - None (00): Copy as-is.
    - ``invert()`` (01): Bitwise complement (NOT).
    - ``reverse()`` (10): Bit-reverse (bit n -> bit 31-n).

    Special behaviors:
    - ``mov(pc, src)``: Unconditional jump.
    - ``mov(exec, src)``: Execute source as instruction on next cycle.
    - ``mov(pins, src)``: Reads using IN pin mapping, writes full 32-bit value.

    Args:
        destination: Where to write (see destinations above).
        source: Where to read from, optionally wrapped in ``invert()``
            or ``reverse()``.

    Returns:
        A ``PIOInstruction`` supporting ``.side()`` and ``[delay]``.
    """
    ...

def in_(source: PIOInSource, bit_count: int) -> PIOInstruction:
    """IN -- Shift bits from source into the Input Shift Register (encoding: 010).

    Shifts ``bit_count`` bits from ``source`` into the ISR. The shift direction
    is configured per state machine by ``SHIFTCTRL_IN_SHIFTDIR``. Additionally
    increases the input shift count by ``bit_count``, saturating at 32.

    IN always uses the **least significant** ``bit_count`` bits of the source
    data. The ISR is first shifted left or right to make room, then the input
    data is copied into the gap. The bit order of the input data is *not*
    dependent on the shift direction.

    **Sources** (bits 7..5):
    - ``pins`` (000): Read from mapped input pins.
    - ``x`` (001): Scratch register X.
    - ``y`` (010): Scratch register Y.
    - ``null`` (011): All zeroes (for padding/alignment).
    - ``isr`` (110): ISR itself (shifts its own contents).
    - ``osr`` (111): Output Shift Register.

    **Autopush**: If enabled (``SHIFTCTRL_PUSH_THRESH``), IN will also push the
    ISR contents to the RX FIFO when the push threshold is reached. IN still
    executes in one cycle whether autopush occurs or not. The state machine
    stalls if the RX FIFO is full during an autopush. An autopush clears the
    ISR to all-zeroes and clears the input shift count.

    **NULL for alignment**: UARTs receive LSB first, so must shift to the right.
    After 8 ``in_(pins, 1)`` instructions, data occupies bits 31..24. An
    ``in_(null, 24)`` shifts in 24 zeroes, aligning data at ISR bits 7..0.

    Args:
        source: What to shift in from (see sources above).
        bit_count: Number of bits to shift (1-32). 32 is encoded as 00000 in
            the instruction.

    Returns:
        A ``PIOInstruction`` supporting ``.side()`` and ``[delay]``.
    """
    ...

@overload
def jmp(target: PIOJumpTarget) -> PIOInstruction: ...
@overload
def jmp(condition: PIOJumpCondition, target: PIOJumpTarget) -> PIOInstruction: ...
def jmp(condition_or_target, target=None):
    """JMP -- Conditionally set program counter (encoding: 000).

    Sets the program counter to ``target`` if ``condition`` is true; otherwise
    no operation (execution continues to the next instruction).

    **Delay cycles on JMP always take effect**, whether the condition is true or
    false. They take place *after* the condition is evaluated and the program
    counter is updated.

    **Conditions** (bits 7..5):
    - *(no condition)* (000): Always jump (unconditional).
    - ``not_x`` (001): ``!X`` -- branch if scratch register X is zero.
    - ``x_dec`` (010): ``X--`` -- always decrement X; branch if X was non-zero
      *before* the decrement.
    - ``not_y`` (011): ``!Y`` -- branch if scratch register Y is zero.
    - ``y_dec`` (100): ``Y--`` -- always decrement Y; branch if Y was non-zero
      *before* the decrement.
    - ``x_not_y`` (101): ``X!=Y`` -- branch if X does not equal Y.
    - ``pin`` (110): Branch on input pin selected by ``EXECCTRL_JMP_PIN``.
      Branch is taken if the GPIO is high.
    - ``not_osre`` (111): ``!OSRE`` -- branch if output shift register not empty.
      Compares bits shifted out since last PULL with ``SHIFTCTRL_PULL_THRESH``.

    **Target**: A program label (string) or integer value representing the
    instruction offset within the program (first instruction = offset 0). JMP
    uses absolute addresses in PIO instruction memory; offsets are adjusted at
    load time by the SDK.

    **X-- and Y-- details**: The decrement is *not* conditional on the current
    register value. The branch is conditioned on the *initial* value (before
    decrement): if initially non-zero, the branch is taken.

    Args:
        condition: Optional jump condition. If omitted, branch is always taken.
        target: Program label (str) or instruction address (int).

    Returns:
        A ``PIOInstruction`` supporting ``.side()`` and ``[delay]``.
    """
    ...

def wait(polarity: Literal[1] | Literal[0], source: PIOWaitSource, index: int) -> PIOInstruction:
    """WAIT -- Stall until some condition is met (encoding: 001).

    Stalls the state machine until the specified pin or IRQ flag matches the
    desired polarity. Like all stalling instructions, delay cycles begin
    *after* the instruction completes (i.e. after the wait condition is met).

    **Polarity** (bit 7):
    - ``1``: Wait for a 1 (high).
    - ``0``: Wait for a 0 (low).

    **Sources** (bits 6..5):
    - ``gpio`` (00): System GPIO input selected by ``index``. This is an
      *absolute* GPIO index and is **not** affected by the state machine's
      input IO mapping.
    - ``pin`` (01): Input pin selected by ``index``. The state machine's input
      IO mapping is applied first, then ``index`` selects which mapped bit to
      wait on. The pin is selected by adding ``index`` to ``PINCTRL_IN_BASE``,
      modulo 32.
    - ``irq`` (10): PIO IRQ flag selected by ``index``.
      - If polarity is 1, the selected IRQ flag is *cleared* upon the wait
        condition being met.
      - Use ``rel(index)`` for relative addressing (adds state machine ID to
        the low 2 bits of index via modulo-4 addition).

    **Index** (bits 4..0): Which pin or IRQ bit to check.

    CAUTION: ``wait(1, irq, x)`` should not be used with IRQ flags presented to
    the interrupt controller, to avoid a race condition with a system interrupt
    handler.

    Args:
        polarity: ``0`` to wait for low, ``1`` to wait for high.
        source: What to wait on -- ``gpio``, ``pin``, or ``irq``.
        index: Pin number or IRQ flag index. For ``gpio``, this is the absolute
            GPIO number. For ``pin``, this is the offset from ``PINCTRL_IN_BASE``.
            For ``irq``, use 0-7 (optionally wrapped in ``rel()``).

    Returns:
        A ``PIOInstruction`` supporting ``.side()`` and ``[delay]``.
    """
    ...

def out(target: PIOOutTarget, bit_count: int) -> PIOInstruction:
    """OUT -- Shift bits out of the Output Shift Register (encoding: 011).

    Shifts ``bit_count`` bits out of the OSR and writes them to ``target``.
    Additionally increases the output shift count by ``bit_count``, saturating
    at 32.

    A 32-bit value is written to the destination: the lower ``bit_count`` bits
    come from the OSR, and the remainder are zeroes. This value is the least
    significant bits of the OSR if ``SHIFTCTRL_OUT_SHIFTDIR`` is to the right,
    otherwise it is the most significant bits.

    **Destinations** (bits 7..5):
    - ``pins`` (000): Drive output pins (uses OUT pin mapping).
    - ``x`` (001): Scratch register X.
    - ``y`` (010): Scratch register Y.
    - ``null`` (011): Discard data (useful to advance OSR without side effects).
    - ``pindirs`` (100): Set pin directions (uses OUT pin mapping).
    - ``pc`` (101): Unconditional jump to shifted-out address.
    - ``isr`` (110): Also sets ISR shift counter to ``bit_count``.
    - ``exec`` (111): Execute OSR shift data as instruction on next cycle.
      The OUT itself executes on one cycle, the instruction from the OSR on the
      next. No restrictions on instruction types. Delay on the OUT is ignored
      but the executee may have its own delay.

    **Autopull**: If enabled, the OSR is automatically refilled from TX FIFO when
    ``SHIFTCTRL_PULL_THRESH`` is reached. Output shift count is cleared to 0.
    In this case, OUT will stall if the TX FIFO is empty, but otherwise still
    executes in one cycle.

    ``out(pc, count)`` behaves as an unconditional jump to an address shifted
    out from the OSR.

    ``pins`` and ``pindirs`` use the OUT pin mapping.

    Args:
        target: Where to write the shifted-out bits (see destinations above).
        bit_count: Number of bits to shift out (1-32). 32 is encoded as 00000.

    Returns:
        A ``PIOInstruction`` supporting ``.side()`` and ``[delay]``.
    """
    ...

def label(label: str) -> None:
    """Define a program label at the current instruction position.

    Labels are used as targets for JMP instructions. They represent instruction
    offsets within the program (first instruction = offset 0).

    Note: Because PIO JMP uses absolute addresses in PIO instruction memory,
    JMPs are adjusted based on the program load offset at runtime. This is
    handled automatically when loading a program with the SDK.

    Args:
        label: A string name for this program position.

    Example::

        label("loop")
        out(pins, 1)
        jmp(not_osre, "loop")   # Jump back to "loop" if OSR not empty
    """
    ...

@overload
def set(dest: PIOSetTarget, value: int) -> PIOInstruction: ...
@overload
def set() -> Set: ...
@overload
def set(iterable: Iterable[T]) -> Set[T]: ...
def set(dest_or_iterable=None, value=None):
    """SET -- Write immediate value to destination (encoding: 111).

    Writes a 5-bit immediate value ``data`` to ``destination``. Executes in one
    clock cycle.

    **Destinations** (bits 7..5):
    - ``pins`` (000): Drive the SET-mapped pins to ``value``.
    - ``x`` (001): 5 LSBs are set to ``value``, all other bits cleared to 0.
    - ``y`` (010): 5 LSBs are set to ``value``, all other bits cleared to 0.
    - ``pindirs`` (100): Set pin directions for SET-mapped pins.

    **Data** (bits 4..0): 5-bit immediate value (0-31).

    The SET and OUT pin mappings are configured independently. They may map to
    distinct pin ranges (e.g. one pin for clock signal, another for data), or
    overlapping ranges (e.g. UART: SET for start/stop bits, OUT for FIFO data,
    same pins).

    Common uses:
    - Assert control signals (clock, chip select).
    - Initialize loop counters: ``set(x, 31)`` for a 32-iteration loop.

    Note: This overloaded function also serves as Python's built-in ``set()``
    when called without PIO arguments.

    Args:
        dest: A SET destination (``pins``, ``x``, ``y``, ``pindirs``).
        value: 5-bit immediate value (0-31).

    Returns:
        A ``PIOInstruction`` supporting ``.side()`` and ``[delay]``.
    """
    ...

@overload
def push() -> PIOInstruction: ...
@overload
def push(modifier: PIOPushPullModifier) -> PIOInstruction: ...
@overload
def push(modifier1: PIOPushPullModifier, modifier2: PIOPushPullModifier) -> PIOInstruction: ...
def push(*modifiers):
    """PUSH -- Push ISR contents to the RX FIFO (encoding: 100, bit 7 = 0).

    Pushes the contents of the Input Shift Register (ISR) into the RX FIFO as
    a single 32-bit word. Clears the ISR to all-zeroes and resets the input
    shift count.

    **Modifiers**:
    - ``iffull``: If 1, do nothing unless the total input shift count has reached
      its threshold (``SHIFTCTRL_PUSH_THRESH``), same as for autopush. Default
      if not specified is IfFull == 0 (always push regardless of shift count).
    - ``block``: If 1 (the **default**), stall execution if RX FIFO is full.
    - ``noblock``: If Block is not set, PUSH does not stall on a full RX FIFO.
      Instead continues immediately; FIFO state/contents unchanged, ISR still
      cleared, ``FDEBUG_RXSTALL`` flag set to indicate data was lost.

    ``push(iffull)`` is useful when IN would stall at an inappropriate time if
    autopush were enabled, e.g. while asserting an external control signal.

    The PIO assembler sets Block by default.

    Assembler syntax forms::

        push()                    # push iffull block (defaults)
        push(iffull)              # push iffull block
        push(iffull, block)       # push iffull block
        push(iffull, noblock)     # push iffull noblock
        push(noblock)             # push noblock

    Returns:
        A ``PIOInstruction`` supporting ``.side()`` and ``[delay]``.
    """
    ...

@overload
def pull() -> PIOInstruction: ...
@overload
def pull(modifier: PIOPushPullModifier) -> PIOInstruction: ...
@overload
def pull(modifier1: PIOPushPullModifier, modifier2: PIOPushPullModifier) -> PIOInstruction: ...
def pull(*modifiers):
    """PULL -- Load a 32-bit word from the TX FIFO into the OSR (encoding: 100, bit 7 = 1).

    Loads a 32-bit word from the TX FIFO into the Output Shift Register (OSR)
    and resets the output shift count to 0.

    **Modifiers**:
    - ``ifempty``: If 1, do nothing unless the total output shift count has
      reached its threshold (``SHIFTCTRL_PULL_THRESH``), same as for autopull.
      Default if not specified is IfEmpty == 0 (always pull).
    - ``block``: If 1 (the **default**), stall if TX FIFO is empty.
      If pulling from an empty FIFO with block, copies scratch X to OSR.
    - ``noblock``: Do not stall if TX FIFO is empty. Instead copies scratch
      register X to OSR (same effect as ``mov(osr, x)``). The program can
      preload X with a default, or ``mov(x, osr)`` after each ``pull(noblock)``
      to recycle the last valid word until new data arrives.

    **Autopull note**: When autopull is enabled, any PULL instruction is a no-op
    when the OSR is full, so PULL behaves as a barrier. Use ``out(null, 32)``
    to explicitly discard OSR contents.

    **Use cases**:
    - Some peripherals (UART, SPI) should halt when no data is available -- use
      ``block`` (default).
    - Others (I2S) should clock continuously and output placeholder/repeated
      data -- use ``noblock``.

    ``pull(ifempty)`` is useful if an OUT with autopull would stall at an
    inappropriate location when the TX FIFO is empty. For example, a UART
    transmitter should not stall right after asserting the start bit.

    Assembler syntax forms::

        pull()                    # pull ifempty block (defaults)
        pull(ifempty)             # pull ifempty block
        pull(ifempty, block)      # pull ifempty block
        pull(ifempty, noblock)    # pull ifempty noblock
        pull(noblock)             # pull noblock

    Returns:
        A ``PIOInstruction`` supporting ``.side()`` and ``[delay]``.
    """
    ...

@overload
def word(instr: int) -> PIOInstruction: ...
@overload
def word(instr: int, label: str) -> PIOInstruction: ...
def word(instr, label=None):
    """Emit a raw 16-bit PIO instruction word.

    Allows inserting arbitrary pre-encoded PIO instructions into the program.
    This is an escape hatch for instructions that cannot be expressed with the
    assembler's built-in functions, or for embedding pre-computed instruction
    words.

    The optional ``label`` parameter attaches a label to this instruction
    position, allowing it to be used as a JMP target.

    Args:
        instr: A 16-bit integer representing the fully encoded PIO instruction.
            See the encoding table in Section 3.4.1 of the RP2040 datasheet.
        label: Optional program label to attach at this instruction's position.

    Returns:
        A ``PIOInstruction`` supporting ``.side()`` and ``[delay]``.
    """
    ...
