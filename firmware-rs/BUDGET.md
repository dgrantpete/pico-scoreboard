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
| cyw43 driver state | 17,928 | **MEASURED** | `CYW43_STATE` 12,696 B (the driver's ioctl state and its 4 + 4 packet channel) plus the runner task's 5,232 B arena. Came in 9 % over SPEC §11's 16 KB guess. |
| embassy-net stack + captive-portal sockets | 12,868 | **MEASURED** | `StackResources<8>` 4,584 B + the net runner's 136 B arena, plus the two AP-mode responders: 4,100 B of payload buffers, 400 B of packet metadata, and 3,920 B of task arenas (the DHCP and DNS scratch live in the arenas, not on a stack). **The TCP socket buffers are not in this line** — poller, HTTP ×2 and OTA are tasks #10/#11/#15 and bring their own. |
| Provisioning (`net::bringup` arena) | 4,400 | **MEASURED** | The boot-time `Store` (2,880 B) plus the scan's 64-entry BSSID table and the credentials. Task-arena, so it is statically allocated but has no reader after the boot. |
| Receive/scratch buffers (wire, HTTP, OTA chunk) | ~40,960 | ESTIMATE | Unioned where phases cannot overlap (OTA vs. poll). |
| Glyph/font tables + compiled sprites | 0 | **MEASURED** | `crates/scoreboard-render`. **9,538 B of flash, 0 B of RAM** — read out of the crate's own object code, not asserted. Breakdown and the command below. |
| Core-1 render-loop task arena | 1,392 | **MEASURED** | `scoreboard-app`. The loop's `LoopState` — frame rail, prepared view, skip memo, probe — plus the display and everything else held across the frame's await. It lives in the **task arena, not on the core-1 stack**: an embassy task's future is a static. See the correction below. |
| Core-0 task arenas | ~24,576 | ESTIMATE | 3,120 B measured today for the three placeholder tasks, and 2,968 B of that is the demo feed's own `ScoreboardSnapshot`, which leaves with it. The real arenas are net/poller/server/OTA. |
| Core-0 stack | — | **MEASURED (not a fixed line)** | With flip-link the stack is the whole remainder below the statics: **415,520 B** today, growing *down*, guarded by MSPLIM at the bottom of RAM. It is not a number to budget; it is what the rest of the table does not spend. |
| Core-1 stack | 8,192 | **MEASURED** | Sized 8 KB by `scoreboard-app`; **high-water 2,480 B (30 %)** across every scenario the frame probe drives. The setup screen, which is the only QR encoder caller and therefore the deepest frame, is not among them yet. Guarded by MSPLIM at its bottom. See the core-1 notes below. |
| Ring log + misc statics | ~8,192 | ESTIMATE | Measured floor today is 869 B — embassy-rp's GPIO/DMA/PIO wakers, the time driver, the critical-section lock, the clock cache, and the PAC singleton flags. The deployed RAM log (SPEC §9) is the bulk of this line. |
| **Projected total** | **204,349** | | **199.6 KiB** |

**Headroom: 61.6 %** (328,131 B free of 532,480 B). Against the 512 KiB the
linker actually declares — see the caveat below — it is 61.0 %. The ≥ 40 %
target holds with room to spare: the remaining estimate may overrun by
114,131 B before it is breached.

The projection *fell* by 42,520 B when Phase 3's networking landed, because two
of SPEC §11's three network guesses were high. The stack and its AP-mode
sockets measured 12,868 B against a 49,152 B estimate — the estimate was
counting TCP socket buffers for the poller, the HTTP server and OTA, which are
real but belong to tasks that have not landed and will be measured with them.
Treat the remaining 61.6 % as provisional until they have.

`SnapshotChannel`'s 8,552 B is now a symbol in a real device ELF, and it came
out byte-for-byte as the host predicted. `Store` and `Slate` are still host
struct sizes — nothing links them until the poller lands — but the same argument
applies: every bounded string in the snapshot carries a `u16` length prefix
rather than a `usize`, so the layout is byte-identical on the host and on
`thumbv8m.main-none-eabihf`.

**Measured today: 143,568 B** (140.2 KiB, 73.0 % headroom) — the whole of
`scoreboard-app`, which is the binary of record. That figure includes 4,150 B
of dev-only defmt/RTT and 2,968 B of demo scaffolding, both of which leave.
Networking added 35,848 B of it.

**Flash: 502,508 B**, 32.7 % of the 1,536 KB active partition — up from
127,948 B, and **232,803 B of the increase is the CYW43's own firmware**
(`43439A0.bin` 231,077 + CLM 984 + board NVRAM 742), which is `include_bytes!`
into `.rodata` and costs no RAM. It is also 232 KB that every OTA image carries
and every OTA transfer moves, which is worth knowing before Phase 4 sizes its
download budget.

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
tight, that is the lever; at 53.6 % headroom it is not worth the coupling.

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

### Correction to SPEC §11 — the core-1 loop state is arena, not stack

SPEC §11 budgets a "Core-1 stack, 8 KB, render loop only" and says nothing about
where the loop's own state lives. On embassy it is not the stack: a task's
future is a `static` in the task's pool, so `LoopState` — the frame rail, the
`PreparedView` with its 343 B QR bitmap, the skip memo, the frame probe — plus
the `Hub75Display` and the channel reader all sit in a 1,392 B arena that
`arm-none-eabi-nm` reports as `render_loop::POOL`.

That is why 8 KB of stack turns out to be generous: with the cross-frame state
elsewhere, the stack only carries renderer call frames and the one deep
transient, `QrBitmap::encode`'s two 211 B Reed-Solomon buffers. Measured
high-water is **2,480 B**, stable to within 4 B across every scenario.

**The QR gap is now closed.** That figure was recorded with a caveat: the setup
screen is the only caller of `QrBitmap::encode` and it was not in the probe's
rotation. Phase 3's networking put it there — an un-provisioned device boots
straight into it — and the setup screen measures **2,476 B**, four bytes *under*
the deepest demo scenario. The QR encoder is not the deepest frame after all;
the live game screens are. The 8 KB stays as sized regardless.

---

## Core 1: measured frame times

**The Phase 3 acceptance measurement.** `scoreboard-render` dropped
MicroPython's strip pre-rendering on the thesis that a compiled glyph blit is
cheap enough to do inline every frame. On silicon (Pico 2 W, 150 MHz, release
profile, `firmware-rs/app`'s frame probe, 100 frames per scenario), it is:

| Scenario | frame mean | frame max | render mean | render max | rebuild |
|---|---:|---:|---:|---:|---:|
| Idle — static screen, every frame skipped | 0.07 ms | 5.92 ms | 0.48 ms | 0.48 ms | 15 µs |
| Startup — static screen, 6 commits in 5 s | 0.39 ms | 6.36 ms | 0.79 ms | 0.89 ms | 14 µs |
| **Final line score, three rows scrolling** | 6.46 ms | 6.84 ms | 1.17 ms | 1.31 ms | 15 µs |
| Final + sticky spinner toast (full-frame dim) | 6.81 ms | 7.30 ms | 1.51 ms | 1.81 ms | 15 µs |
| Menu — 5 rows, marquee every frame | 6.85 ms | 7.19 ms | 1.57 ms | 1.69 ms | 14 µs |
| **MLB live + 255-byte play flash scrolling** | 6.90 ms | 7.41 ms | 1.59 ms | 1.96 ms | 116 µs |

**Worst frame measured: 7.41 ms of the 50 ms budget — 15 %.** Zero overruns in
any scenario; core 1 held 20 FPS exactly (200 ticks per 10 s window) through a
5-minute soak, 58 consecutive scenario reports.

The thesis holds by a wide margin. The two cases MicroPython could not draw
inline are the two in bold: three line-score rows measured ~41 ms there and
**1.17 ms** here; a long play line measured over 50 ms there and **1.96 ms**
here (worst case, at the wire format's 255-byte ceiling). The strip pool, its
capacity invariants and its per-glyph fallback bought about 35× of something
this port does not need.

The `frame max` column for the two static scenarios is not a slow frame — it is
the one frame in a hundred that actually drew, sitting next to ninety-nine that
skipped. Read the `render` columns for what drawing costs.

### Re-measured with the network stack up

The numbers above were taken with core 0 running three placeholder tasks. The
question Phase 3's networking has to answer is whether core 0's real load —
cyw43's SPI runner, smoltcp's poll loop, DHCP, a TCP connection to the backend
— starves core 1's executor. It does not:

| Scenario | frame mean | frame max | render max | Δ frame max |
|---|---:|---:|---:|---:|
| Idle | 0.08 ms | 5.56 ms | 0.53 ms | −0.36 ms |
| Startup | 0.39 ms | 6.03 ms | 0.93 ms | −0.33 ms |
| Final line score | 6.35 ms | 6.72 ms | 1.48 ms | −0.12 ms |
| Final + spinner toast | 6.68 ms | 7.20 ms | 2.18 ms | −0.10 ms |
| Menu | 6.70 ms | 7.18 ms | 1.73 ms | −0.01 ms |
| MLB live + 255-byte play flash | 6.72 ms | 7.15 ms | 1.98 ms | −0.26 ms |

Every window reported exactly **200 ticks per 10 s — 20.0 FPS — with zero
overruns**, across the station bench run, the AP-fallback run, and the
un-provisioned run. The deltas are all *negative* and all within the build noise
the section below warns about (flash placement moves the XIP cache under
hub75's packing loop); the honest reading is not "networking made rendering
faster" but "networking costs core 1 nothing that this instrument can see".

That is the expected result rather than a lucky one. The two cores share no
lock, no allocator and no data path except the snapshot channel's single atomic
swap and the brightness atomic; the network lives entirely on core 0's
executor, and core 1's frame is dominated (~76 %) by `show`, which is DMA setup
and a pure-CPU repack that touches neither.

### Where a frame actually goes

`show` — `load_rgb565` + `flip`, repacking 8,192 RGB565 pixels into eight BCM
bitplanes — costs **5.25 ms** and is flat across every scenario, because it does
not depend on what was drawn. That is **~76 % of a drawn frame**, and it is
`crates/hub75`, not the render path. Two consequences:

- The render budget question is settled and needs no further work. If frame time
  ever has to come down, the driver's packer is where the time is, not the
  glyphs (BACKLOG item 63). Nothing today justifies touching it — 7.41 ms
  against 50 ms is not a problem to solve.
- A *skipped* frame costs 0.07 ms, two orders of magnitude less than a drawn
  one. The static-screen skip is worth every line it costs.

`rebuild` (the prepared view, on commit frames only) is 14–16 µs for everything
except the 255-byte play line, where measuring 255 glyphs to size the flash
window takes **116 µs** — still a rounding error, and it happens once per
commit, not once per frame.

`brightness` — core 0 moving the atomic, core 1 re-deriving the driver's OE
timing stream — costs **112–138 µs mean, 190 µs worst**, and only on the frames
where the value actually changed (25 of every 100 ticks under the demo's
deliberately brisk sweep; a real light sensor moves far slower). It is not a
per-frame cost and it is not hiding in the render number.

One caution on precision: `show` moved from 5.07 to 5.25 ms across two builds
that differed only by a reordered statement and 120 B of code. Flash placement
moves the XIP cache under the hot packing loop, so treat the third significant
figure as build noise and compare orders of magnitude, not percentages.

Reproduce: flash `firmware-rs/app` and capture a full 30 s scenario cycle —

```sh
cd firmware-rs/app
cargo build --release --locked --target thumbv8m.main-none-eabihf
probe-rs download --chip RP235x target/thumbv8m.main-none-eabihf/release/scoreboard-app
timeout 80 probe-rs run --chip RP235x target/thumbv8m.main-none-eabihf/release/scoreboard-app > session.log 2>&1
```

The demo task that drives those scenarios is scaffolding for this measurement
and leaves with the real poller. Re-run it before it does.

---

## Measured breakdown — `scoreboard-app`, release, `thumbv8m.main-none-eabihf`

Standalone link profile, with flip-link. **502,508 B flash** (32.7 % of the
1,536 KB active partition), **9,080 B `.data`**, **130,392 B `.bss`**,
**4,096 B `.uninit`** → **143,568 B of RAM statics**.

| Symbol | Bytes | Owner |
|---|---:|---|
| `hub75::driver::FRAMEBUFFERS` | 65,536 | hub75 — the two BCM bitplane buffers |
| `scoreboard_app::FRAME` | 16,385 | app — RGB565 drawing surface (16,384 + 1 B `ConstStaticCell` flag) |
| `net::CYW43_STATE` | 12,696 | cyw43's ioctl state and its 4-deep packet channel in each direction |
| `scoreboard_app::CHANNEL` | 8,552 | the three-slot snapshot handoff. **In `.data`, not `.bss`** — see below |
| `scoreboard_app::CORE1_STACK` | 8,192 | core 1's stack; 2,480 B high-water |
| `net::cyw43_runner::POOL` | 5,232 | the radio runner's arena — its SPI scratch dominates |
| `net::STACK_RESOURCES` | 4,584 | `StackResources<8>` — smoltcp's `SocketSet` storage |
| `net::bringup::POOL` | 4,400 | provisioning's arena: the boot `Store` (2,880 B) + the scan's BSSID table |
| `defmt_rtt::BUFFER` | 4,096 | defmt ring, `.uninit`. **Dev-only** |
| `demo::feed::POOL` | 2,968 | the demo task's own `ScoreboardSnapshot`. **Scaffolding**; leaves with the demo |
| `net::dhcp_server::serve::POOL` | 2,752 | the DHCP task's two 1 KiB packet scratch buffers and its lease table |
| `display_core1::render_loop::POOL` | 1,392 | core 1's task arena — the loop state |
| `net::captive_dns::serve::POOL` | 1,168 | the DNS task's 512 B query and 528 B response scratch |
| `net::{dhcp_server,captive_dns}::{RX,TX}_BUFFER` | 4,100 | 4 × 1,025 — the four UDP socket payload buffers |
| `net::…::{RX,TX}_META` | 400 | UDP packet metadata: 8 slots each for DNS, 4 each for DHCP |
| `embassy_rp::gpio::BANK0_WAKERS` | 240 | embassy-rp |
| `net::net_runner::POOL` | 136 | smoltcp's driver arena |
| `embassy_rp::dma::CHANNEL_WAKERS` | 128 | embassy-rp, for CH0 |
| `embassy_rp::pio::…::WAKERS` | 96 | embassy-rp, for PIO2 |
| `supervise::liveness::POOL` + `net::watch_link::POOL` + `demo::brightness::POOL` | 224 | |
| `.L_MergedGlobals` | 120 | hub75 driver statics (73 B) + embassy/defmt/probe singletons |
| `hub75::driver::TIMING_BUFFER` | 64 | the OE/BCM timing stream |
| `net::hosts::HOSTS` | 64 | the names the HTTP server answers to |
| `_SEGGER_RTT` + `defmt_rtt::NAME` | 54 | `.data`, RTT control block. Dev-only |
| everything else (wakers, flags, padding) | ~490 | |

**The flash number is mostly not code.** `.rodata` is 269,660 B, and 232,803 B
of that is the CYW43's firmware, CLM and board NVRAM — `include_bytes!`
(`firmware-rs/app/cyw43-firmware/`). The radio has no flash of its own, so the
host uploads the image over SPI at every boot; there is no way to not carry it.
Phase 4 should note that every OTA image and every OTA transfer includes those
232 KB.

**`CHANNEL` is initialized data, so it costs its 8,552 B twice** — once in RAM
and once in flash, plus a boot-time copy. `ScoreboardSnapshot::new()` sets
`UiColors` to white, so the initializer is not all-zero and the linker cannot
put it in `.bss`. Harmless at 0.5 % of the active partition, and noted here so
the flash number is not a surprise later.

Reproduce (needs `binutils-arm-none-eabi`; CI runs exactly this):

```sh
cd firmware-rs/app
cargo build --release --locked --target thumbv8m.main-none-eabihf
ELF=target/thumbv8m.main-none-eabihf/release/scoreboard-app
arm-none-eabi-size -B -d "$ELF"
arm-none-eabi-nm -C --size-sort -r -S -td "$ELF" | awk '$3 ~ /^[bBdD]$/'
```

---

## Measured breakdown — `hub75-diag`, release, `thumbv8m.main-none-eabihf`

The panel-driver reference point, kept so the hub75 numbers stay comparable
across commits. Links plain (no flip-link), which is why its symbol addresses
are not comparable with the app's.
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

- **Stacks are still not in `size`'s output**, but both are now measured
  another way. `arm-none-eabi-size` reports statics only. Core 1's stack is an
  8,192 B static painted with 0xAA before the core starts, and
  `supervise::liveness` reports the deepest byte touched every 10 s — 2,480 B.
  Core 0's is the remainder below the statics (379,672 B under flip-link, down
  35,848 B as networking took its share of the statics) and has no equivalent
  probe, because nothing sizes it: it is what is left. `hub75-diag` still has
  neither.
- **`flip-link` is wired up for `firmware-rs/app`**, and the numbers above are
  measured with it. Core 0's stack sits below `.bss`/`.data`
  (`_stack_start = 0x2006_5b20`, `_stack_end = 0x2000_0000`) and
  `install_core0_stack_guard()` arms MSPLIM at the bottom, so overflow faults
  rather than eating `FRAMEBUFFERS`. Core 1 gets the same protection by a
  different route: `spawn_core1` arms MSPLIM at the bottom of the static stack
  it is handed. `hub75-diag` still links plain — a bench binary with one shallow
  task and 429.9 KiB of slack, where the guard buys little.
- **512 KiB vs. 520 KiB.** `memory.x` declares `RAM : LENGTH = 512K` — the
  contiguous striped banks. The RP2350's other 8 KiB (two non-striped 4 KiB
  banks) is not in the linker's map, so it cannot be spent without a
  deliberate section placement. The headroom target is stated against 520 KiB
  per the spec; the 512 KiB figure is what is actually reachable today, and
  both clear 40 %.
- **defmt/RTT is dev-only.** 4,150 B in the app (a 4 KB ring, raised from the
  default because two cores log into it, plus the control block) and 1,078 B in
  `hub75-diag`. Both leave the release image; the deployed build spends its
  logging RAM on the ring buffer in the "Ring log + misc statics" line instead.
- **The demo feed is scaffolding.** 2,968 B of the app's arena is a
  `ScoreboardSnapshot` inside the placeholder core-0 task that exists to drive
  the frame probe. It leaves when the real poller lands, and the poller will
  want a `Store` (2,880 B) in roughly the same place — so treat it as a
  placeholder for that line rather than as a saving.
