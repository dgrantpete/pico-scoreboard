# scoreboard-app — the firmware binary

Embassy on both cores. Core 1 runs the render loop against `crates/hub75`;
core 0 owns the peripherals and, eventually, everything else. Standalone cargo
workspace (embassy-rp is device-only), same arrangement as `hub75-diag` and
`boot-spike`.

```
cyw43-firmware/  the radio's own firmware, uploaded over SPI at every boot.
                 Provenance and hashes in its README; 232 KB of flash, 0 RAM.
layout/   scoreboard-layout — flash/RAM constants + memory.x generation.
          THE source: build.rs reads it, and Phase 4's OTA path will too.
src/
  main.rs           init, the two-PAC ownership contract, spawning both cores.
                    Every flash access happens here, before spawn_core1 — see
                    storage.rs for why that is free and why it is not later
  display_core1.rs  the render loop: 60 FPS deadline pacing, snapshot latch,
                    prepared-view rebuild, static-screen skip, frame_seq
  probe.rs          the frame-time probe (the Phase 3 acceptance instrument)
  storage.rs        sequential-storage over the 980 KB region: the config
                    document and the crash breadcrumb. Blocking, on purpose
  config.rs         the running configuration, and where dev.toml still fits
  inputs.rs         PIO1, two state machines, a 50 ms drain. The decisions are
                    in crates/scoreboard-input
  brightness.rs     the 5 Hz auto-brightness loop, sole owner of the panel's
                    brightness
  veml7700.rs       the light sensor's four register writes and one read
  supervise.rs      core-1 liveness + stack high-water, the watchdog and its
                    health gate, the panic handler, and force_reset
  net/
    mod.rs          the resource map, cyw43 + embassy-net bringup, and the
                    boot path that hands off to whichever mode won
    wifi.rs         provisioning: three station attempts with main.py's exact
                    retry rules, then the open AP
    captive_dns.rs  the setup-mode DNS responder's socket loop
    dhcp_server.rs  the setup-mode DHCP server (embassy-net has no server;
                    MicroPython got lwIP's for free)
    hosts.rs        where task #10 reads the names this device answers to
    probe.rs        --features net-probe only: one plain-HTTP GET, to prove
                    the stack moves bytes. Never in a shipped build.
```

Two subsystems keep their decisions in host-tested crates and leave only the
hardware here. The portal's pure half — DNS answer construction and the
`Host`-header check — is `crates/scoreboard-portal`. The input half — the button
debounce **program itself**, the timestamp reconstruction, the short/long fold,
the league menu session and the brightness curve — is
`crates/scoreboard-input`, where the PIO program is replayed against
`tools/pio_sim.py`'s scenarios by a cycle-accurate interpreter.

`ota.rs` from SPEC §2's tree is the remaining Phase 4 task and lands beside
these.

## Build, flash, watch

```sh
cargo build --release                        # standalone profile (Phase 3)
cargo run --release                          # flash + attach, via probe-rs
cargo run --release --features net-probe     # ...and fetch {api}/time once
cargo run --release --features induce-panic  # ...and add POST /api/induce-panic
```

**Release, not debug, is the profile that exercises the crash path.** A debug
build keeps `panic-probe` (print over RTT, trap for the debugger); a release
build installs `supervise::panic`, which stashes a breadcrumb in `.uninit` RAM
and resets, and the next boot serves it at `GET /api/logs/previous`.

**`dev.toml` is only read when the storage region holds no configuration.** Once
the device has saved one — the first `PUT /api/config`, or a provisioning save —
the build's values are out of the picture, which is the point. To get back to a
device that reads `dev.toml` again, erase the region: `probe-rs erase --chip
RP235x`, then reflash.

Cargo compares `dev.toml`'s mtime, so restoring a file you moved aside needs a
`touch` or the build will keep the values from the run that did not see it.

To join a real network on the bench, copy `dev.example.toml` to `dev.toml`
(**gitignored — it holds a real passphrase**) and fill it in. With no such file
the image boots into AP setup mode, which is the un-provisioned path a device
out of the box takes. See `firmware-rs/TOOLCHAIN.md`.

`cargo run` needs a Raspberry Pi Debug Probe on SWD. To capture a session
instead of watching it:

```sh
timeout 80 probe-rs run --chip RP235x \
  target/thumbv8m.main-none-eabihf/release/scoreboard-app > session.log 2>&1
sed -e 's/\x1b\[[0-9;]*m//g' session.log
```

80 s covers two and a half full scenario cycles. Build/flash details,
`flip-link`, and the second link profile are in `firmware-rs/TOOLCHAIN.md`;
the measured numbers are in `firmware-rs/BUDGET.md`.

## Two things that are contracts, not code

**`hub75` owns PIO0 and DMA channels 12-15; the radio owns PIO2 and DMA CH0.**
`hub75` reaches its silicon through `rp235x-pac` while embassy-rp reaches
everything else through `rp-pac`, and neither PAC can see the other's
bookkeeping. `main` parks embassy's handles for the panel's silicon in a binding
that never releases, so taking them back is an edit to one visible line rather
than a silent double-claim. The full map, including why `dma::Channel::new`
writing `DMA.INTE0` is safe next to a driver that also drives DMA, is in
`net`'s module docs. PIO1 is unclaimed and reserved for the buttons.

**Every flash access is a frame the panel does not draw.** A program or erase
runs from RAM with XIP disabled, and embassy-rp arranges that by parking core 1
for the duration. Measured: one 942 B config save takes its frame to 14.5 ms,
which fit a 50 ms budget with room and fits the 16.7 ms one with 2.1 ms to
spare — the tightest margin the firmware has (BUDGET.md). That is why `storage`'s
API is blocking rather than `async` — an `async fn` would suggest other tasks
run meanwhile, and they do not — and why every boot-time read happens before
`spawn_core1`, where parking core 1 costs nothing because there is no core 1.

**Nothing on core 1 mutates a `static`.** All cross-frame state is
`display_core1`'s `LoopState`, a local; renderers receive `WallMs` and
`FrameElapsed` *values* and cannot name it. `FRAME_SEQ` and `BRIGHTNESS` are the
two deliberate cross-core atomics. `scoreboard-render`'s crate docs carry the
full table this comes from.


## The two link profiles

`link-standalone` (default) links at flash offset 0 with no bootloader.
`cargo run` over a probe, nothing staged, nothing swapped — the Phase 3 world,
and still the bench. **It has no OTA install path and never will**: the
`__bootloader_*` symbols are deliberately not emitted for it, so building the
updater against it is a link error, which is the intended outcome. It can still
*check* for an update; it just answers that this build cannot take one.

`link-boot-integrated` links at the active partition behind `firmware-rs/boot`.
It is the only profile that can install an update, it carries ~74 KB more
(ed25519, sha2, embassy-boot), and it inherits an 8 s watchdog that was already
running before its first instruction.

```sh
cargo run --release                                                   # standalone
cargo build --release --no-default-features --features link-boot-integrated
python ../../tools/build.py publish-fw --channel dev --deploy         # the real path
```

Both profiles stop below the storage region, so a configuration written under
one is read back under the other — which is what makes the flip, and the
migration of the living-room unit, safe. `firmware-rs/layout`'s
`neither profile reaches into storage` test is the guarantee.
