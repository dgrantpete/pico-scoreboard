import micropython
import rp2
import machine
import uctypes
import re
from array import array
from micropython import const
from pio_types import *

_DEFAULT_PIO_INDEX = const(0)
_STATE_MACHINE_OFFSET = const(0)

_DMA_32BIT_TRANSFER_SIZE = const(2)

# PIO register base addresses (same on RP2040 and RP2350)
_PIO_BASE_ADDRESSES = (
    const(0x50200000),  # PIO0
    const(0x50300000),  # PIO1
    const(0x50400000),  # PIO2 (RP2350 only)
)

# Offset from PIO base to RXF0 (RX FIFO for SM0). Each subsequent SM is +4 bytes.
_PIO_RXF_BASE_OFFSET = const(0x020)

_PIO_INDEX_EXPRESSION = re.compile(r'PIO\((\d)\)')
_PIN_GPIO_EXPRESSION = re.compile(r'Pin\(GPIO(\d+)')


@rp2.asm_pio(in_shiftdir=rp2.PIO.SHIFT_LEFT)
def _rotary_encoder_pio():
    # -----------------------------------------------------------
    # THE REORDERED JUMP TABLE
    # Index = [X1, X0, PinA, PinB]
    # -----------------------------------------------------------
    jmp("read")         # 0000 (0)  Invalid (X:11, Pins:00)
    jmp("increment")    # 0001 (1)  CW      (X:11, Pins:01)
    jmp("decrement")    # 0010 (2)  CCW     (X:11, Pins:10)
    jmp("read")         # 0011 (3)  Detent  (X:11, Pins:11) -> Resting State!

    jmp("increment")    # 0100 (4)  CW      (X:01, Pins:00)
    jmp("read")         # 0101 (5)  No Chg  (X:01, Pins:01)
    jmp("read")         # 0110 (6)  Invalid (X:01, Pins:10)
    jmp("decrement")    # 0111 (7)  CCW     (X:01, Pins:11)

    jmp("read")         # 1000 (8)  No Chg  (X:00, Pins:00)
    jmp("decrement")    # 1001 (9)  CCW     (X:00, Pins:01)
    jmp("increment")    # 1010 (10) CW      (X:00, Pins:10)
    jmp("read")         # 1011 (11) Invalid (X:00, Pins:11)

    jmp("decrement")    # 1100 (12) CCW     (X:10, Pins:00)
    jmp("read")         # 1101 (13) Invalid (X:10, Pins:01)
    jmp("read")         # 1110 (14) No Chg  (X:10, Pins:10)
    jmp("increment")    # 1111 (15) CW      (X:10, Pins:11)

    # -----------------------------------------------------------
    # THE STATE MACHINE
    # -----------------------------------------------------------
    label("read")
    wrap_target()
    mov(isr, null)      # Clear the ISR

    in_(x, 2)           # Pull the lowest 2 bits of Absolute Position X into ISR
                        # This intrinsically acts as our "Previous State"

    in_(pins, 2)        # Pull current pins. ISR is now exactly our 4-bit table index

    mov(pc, isr)        # Jump based on the ISR value

    # --- Math Helpers ---
    label("increment")
    mov(x, invert(x))            # X = ~X
    jmp(x_dec, "post_increment") # X = ~X - 1
    label("post_increment")
    mov(x, invert(x))            # X = ~(~X - 1) -> X = X + 1
    jmp("update")

    label("decrement")
    jmp(x_dec, "update")         # X = X - 1

    # --- Output ---
    label("update")
    mov(isr, x)         # Prepare absolute count for the CPU
    push(noblock)        # Push to RX FIFO. If full, silently drop it so PIO never stalls
    wrap()
    nop()
    nop()
    nop()
    nop()
    nop()


class RotaryEncoder:

    @staticmethod
    @micropython.native
    def _get_pio_index(pio: rp2.PIO) -> int:
        match = _PIO_INDEX_EXPRESSION.match(repr(pio))
        if not match:
            raise ValueError(f"Could not determine PIO index: '{pio!r}'")
        return int(match.group(1))

    @staticmethod
    @micropython.native
    def _get_pin_gpio_number(pin: machine.Pin) -> int:
        match = _PIN_GPIO_EXPRESSION.match(repr(pin))
        if not match:
            raise ValueError(f"Could not determine GPIO number: '{pin!r}'")
        return int(match.group(1))

    @staticmethod
    @micropython.native
    def _get_absolute_state_machine_index(pio_block_index: int, state_machine_offset: int) -> int:
        return pio_block_index * 4 + state_machine_offset

    @staticmethod
    @micropython.native
    def _get_pio_rx_data_request_index(pio_block_index: int, state_machine_offset: int) -> int:
        # RP2040/RP2350 DREQ layout per PIO block (8 DREQs each):
        #   TX SM0..3 = base+0..3, RX SM0..3 = base+4..7
        #   Bits: [pio_block_index:1][rx_not_tx:1][state_machine:2]
        return (pio_block_index << 3) | 0b100 | (state_machine_offset & 0b11)

    @micropython.native
    def __init__(
        self,
        *,
        base_channel_pin: machine.Pin,
        pio: rp2.PIO | None = None,
        reverse: bool = False,
    ):
        self._base_channel_pin = base_channel_pin
        self._reverse = reverse

        # Create the companion pin (base + 1) with PULL_UP to prevent floating input.
        # The PIO program reads 2 consecutive pins via in_(pins, 2) starting from in_base.
        base_gpio_number = self.__class__._get_pin_gpio_number(base_channel_pin)
        self._companion_pin = machine.Pin(base_gpio_number + 1, machine.Pin.IN, machine.Pin.PULL_UP)
        base_channel_pin.init(machine.Pin.IN, machine.Pin.PULL_UP)

        # Resolve PIO block
        self._pio = pio if pio is not None else rp2.PIO(_DEFAULT_PIO_INDEX)
        self._pio_block_index = self.__class__._get_pio_index(self._pio)

        # Allocate shared memory for DMA target (signed 32-bit int handles negative rotations)
        self._position_buffer = array('i', [0])
        position_buffer_address = uctypes.addressof(self._position_buffer)

        # Create state machine (we own the entire PIO block, always use SM0)
        absolute_state_machine_index = self.__class__._get_absolute_state_machine_index(
            self._pio_block_index, _STATE_MACHINE_OFFSET
        )
        self._state_machine = rp2.StateMachine(
            absolute_state_machine_index,
            _rotary_encoder_pio,
            in_base=self._base_channel_pin,
        )

        # Compute RX FIFO hardware address dynamically from PIO block and SM offset
        pio_base_address = _PIO_BASE_ADDRESSES[self._pio_block_index]
        rx_fifo_address = pio_base_address + _PIO_RXF_BASE_OFFSET + (_STATE_MACHINE_OFFSET * 4)

        # Compute DREQ index for RX FIFO (note the 0b100 bit that selects RX vs TX)
        data_request_index = self.__class__._get_pio_rx_data_request_index(
            self._pio_block_index, _STATE_MACHINE_OFFSET
        )

        # Configure DMA to continuously transfer PIO RX FIFO -> position buffer
        self._dma = rp2.DMA()
        self._dma.config(
            read=rx_fifo_address,
            write=position_buffer_address,
            count=0xFFFFFFFF,
            ctrl=self._dma.pack_ctrl(
                size=_DMA_32BIT_TRANSFER_SIZE,
                inc_read=False,
                inc_write=False,
                treq_sel=data_request_index,
            )
        )

        # Activate DMA first (must be ready to drain RX FIFO before SM starts pushing),
        # then activate the state machine
        self._dma.active(1)
        self._state_machine.active(1)

        # Capture baseline so value starts at 0
        self._baseline_raw = self._position_buffer[0]

    @property
    @micropython.native
    def value(self) -> int:
        # Convert quadrature edge count to detent count (4 edges per mechanical detent).
        # The +2 provides proper rounding to the nearest detent.
        raw_detents = (self._position_buffer[0] - self._baseline_raw + 2) >> 2
        return -raw_detents if self._reverse else raw_detents

    @property
    @micropython.native
    def raw_value(self) -> int:
        return self._position_buffer[0]

    @micropython.native
    def reset(self) -> None:
        self._baseline_raw = self._position_buffer[0]

    @micropython.native
    def deinit(self) -> None:
        # Deactivate state machine first (stops pushing to RX FIFO)
        self._state_machine.active(0)

        # Close DMA (stops reading from RX FIFO)
        self._dma.close()

        # Remove PIO program to free instruction memory (pass specific program
        # reference so it's safe if another driver shares the PIO block)
        self._pio.remove_program(_rotary_encoder_pio)
