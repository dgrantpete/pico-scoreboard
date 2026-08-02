#!/usr/bin/env python3
"""Cycle-accurate simulator for firmware/src/lib/button.py's PIO program.

Imports the REAL button.py with stub rp2/machine/micropython modules, captures
the instruction stream emitted by the @asm_pio-decorated function (including
delays and labels), then executes it with RP2040-datasheet semantics:

  - jmp(x_dec/y_dec): branch if register nonzero BEFORE decrement; the
    register is ALWAYS decremented (0 wraps to 0xFFFFFFFF).
  - Delay cycles apply whether or not a branch is taken.
  - in_ with SHIFT_LEFT: ISR = (ISR << n) | (src & mask(n)).
  - mov invert = ~v, reverse = 32-bit bit reversal.
"""
import sys, types
from pathlib import Path

M32 = 0xFFFFFFFF

# ---------------------------------------------------------------- stubs
class _Instr:
    __slots__ = ("op", "args", "delay")
    def __init__(self, op, args):
        self.op, self.args, self.delay = op, args, 0
    def __getitem__(self, d):
        assert 0 <= d <= 31, f"delay {d} out of range"
        self.delay = d
        return self
    def __repr__(self):
        return f"{self.op}{self.args}[{self.delay}]"

class _Tok:
    def __init__(self, name): self.name = name
    def __repr__(self): return self.name
    def __eq__(self, o): return isinstance(o, _Tok) and o.name == self.name
    def __hash__(self): return hash(self.name)
    def __or__(self, o): return object  # for "rp2.PIO | None" annotations

TOKS = {n: _Tok(n) for n in
        "x y osr isr null pin block not_x not_y x_dec y_dec pins status".split()}

class _Wrapped:
    def __init__(self, kind, tok): self.kind, self.tok = kind, tok
    def __repr__(self): return f"{self.kind}({self.tok})"

PROGRAM = []
def _emit(op):
    def f(*args):
        i = _Instr(op, args); PROGRAM.append(i); return i
    return f

ASM_ENV = dict(TOKS)
ASM_ENV.update(
    jmp=_emit("jmp"), mov=_emit("mov"), in_=_emit("in"), push=_emit("push"),
    pull=_emit("pull"), nop=_emit("nop"), set_=_emit("set"),
    label=_emit("label"), wrap_target=_emit("wrap_target"), wrap=_emit("wrap"),
    invert=lambda t: _Wrapped("invert", t), reverse=lambda t: _Wrapped("reverse", t),
    PIORegister=object, PIODelayableInstruction=object,
)

def install_stubs():
    rp2 = types.ModuleType("rp2")
    class PIO:
        SHIFT_LEFT = 0; SHIFT_RIGHT = 1
        def __init__(self, i): self._i = i
        def remove_program(self, p): pass
    def asm_pio(**kw):
        def deco(fn):
            fn2 = types.FunctionType(fn.__code__, {**ASM_ENV, "rp2": rp2}, fn.__name__)
            fn2()  # execute now, capturing instructions into PROGRAM
            fn2.asm_kwargs = kw
            return fn2
        return deco
    rp2.PIO, rp2.asm_pio = PIO, asm_pio
    class StateMachine:
        def __init__(self, *a, **kw): pass
        def put(self, v): pass
        def active(self, v): pass
        def rx_fifo(self): return 0
        def get(self): return 0
    rp2.StateMachine = StateMachine

    machine = types.ModuleType("machine")
    class Pin:
        def value(self): return 0
    machine.Pin = Pin

    micropython = types.ModuleType("micropython")
    micropython.native = lambda f: f
    micropython.const = lambda v: v

    pio_types = types.ModuleType("pio_types")
    pio_types.__dict__.update(PIORegister=object, PIODelayableInstruction=object)
    pio_types.__all__ = ["PIORegister", "PIODelayableInstruction"]

    sys.modules.update(rp2=rp2, machine=machine, micropython=micropython,
                       pio_types=pio_types)

install_stubs()
if len(sys.argv) > 1:
    _lib = Path(sys.argv[1])
else:
    # Prefer the repo-relative location (script lives in tools/); fall back
    # to an explicit path argument for running from anywhere else.
    _lib = Path(__file__).resolve().parent.parent / "firmware" / "src" / "lib"
    if not _lib.is_dir():
        sys.exit(f"firmware lib not found at {_lib}; pass its path as argv[1]")
sys.path.insert(0, str(_lib))
import button  # noqa: E402  (captures the program)

# ---------------------------------------------------------------- assemble
def assemble(prog):
    """Strip pseudo-ops; resolve labels and wrap points to addresses."""
    instrs, labels = [], {}
    wrap_target = 0
    wrap_after = None
    for ins in prog:
        if ins.op == "label":
            labels[ins.args[0]] = len(instrs)
        elif ins.op == "wrap_target":
            wrap_target = len(instrs)
        elif ins.op == "wrap":
            wrap_after = len(instrs) - 1
        else:
            instrs.append(ins)
    if wrap_after is None:
        wrap_after = len(instrs) - 1
    return instrs, labels, wrap_target, wrap_after

INSTRS, LABELS, WRAP_TARGET, WRAP_AFTER = assemble(PROGRAM)
assert len(INSTRS) <= 32, f"program too big: {len(INSTRS)} instructions"

def rev32(v):
    return int(f"{v & M32:032b}"[::-1], 2)

class Sim:
    def __init__(self, tx_seed, pin_wave):
        """pin_wave: list of (level, n_cycles). Held at last level after end."""
        self.x = self.y = self.isr = self.osr = 0
        self.pc = 0
        self.cycle = 0
        self.tx = [tx_seed]
        self.fifo = []            # (cycle, word)
        self.x_dec_cycles = []    # cycle stamp of every x decrement
        self.wave = pin_wave
        self._flat = []
        for lvl, n in pin_wave:
            self._flat.append((lvl, n))
        self.trace = []

    def pin(self):
        c = self.cycle
        for lvl, n in self._flat:
            if c < n:
                return lvl
            c -= n
        return self._flat[-1][0]

    def val(self, src):
        if isinstance(src, _Wrapped):
            v = self.val(src.tok)
            return (~v & M32) if src.kind == "invert" else rev32(v)
        n = src.name
        if n == "x": return self.x
        if n == "y": return self.y
        if n == "null": return 0
        if n == "osr": return self.osr
        if n == "isr": return self.isr
        raise AssertionError(f"mov source {n}")

    def step(self):
        ins = INSTRS[self.pc]
        op, a = ins.op, ins.args
        next_pc = self.pc + 1 if self.pc != WRAP_AFTER else WRAP_TARGET
        cost = 1 + ins.delay
        if op == "jmp":
            if len(a) == 1:
                cond, target = None, a[0]
            else:
                cond, target = a
            taken = False
            if cond is None:
                taken = True
            elif cond.name == "not_x":
                taken = self.x == 0
            elif cond.name == "not_y":
                taken = self.y == 0
            elif cond.name == "x_dec":
                taken = self.x != 0
                self.x = (self.x - 1) & M32
                self.x_dec_cycles.append(self.cycle)
            elif cond.name == "y_dec":
                taken = self.y != 0
                self.y = (self.y - 1) & M32
            elif cond.name == "pin":
                taken = self.pin() == 1
            else:
                raise AssertionError(f"jmp cond {cond}")
            if taken:
                next_pc = LABELS[target]
        elif op == "mov":
            dst, src = a
            v = self.val(src)
            if dst.name == "x": self.x = v
            elif dst.name == "y": self.y = v
            elif dst.name == "isr": self.isr = v
            elif dst.name == "osr": self.osr = v
            else: raise AssertionError(f"mov dest {dst}")
        elif op == "in":
            src, n = a
            self.isr = ((self.isr << n) | (self.val(src) & ((1 << n) - 1))) & M32
        elif op == "push":
            assert len(self.fifo) < 1000
            self.fifo.append((self.cycle, self.isr))
            self.isr = 0
        elif op == "pull":
            assert self.tx, "pull with empty TX FIFO would block forever"
            self.osr = self.tx.pop(0)
        elif op == "nop":
            pass
        else:
            raise AssertionError(f"op {op}")
        self.cycle += cost
        self.pc = next_pc

    def run(self, cycles):
        while self.cycle < cycles:
            self.step()

def decode(word):
    state = (word >> 31) & 1
    ticks = word & 0x7FFFFFFF
    return state, ticks

IT = 16  # cycles per loop iteration
TICK = 32  # cycles per FIFO duration tick

def report(name, ok, detail=""):
    print(f"  {'PASS' if ok else 'FAIL'}  {name}" + (f"  ({detail})" if detail else ""))
    if not ok:
        global FAILURES
        FAILURES += 1

FAILURES = 0
print(f"program: {len(INSTRS)} instructions, wrap_target={WRAP_TARGET}, wrap_after={WRAP_AFTER}")

# ---------------------------------------------------------------- scenario 1
# Clean press/release with bounce at both edges. reload=8 iterations (4 ticks).
print("\n[1] press/release with bounce, reload=8")
R = 8
wave = [(0, 300*IT)]                       # stable low
for lvl, n in [(1,1),(0,1),(1,2),(0,1)]:   # press bounce (each < reload)
    wave.append((lvl, n*IT))
wave += [(1, 200*IT)]                      # stable high
for lvl, n in [(0,1),(1,1),(0,2)]:         # release bounce
    wave.append((lvl, n*IT))
wave += [(0, 400*IT)]                      # stable low
s = Sim(R, wave)
s.run(sum(n for _, n in wave) - 10)
events = [(c, *decode(w)) for c, w in s.fifo]
report("exactly 2 events", len(events) == 2, f"{events}")
if len(events) == 2:
    (c1, st1, t1), (c2, st2, t2) = events
    report("event1 = HIGH (press)", st1 == 1)
    report("event2 = LOW (release)", st2 == 0)
    # event1 duration = the ~300 low iterations -> ~150 ticks
    report("press duration ~150 ticks", abs(t1 - 150) <= 1, f"t1={t1}")
    # event2 duration = press bounce (5 iters) + 200 high + release start -> ~102-104 ticks
    report("release duration ~102 ticks", abs(t2 - 102) <= 2, f"t2={t2}")
    # zero-latency: event1 cycle stamp lands within ~2 iterations of the first high sample
    first_high = 300*IT
    report("press event within 2 iterations of edge", 0 <= c1 - first_high <= 2*IT + 16,
           f"latency={c1 - first_high} cycles")

# ---------------------------------------------------------------- scenario 2
# THE MONEY TEST: constant tick spacing across debounce AND armed phases.
print("\n[2] x decrements exactly every 16 cycles in steady state (both states)")
s2 = Sim(R, [(0, 500*IT), (1, 500*IT)])
s2.run(990*IT)
deltas = [b - a for a, b in zip(s2.x_dec_cycles, s2.x_dec_cycles[1:])]
# The single accepted transition shows up as one 32-cycle gap (the transit
# path is exactly 2 iterations on both edge directions); everything else,
# across BOTH states and both debounce phases (y>0 and y==0), must be 16.
non16 = [d for d in deltas if d != IT]
report("all steady-state deltas == 16", non16 == [2*IT],
       f"decrements={len(deltas)}, non-16 deltas={non16[:5]}")

# ---------------------------------------------------------------- scenario 3
# Startup with pin HIGH: no spurious event; first release reported correctly.
print("\n[3] startup pin high -> no spurious push; release accepted after debounce")
s3 = Sim(R, [(1, 100*IT), (0, 100*IT)])
s3.run(195*IT)
ev3 = [(c, *decode(w)) for c, w in s3.fifo]
report("exactly 1 event", len(ev3) == 1, f"{ev3}")
if len(ev3) == 1:
    report("event = LOW", ev3[0][1] == 0)
    report("duration ~50 ticks (the high period)", abs(ev3[0][2] - 50) <= 1, f"t={ev3[0][2]}")

# ---------------------------------------------------------------- scenario 4
# Saturation: poke x small once armed; expect same-state push, field=0, clean resume.
print("\n[4] saturation event (x poked to 5)")
s4 = Sim(R, [(0, 100000*IT)])
s4.run(50*IT)              # settle into armed low loop
s4.x = 5                   # fast-forward the 2^32 wait
s4.run(50*IT + 40*IT)
ev4 = [(c, *decode(w)) for c, w in s4.fifo]
report("exactly 1 event", len(ev4) == 1, f"{ev4}")
if len(ev4) == 1:
    report("saturation event = LOW (same state)", ev4[0][1] == 0)
    report("duration field decodes 0 (documented quirk)", ev4[0][2] == 0, f"t={ev4[0][2]}")
# after saturation it must keep ticking normally
d4 = [b - a for a, b in zip(s4.x_dec_cycles[-20:], s4.x_dec_cycles[-19:])]
report("ticking resumes at 16 cycles", all(d == IT for d in d4), f"{sorted(set(d4))}")

# ---------------------------------------------------------------- scenario 5
# Sub-debounce press is swallowed -> surfaces as same-state event on next press.
print("\n[5] press shorter than debounce is swallowed (documented trade-off)")
# Swallow semantics: the 3-iteration press fires HIGH (zero latency), its
# release is rejected (high never armed), so the NEXT press arrives as a
# same-state HIGH event. Release of the long press is a normal LOW.
s5 = Sim(R, [(0, 100*IT), (1, 3*IT), (0, 100*IT), (1, 50*IT), (0, 100*IT)])
s5.run(350*IT)
ev5 = [decode(w) for _, w in s5.fifo]
report("3 events (short press's release swallowed)", len(ev5) == 3, f"{ev5}")
if len(ev5) == 3:
    report("sequence HIGH, HIGH(same-state), LOW",
           [e[0] for e in ev5] == [1, 1, 0], f"{[e[0] for e in ev5]}")
    report("2nd duration spans swallow + low period (~51)", abs(ev5[1][1] - 51) <= 2)

# ---------------------------------------------------------------- scenario 6
# Durations must be exact across MANY random stable periods (no drift).
print("\n[6] duration exactness over 40 random alternations (all > debounce)")
import random
random.seed(7)
periods = [random.randrange(20, 400) for _ in range(40)]
wave6 = [(i % 2, n*IT) for i, n in enumerate(periods)]  # low, high, low, ...
s6 = Sim(R, wave6 + [(0, 500*IT)])
s6.run(sum(n for _, n in wave6) + 100*IT)
ev6 = [decode(w) for _, w in s6.fifo]
# Event i fires at the transition out of period i: duration ~= periods[i]//2,
# state = level of period i+1 (the appended tail is low, matching i%2 flip).
errs = []
for i, (st, t) in enumerate(ev6):
    exp_t = periods[i] // 2
    exp_st = (i + 1) % 2
    if abs(t - exp_t) > 2 or st != exp_st:
        errs.append((i, st, t, exp_st, exp_t))
report(f"{len(ev6)} events, states alternate, durations within +/-2 ticks",
       len(ev6) == len(periods) and not errs, f"errors: {errs[:4]}")
cum_err = sum(t - periods[i] // 2 for i, (st, t) in enumerate(ev6))
report("no cumulative drift beyond 1 tick/event", abs(cum_err) <= len(ev6),
       f"net error {cum_err} ticks over {len(ev6)} events")

# ---------------------------------------------------------------- scenario 7
# The real Button class end-to-end: events-only API, decode, fold, filters.
print("\n[7] Button class: events-only API (decode / timestamp fold / rollover filter)")

class FakeTime:
    now = 1000
    @staticmethod
    def ticks_ms(): return FakeTime.now
    @staticmethod
    def ticks_add(a, b): return a + b
button.time = FakeTime

class FakeSM:
    def __init__(self): self.words = []; self.tx = []; self.on = 0
    def rx_fifo(self): return len(self.words)
    def get(self): return self.words.pop(0)
    def put(self, v): self.tx.append(v)
    def active(self, v): self.on = v

class FakeBlock:
    def __init__(self, sm): self.sm = sm; self.args = None; self.kw = None
    def state_machine(self, offset, prog, **kw):
        self.args = (offset, prog); self.kw = kw; return self.sm
    def remove_program(self, prog): pass

class FakePin:
    def __init__(self, v): self._v = v
    def value(self): return self._v

sm7 = FakeSM()
blk = FakeBlock(sm7)
b = button.Button(pin=FakePin(0), debounce_ms=20, pio=blk, sm_offset=2, active_low=False)
report("debounce reload seeded via put() (2*20//1 = 40)", sm7.tx == [40], f"{sm7.tx}")
report("constructed via block factory, sm_offset honored", blk.args[0] == 2 and "jmp_pin" in blk.kw)
report("initial == (False, 1000)", tuple(b.initial) == (False, 1000), f"{tuple(b.initial)}")
report("SM activated", sm7.on == 1)

r0 = b.read(); r0b = b.read()
report("idle read() -> shared empty tuple, identity-stable",
       r0 is button._NO_EVENTS and r0b is r0)

S = 0x80000000
sm7.words = [S | 500, 200, S | 100, S | 50]  # press, release, press, swallow re-press
evs = b.read()
expect = [(True, 1500), (False, 1700), (True, 1800), (True, 1850)]
report("decode + timestamp fold (incl. same-state swallow kept)",
       [tuple(e) for e in evs] == expect, f"{[tuple(e) for e in evs]}")

sm7.words = [S | 0]                          # rollover marker: same state, zero duration
r = b.read()
report("rollover marker filtered (returns shared empty)", r is button._NO_EVENTS, f"{r}")
sm7.words = [300]                            # release after marker
r = b.read()
report("anchor unaffected by filtered marker", r and tuple(r[0]) == (False, 2150),
       f"{tuple(r[0]) if r else r}")

state = b.initial
for e in tuple(evs) + tuple(r):
    state = e
report("consumer fold reproduces final state (released)", state.pressed is False)

b2 = button.Button(pin=FakePin(1), pio=FakeBlock(FakeSM()), active_low=True)
report("active_low: pin high at init -> not pressed", b2.initial.pressed is False)
try:
    button.Button(pin=FakePin(0), debounce_ms=0, pio=FakeBlock(FakeSM()))
    report("debounce_ms below one tick rejected", False)
except ValueError:
    report("debounce_ms below one tick rejected", True)

print(f"\n{'ALL TESTS PASSED' if not FAILURES else f'{FAILURES} FAILURES'}")
sys.exit(1 if FAILURES else 0)
