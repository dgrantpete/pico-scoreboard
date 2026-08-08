# RAM Budget

The living version of SPEC.md §11. Seeded from the spec's estimates; lines get
replaced by measured values as each phase lands.

**Rule: any PR that adds a static ≥ 1 KB updates this table in the same PR.**

**Target: ≥ 40 % headroom** against the RP2350's 520 KiB (532,480 B) of SRAM —
a ceiling of 319,488 B for statics + stacks combined.

Every line says whether it is **MEASURED** (read out of a real ELF, with the
command that produced it below) or **ESTIMATE** (SPEC §11's seed, unverified).
No allocator exists, so this table plus the stacks *is* the RAM story — there is
no heap to absorb a mistake.

---

## The table

| Item | Bytes | Status | Notes |
|---|---:|---|---|
| HUB75 BCM framebuffers ×2 + driver statics | 65,609 | **MEASURED** | `crates/hub75`. 2 × 32,768 B bitplane buffers + 64 B timing stream + 8 B DMA pointer words + 1 B construction guard. See the breakdown below. |
| `Hub75Display` RGB565 frame buffer | 16,384 | **MEASURED** | 128 × 64 × 2 B. App-owned, not a driver static — the driver never allocates the drawing surface. Costs 16,385 B as linked (`ConstStaticCell` appends a 1 B taken flag). |
| Gamma LUT | 256 | **MEASURED** | `[u8; 256]` **inside `Hub75Driver`**, not a separate static. It lands in whichever arena owns the driver — do not add it again to the core-1 line. |
| Snapshot handoff — `SnapshotChannel` | 8,552 | **MEASURED** | `crates/scoreboard-model`. Three `ScoreboardSnapshot` slots of 2,848 B + 2 B of index + padding. Three, not two — see the correction below. |
| `Store` (core-0 authoritative state) | 2,880 | **MEASURED** | One more snapshot plus the startup flag and the soccer stale-clock guard. Core 0 mutates this and publishes clones of it. |
| `Slate` (merged games list + rotation) | 4,596 | **MEASURED** | 160 entries × (source, state, 20-byte game id) + the rotation order + 8 league descriptors. Sized for a college-football Saturday alongside a full MLB slate. |
| embassy-net buffers + sockets | ~49,152 | ESTIMATE | Poller + HTTP server ×2 + DNS + OTA. |
| cyw43 driver state | ~16,384 | ESTIMATE | |
| Receive/scratch buffers (wire, HTTP, OTA chunk) | ~40,960 | ESTIMATE | Unioned where phases cannot overlap (OTA vs. poll). |
| Glyph/font tables + compiled sprites | 0 | **MEASURED** | `crates/scoreboard-render`. **9,538 B of flash, 0 B of RAM** — read out of the crate's own object code, not asserted. Breakdown and the command below. |
| Core-0 task arenas + stack | ~24,576 | ESTIMATE | |
| Core-1 stack | 8,192 | ESTIMATE | Render loop only. |
| Ring log + misc statics | ~8,192 | ESTIMATE | Measured floor today is 272 B — embassy-rp GPIO wakers, the time driver, the critical-section lock, and the PAC singleton flags. The deployed RAM log (SPEC §9) is the bulk of this line. |
| **Projected total** | **245,477** | | **239.7 KiB** |

**Headroom: 53.9 %** (287,003 B free of 532,480 B). Against the 512 KiB that
`hub75-diag/memory.x` actually declares — see the caveat below — it is 53.2 %.
Either way the ≥ 40 % target holds with room to spare: the estimates above may
overrun by a combined 74,011 B before the target is breached.

The three `scoreboard-model` lines are **struct sizes measured on the host**,
not symbols read out of a device ELF — no binary links them yet. They are exact
anyway: every bounded string in the snapshot carries a `u16` length prefix
rather than a `usize`, so the layout is byte-identical on the host and on
`thumbv8m.main-none-eabihf`. Verified by compiling a deliberate type mismatch
for the ARM target and reading the size out of the error.

**Measured today: 84,120 B** (82.1 KiB, 84.2 % headroom) — that is the whole of
`hub75-diag`, the only device binary that exists. Everything past the first
three rows is still a seed estimate.

### Correction to SPEC §4 — the handoff needs three buffers, not two

SPEC §4 specifies `static SNAPSHOTS: [ScoreboardSnapshot; 2]`: core 0 fills the
inactive buffer and publishes by storing its index. That is one buffer short of
correct. Core 1 latches an index at the top of a frame and reads from it for the
whole 50 ms frame, so the buffer it latched is still live *after* core 0 has
published a newer one — and core 0's next publish targets exactly that buffer.
Two publishes inside one frame is not a corner case: every live commit is
followed immediately by the play-flash commit, microseconds later.

Three slots is provably sufficient for one writer and one reader — one
published, one possibly latched, the writer takes the third. That is what the
MicroPython `TripleBufferedState` does (with a lock around the index
bookkeeping); `scoreboard-model`'s `SnapshotChannel` reaches the same guarantee
with one atomic swap per side and no lock, so core 0 never waits on core 1.

The cost is one extra 2,848 B slot over the spec's shape. The `Store` line is a
fourth copy: core 0 keeps the authoritative snapshot and publishes clones of it.
Folding those together — mutating the publisher's back buffer in place and
carrying forward from the just-published slot — would recover 2,848 B at the
price of making the whole state machine borrow the channel. If RAM ever gets
tight, that is the lever; at 53.9 % headroom it is not worth the coupling.

### The render tables are flash, and that is checked rather than assumed

SPEC §11 budgeted "Glyph/font tables: 0 RAM — `&'static` in flash". True, but a
`static` only lands in `.rodata` while nothing can write to it, and a single
`static mut` or a `Cell` in a palette would move kilobytes into `.bss` without a
compiler complaint. `scoreboard-render` forbids `unsafe`, mutates no static at
all (the mutation-contract table in its crate docs says why), and the result is
visible in the symbol table:

| Table | Bytes | Section |
|---|---:|---|
| `unscii_16` heap + index | 3,888 | `.rodata` |
| `unscii_8` heap + index | 2,360 | `.rodata` |
| `spleen_5x8` heap + index | 1,410 | `.rodata` |
| 10 compiled sprites (pixels + palettes) | 1,880 | `.rodata` |
| **Total** | **9,538** | **flash, 0 B RAM** |

The 19 Aseprite slices cost nothing at all: they are `const` rectangles, folded
into the code that reads them.

Reproduce (needs `binutils-arm-none-eabi`):

```sh
cargo build -p scoreboard-render --release --target thumbv8m.main-none-eabihf
RLIB=target/thumbv8m.main-none-eabihf/release/libscoreboard_render.rlib
# Every generated table, by section. All 'R' (read-only) is the claim.
arm-none-eabi-nm -S -td "$RLIB" | grep scoreboard_render9generated
# Writable statics from this crate. Zero is the claim.
arm-none-eabi-nm -S -td "$RLIB" | awk '$3 ~ /^[bBdD]$/' | grep scoreboard_render
```

The QR encoder is the crate's only sizeable working memory and it is **stack**,
not static: two 211 B buffers live inside `QrBitmap::encode` for the duration of
one call. The 343 B bitmap it fills is a field of the app's `PreparedView`,
which lands wherever the render loop's locals do — a core-1 stack line, not a
static.

### Correction to SPEC §11

The spec's framebuffer line read "~48 KB · 64×32×8 planes ×2 buffers". Both
halves were wrong: the shipping panel is 128×64, not 64×32, and ~48 KB did not
follow from the 64×32 geometry it cited either (that pairing works out to
16 KiB). The measured figure for the real panel is 65,536 B of bitplane buffers
— four times what the seed's own stated geometry implies, and the largest single
item in the budget.

---

## Measured breakdown — `hub75-diag`, release, `thumbv8m.main-none-eabihf`

The reference point every future measurement is compared against.
**32,512 B flash** (`.text` + `.rodata` + vector/boot blocks), **56 B `.data`**,
**83,040 B `.bss`**, **1,024 B `.uninit`** → **84,120 B of RAM statics**.

| Symbol | Bytes | Owner |
|---|---:|---|
| `hub75::driver::FRAMEBUFFERS` | 65,536 | hub75 — the two BCM bitplane buffers |
| `hub75_diag::FRAME` | 16,385 | app — RGB565 drawing surface (16,384 + 1 B `ConstStaticCell` flag) |
| `defmt_rtt::BUFFER` | 1,024 | defmt-rtt ring, `.uninit`. **Dev-only**; not in the deployed budget |
| `hub75_diag::__embassy_main::POOL` | 768 | embassy task arena — holds the `Hub75Display`, hence the gamma LUT |
| `embassy_rp::gpio::BANK0_WAKERS` | 240 | embassy-rp |
| `.L_MergedGlobals` | 104 | 73 B of hub75 driver statics (`TIMING_BUFFER` 64, `TIMING_BUFFER_PTR` 4, `ACTIVE_BUFFER_PTR` 4, `DRIVER_TAKEN` 1) + 28 B embassy/defmt singletons + 3 B padding |
| `_SEGGER_RTT` + `defmt_rtt::NAME` | 54 | `.data`, RTT control block. Dev-only |
| `panic_probe::…::PANICKED` | 1 | |
| inter-symbol alignment padding | 8 | |

Reproduce (needs `binutils-arm-none-eabi`; CI runs exactly this and echoes it
into the job summary):

```sh
cd firmware-rs/hub75-diag
cargo build --release --locked --target thumbv8m.main-none-eabihf
ELF=target/thumbv8m.main-none-eabihf/release/hub75-diag
arm-none-eabi-size -B -d "$ELF"   # headline text/data/bss
arm-none-eabi-size -A -d "$ELF"   # per-section; ignore .debug_* and its Total
arm-none-eabi-nm -C --size-sort -r -S -td "$ELF" | awk '$3 ~ /^[bBdD]$/'
```

---

## Caveats to close before the numbers can be trusted end to end

- **Stacks are not in this measurement.** `arm-none-eabi-size` reports statics
  only. `hub75-diag` has no explicit stack sizing: `cortex-m-rt` puts
  `_stack_start` at the top of RAM and it grows down toward the statics, which
  end at `0x2001_4898`, leaving 440,168 B (429.9 KiB) unclaimed between the two.
  The core-0/core-1 stack lines above are the spec's intent, not anything the
  linker enforces yet.
- **`flip-link` is not wired up.** SPEC §2 calls for it precisely so a stack
  overflow faults instead of quietly eating `.bss`; `hub75-diag/.cargo/config.toml`
  passes only `--nmagic`, `-Tlink.x`, `-Tdefmt.x`. Until it is added, a deep
  call chain corrupts `FRAMEBUFFERS` silently. TOOLCHAIN.md has the two-line
  change; it moves symbol addresses, so it lands with a re-measure of this
  table. Scheduled for Phase 3 with the app shell.
- **512 KiB vs. 520 KiB.** `memory.x` declares `RAM : LENGTH = 512K` — the
  contiguous striped banks. The RP2350's other 8 KiB (two non-striped 4 KiB
  banks) is not in the linker's map, so it cannot be spent without a
  deliberate section placement. The headroom target is stated against 520 KiB
  per the spec; the 512 KiB figure is what is actually reachable today, and
  both clear 40 %.
- **defmt/RTT is dev-only.** The 1,078 B of `defmt_rtt` + `_SEGGER_RTT` above
  leave the release image; the deployed build spends its logging RAM on the
  ring buffer in the "Ring log + misc statics" line instead.
