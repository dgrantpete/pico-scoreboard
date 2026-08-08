# Firmware Toolchain

How to build, flash, and read logs from the Rust firmware. The design rationale
lives in SPEC.md §2; this is the operator's copy. RAM/flash numbers and how to
reproduce them are in BUDGET.md.

## The pin

`rust-toolchain.toml` at the repo root pins the **stable** channel and declares
`thumbv8m.main-none-eabihf` plus `clippy`/`rustfmt`. rustup materializes all of
it on the first `cargo` invocation, so a fresh clone needs no `rustup target
add`. CI adds the same target and component explicitly anyway — idempotent, and
it keeps the job working if the file is ever moved.

Stable floats deliberately. The cost is that a compiler bump can shift the size
numbers CI reports and can introduce clippy lints that fail an unrelated PR.

**Both costs came due in Phase 3, task #10, and floating was the right call.**
picoserve `0.19.0` declares `rust-version = "1.93"`, so `cargo` refused to build
it against the 1.92.0 that happened to be installed locally — while CI, which
materializes whatever `stable` currently is, had already moved to 1.97.1. The
pin was not wrong; the local toolchain had simply gone stale, and `rustup update
stable` was the whole fix. Two things then surfaced, both of which CI would have
caught first:

- `clippy::manual_checked_division` is new since 1.92 and failed `-D warnings`
  on `probe.rs`'s frame-time mean. Fixed, not allowed.
- The size numbers moved, which is why BUDGET.md is re-measured in the same
  commit.

The lesson for the next stale checkout: if `cargo` reports `rustc X is not
supported by the following package`, the answer is almost always `rustup update
stable` rather than pinning the dependency back.

If floating does become a nuisance, freeze it:

```toml
channel = "1.97.1"   # instead of "stable"
```

and re-measure BUDGET.md in the same PR, since the numbers are only comparable
across commits when the compiler is fixed.

**Divergence worth knowing:** `backend/Dockerfile` builds the deployed backend
on `rust:1.91-bookworm`, so the shipped service is not built with the toolchain
CI validates. That is pre-existing and harmless today (edition 2024 has been
stable since 1.85), and `rust-toolchain.toml` does not reach the image — the
build context is the repo root but `Dockerfile.dockerignore` is an allowlist
(`*` then `!Cargo.toml`, `!crates/**`, …) that never lets the file in. Worth
revisiting if the backend ever starts depending on a newer language feature.

## What builds where

| Command | From | Builds |
|---|---|---|
| `cargo test --workspace` | repo root | backend + `crates/*` on the host |
| `cargo build -p scoreboard-wire -p hub75 --target thumbv8m.main-none-eabihf` | repo root | the `no_std` crates, cross-compiled |
| `cargo build --release` | `firmware-rs/app` | **the firmware** |
| `cargo build --release` | `firmware-rs/hub75-diag` | the panel bench binary |

`firmware-rs/app` and `firmware-rs/hub75-diag` are **standalone workspaces** (an
empty `[workspace]` table in each `Cargo.toml`) with their own `Cargo.lock`.
They have to be: they depend on embassy-rp, which only builds for the device, so
they cannot be members of a root workspace that also builds on the host. Their
`.cargo/config.toml` sets `build.target`, so `--target` is optional there — CI
passes it anyway to keep the artifact path unambiguous.

`firmware-rs/app/layout` (`scoreboard-layout`) is a path dependency *inside* the
app's workspace directory, so cargo makes it a member automatically. It holds
the flash/RAM constants and generates `memory.x` from them, and it is the one
crate in that workspace with host tests:

```sh
cd firmware-rs/app
cargo test -p scoreboard-layout --features std --target x86_64-pc-windows-msvc
```

The explicit `--target` is needed because `build.target` points at the device.

All three lockfiles are committed and CI builds `--locked`. A stale lockfile is
a CI failure, not a silent update.

### The app's two link profiles

The same binary links at one of two flash addresses, selected by a feature and
turned into a `memory.x` by `build.rs`:

```sh
cargo build --release                                                  # standalone: 0x1000_0000
cargo build --release --no-default-features --features link-boot-integrated  # active partition: 0x1000_A000
```

Standalone is Phase 3's world: probe-flashed, no bootloader, `cargo run` works.
Boot-integrated is Phase 4's, and additionally emits the `__bootloader_*`
symbols embassy-boot's `FirmwareUpdater` reads. Both stop below the storage
region, so config written under one profile survives the switch to the other.
CI builds both.

### Bench credentials — `dev.toml`

Device config storage lives in flash and is task #12's. Until it exists, a
probe-flashed image learns a network from `firmware-rs/app/dev.toml`, which
`build.rs` reads into compile-time env vars. **The file is gitignored and must
stay that way** — it holds a real passphrase. `dev.example.toml` is the tracked
template:

```sh
cd firmware-rs/app
cp dev.example.toml dev.toml    # then fill in ssid / password
cargo run --release
```

**With no `dev.toml` the image builds fine** and boots straight into AP setup
mode, which is also the path a device out of the box takes — so its absence is
a supported configuration and CI needs no file. An unknown key in the file is a
build failure rather than a silently-ignored line, because a typo'd key and a
missing file produce the same symptom (a device in setup mode) and only one of
them is intentional.

The one function this replaces is `net::wifi::Credentials::from_dev_build`.

### `--features net-probe`

A bench instrument, never in a shipped build. Once station mode is up it fetches
`{DEV_API_URL}/time` over plain HTTP and logs the response, which is how the
network stack was shown to move bytes end-to-end against the real backend
before the client in task #11 existed.

```sh
cargo run --release --features net-probe
```

It is not the time sync: parsing the timestamp, setting the clock offset, and
keeping `utc_offset: 0` distinct from "sync failed" are all task #11's.

## probe-rs — flash and debug

The primary loop. Install the CLI:

```sh
cargo install probe-rs-tools --locked
```

With a Raspberry Pi Debug Probe wired to SWD, `cargo run` flashes and attaches
in one step, because `firmware-rs/hub75-diag/.cargo/config.toml` sets:

```toml
runner = "probe-rs run --chip RP2350"
```

If probe-rs rejects the chip name, it is a version skew — older releases spell
it `RP2350A` or `RP235x`. `probe-rs chip list | grep -i rp23` settles it.

## defmt — logging

`defmt` + `defmt-rtt` carry log output over RTT on the same SWD connection;
`probe-rs run` prints it. Level is set at **compile time** by the `DEFMT_LOG`
environment variable, already defaulted in that same config file:

```toml
[env]
DEFMT_LOG = "info"
```

Changing it forces a rebuild of the crates that log — that is expected, not a
cache bug. Formatting stays on the host: the device transmits indices into a
string table kept in the ELF's `.defmt` section (18 B today), which is why
logging costs so little flash.

`panic-probe` with `print-defmt` routes panics through the same channel.

RTT costs 1,078 B of RAM (a 1,024 B ring in `.uninit` plus the control block in
`.data`). That is **dev-only** and leaves the release image; SPEC §9's deployed
story is a RAM ring buffer served over `/api/logs` instead. The app raises the
ring to 4 KB (`DEFMT_RTT_BUFFER_SIZE` in its `.cargo/config.toml`) because two
cores log into it.

### Capturing a session to a file

`probe-rs run --chip RP235x <elf> > session.log` works, redirected stdout and
all — verified on probe-rs 0.32 capturing the app's frame-time probe (2026-08-08).
It streams until interrupted, so wrap it in a `timeout` for a fixed-length
capture, and strip the ANSI colour codes if the log is going to be read as text:

```sh
timeout 80 probe-rs run --chip RP235x target/thumbv8m.main-none-eabihf/release/scoreboard-app \
  > session.log 2>&1
sed -e 's/\x1b\[[0-9;]*m//g' session.log
```

`boot-spike/demo/rtt_poll.py` exists because that session model does not survive
the *bootloader* demo: `probe-rs run` attaches to one image, and the spike needed
to attribute frames across resets, swaps and watchdog reboots. For a single
image that stays put, it is not needed.

## flip-link — enabled for the app

SPEC §2 calls for `flip-link` so a stack overflow faults instead of quietly
corrupting `.bss`. It is **on for `firmware-rs/app`** as of the Phase 3 shell:

```sh
cargo install flip-link
```

```toml
# firmware-rs/app/.cargo/config.toml
[target.thumbv8m.main-none-eabihf]
linker = "flip-link"
```

It inverts the memory layout so the stack sits *below* the statics and an
overflow runs off the bottom of RAM into a fault instead of eating the 64 KB of
hub75 framebuffers. Verify it took by checking that `_stack_start` is *below*
the top of RAM:

```sh
arm-none-eabi-nm target/thumbv8m.main-none-eabihf/release/scoreboard-app | grep _stack
```

Two things worth knowing:

- **It shells out to `rust-lld` by name.** That binary is not on `PATH` from a
  normal shell (it lives in the toolchain's `lib/rustlib/<host>/bin`), but rustc
  adds it to the environment of whatever it invokes as the linker, so nothing
  has to be configured. `flip-link --version` from a plain shell fails with
  "Could not find the default linker" — that is the tool being run outside its
  job, not a broken install.
- **The `linker` key only works on a triple-keyed table**, not on the
  `[target.'cfg(...)']` form `hub75-diag` uses. The app therefore keys
  `linker`, `runner` and `rustflags` off the triple together, so there is no
  question about which table wins.

`hub75-diag` still links plain (`--nmagic`, `-Tlink.x`, `-Tdefmt.x`). It is a
bench binary with one shallow task and 429.9 KiB of slack; the guard is worth
having where the real task stacks are.

Core 1 does not go through flip-link at all — its stack is a static array
handed to `spawn_core1`. embassy-rp arms MSPLIM at that array's bottom, which
gives the same fault-on-overflow behaviour by a different mechanism.
`install_core0_stack_guard()` (called first thing in `main`) arms the same
register on core 0 at the bottom flip-link chose.

## picotool / UF2 — the probe-less fallback

Hold BOOTSEL while plugging the board in and the RP2350 enumerates as a USB
mass-storage device; copying a UF2 onto it flashes and reboots. `picotool`
converts a built ELF to UF2 (`picotool uf2 convert`, see `picotool help`) and
can also read back the binary-info block that `memory.x` reserves space for
(`.start_block` / `.bi_entries` / `.end_block`).

This path stays supported because it needs no hardware beyond a USB cable, and
because OTA images (§8) are built from the same ELF — as raw `bin`, not UF2.

## Size and the budget

```sh
cd firmware-rs/app
cargo build --release --locked --target thumbv8m.main-none-eabihf
arm-none-eabi-size -B -d target/thumbv8m.main-none-eabihf/release/scoreboard-app
```

`firmware-rs/app` is the binary of record for the budget; `hub75-diag` gets the
same treatment so the panel-driver numbers stay comparable across commits.
Needs `binutils-arm-none-eabi` (the Arm GNU toolchain ships it on Windows).
`cargo size` from `cargo-binutils` works equally well; CI uses the GNU tool so
its numbers and BUDGET.md's cannot drift apart. `.github/workflows/rust.yml`
runs this on every push and echoes the section table plus the largest RAM
symbols into the job summary.

The release profile keeps `debug = 2` for probe-rs, so `size -A` also lists
`.debug_*` and a `Total` that counts them — neither is on the device. Read the
`-B` output, or the filtered table CI prints.

**Any PR adding a static ≥ 1 KB updates BUDGET.md in the same PR.**
