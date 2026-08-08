//! The hardware driver: owns PIO0 (both state machines and all program
//! memory), four DMA channels, and the double-buffered BCM framebuffers.
//! Port of driver.py's `Hub75Driver`.
//!
//! # DMA architecture
//!
//! Two self-perpetuating control/data channel pairs, exactly as in the
//! Python driver:
//!
//! * data path: [`DATA_BUFFER_DMA_CH`] streams the active framebuffer into
//!   the data SM's TX FIFO (paced by its DREQ), then chains to
//!   [`DATA_CONTROL_DMA_CH`], a single-word transfer that copies
//!   [`ACTIVE_BUFFER_PTR`] into the buffer channel's READ_ADDR_TRIG alias —
//!   re-arming it and re-reading the pointer every frame, forever, with
//!   zero CPU involvement.
//! * address path: identical structure streaming the 16-word timing buffer
//!   into the address SM.
//!
//! embassy-rp's DMA API cannot express this chaining, which is why the
//! whole driver is programmed against `rp235x-pac` directly. [`flip`]
//! (Hub75Driver::flip) is therefore two stores: toggle the index, publish
//! the new buffer address. The control channel picks it up at the next
//! frame boundary — no tearing, no blocking.
//!
//! # Safety model
//!
//! All `unsafe` is confined to this crate. The framebuffers, timing buffer,
//! and pointer words are `static`s so the DMA reads stable addresses; a
//! `DRIVER_TAKEN` flag makes the driver a singleton, and `&mut self` on all
//! mutating methods guarantees the CPU side holds at most one `&mut` into
//! the shared statics at a time. Hardware (DMA) reads are outside Rust's
//! aliasing model; CPU/DMA races on the timing words and pointer word go
//! through atomics.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering, compiler_fence};

use rp235x_pac as pac;

use crate::gamma::{Gamma, GammaTable};
use crate::geometry::{
    BITPLANE_BUFFER_BYTES, RGB565_FRAME_BYTES, RGB888_FRAME_BYTES, SHIFT_REGISTER_DEPTH,
    TIMING_WORDS,
};
use crate::packing;
use crate::programs;
use crate::timing;

/// SM 0 of PIO0: pixel data.
pub const DATA_STATE_MACHINE: usize = 0;
/// SM 1 of PIO0: row address + BCM timing.
pub const ADDRESS_STATE_MACHINE: usize = 1;

/// The four DMA channels the driver claims. The top of the channel range so
/// the application's low-numbered channels stay free; the Phase 3 app must
/// not hand these (or PIO0) to embassy-rp.
pub const DATA_BUFFER_DMA_CH: usize = 12;
pub const DATA_CONTROL_DMA_CH: usize = 13;
pub const ADDRESS_TIMING_DMA_CH: usize = 14;
pub const ADDRESS_CONTROL_DMA_CH: usize = 15;

/// GPIO assignments. Consecutive-pin requirements are PIO `out`/side-set
/// pin-group constraints, same as the MicroPython driver's.
#[derive(Clone, Copy, Debug)]
pub struct Pins {
    /// R1; G1, B1, R2, G2, B2 must be on the next five consecutive GPIOs.
    pub data_base: u8,
    /// CLK; LAT must be on the very next GPIO.
    pub clock_base: u8,
    /// OE (active low).
    pub output_enable: u8,
    /// Address line A; B..E on the next four consecutive GPIOs.
    pub address_base: u8,
}

impl Pins {
    /// Production scoreboard wiring (display.py `init_display`).
    pub const SCOREBOARD: Pins = Pins {
        data_base: 16,
        clock_base: 26,
        output_enable: 28,
        address_base: 11,
    };
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub pins: Pins,
    /// The system clock the caller has configured. There is no
    /// `machine.freq()` to consult on bare metal; clock changes after
    /// construction are not supported (driver.py's `sync_system_frequency`
    /// has no Rust counterpart).
    pub system_clock_hz: u32,
    /// Pixel clock in Hz; the data SM runs at twice this (rising + falling
    /// edge per pixel).
    pub data_clock_hz: u32,
    /// Initial brightness in `[0.0, 1.0]`; clamped.
    pub brightness: f64,
    /// Dead time between row switches in nanoseconds (anti-ghosting).
    pub blanking_time_ns: u32,
    pub gamma: Gamma,
    /// Snapped to the closest achievable rate, per `set_target_refresh_rate`.
    pub target_refresh_rate_hz: f64,
}

impl Config {
    /// driver.py's construction defaults with the production pinout.
    pub const fn defaults(system_clock_hz: u32) -> Config {
        Config {
            pins: Pins::SCOREBOARD,
            system_clock_hz,
            data_clock_hz: 20_000_000,
            brightness: 1.0,
            blanking_time_ns: 0,
            gamma: Gamma::Srgb,
            target_refresh_rate_hz: 120.0,
        }
    }
}

#[repr(align(4))]
struct BitplaneBuffers([[u8; BITPLANE_BUFFER_BYTES]; 2]);

struct SharedBuffers(UnsafeCell<BitplaneBuffers>);
// SAFETY: the DRIVER_TAKEN singleton flag plus `&mut self` on every accessor
// guarantee at most one CPU-side reference exists at a time; concurrent DMA
// reads are hardware, outside the aliasing model.
unsafe impl Sync for SharedBuffers {}

/// The double-buffered BCM framebuffers: 2 × 32,768 B in .bss.
static FRAMEBUFFERS: SharedBuffers =
    SharedBuffers(UnsafeCell::new(BitplaneBuffers([[0; BITPLANE_BUFFER_BYTES]; 2])));

/// One-word "which buffer" indirection read by the data control channel at
/// every frame boundary. Written only by `flip()`.
static ACTIVE_BUFFER_PTR: AtomicU32 = AtomicU32::new(0);

/// The 16-word `[off, on]` timing stream read by the address timing channel.
/// Rewritten in place by brightness/blanking/refresh changes while the DMA
/// keeps streaming — a mid-rewrite frame mixes old and new words for one
/// refresh cycle, same as the Python driver.
static TIMING_BUFFER: [AtomicU32; TIMING_WORDS] = [const { AtomicU32::new(0) }; TIMING_WORDS];

/// One-word indirection for the address control channel (never changes
/// after init, but the control DMA re-reads it every cycle by design).
static TIMING_BUFFER_PTR: AtomicU32 = AtomicU32::new(0);

static DRIVER_TAKEN: AtomicBool = AtomicBool::new(false);

fn buffer_addr(index: usize) -> u32 {
    let buffers = FRAMEBUFFERS.0.get();
    // SAFETY: raw-pointer projection only; no reference is materialized.
    unsafe { (&raw const (*buffers).0[index]) as usize as u32 }
}

/// The panel driver. Construct with [`Hub75Driver::new`]; the panel then
/// refreshes continuously in hardware until [`Hub75Driver::deinit`].
pub struct Hub75Driver {
    pio: pac::PIO0,
    dma: pac::DMA,
    active_index: usize,
    base_cycles: u32,
    brightness: f64,
    blanking_time_ns: u32,
    gamma: Gamma,
    gamma_lut: [u8; 256],
    system_clock_hz: u32,
    data_clock_hz: u32,
}

impl Hub75Driver {
    /// Initialize the driver and start the PIO + DMA refresh chain.
    ///
    /// Taking the PAC singletons by value is the ownership proof: the driver
    /// claims the whole PIO0 block and DMA channels 12–15. Call while no
    /// other code is concurrently reconfiguring RESETS, IO_BANK0, or
    /// PADS_BANK0 (in practice: during single-threaded init).
    ///
    /// Panics if a driver already exists (`deinit` the old one first).
    pub fn new(pio: pac::PIO0, dma: pac::DMA, config: Config) -> Hub75Driver {
        assert!(
            !DRIVER_TAKEN.swap(true, Ordering::AcqRel),
            "Hub75Driver already constructed"
        );
        let pins = config.pins;
        assert!(pins.data_base <= 24 && pins.clock_base <= 28 && pins.output_enable <= 29
            && pins.address_base <= 25, "pin group exceeds GPIO bank 0");

        let mut driver = Hub75Driver {
            pio,
            dma,
            active_index: 0,
            base_cycles: 1,
            brightness: config.brightness.clamp(0.0, 1.0),
            blanking_time_ns: config.blanking_time_ns,
            gamma: config.gamma,
            gamma_lut: config.gamma.build_lut(),
            system_clock_hz: config.system_clock_hz,
            data_clock_hz: config.data_clock_hz,
        };

        driver.unreset_peripherals();

        // A previous deinit leaves old frame data in the statics; the panel
        // must come back up dark, and load_* only writes the inactive half.
        {
            // SAFETY: singleton established above; DMA not yet running.
            let buffers = unsafe { &mut *FRAMEBUFFERS.0.get() };
            buffers.0[0].fill(0);
            buffers.0[1].fill(0);
        }
        ACTIVE_BUFFER_PTR.store(buffer_addr(0), Ordering::Release);
        TIMING_BUFFER_PTR.store(TIMING_BUFFER.as_ptr() as usize as u32, Ordering::Release);

        driver.set_target_refresh_rate(config.target_refresh_rate_hz);
        driver.connect_pins(pins);
        let address_origin = driver.load_programs();
        driver.configure_state_machines(pins, address_origin);
        driver.start_dma();
        // Both SMs in one write; the IRQ handshake orders them from there.
        driver
            .pio
            .ctrl()
            .modify(|_, w| unsafe { w.sm_enable().bits(0b0011) });
        driver
    }

    fn unreset_peripherals(&self) {
        // SAFETY: RMW on shared RESETS registers — see the single-threaded
        // init precondition on `new`. Only clears (never sets) reset bits.
        let resets = unsafe { pac::RESETS::steal() };
        resets.reset().modify(|_, w| {
            w.pio0().clear_bit();
            w.dma().clear_bit();
            w.io_bank0().clear_bit();
            w.pads_bank0().clear_bit()
        });
        loop {
            let done = resets.reset_done().read();
            if done.pio0().bit() && done.dma().bit() && done.io_bank0().bit()
                && done.pads_bank0().bit()
            {
                break;
            }
        }
    }

    fn connect_pins(&self, pins: Pins) {
        // SAFETY: per-GPIO registers; we touch only the pins in `pins`,
        // which the caller has dedicated to the panel.
        let io = unsafe { pac::IO_BANK0::steal() };
        let pads = unsafe { pac::PADS_BANK0::steal() };
        for pin in Self::pin_numbers(pins) {
            let n = pin as usize;
            // RP2350 pads power up isolated; clear ISO or nothing reaches
            // the pin. Remaining pad defaults (4 mA drive) are fine.
            pads.gpio(n).modify(|_, w| w.iso().clear_bit());
            io.gpio(n).gpio_ctrl().write(|w| w.funcsel().pio0());
        }
    }

    fn pin_numbers(pins: Pins) -> impl Iterator<Item = u8> {
        (pins.data_base..pins.data_base + 6)
            .chain(pins.clock_base..pins.clock_base + 2)
            .chain(core::iter::once(pins.output_enable))
            .chain(pins.address_base..pins.address_base + 5)
    }

    /// Load both programs into PIO instruction memory: data at offset 0,
    /// address immediately after. Returns the address program's origin.
    fn load_programs(&self) -> u8 {
        let data = programs::data_program();
        let address = programs::address_program();
        let address_origin = data.code.len() as u8;

        for (i, &instruction) in data.code.iter().enumerate() {
            self.pio
                .instr_mem(i)
                .write(|w| unsafe { w.bits(instruction as u32) });
        }
        for (i, &instruction) in address.code.iter().enumerate() {
            self.pio
                .instr_mem(address_origin as usize + i)
                .write(|w| unsafe {
                    w.bits(programs::relocate(instruction, address_origin) as u32)
                });
        }
        address_origin
    }

    fn configure_state_machines(&self, pins: Pins, address_origin: u8) {
        let data = programs::data_program();
        let address = programs::address_program();

        let data_sm = self.pio.sm(DATA_STATE_MACHINE);
        let address_sm = self.pio.sm(ADDRESS_STATE_MACHINE);

        // Initial pin directions and levels, driven through forced SET
        // instructions while PINCTRL temporarily maps the SET group (side-set
        // count is still 0, so the forced SETs can't disturb side pins).
        // OE initializes HIGH = deasserted; everything else low.
        Self::init_pin_group(data_sm, pins.data_base, 5, 0);
        Self::init_pin_group(data_sm, pins.data_base + 5, 1, 0);
        Self::init_pin_group(data_sm, pins.clock_base, 2, 0);
        Self::init_pin_group(address_sm, pins.address_base, 5, 0);
        Self::init_pin_group(address_sm, pins.output_enable, 1, 1);

        // Data SM at data_clock * 2 (an edge per SM cycle); address SM at
        // full system clock, exactly as MicroPython's StateMachine defaults.
        data_sm
            .sm_clkdiv()
            .write(|w| unsafe { w.bits(self.clkdiv_bits(self.data_clock_hz * 2)) });
        address_sm.sm_clkdiv().write(|w| unsafe { w.bits(1 << 16) });

        data_sm.sm_execctrl().write(|w| unsafe {
            w.wrap_top().bits(data.wrap.source);
            w.wrap_bottom().bits(data.wrap.target)
        });
        address_sm.sm_execctrl().write(|w| unsafe {
            w.wrap_top().bits(address_origin + address.wrap.source);
            w.wrap_bottom().bits(address_origin + address.wrap.target)
        });

        // SHIFT_RIGHT everywhere, autopull at 32 (threshold 0 encodes 32).
        // Written twice with FJOIN_RX toggled: changing a FIFO join clears
        // both FIFOs, purging anything a previous driver left in them.
        for sm in [data_sm, address_sm] {
            for join_rx in [true, false] {
                sm.sm_shiftctrl().write(|w| unsafe {
                    w.autopull().set_bit();
                    w.pull_thresh().bits(0);
                    w.out_shiftdir().set_bit();
                    w.in_shiftdir().set_bit();
                    w.fjoin_rx().bit(join_rx)
                });
            }
        }

        // SET_COUNT resets to 5; zero it so nothing maps the SET group.
        data_sm.sm_pinctrl().write(|w| unsafe {
            w.out_base().bits(pins.data_base);
            w.out_count().bits(6);
            w.sideset_base().bits(pins.clock_base);
            w.sideset_count().bits(2);
            w.set_count().bits(0)
        });
        address_sm.sm_pinctrl().write(|w| unsafe {
            w.out_base().bits(pins.address_base);
            w.out_count().bits(5);
            w.sideset_base().bits(pins.output_enable);
            w.sideset_count().bits(1);
            w.set_count().bits(0)
        });

        self.pio
            .ctrl()
            .modify(|_, w| unsafe { w.sm_restart().bits(0b0011).clkdiv_restart().bits(0b0011) });

        // Force each PC to its program's entry point. The address SM's jump
        // carries side-set 1 so OE stays deasserted.
        Self::exec(data_sm, programs::jmp_instruction(0, 0));
        Self::exec(address_sm, programs::jmp_instruction(address_origin, 1 << 4));

        // Clear stale state a previous run may have left: sticky FIFO debug
        // flags, and the inter-SM IRQ flags — a leftover latch-safe flag
        // would let the data SM skip its first wait and offset every row by
        // one (the driver.py deinit bug note).
        self.pio.fdebug().write(|w| unsafe { w.bits(0xFFFF_FFFF) });
        self.pio.irq().write(|w| unsafe { w.bits(0xFF) });

        // Seed the pixel-count reload value; must hit the FIFO before the
        // DMA starts filling it behind this word.
        self.pio
            .txf(DATA_STATE_MACHINE)
            .write(|w| unsafe { w.bits((SHIFT_REGISTER_DEPTH - 1) as u32) });
    }

    fn init_pin_group(sm: &pac::pio0::SM, base: u8, count: u8, level_mask: u8) {
        const SET_PINS: u16 = 0xE000;
        const SET_PINDIRS: u16 = 0xE080;
        sm.sm_pinctrl().write(|w| unsafe {
            w.set_base().bits(base);
            w.set_count().bits(count)
        });
        Self::exec(sm, SET_PINDIRS | ((1u16 << count) - 1));
        Self::exec(sm, SET_PINS | level_mask as u16);
    }

    /// Execute one instruction on a (disabled) state machine via SM_INSTR.
    fn exec(sm: &pac::pio0::SM, instruction: u16) {
        sm.sm_instr()
            .write(|w| unsafe { w.bits(instruction as u32) });
    }

    /// driver.py `set_frequency`'s divider encoding: truncated integer part
    /// in bits 31:16, truncated 1/256 fractional part in 15:8.
    fn clkdiv_bits(&self, sm_clock_hz: u32) -> u32 {
        let divider = self.system_clock_hz as f64 / sm_clock_hz as f64;
        debug_assert!((1.0..65536.0).contains(&divider), "clock divider out of range");
        let integer = divider as u32;
        let fractional = ((divider - integer as f64) * 256.0) as u32;
        (integer << 16) | (fractional << 8)
    }

    fn start_dma(&self) {
        // The framebuffers were zero-filled with plain writes; make sure
        // those retire before the hardware is pointed at them.
        compiler_fence(Ordering::SeqCst);

        let words_per_buffer = (BITPLANE_BUFFER_BYTES / 4) as u32;

        let data_buffer = self.dma.ch(DATA_BUFFER_DMA_CH);
        data_buffer
            .ch_read_addr()
            .write(|w| unsafe { w.bits(buffer_addr(0)) });
        data_buffer.ch_write_addr().write(|w| unsafe {
            w.bits(self.pio.txf(DATA_STATE_MACHINE).as_ptr() as usize as u32)
        });
        data_buffer
            .ch_trans_count()
            .write(|w| unsafe { w.mode().normal().count().bits(words_per_buffer) });
        data_buffer.ch_al1_ctrl().write(|w| unsafe {
            w.en().set_bit();
            w.data_size().size_word();
            w.incr_read().set_bit();
            w.incr_write().clear_bit();
            w.chain_to().bits(DATA_CONTROL_DMA_CH as u8);
            w.treq_sel().pio0_tx0();
            w.irq_quiet().set_bit()
        });

        let address_timing = self.dma.ch(ADDRESS_TIMING_DMA_CH);
        address_timing
            .ch_read_addr()
            .write(|w| unsafe { w.bits(TIMING_BUFFER.as_ptr() as usize as u32) });
        address_timing.ch_write_addr().write(|w| unsafe {
            w.bits(self.pio.txf(ADDRESS_STATE_MACHINE).as_ptr() as usize as u32)
        });
        address_timing
            .ch_trans_count()
            .write(|w| unsafe { w.mode().normal().count().bits(TIMING_WORDS as u32) });
        address_timing.ch_al1_ctrl().write(|w| unsafe {
            w.en().set_bit();
            w.data_size().size_word();
            w.incr_read().set_bit();
            w.incr_write().clear_bit();
            w.chain_to().bits(ADDRESS_CONTROL_DMA_CH as u8);
            w.treq_sel().pio0_tx1();
            w.irq_quiet().set_bit()
        });

        // Control channels: single unpaced word from the pointer static into
        // the paired channel's READ_ADDR_TRIG alias. CHAIN_TO = own channel
        // means "no chain"; the CTRL_TRIG write starts the whole machine.
        self.start_control_channel(
            DATA_CONTROL_DMA_CH,
            &ACTIVE_BUFFER_PTR,
            data_buffer.ch_al3_read_addr_trig().as_ptr() as usize as u32,
        );
        self.start_control_channel(
            ADDRESS_CONTROL_DMA_CH,
            &TIMING_BUFFER_PTR,
            address_timing.ch_al3_read_addr_trig().as_ptr() as usize as u32,
        );
    }

    fn start_control_channel(&self, channel: usize, source: &AtomicU32, target_addr: u32) {
        let control = self.dma.ch(channel);
        control
            .ch_read_addr()
            .write(|w| unsafe { w.bits(source.as_ptr() as usize as u32) });
        control
            .ch_write_addr()
            .write(|w| unsafe { w.bits(target_addr) });
        control
            .ch_trans_count()
            .write(|w| unsafe { w.mode().normal().count().bits(1) });
        control.ch_ctrl_trig().write(|w| unsafe {
            w.en().set_bit();
            w.data_size().size_word();
            w.incr_read().clear_bit();
            w.incr_write().clear_bit();
            w.chain_to().bits(channel as u8);
            w.treq_sel().permanent();
            w.irq_quiet().set_bit()
        });
    }

    /// Convert an RGB888 frame into the inactive framebuffer (visible after
    /// [`flip`](Self::flip)). Gamma is applied during conversion.
    pub fn load_rgb888(&mut self, frame: &[u8; RGB888_FRAME_BYTES]) {
        let lut = self.gamma_lut;
        packing::pack_rgb888(frame, &lut, self.inactive_buffer());
    }

    /// Convert an RGB565 frame (little-endian, `framebuf.RGB565` layout)
    /// into the inactive framebuffer. Gamma is applied during conversion.
    pub fn load_rgb565(&mut self, frame: &[u8; RGB565_FRAME_BYTES]) {
        let lut = self.gamma_lut;
        packing::pack_rgb565(frame, &lut, self.inactive_buffer());
    }

    /// Zero the inactive framebuffer. Takes effect on the next `flip()`.
    pub fn clear(&mut self) {
        self.inactive_buffer().fill(0);
    }

    /// Atomically swap the active and inactive framebuffers. The data
    /// control DMA picks the new pointer up at the next frame boundary, so
    /// this neither blocks nor tears.
    ///
    /// # The one window, stated as a condition rather than a rate
    ///
    /// As in the Python driver, the DMA keeps scanning the *old* buffer until
    /// that boundary — up to one refresh period — and the old buffer is the one
    /// the next `load_*` writes into. A caller is safe while
    ///
    /// ```text
    /// (time between one flip and the next load) > 1 / refresh_rate
    /// ```
    ///
    /// which for a render loop is its frame period minus what it spends
    /// drawing. At 120 Hz that budget is 8.3 ms, against 48 ms at 20 FPS and
    /// **15 ms at 60 FPS** — still clear, but the margin fell from 40 ms to
    /// 6.7 ms when the app's loop sped up, and it closes entirely if the
    /// configured refresh rate drops below about 66 Hz. It is a driver property
    /// and not an app one, so it is written here as the inequality; BACKLOG 84
    /// carries the app-side number and the fact that it has never been observed
    /// rather than merely computed.
    pub fn flip(&mut self) {
        self.active_index = 1 - self.active_index;
        ACTIVE_BUFFER_PTR.store(buffer_addr(self.active_index), Ordering::Release);
    }

    fn inactive_buffer(&mut self) -> &mut [u8; BITPLANE_BUFFER_BYTES] {
        // SAFETY: singleton driver + `&mut self` ⇒ this is the only CPU
        // reference into FRAMEBUFFERS; the DMA reads the other buffer.
        unsafe { &mut (*FRAMEBUFFERS.0.get()).0[1 - self.active_index] }
    }

    /// Set brightness (OE duty cycle per bitplane) in `[0.0, 1.0]`, clamped.
    /// Does not change the refresh rate, but bounds the achievable maximum.
    /// Cheap enough for the render loop to call when the auto-brightness
    /// atomic changes. Returns the applied value.
    pub fn set_brightness(&mut self, brightness: f64) -> f64 {
        self.brightness = brightness.clamp(0.0, 1.0);
        self.publish_timing();
        self.brightness
    }

    pub fn brightness(&self) -> f64 {
        self.brightness
    }

    /// Set the PIO data clock, in Hz. Returns the requested value.
    ///
    /// Port of driver.py `set_frequency` (`:416-442`), including its two
    /// caveats. The divider is rewritten **without stopping the state
    /// machine** — the running transfer finishes at the old rate and the next
    /// one starts at the new rate, which is what makes this safe to call from
    /// a config change while the panel is refreshing. And the refresh rate is
    /// **not** re-balanced: the base-cycle count still encodes the old clock,
    /// so a caller that cares about hitting a target rate follows this with
    /// [`set_target_refresh_rate`](Self::set_target_refresh_rate). `PUT
    /// /api/config` does exactly that, in that order, as `api_routes.py` did.
    ///
    /// The achieved clock differs slightly from the request: the divider is an
    /// integer plus a 1/256 fraction, both truncated.
    pub fn set_data_clock(&mut self, data_clock_hz: u32) -> u32 {
        self.data_clock_hz = data_clock_hz;
        // Twice the pixel clock: the data SM drives an edge per cycle.
        let bits = self.clkdiv_bits(self.data_clock_hz * 2);
        self.pio.sm(DATA_STATE_MACHINE).sm_clkdiv().write(|w| unsafe { w.bits(bits) });
        self.data_clock_hz
    }

    pub fn data_clock_hz(&self) -> u32 {
        self.data_clock_hz
    }

    /// Set the dead time inserted around row switches (reduces ghosting at
    /// the cost of maximum refresh rate). Returns the applied value.
    pub fn set_blanking_time(&mut self, nanoseconds: u32) -> u32 {
        self.blanking_time_ns = nanoseconds;
        self.publish_timing();
        self.blanking_time_ns
    }

    pub fn blanking_time_ns(&self) -> u32 {
        self.blanking_time_ns
    }

    /// Install an already-built gamma table. Applies to subsequent `load_*`
    /// calls; the displayed frame is not retroactively corrected.
    ///
    /// Takes a [`GammaTable`] rather than a [`Gamma`] so that building it —
    /// 27.6 ms for a `Power` curve — cannot land inside a frame. See
    /// [`GammaTable`]'s docs.
    pub fn set_gamma(&mut self, table: GammaTable) -> Gamma {
        self.gamma = table.gamma();
        self.gamma_lut = *table.lut();
        self.gamma
    }

    pub fn gamma(&self) -> Gamma {
        self.gamma
    }

    /// Estimated refresh rate under the current timing parameters, in Hz.
    pub fn refresh_rate(&self) -> f64 {
        timing::estimate_refresh_rate(
            self.base_cycles as u64,
            self.brightness,
            self.blanking_time_ns,
            self.system_clock_hz,
            self.data_clock_hz,
        )
    }

    /// Snap to the achievable refresh rate closest to `target_hz` given the
    /// current brightness, blanking, and clocks; returns the achieved rate.
    pub fn set_target_refresh_rate(&mut self, target_hz: f64) -> f64 {
        let (base_cycles, rate) = timing::base_cycles_for_target(
            target_hz,
            self.brightness,
            self.blanking_time_ns,
            self.system_clock_hz,
            self.data_clock_hz,
        );
        self.base_cycles = base_cycles.try_into().expect("base cycles exceed u32");
        self.publish_timing();
        rate
    }

    fn publish_timing(&self) {
        let words = timing::timing_words(
            self.base_cycles as u64,
            self.brightness,
            self.blanking_time_ns,
            self.system_clock_hz,
        );
        for (slot, word) in TIMING_BUFFER.iter().zip(words) {
            slot.store(word, Ordering::Relaxed);
        }
    }

    /// Gracefully stop the refresh and release the hardware, after which a
    /// new driver may be constructed. Port of driver.py `deinit`: the DMA
    /// chain is *broken*, not force-stopped, so every channel ends a clean
    /// transfer boundary.
    pub fn deinit(self) {
        let data_buffer = self.dma.ch(DATA_BUFFER_DMA_CH);

        // Retarget the data channel's chain to itself (= no chain) and
        // unquiet its IRQ in one write. Quiet mode kept the raw INTR flag
        // clear, so after clearing any stale bit, the first flag to appear
        // marks the completion that did NOT re-chain.
        self.dma
            .intr()
            .write(|w| unsafe { w.bits(1 << DATA_BUFFER_DMA_CH) });
        data_buffer.ch_al1_ctrl().modify(|_, w| unsafe {
            w.chain_to().bits(DATA_BUFFER_DMA_CH as u8);
            w.irq_quiet().clear_bit()
        });
        while self.dma.intr().read().bits() & (1 << DATA_BUFFER_DMA_CH) == 0 {
            core::hint::spin_loop();
        }

        // Controls first so nothing can re-arm the stream channels, then
        // abort whatever is left in flight (the timing channel is usually
        // parked mid-block on the address SM's full FIFO).
        for channel in [
            DATA_CONTROL_DMA_CH,
            ADDRESS_CONTROL_DMA_CH,
            DATA_BUFFER_DMA_CH,
            ADDRESS_TIMING_DMA_CH,
        ] {
            self.dma
                .ch(channel)
                .ch_al1_ctrl()
                .write(|w| unsafe { w.bits(0) });
        }
        let abort_mask = (1 << DATA_CONTROL_DMA_CH)
            | (1 << ADDRESS_CONTROL_DMA_CH)
            | (1 << DATA_BUFFER_DMA_CH)
            | (1 << ADDRESS_TIMING_DMA_CH);
        self.dma
            .chan_abort()
            .write(|w| unsafe { w.chan_abort().bits(abort_mask) });
        while self.dma.chan_abort().read().bits() != 0 {
            core::hint::spin_loop();
        }

        // With the DMA gone the address SM can stall on `out` before ever
        // firing latch-safe, leaving the data SM blocked on its wait — force
        // both handshake flags to unstick whichever SM is waiting.
        self.pio.irq_force().write(|w| unsafe { w.bits(0b11) });

        // The data SM now drains its FIFO (at most 4 words + OSR — less than
        // one row) and stalls on an empty `out`; wait for a fresh TX stall.
        self.pio
            .fdebug()
            .write(|w| unsafe { w.txstall().bits(1 << DATA_STATE_MACHINE) });
        while self.pio.fdebug().read().txstall().bits() & (1 << DATA_STATE_MACHINE) == 0 {
            core::hint::spin_loop();
        }

        self.pio
            .ctrl()
            .modify(|_, w| unsafe { w.sm_enable().bits(0) });

        // Clear leftover handshake flags — a stale latch-safe flag would
        // offset every row by one on the next init.
        self.pio.irq().write(|w| unsafe { w.bits(0xFF) });

        DRIVER_TAKEN.store(false, Ordering::Release);
    }
}
