# scoreboard-app — the firmware binary

Embassy on both cores. Core 1 runs the render loop against `crates/hub75`;
core 0 owns the peripherals and, eventually, everything else. Standalone cargo
workspace (embassy-rp is device-only), same arrangement as `hub75-diag` and
`boot-spike`.

```
layout/   scoreboard-layout — flash/RAM constants + memory.x generation.
          THE source: build.rs reads it, and Phase 4's OTA path will too.
src/
  main.rs           init, the two-PAC ownership contract, spawning both cores
  display_core1.rs  the render loop: 20 FPS deadline pacing, snapshot latch,
                    prepared-view rebuild, static-screen skip, frame_seq
  probe.rs          the frame-time probe (the Phase 3 acceptance instrument)
  demo.rs           PLACEHOLDER core 0 — cycles six scenarios so the probe has
                    something to measure. Leaves with the real poller.
  supervise.rs      core-1 liveness + stack high-water. The watchdog feeds
                    from this signal once SPEC §12 lands.
```

`net/`, `ota.rs`, `storage.rs` and `inputs.rs` from SPEC §2's tree are the
remaining Phase 3 tasks and land beside these.

## Build, flash, watch

```sh
cargo build --release                       # standalone profile (Phase 3)
cargo run --release                         # flash + attach, via probe-rs
```

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

**`hub75` owns PIO0 and DMA channels 12-15.** It reaches them through
`rp235x-pac` while embassy-rp reaches everything else through `rp-pac`, and
neither PAC can see the other's bookkeeping. `main` parks embassy's handles for
that silicon in a binding that never releases, so taking them back is an edit
to one visible line rather than a silent double-claim.

**Nothing on core 1 mutates a `static`.** All cross-frame state is
`display_core1`'s `LoopState`, a local; renderers receive `WallMs` and
`FrameElapsed` *values* and cannot name it. `FRAME_SEQ` and `BRIGHTNESS` are the
two deliberate cross-core atomics. `scoreboard-render`'s crate docs carry the
full table this comes from.
