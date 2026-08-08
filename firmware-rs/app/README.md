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
  main.rs           init, the two-PAC ownership contract, spawning both cores
  display_core1.rs  the render loop: 20 FPS deadline pacing, snapshot latch,
                    prepared-view rebuild, static-screen skip, frame_seq
  probe.rs          the frame-time probe (the Phase 3 acceptance instrument)
  demo.rs           PLACEHOLDER core 0 — cycles six scenarios so the probe has
                    something to measure. Leaves with the real poller.
  supervise.rs      core-1 liveness + stack high-water. The watchdog feeds
                    from this signal once SPEC §12 lands.
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

The portal's pure half — DNS answer construction and the `Host`-header check —
is `crates/scoreboard-portal`, so it is host-tested; the firmware keeps the
sockets.

`ota.rs`, `storage.rs`, `inputs.rs` and the HTTP server from SPEC §2's tree are
the remaining Phase 3/4 tasks and land beside these.

## Build, flash, watch

```sh
cargo build --release                       # standalone profile (Phase 3)
cargo run --release                         # flash + attach, via probe-rs
cargo run --release --features net-probe    # ...and fetch {api}/time once
```

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

**Nothing on core 1 mutates a `static`.** All cross-frame state is
`display_core1`'s `LoopState`, a local; renderers receive `WallMs` and
`FrameElapsed` *values* and cannot name it. `FRAME_SEQ` and `BRIGHTNESS` are the
two deliberate cross-core atomics. `scoreboard-render`'s crate docs carry the
full table this comes from.
