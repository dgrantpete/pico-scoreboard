# pico-scoreboard Firmware Rewrite — Rust/Embassy Specification

**Status:** Draft for review · **Scope:** `firmware/` only — the backend stays as-is
**Target:** Raspberry Pi Pico 2 W (RP2350A + CYW43439), Cortex-M33, `thumbv8m.main-none-eabihf`
**Prime directive:** feature parity with the MicroPython firmware, then the stretch goals — never both at once.

---

## 1. Goals and non-goals

### Goals

1. Full feature parity with the current MicroPython firmware: Wi-Fi provisioning with AP fallback and captive portal, SPA web UI with the existing REST surface, backend polling over the packed wire format, per-sport rendering at 20 FPS on core 1, OTA with staged/trial/rollback semantics, watchdog + thread-health supervision, auto-brightness, buttons/encoder, QR setup flow.
2. **Single source of truth for the wire format**: one `no_std` crate consumed by both the backend and the firmware. `tools/wire_format_check.py` is retired.
3. **No-alloc policy**: no `#[global_allocator]` is installed. Total RAM usage = statics + stacks, verifiable at compile time. The escape hatch (allocator that forbids post-init allocation) exists as policy but is not used unless a dependency forces it — see §10.
4. A written, maintained **RAM budget** (§11) from day one.
5. Host-testable logic: everything that isn't touching a peripheral compiles and tests on the desktop.

### Non-goals (for the parity release)

- Direct-to-ESPN standalone mode and the streaming PNG decoder. These are Phase S (§14) and must not leak requirements into the parity design beyond the seams noted there.
- Matter/HomeKit integration (bridged via Home Assistant per existing plan; no device-side work).
- Backend changes beyond: (a) consuming the shared wire crate, (b) serving signed whole-image OTA artifacts (§8).
- Supporting the original RP2040 Pico W. RP2350-only simplifies flash/boot decisions.

---

## 2. Toolchain and workspace

### Toolchain

Install/flash/logging instructions are `firmware-rs/TOOLCHAIN.md`; what follows
is the rationale.

- Stable Rust, `thumbv8m.main-none-eabihf` target — pinned in `rust-toolchain.toml`.
- `probe-rs` + Raspberry Pi Debug Probe for flash/debug; `defmt` + `defmt-rtt` for logging; `panic-probe` for panics; `flip-link` for stack-overflow protection (stack placed below `.bss`/`.data` so overflow faults instead of corrupting statics).
- `picotool`/UF2 remains available as the probe-less fallback and is the format OTA images are built from (bin, not UF2, for the OTA path).
- CI: `cargo build` for the firmware target, `cargo test` on host for all logic crates, `cargo size` output tracked per commit (budget regression check).

### Workspace layout

```
pico-scoreboard/
├── backend/                  # unchanged, now depends on crates/scoreboard-wire
├── crates/
│   ├── scoreboard-wire/      # no_std, no-alloc. THE wire format. Shared.
│   ├── scoreboard-model/     # no_std. Sport state models + display-facing view logic
│   ├── scoreboard-render/    # no_std. Layout, textfold, geometry, menu, glyph blitting
│   ├── hub75/                # no_std. The driver: PIO programs, BCM, DMA chaining
│   └── png-stream/           # (Phase S) no_std streaming PNG→sprite decoder
├── firmware-rs/
│   ├── boot/                 # embassy-boot bootloader binary
│   └── app/                  # the application binary
│       ├── layout/           # scoreboard-layout: flash/RAM constants, THE source
│       ├── build.rs          # font + SPA asset embedding, memory.x generation
│       └── src/
│           ├── main.rs           # init, task spawning, core-1 launch
│           ├── net/              # wifi.rs (provisioning), captive_dns.rs,
│           │                     # api_client.rs (reqwless), server.rs (picoserve)
│           ├── ota.rs            # FirmwareUpdater glue, manifest client
│           ├── storage.rs        # sequential-storage keys, config load/save
│           ├── supervise.rs      # watchdog, ThreadHealth, boot-fail counter
│           ├── inputs.rs         # buttons, rotary encoder, light sensor
│           └── display_core1.rs  # core-1 executor: render loop @ 20 FPS
└── firmware/                 # MicroPython tree, kept until Phase 4 sign-off
```

**Crate boundary rule:** `crates/*` never import embassy or touch hardware; they are pure `core` (+`heapless`) and fully host-testable. `firmware-rs/app` owns all peripherals and I/O and stays thin. This is the same discipline as the current `scoreboard/` package vs `main.py` split, made compile-enforced.

---

## 3. `scoreboard-wire` — the shared crate (Phase 0, ships before any firmware exists)

- Extract the normative definitions from `backend/src/wire.rs` into `crates/scoreboard-wire`: header layout (version byte, state byte), game-state codes, length-prefixed strings, game-list entries, `TeamColors`, `TeamState`, `PregameTeam`, `LastPlay`, per-sport payload layouts (MLB / NBA / soccer).
- `#![no_std]`, no `alloc`. Strings are represented as offsets/lengths into the caller's receive buffer (`&str` borrows) or bounded `heapless::String<N>` where ownership is required — decide per field from actual max lengths observed in `backend/testdata/` (add a host test that asserts the corpus fits the bounds).
- Two API surfaces: `encode` (used by backend; std-compatible because no_std code runs fine under std) and `decode` (used by firmware, and by backend tests for round-tripping).
- Error type mirrors today's `DeserializeError` including byte-offset context (`@29`-style) — this diagnostic has already earned its keep.
- Backend migrates to this crate immediately. **This step has standalone value even if the rewrite stalls**: wire drift becomes a compile error, and `wire_format_check.py` (≈1.4 k lines) is deleted.
- `WIRE_VERSION` stays 2; the rewrite must not require a wire bump.

---

## 4. Concurrency architecture

- **Core 0:** embassy executor. Tasks: net supervisor (wifi state machine), poller, HTTP server (picoserve, N=2 connections), OTA task, storage/log flush, brightness loop, input loop, watchdog feeder, time-sync.
- **Core 1:** second embassy executor (embassy-rp multicore), running exactly one task: the render loop. Core 1 never touches the network, storage, or the allocator-that-doesn't-exist. Its stack lives in SRAM allocated at spawn (embassy-rp `spawn_core1` takes an explicit static stack — size it in the budget, §11).
- **Core 0 → Core 1 handoff** replaces today's "immutable after construction, read by the display thread" convention with a compile-checked equivalent: a double-buffered snapshot —
  - `scoreboard-model`'s `SnapshotChannel`: **three** `ScoreboardSnapshot` slots in a `static`, one atomic index cell, ownership moving by atomic swap. Core 0 publishes; core 1 latches at the top of each frame and renders from that reference for the whole frame. The seed read "`[ScoreboardSnapshot; 2]`" — a double buffer races, because a latched frame outlives the publish that supersedes it and two commits can land inside one frame. Rationale and the measured cost: BUDGET.md, "Correction to SPEC §4".
  - `ScoreboardSnapshot` lives in `scoreboard-model`, contains no borrows into network buffers (bounded owned fields), and is `Copy`-free but `Sync`.
  - Anything higher-rate (brightness value) is a plain `AtomicU8`.
- MicroPython idiom → embassy mapping (for the mechanical port): `create_task` → `#[embassy_executor::task]` + `spawner.spawn`; `asyncio.Event` → `embassy_sync::signal::Signal`; `wait_for(x, t)` → `embassy_time::with_timeout`; `sleep_ms` → `Timer::after_millis`; `ticks_ms` arithmetic → `Instant`/`Duration`.

---

## 5. HUB75 driver (`crates/hub75`)

Port of `firmware/src/lib/hub75/`, same hardware strategy, new implementation:

- PIO programs re-expressed with `pio::pio_asm!` (compile-time assembly). Two state machines as today: data SM and row-address SM, latch-safe/latch-complete IRQ handshake preserved.
- **DMA:** embassy-rp's DMA API does not express the read-address-trigger chaining the current driver uses. The driver programs DMA control blocks directly against `rp235x-pac` (alias channels, `READ_ADDR_TRIG` at the documented offsets — the register constants come straight from the PAC, closing the loop with the SVD work). Wrap the unsafe PAC use inside the crate; the public API is safe.
- BCM bitplane framebuffers: `static` arrays, double-buffered, sized by `const` panel geometry (compile-time features or const generics for 64×32 / chained variants — match whatever the current build system parameterizes).
- Gamma: `const` LUT generated by a `const fn` or `build.rs` (replaces `gamma.py`).
- Public API mirrors the current `Hub75Driver`/`Hub75Display` split: driver owns PIO+DMA+timing; display exposes pixel/blit/fill on the back buffer plus `swap()`.
- The crate ships with a host-side "simulator" feature: same display API rendering into a plain in-memory buffer, so `scoreboard-render` tests can assert pixels on the desktop (replaces `tools/preview/` role for unit tests; the interactive preview tool can keep using it later).
- Brightness control (global dim via OE timing) preserved; hook for the auto-brightness loop is a single atomic, per §4.

**Risk note:** this is the subsystem with the most hardware-timing risk. It is deliberately scheduled first among firmware crates (Phase 1) and validated standalone with test patterns before anything else is built on it.

---

## 6. Rendering and application logic (`scoreboard-model`, `scoreboard-render`)

Mechanical port; largest volume, lowest risk:

- `state.py` → `scoreboard-model`: the sport/state machine, poller-facing update logic, snapshot construction. Plain enums + structs; the MicroPython "plain attributes, no property descriptors" performance contortion disappears.
- `display.py`, `screen_geometry.py`, `textfold.py`, `menu.py`, per-sport render modules (`mlb.py`, `nba.py`, `soccer.py`, `football.py`, `inning_half.py`) → `scoreboard-render`. Viper blit kernels become ordinary Rust.
- **Fonts:** retarget `tools/compile_fonts.py` to emit Rust (`build.rs` invokes it, or port the generator): `static GLYPHS: [Glyph; N]` with the same MONO_HLSB packing; `FontWriter` API preserved. All `&'static`, zero init cost, no import-time table building.
- **QR codes:** setup-flow QR (replaces `miqro`). Preferred: a no-alloc no_std QR crate; the encoder needs ~4 KB of scratch for the largest version used — provide it as a caller-owned buffer. If no suitable crate passes the no-alloc audit (§10), port the needed subset of miqro (it is small and the format is stable).
- Text scrolling, animations, brightness curves: direct port; assert frame-time headroom with a defmt timing probe around the render loop (budget: ≤ 50 ms per frame at 20 FPS; expected: low single-digit ms). **Measured on silicon, Phase 3: worst frame 7.4 ms, worst render 2.0 ms, zero overruns** — see BUDGET.md, "Core 1: measured frame times". Dropping MicroPython's strip pre-rendering was the right call by roughly 35×.
- Host tests: golden-image tests per sport/state using `backend/testdata/*.json` replayed through the backend encoder → wire bytes → firmware decode → render into the simulator buffer. This is the parity harness for the whole port. **Built and green — see `firmware-rs/PARITY.md`** for the verdict table, how time is pinned, and what is deliberately out of scope.

---

## 7. Networking

### 7.1 Stack

`cyw43` (with firmware blobs checked in) + `embassy-net` (smoltcp). Buffer sizes are budget lines (§11): sockets for {poller, HTTP server ×2, DNS, OTA}. DHCP client in station mode; static 192.168.4.1/24 + DHCP server behavior in AP mode as today.

**Built and measured on silicon, Phase 3.** The resource map, the socket table
and the reasoning behind both live in `firmware-rs/app/src/net/`'s module docs;
the sizes are BUDGET.md's. Three things this section did not anticipate:

- **The radio takes PIO2 and DMA CH0.** PIO2 exists only on the RP2350, so
  §1's RP2040 ban buys the panel an undivided PIO0 and leaves PIO1 for the
  buttons. embassy's `dma::Channel::new` writes `DMA.INTE0`, which `hub75`
  also drives through the other PAC — safe because the write goes through the
  RP2350's atomic set alias and `hub75` never unmasks a DMA interrupt at all.
- **`cyw43_pio::DEFAULT_CLOCK_DIVIDER` is wrong for this chip.** It lands the
  GSPI clock at 37.5 MHz, which the RP2350 does not reliably survive
  (embassy-rs/embassy#3960). Use `RM2_CLOCK_DIVIDER` (÷3 → 25 MHz), as
  embassy's own rp235x example does.
- **The firmware blobs are vendored, not a crate.** The one `cyw43-firmware`
  crate on crates.io is a third-party republish that predates `cyw43` 0.7.0's
  NVRAM argument and ships no NVRAM blob, so it cannot satisfy the API. The
  three files, their upstream commit and their SHA-256s are in
  `firmware-rs/app/cyw43-firmware/README.md`. They cost **232,803 B of flash**
  and no RAM, and Phase 4 should count them in every OTA transfer.

### 7.2 Provisioning / captive portal

Port of the `main.py` flow: try station with stored credentials (bounded retries, status-string mapping) → on failure, AP mode with SSID per current scheme, QR on panel, captive DNS answering all A queries with the AP IP (`captive_dns.rs`, ~100 lines on a UDP socket), HTTP catch-all redirecting hijacked Host headers to `/#/setup`, exactly mirroring today's semantics (legit Host → 404, foreign Host → 302).

**Built and bench-validated, Phase 3**, with the AP-mode DHCP server that this
section forgot MicroPython was getting free from lwIP (decision record in
Appendix A). The retry semantics are `net::wifi`'s module docs, including the
table that maps every `wlan.status()` code the state machine acted on onto a
`cyw43::JoinError`, and the three deviations the change of mechanism forced.

The portal's two *decisions about bytes* — what a DNS answer looks like, and
whether a `Host` header names this device — live in `crates/scoreboard-portal`
rather than in the firmware, so they are host-tested (§2's crate-boundary rule).
The firmware keeps the sockets. Task #10 gets `MyHosts` ready-made; note the two
deviations its docs record, one of which fixes a latent `main.py` bug where a
station-mode request for an unknown path was answered with a redirect to
192.168.4.1.

**`.local` resolution is DHCP option 12, not mDNS.** embassy-net's `mdns`
feature is deliberately off: MicroPython only ever called
`network.hostname()`, so the name reaches clients through the router in both
firmwares. A device on a network whose router does not register DHCP hostnames
was never reachable by name, and still is not.

### 7.3 HTTP server (picoserve)

- Routes: port of `api_routes.py` + `main.py` handlers (status, config get/set, network scan/join, OTA trigger, memory/health stats — memory stats become static-budget + stack-watermark readouts instead of GC numbers).
- SPA: `index.html.gz` embedded via `include_bytes!` in `build.rs` from `frontend/`'s build output; served with `Content-Encoding: gzip` and a build-time ETag (hash computed in `build.rs`, replacing the runtime `_compute_index_etag`). The `/rom/` fallback path disappears — there is no filesystem.
- JSON request/response bodies: `serde` with `derive` into fixed/borrowed types (serde works no-alloc when deserializing to borrowed `&str`/bounded types); responses serialized into a caller-owned buffer.

### 7.4 Backend client (reqwless)

- Port of `api_client.py` + `poller.py`: plain-HTTP as today (the https→http downgrade becomes simply configuring an http URL; keep the config-side https URL rewrite behavior so existing configs migrate), `Accept: application/x-scoreboard-struct`, ETag/backoff/jitter semantics copied from the Python, decode via `scoreboard-wire` straight out of the receive buffer, snapshot publish per §4.
- Time sync: same backend endpoint as today over plain HTTP; feeds an `embassy_time`-anchored wall-clock offset (no RTC dependency).

---

## 8. OTA (the one subsystem that changes shape)

- **Model:** whole-image A/B via `embassy-boot` — bootloader partition + active + DFU + state, trial-boot with automatic rollback. This absorbs, and improves on, today's `apply_staged()` / `recover()` / boot-fail-counter logic; the counter machinery in `main.py` is retired in favor of the bootloader's revert, with one app-side duty: call `mark_booted()` only after a health gate (Wi-Fi up OR AP mode reached, render loop alive for N seconds) — this preserves today's "safe mode after repeated failures" intent.
- **Transport & trust:** plain HTTP; authenticity moves from the transport to the artifact. Backend signs images (ed25519); device verifies via embassy-boot's signature-verification feature with the public key baked into the app. **Device-side TLS is thereby removed from the parity scope entirely.**
- **Backend work (small):** `/app/manifest` gains image size/hash/signature/version fields; `/app/image` serves the signed binary. Signing key lives in backend deploy secrets; a `tools/` script signs as part of the release pipeline.
- **Flash layout (4 MB, RP2350):** CONFIRMED on hardware by the Phase 4 spike — see `firmware-rs/boot-spike/PARTITIONS.md`, which corrects this section's draft (embassy-boot requires DFU ≥ active + one 4 KB erase page): boot 32 KB · state 8 KB · active 1536 KB · DFU 1540 KB · storage 980 KB. Encoded in `memory.x`, generated by `build.rs` from one constants file so app, bootloader, and OTA client can't disagree.
- **Dev-loop note (accepted cost):** no more "push one .py"; every change is a full image. Mitigation is the probe (`cargo run` ≈ seconds) and the host-test-first workflow; the `dev_marker` concept survives as a config flag that pins the device to the staging manifest channel, as today.

---

## 9. Persistence (no filesystem)

`sequential-storage` (map API) over a dedicated flash region:

- Keys: wifi credentials, device config (port of `config.py` fields), OTA channel/dev flag, sticky user prefs (brightness lock, team follows — per current config surface).
- Writes are rare (config changes); reads happen once at boot into a `static` config struct — no steady-state flash traffic.
- **Logging changes shape:** `logger.py`'s file-flush model is replaced by defmt over RTT for development and a RAM ring buffer (fixed `heapless` deque) exposed via a `/api/logs` endpoint for deployed units — pull, not persist. Persistent crash breadcrumbs: a single small sequential-storage record written from the panic handler (panic message + snapshot of task watermarks), read and reported at next boot.
- Migration: first Rust boot finds no storage region → runs the normal "unprovisioned" path. No attempt to read MicroPython's littlefs; gift units get reprovisioned once. (Documented, acceptable.)

---

## 10. No-alloc policy and dependency audit

- No `#[global_allocator]` anywhere in the workspace. `alloc` is not linked; any dependency that requires it fails the build — that failure is the enforcement mechanism.
- Every third-party crate enters through an audit line in this spec's appendix: name, version, `no_std`? , allocates? , buffers caller-owned? Current expected set: embassy-{executor, rp, time, sync, net, boot}, cyw43, smoltcp, picoserve, reqwless, sequential-storage, heapless, static-cell, defmt, panic-probe, pio, rp235x-pac, serde(+derive), qr crate TBD. All are believed no-alloc-clean; the audit verifies at pin time.
- Bounded-capacity decisions (`heapless` `N`s, buffer sizes) cite their source: measured maxima from `backend/testdata/` corpus + margin, or protocol limits. Each lands in the RAM budget.
- Escape hatch (documented, unused): if exactly one indispensable crate demands `alloc`, install `embedded-alloc` over a small fixed arena wrapped in an allocator that panics after an `init_complete` flag — allocation becomes boot-only, fragmentation stays impossible. Adopting this requires editing this section, deliberately.

---

## 11. RAM budget (living document, seeded here)

**`firmware-rs/BUDGET.md` is the authoritative table** — it is maintained per PR
and checked against the size report in CI. What follows is the original seed;
consult BUDGET.md for what is measured and what is still a guess.

| Item | Est. | Notes |
|---|---|---|
| HUB75 BCM framebuffers (×2) | 64 KB | **Measured, Phase 1**: 2 × 32,768 B. The seed read "~48 KB, 64×32" — the shipping panel is 128×64 |
| Snapshot double buffer | ~8 KB | bounded strings dominate; measure from model |
| embassy-net buffers + sockets | ~48 KB | poller + server×2 + DNS + OTA |
| cyw43 driver state | ~16 KB | |
| Receive/scratch buffers (wire, HTTP, OTA chunk) | ~40 KB | unioned where phases can't overlap (OTA vs. poll) |
| Glyph/font tables | 0 RAM | `&'static` in flash |
| Core-0 task arenas + stack | ~24 KB | |
| Core-1 stack | 8 KB | render loop only |
| Ring log + misc statics | ~8 KB | |
| **Total (headroom target ≥ 40 %)** | **~232 KB / 520 KB** | 55 % headroom with the framebuffer line corrected — see BUDGET.md |

Rule: any PR that adds a static ≥ 1 KB updates the table in the same PR.

---

## 12. Supervision and health

- Hardware watchdog fed by a core-0 task gated on a `ThreadHealth`-equivalent: core 1 publishes a frame counter (atomic); net supervisor publishes liveness; feeder starves the watchdog if either stalls — port of today's semantics.
- Panic path: defmt + panic-probe in dev; in release, panic handler writes the breadcrumb (§9) then resets into the watchdog/trial-boot machinery, which is what ultimately triggers rollback after a bad OTA.
- `hardware_diagnostic.py`'s role becomes a `--features diag` build of the app (test patterns, input echo) rather than a separate script.

---

## 13. Migration plan

Parallel-track: the MicroPython firmware remains the shipping firmware until Phase 4 exit.

- **Phase 0 — Wire crate (pure win, do immediately).** Extract `scoreboard-wire`, backend consumes it, round-trip tests against `testdata/`, delete `wire_format_check.py`. *Exit: backend deployed on shared crate, zero wire diffs.*
- **Phase 1 — `hub75` crate.** PIO + PAC-level DMA + BCM on real hardware; simulator feature; test patterns, brightness, timing probes. *Exit: stable 20 FPS test animation, timings ≥ parity with MicroPython driver, budget lines measured.*
- **Phase 2 — Render stack on host.** `scoreboard-model` + `scoreboard-render` + fonts build pipeline; golden-image parity harness over the full testdata corpus. *Exit: pixel-parity (or reviewed-and-accepted diffs) across all sports/states — no hardware involved.*
- **Phase 3 — App shell on device.** Wi-Fi/provisioning/captive portal, picoserve + SPA, reqwless poller, storage, inputs, brightness, supervision. First end-to-end live game on Rust. *Exit: feature-parity checklist against the MicroPython firmware, one week of continuous soak on the dev unit.*
- **Phase 4 — Boot + OTA.** embassy-boot integration, partition layout, backend signing + endpoints, kill-and-rollback drills (pull power mid-write; ship a deliberately-broken image and watch it revert). *Exit: three consecutive successful OTA cycles + one induced rollback on the dev unit; then a ≥ 2-week soak before any gift unit is migrated.*
- **Phase S — Stretch (separately scoped):** `png-stream` crate (streaming inflate → box-downsample → sprite), direct-ESPN degraded fallback mode. Explicitly out of parity scope; only constraint on earlier phases: the poller's data-source trait should not hard-code "backend" in `scoreboard-model` (keep the source behind one interface so S plugs in).

Deliberate deviation from "smallest first": Phase 1 front-loads the riskiest hardware work so any nasty surprise arrives before the mechanical bulk is invested.

---

## 14. Risks

| Risk | Exposure | Mitigation |
|---|---|---|
| DMA chaining subtleties differ under PAC-level control | P1 | standalone driver phase; logic analyzer on latch/OE; keep MicroPython driver as reference oracle |
| embassy-boot × RP2350 image-definition/partition interactions | P4 | prototype boot+swap on a bare "blinky A/B" before integrating the app |
| A required crate demands `alloc` | any | audit at pin time (§10); escape hatch documented |
| picoserve/reqwless API churn (pre-1.0 ecosystem) | P3 | pin exact versions; upgrade deliberately, never incidentally |
| Bounded-string sizes wrong for real-world data | P2–3 | bounds derived from testdata corpus + assert-on-corpus test; truncation behavior defined in `scoreboard-wire`, never a panic |
| Dev-loop regression (no REPL, whole-image updates) | ongoing | host-first testing; probe workflow; simulator feature |

---

## Appendix A — Dependency audit table

Audited 2026-08-07 against the tree as it stands after Phase 2's render foundation:
`crates/scoreboard-wire`, `crates/scoreboard-model`, `crates/scoreboard-render`,
`crates/hub75`, and `firmware-rs/hub75-diag`. Versions are read from the two committed
lockfiles (root `Cargo.lock`; `firmware-rs/hub75-diag/Cargo.lock`, a standalone workspace).
Everything embedded is exact-pinned per §14. "Buffers" answers *are the buffers
caller-owned?* — `n/a` means the crate holds no runtime buffer at all.

**Direct dependencies, pinned today**

| Crate | Ver | Used by | no_std | no-alloc | Buffers | Notes |
|---|---|---|---|---|---|---|
| *(none)* | | scoreboard-wire | yes | yes | caller | The crate has **zero dependencies**. `#![no_std]`, decode borrows the caller's receive buffer, encode writes through a `Sink`. |
| `heapless` | =0.9.3 | scoreboard-model, scoreboard-render | yes | yes² | inline | ²Off-by-default `alloc` feature; not activated. Bounded strings and vectors, all inline in the owning struct. Strings carry a `u16` length so a snapshot has the same layout on the host and on `thumbv8m` — that is what makes the BUDGET.md figures host-measurable. |
| `scoreboard-wire` | path | scoreboard-model | yes | yes | caller | The model builds its bounded owned views straight out of the borrowed decode, with no intermediate owned copy. |
| `scoreboard-model` | path | scoreboard-render | yes | yes | inline | The renderer reads a snapshot and writes nothing back. |
| `qrcodegen-no-heap` | =1.8.1 | scoreboard-render | yes | yes | caller | Nayuki's reference QR encoder with every buffer moved to the caller — **zero dependencies**, `#![no_std]`, `#![forbid(unsafe_code)]`, no allocation of any kind. The two working buffers are stack arrays sized from `Version::buffer_len()` (211 B each at the version cap). Replaces `lib/miqro`, whose encoder shipped as opaque precompiled ARM `.mpy`; §6's "port the miqro subset" fallback was not needed. |
| `libm` | =0.2.16 | hub75 | yes | yes | n/a | `pow`/`floor`/`fmod` behind `Gamma::Power`. Pure computation, no state. |
| `pio` | =0.3.0 | hub75 | yes | yes | n/a | Compile-time only: `pio_asm!` assembles both PIO programs into a `Program<32>` (inline `ArrayVec`, no heap). Its `pio-proc` half is a proc macro — it allocates on the *host* at build time; nothing of it ships. `pio-core` pulls `arrayvec` with `default-features = false`. |
| `rp235x-pac` | =0.2.0 | hub75, hub75-diag | yes | yes | n/a | Register definitions; the driver's PAC-level DMA chaining. hub75-diag enables `critical-section`. See the two-PAC note below. |
| `embassy-executor` | =0.10.0 | hub75-diag | yes¹ | yes | n/a | Task arenas are statics (`POOL`, 768 B measured). ¹`no_std` unless `platform-std`/`platform-wasm` — host-test features, not enabled. Built with `platform-cortex-m`. |
| `embassy-rp` | =0.10.0 | hub75-diag | yes | yes | n/a | Unconditionally `#![no_std]`. Clocks, GPIO, time driver. |
| `embassy-time` | =0.5.1 | hub75-diag | yes¹ | yes | n/a | ¹`no_std` unless `std`/`wasm`. |
| `cortex-m-rt` | =0.7.6 | hub75-diag | yes | yes | n/a | Reset vector, `.bss`/`.data` init, `link.x`. |
| `static_cell` | =2.1.1 | hub75-diag | yes | yes | caller | `ConstStaticCell<FrameBytes>` backs the 16 KB frame static; adds a 1 B taken flag (BUDGET.md). |
| `defmt` | =1.1.1 | hub75-diag | yes | yes² | caller | ²**Ships an off-by-default `alloc` feature** (`Format` impls for `String`/`Vec`). Not activated anywhere — verified with `cargo tree -e features`. |
| `defmt-rtt` | =1.3.0 | hub75-diag | yes | yes | own static | 1,024 B ring in `.uninit` + 48 B control block. Dev-only, out of the deployed budget. |
| `panic-probe` | =1.0.0 | hub75-diag | yes | yes | n/a | `print-defmt`; one 1 B flag. |

**Networking, pinned in Phase 3** (`firmware-rs/app`; measured sizes in BUDGET.md)

| Crate | Ver | Used by | no_std | no-alloc | Buffers | Notes |
|---|---|---|---|---|---|---|
| `embassy-net` | =0.9.1 | scoreboard-app | yes¹ | yes | caller | ¹`no_std` unless `std`. Every buffer is caller-supplied: `StackResources<N>` is the socket table (4,584 B at N=8), and each socket's payload and metadata buffers are passed to its constructor. Features `tcp udp dns dhcpv4 dhcpv4-hostname proto-ipv4 medium-ethernet` — `mdns` deliberately off, see §7.2. |
| `smoltcp` | 0.13.1 | via embassy-net | yes | yes | caller | The TCP/IP stack itself. Its default feature set *is* `std`+`alloc`; embassy-net turns defaults off and selects socket types individually, so the no_std build is not accidental. `Ipv4Address` is a re-export of `core::net::Ipv4Addr`, which is what lets it, embassy-net and edge-dhcp share one address type with no conversion. |
| `cyw43` | =0.7.0 | scoreboard-app | yes | yes | inline | The radio driver. `State` is a caller-placed 12,696 B static; the packet path is an `embassy-net-driver-channel` with 4 buffers each way, inline in it. Features `defmt firmware-logs`; **`bluetooth` off**, which keeps `bt-hci` out of the tree. |
| `cyw43-pio` | =0.10.0 | scoreboard-app | yes | yes | n/a | The GSPI bit-banger. Compile-time PIO program, one state machine, one DMA channel. Use `RM2_CLOCK_DIVIDER`, not the default — see §7.1. |
| `edge-dhcp` | =0.8.0 | scoreboard-app | yes | yes | caller | The AP-mode DHCP server's packet codec and lease table. **`default-features = false`**, which drops its `io` module and with it `edge-nal`, `embassy-futures` and `embassy-time`; the socket loop is the firmware's. `Packet` borrows the decode buffer, options go into a caller-owned `[DhcpOption; 8]`, and leases live in a `heapless::LinearMap<_, _, 8>`. Audit row expanded below. |
| `edge-raw` | 0.8.0 | via edge-dhcp | yes | yes | caller | Byte cursors (`BytesIn`/`BytesOut`). With default features off it has **zero dependencies**. |
| `num_enum` | 0.7.6 | via edge-dhcp | yes | yes | n/a | Derives the DHCP option-code conversions. A proc macro plus a tiny trait crate; the macro half allocates on the *host*, nothing of it ships. |
| `rand_core` | 0.10.1 | via edge-dhcp | yes | yes | n/a | Trait definitions only, `default-features = false`. Not actually reached by the server path. |
| `scoreboard-portal` | path | scoreboard-app | yes | yes | caller | First-party. The captive portal's pure half — DNS answer construction and the `Host`-header check — so both are host-tested. `#![forbid(unsafe_code)]`, one dependency (`heapless`). |
| `embedded-io-async` | =0.7.0 | scoreboard-app | yes | yes | caller | **Optional, `net-probe` only.** The `Write` trait behind `TcpSocket::write_all` for the bench probe. Not in a shipped build; reqwless brings its own in Phase 3's client task. |
| `defmt` | 0.3.100 | via embassy-net, smoltcp, embassy-net-driver | yes | yes | caller | `cargo tree -d` reports defmt twice, and that is **expected, not drift**: 0.3.100 is upstream's compatibility shim, a crate whose only dependency is `defmt = "1"` and whose only content is re-exports of the 0.3 API. So there are two crate *versions* in the graph and one implementation, one wire format and one `.defmt` string table (147 B in the linked ELF) underneath. Re-check this on any defmt bump — two real defmts would silently split the string table and the decoder would reject the stream. |

**Transitive, load-bearing**

| Crate | Ver | Via | no_std | no-alloc | Buffers | Notes |
|---|---|---|---|---|---|---|
| `heapless` | 0.9.3 | embassy-rp | yes | yes² | caller | ²**Off-by-default `alloc` feature.** Not activated. The bounded-capacity workhorse for Phases 2–3. |
| `embassy-sync` | 0.8.0 | embassy-rp | yes¹ | yes | caller | ¹`no_std` unless `std`. |
| `critical-section` | 1.2.0 | embassy-rp, PAC | yes | yes | n/a | Impl supplied by embassy-rp's `critical-section-impl`; exactly one in the binary. |
| `cortex-m` | 0.7.8 | rp235x-pac | yes | yes | n/a | |
| `arrayvec` | 0.7.8 | pio-core | yes | yes | inline | Its default feature *is* `std`; pio-core turns defaults off, so the no_std build is not accidental. |
| `rp-pac` | 7.0.0 | embassy-rp | yes | yes | n/a | embassy-rp's own PAC, coexisting with `rp235x-pac`. See below. |

**Not yet pinned** — audit when each lands.

| Crate | Ver | Phase | Notes |
|---|---|---|---|
| `picoserve` | not yet pinned | 3 | HTTP server (§7.3) |
| `reqwless` | not yet pinned | 3 | Backend client (§7.4) |
| `sequential-storage` | not yet pinned | 3 | Persistence (§9) |
| `serde` (+`derive`) | not yet pinned | 3 | Must deserialize to borrowed/bounded types only |
| `embassy-boot`(-rp) | not yet pinned | 4 | OTA + signature verification (§8) |
| `flip-link` | not yet pinned | 3 | Linker wrapper, §2. **Not yet in use** — `hub75-diag` links without it, so stack overflow currently corrupts `.bss` instead of faulting. |
| `png-stream` deps | not yet pinned | S | Out of parity scope |

**The AP-mode DHCP server — decision record.** embassy-net has a DHCP *client*
and no server, and MicroPython got a server for free: `network.WLAN(AP_IF)
.active(True)` starts lwIP's `shared/netutils/dhcpserver.c` inside the port. The
behaviour is not optional — the captive portal works because a joining client is
handed 192.168.4.1 as its DNS server, and a phone with an address but no DNS
pointer sits on the setup network showing "no internet" and never opens a page.
Three options were weighed against §10:

| Option | Verdict |
|---|---|
| `edge-dhcp` with its `io` feature | **No.** `io` pulls `edge-nal`, which is a socket abstraction embassy-net does not implement — it would need `edge-nal-embassy` as well, three crates and an adapter layer to run a loop that is thirty lines. |
| `edge-dhcp` protocol-only, own socket loop | **Adopted.** `default-features = false` links the packet codec and the lease table and nothing else: `no_std`, no allocation, `Packet` borrows the caller's decode buffer, options go into a caller-owned array, leases into a `heapless::LinearMap` whose capacity we choose. Owning the loop is what keeps the never-die-on-a-malformed-packet rule identical to the captive DNS responder's, and it is where RFC 2131 §4.1's reply-destination rules live. |
| Hand-rolled, no dependency | Viable — the wire format is 236 fixed bytes plus TLV options, comparable in size to the captive DNS — but it would be a second BOOTP parser to get right and to test, for no reduction in the audit surface that matters. |

The adopted split is ~120 lines in `net::dhcp_server`, matched to
`dhcpserver.c` where MicroPython made a choice (pool `.16`–`.23`, 24 h lease,
subnet, router and DNS all pointing at the AP) and to the RFC where it did not
(MicroPython always broadcasts the reply; this unicasts a renewal to a client
that already holds an address).

**Findings.** No crate in the tree requires `alloc`, and no `#[global_allocator]`
is linked — the §10 policy holds today. Two answers are not unconditionally
clean and are worth re-checking at every version bump: **`defmt` and `heapless`
both carry an off-by-default `alloc` feature**, and Cargo feature unification is
additive, so a future dependency that enables either one turns it on for the
whole binary rather than for itself. The build would still fail (nothing links
an allocator), which is the enforcement §10 relies on — but the failure would
surface as a link error far from its cause, so the audit note is the early
warning.

Separately, `hub75-diag` links **two independent PACs**: `rp235x-pac` (hub75's
DMA/PIO register access) and `rp-pac` (inside embassy-rp). Each has its own
`Peripherals::take()` singleton guard — `DEVICE_PERIPHERALS` and
`_EMBASSY_DEVICE_PERIPHERALS` are separate 1-byte statics — so taking a
peripheral from one proves nothing about the other. `Hub75Driver::new`'s
by-value PAC ownership is therefore a proof *within* `rp235x-pac` only; the
"don't reconfigure RESETS/IO_BANK0/PADS_BANK0 concurrently" contract in its
docs is what actually holds the line. Worth revisiting in Phase 3 when more of
the chip is in use.

## Appendix B — Parity checklist (fill during Phase 3 from MicroPython feature inventory)

| Item | State |
|---|---|
| Per-sport screens vs. golden corpus | **Done** — `firmware-rs/PARITY.md` |
| Wi-Fi join / retry / status mapping | **Done, bench-validated** — `net::wifi` docs carry the status-code table and three deviations |
| AP fallback + setup reason + QR | **Done, bench-validated** — all three reasons reachable; `no_network_configured` and `bad_auth` demonstrated on silicon |
| Captive portal — DNS half | **Done** — `crates/scoreboard-portal::dns`, host-tested |
| Captive portal — DHCP server | **Done** — new work; embassy-net has no server, `dhcpserver.c` matched (Appendix A) |
| Captive portal — HTTP `Host` check | **Logic done and host-tested** (`scoreboard-portal::hosts`); wiring is task #10 |
| SPA + every `api_routes.py` endpoint | Task #10 |
| Poller ETag / backoff semantics | Task #11 |
| Time sync (incl. `utc_offset: 0` ≠ `None`) | Task #11 — the endpoint is reachable, proved by `--features net-probe` |
| Menu + buttons + encoder | Task #12 |
| Auto-brightness curve | Task #12 |
| Watchdog behaviour | Task #12 — core-1 liveness signal already published |
| OTA trigger endpoint + safe-mode semantics | Task #15 |
