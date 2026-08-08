"""Golden-value generator for the hub75 crate's parity tests.

Loads the REAL MicroPython driver (firmware/src/lib/hub75/driver.py) under
CPython with the hardware modules stubbed out, runs its pure-math methods
(gamma LUT construction, timing-buffer computation, refresh-rate estimation,
target-refresh-rate search), and emits the results as Rust constants into
tests/goldens/mod.rs.

The point is that the Rust port is tested against the actual Python
implementation, not against a transcription of it. Re-run after any change
to driver.py's math:

    py crates/hub75/tests/gen_goldens.py

CPython floats are IEEE f64, same as Rust's f64, so float goldens are emitted
as exact bit patterns and compared exactly. round() halfway ties would differ
between CPython (half-even) and MicroPython (half-away); the generator
asserts no gamma entry lands within 1e-9 of a tie so the distinction never
matters.
"""

import importlib.util
import struct
import sys
import types
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
HUB75_DIR = REPO / "firmware" / "src" / "lib" / "hub75"
OUT_PATH = Path(__file__).resolve().parent / "goldens" / "mod.rs"

# Production geometry (display.py init_display + INVENTORY.md §3)
ROW_ADDRESS_COUNT = 32
SHIFT_REGISTER_DEPTH = 128
ADDRESS_UPDATE_CYCLES = 2  # Binary row addressing
BITPLANE_TRANSITION_EXTRA_CYCLES = 8  # Binary row addressing


def load_real_driver():
    stubs = {}
    for name in ("micropython", "rp2", "machine", "uctypes", "_thread", "pio_types"):
        stubs[name] = types.ModuleType(name)
    stubs["micropython"].native = lambda f: f
    stubs["micropython"].viper = lambda f: f
    stubs["micropython"].const = lambda x: x
    stubs["machine"].Pin = type("Pin", (), {})
    stubs["rp2"].PIO = type("PIO", (), {})
    stubs["rp2"].StateMachine = type("StateMachine", (), {})
    stubs["rp2"].DMA = type("DMA", (), {})
    stubs["rp2"].asm_pio = lambda **kwargs: (lambda f: f)
    sys.modules.update(stubs)

    pkg = types.ModuleType("hub75real")
    pkg.__path__ = [str(HUB75_DIR)]
    sys.modules["hub75real"] = pkg
    native = types.ModuleType("hub75real.native")
    sys.modules["hub75real.native"] = native

    spec = importlib.util.spec_from_file_location(
        "hub75real.driver", HUB75_DIR / "driver.py"
    )
    mod = importlib.util.module_from_spec(spec)
    sys.modules["hub75real.driver"] = mod
    spec.loader.exec_module(mod)
    return mod


driver_mod = load_real_driver()
gamma_mod = sys.modules["hub75real.gamma"]
Hub75Driver = driver_mod.Hub75Driver


def make_fake(brightness, blanking_time, system_frequency, data_frequency):
    from array import array

    ns = types.SimpleNamespace()
    ns.row_address_count = ROW_ADDRESS_COUNT
    ns._shift_register_depth = SHIFT_REGISTER_DEPTH
    ns._data_frequency = data_frequency
    ns._address_update_cycles = ADDRESS_UPDATE_CYCLES
    ns._bitplane_transition_extra_cycles = BITPLANE_TRANSITION_EXTRA_CYCLES
    ns._brightness = brightness
    ns._blanking_time = blanking_time
    ns._system_frequency = system_frequency
    ns._timing_buffer = array("I", [0] * 16)
    ns._estimate_refresh_rate = types.MethodType(Hub75Driver._estimate_refresh_rate, ns)
    ns._update_timing_buffer = types.MethodType(Hub75Driver._update_timing_buffer, ns)
    return ns


def f64_bits(x):
    return struct.unpack("<Q", struct.pack("<d", float(x)))[0]


def assert_no_gamma_ties(gamma):
    max_value = 255
    for i in range(256):
        x = i / max_value
        if gamma is None:
            continue
        if isinstance(gamma, gamma_mod.SRGB):
            linear = x / 12.92 if x <= 0.04045 else ((x + 0.055) / 1.055) ** 2.4
        elif isinstance(gamma, gamma_mod.Power):
            if gamma.value == 1.0:
                continue
            linear = x**gamma.value
        v = max_value * linear
        frac = abs(v - int(v) - 0.5)
        assert frac > 1e-9, f"gamma rounding tie at index {i}: {v!r}"


GAMMA_CASES = [
    ("SRGB", gamma_mod.SRGB()),
    ("POWER_2_2", gamma_mod.Power(2.2)),
    ("POWER_2_4", gamma_mod.Power(2.4)),
    ("POWER_0_5", gamma_mod.Power(0.5)),
    ("POWER_1_0", gamma_mod.Power(1.0)),
    ("IDENTITY", None),
]

# (base_cycles, brightness, blanking_ns, system_hz)
TIMING_CASES = [
    (1, 1.0, 0, 150_000_000),
    (1, 0.0, 0, 150_000_000),
    (2, 1.0, 0, 150_000_000),
    (3, 0.8, 0, 150_000_000),
    (3, 0.8, 1000, 150_000_000),
    (7, 0.5, 500, 150_000_000),
    (100, 0.25, 3000, 150_000_000),
    (100, 0.25, 3000, 125_000_000),
    (183, 1.0, 0, 150_000_000),
    (183, 0.999, 0, 150_000_000),
    (1000, 0.5, 2000, 200_000_000),
    (25_000, 0.75, 100, 150_000_000),
]

# (base_cycles, brightness, blanking_ns, system_hz, data_hz)
ESTIMATE_CASES = [
    (1, 1.0, 0, 150_000_000, 20_000_000),
    (1, 0.5, 0, 150_000_000, 20_000_000),
    (2, 1.0, 0, 150_000_000, 20_000_000),
    (3, 1.0, 0, 150_000_000, 20_000_000),
    (3, 0.8, 1000, 150_000_000, 20_000_000),
    (5, 0.8, 0, 150_000_000, 10_000_000),
    (10, 1.0, 500, 150_000_000, 20_000_000),
    (50, 0.25, 0, 150_000_000, 20_000_000),
    (183, 1.0, 0, 150_000_000, 20_000_000),
    (183, 0.8, 0, 125_000_000, 20_000_000),
    (1000, 1.0, 2000, 150_000_000, 15_000_000),
    (10_000, 0.9, 0, 200_000_000, 20_000_000),
]

# (target_hz, brightness, blanking_ns, system_hz, data_hz)
TARGET_CASES = [
    (30.0, 1.0, 0, 150_000_000, 20_000_000),
    (60.0, 1.0, 0, 150_000_000, 20_000_000),
    (90.0, 0.8, 0, 150_000_000, 20_000_000),
    (120.0, 1.0, 0, 150_000_000, 20_000_000),
    (120.0, 0.8, 0, 150_000_000, 20_000_000),
    (120.0, 0.8, 1000, 150_000_000, 20_000_000),
    (120.0, 0.5, 3000, 125_000_000, 10_000_000),
    (150.0, 1.0, 0, 150_000_000, 20_000_000),
    (240.0, 1.0, 0, 150_000_000, 20_000_000),
    (500.0, 0.9, 500, 150_000_000, 20_000_000),
    (1000.0, 1.0, 0, 150_000_000, 20_000_000),
    (5000.0, 1.0, 0, 150_000_000, 20_000_000),
    (100_000.0, 1.0, 0, 150_000_000, 20_000_000),
    (1e9, 1.0, 0, 150_000_000, 20_000_000),
]


def main():
    lines = []
    w = lines.append
    w("// GENERATED by tests/gen_goldens.py -- do not edit by hand.")
    w("// Values computed by running firmware/src/lib/hub75/driver.py under CPython.")
    w("#![allow(dead_code)]")
    w("")
    w("pub struct TimingCase {")
    w("    pub base_cycles: u32,")
    w("    pub brightness: f64,")
    w("    pub blanking_ns: u32,")
    w("    pub system_hz: u32,")
    w("    pub words: [u32; 16],")
    w("}")
    w("")
    w("pub struct EstimateCase {")
    w("    pub base_cycles: u32,")
    w("    pub brightness: f64,")
    w("    pub blanking_ns: u32,")
    w("    pub system_hz: u32,")
    w("    pub data_hz: u32,")
    w("    pub rate_bits: u64,")
    w("}")
    w("")
    w("pub struct TargetCase {")
    w("    pub target_hz: f64,")
    w("    pub brightness: f64,")
    w("    pub blanking_ns: u32,")
    w("    pub system_hz: u32,")
    w("    pub data_hz: u32,")
    w("    pub base_cycles: u32,")
    w("    pub rate_bits: u64,")
    w("    pub words: [u32; 16],")
    w("}")
    w("")

    for name, g in GAMMA_CASES:
        assert_no_gamma_ties(g)
        lut = Hub75Driver._create_gamma_lut(g)
        w(f"pub const GAMMA_{name}: [u8; 256] = [")
        for i in range(0, 256, 16):
            w("    " + ", ".join(str(b) for b in lut[i : i + 16]) + ",")
        w("];")
        w("")

    w(f"pub const TIMING_CASES: [TimingCase; {len(TIMING_CASES)}] = [")
    for base, brightness, blanking, sys_hz in TIMING_CASES:
        fake = make_fake(brightness, blanking, sys_hz, 20_000_000)
        fake._update_timing_buffer(base, brightness, blanking, sys_hz)
        words = ", ".join(str(v) for v in fake._timing_buffer)
        w("    TimingCase {")
        w(f"        base_cycles: {base},")
        w(f"        brightness: f64::from_bits({f64_bits(brightness)}),")
        w(f"        blanking_ns: {blanking},")
        w(f"        system_hz: {sys_hz},")
        w(f"        words: [{words}],")
        w("    },")
    w("];")
    w("")

    w(f"pub const ESTIMATE_CASES: [EstimateCase; {len(ESTIMATE_CASES)}] = [")
    for base, brightness, blanking, sys_hz, data_hz in ESTIMATE_CASES:
        fake = make_fake(brightness, blanking, sys_hz, data_hz)
        rate = fake._estimate_refresh_rate(base, brightness, blanking, sys_hz)
        w("    EstimateCase {")
        w(f"        base_cycles: {base},")
        w(f"        brightness: f64::from_bits({f64_bits(brightness)}),")
        w(f"        blanking_ns: {blanking},")
        w(f"        system_hz: {sys_hz},")
        w(f"        data_hz: {data_hz},")
        w(f"        rate_bits: {f64_bits(rate)}, // {rate!r} Hz")
        w("    },")
    w("];")
    w("")

    w(f"pub const TARGET_CASES: [TargetCase; {len(TARGET_CASES)}] = [")
    for target, brightness, blanking, sys_hz, data_hz in TARGET_CASES:
        fake = make_fake(brightness, blanking, sys_hz, data_hz)
        rate = Hub75Driver.set_target_refresh_rate(fake, target)
        words = ", ".join(str(v) for v in fake._timing_buffer)
        w("    TargetCase {")
        w(f"        target_hz: f64::from_bits({f64_bits(target)}), // {target!r}")
        w(f"        brightness: f64::from_bits({f64_bits(brightness)}),")
        w(f"        blanking_ns: {blanking},")
        w(f"        system_hz: {sys_hz},")
        w(f"        data_hz: {data_hz},")
        w(f"        base_cycles: {fake._base_cycles},")
        w(f"        rate_bits: {f64_bits(rate)}, // {rate!r} Hz")
        w(f"        words: [{words}],")
        w("    },")
    w("];")

    OUT_PATH.parent.mkdir(parents=True, exist_ok=True)
    OUT_PATH.write_text("\n".join(lines) + "\n")
    print(f"wrote {OUT_PATH} ({len(lines)} lines)")


if __name__ == "__main__":
    main()
