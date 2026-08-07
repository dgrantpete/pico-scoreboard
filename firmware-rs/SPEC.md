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

- Stable Rust, `thumbv8m.main-none-eabihf` target.
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
  - `static SNAPSHOTS: [ScoreboardSnapshot; 2]` in `StaticCell`s; core 0 writes the inactive buffer, then publishes by storing its index into an `AtomicU8` (release ordering); core 1 loads (acquire) at the top of each frame and renders from that reference for the whole frame.
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
- Text scrolling, animations, brightness curves: direct port; assert frame-time headroom with a defmt timing probe around the render loop (budget: ≤ 50 ms per frame at 20 FPS; expected: low single-digit ms).
- Host tests: golden-image tests per sport/state using `backend/testdata/*.json` replayed through the backend encoder → wire bytes → firmware decode → render into the simulator buffer. This is the parity harness for the whole port.

---

## 7. Networking

### 7.1 Stack

`cyw43` (with `cyw43-firmware` blobs checked in) + `embassy-net` (smoltcp). Buffer sizes are budget lines (§11): sockets for {poller, HTTP server ×2, DNS, OTA}. DHCP client in station mode; static 192.168.4.1/24 + DHCP server behavior in AP mode as today.

### 7.2 Provisioning / captive portal

Port of the `main.py` flow: try station with stored credentials (bounded retries, status-string mapping) → on failure, AP mode with SSID per current scheme, QR on panel, captive DNS answering all A queries with the AP IP (`captive_dns.rs`, ~100 lines on a UDP socket), HTTP catch-all redirecting hijacked Host headers to `/#/setup`, exactly mirroring today's semantics (legit Host → 404, foreign Host → 302).

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
- **Flash layout (4 MB, RP2350; sizes to confirm against actual image size in Phase 3):** boot 32 KB · state 8 KB · active 1.5 MB · DFU 1.5 MB · storage region (§9) the remainder. Encoded in `memory.x`, generated by `build.rs` from one constants file so app, bootloader, and OTA client can't disagree.
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

Maintained as `firmware-rs/BUDGET.md`, checked against `cargo size` in CI. Seed estimates to be replaced by measured values in Phase 1–2:

| Item | Est. | Notes |
|---|---|---|
| HUB75 BCM framebuffers (×2) | ~48 KB | 64×32×8 planes ×2 buffers — confirm vs. panel config |
| Snapshot double buffer | ~8 KB | bounded strings dominate; measure from model |
| embassy-net buffers + sockets | ~48 KB | poller + server×2 + DNS + OTA |
| cyw43 driver state | ~16 KB | |
| Receive/scratch buffers (wire, HTTP, OTA chunk) | ~40 KB | unioned where phases can't overlap (OTA vs. poll) |
| Glyph/font tables | 0 RAM | `&'static` in flash |
| Core-0 task arenas + stack | ~24 KB | |
| Core-1 stack | 8 KB | render loop only |
| Ring log + misc statics | ~8 KB | |
| **Total (headroom target ≥ 40 %)** | **~200 KB / 520 KB** | |

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

## Appendix A — Dependency audit table (fill at pin time)

| Crate | Ver | no_std | no-alloc | Buffers | Notes |
|---|---|---|---|---|---|
| … | | | | | |

## Appendix B — Parity checklist (fill during Phase 3 from MicroPython feature inventory)

Wi-Fi join/retry/status strings · AP fallback + QR + captive portal · SPA + every `api_routes.py` endpoint · poller ETag/backoff semantics · per-sport screens vs. golden corpus · menu + buttons + encoder · auto-brightness curve · watchdog behavior · OTA trigger endpoint · safe-mode semantics · time sync.
