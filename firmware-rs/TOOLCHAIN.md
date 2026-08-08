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
numbers CI reports and can introduce clippy lints that fail an unrelated PR. If
that becomes a nuisance, freeze it:

```toml
channel = "1.92.0"   # instead of "stable"
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
| `cargo build --release` | `firmware-rs/hub75-diag` | the device binary |

`firmware-rs/hub75-diag` is a **standalone workspace** (an empty `[workspace]`
table in its `Cargo.toml`) with its own `Cargo.lock`. It has to be: it depends
on embassy-rp, which only builds for the device, so it cannot be a member of a
root workspace that also builds on the host. Its `.cargo/config.toml` sets
`build.target`, so `--target` is optional there — CI passes it anyway to keep
the artifact path unambiguous.

Both lockfiles are committed and CI builds `--locked`. A stale lockfile is a CI
failure, not a silent update.

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
story is a RAM ring buffer served over `/api/logs` instead.

## flip-link — not yet enabled

SPEC §2 calls for `flip-link` so a stack overflow faults instead of quietly
corrupting `.bss`. It is **not wired up**: `hub75-diag` links with only
`--nmagic`, `-Tlink.x`, `-Tdefmt.x`, so the stack grows down from the top of RAM
toward the statics with nothing between them. Today that is theoretical —
there is 429.9 KiB of slack below the stack (BUDGET.md) — but it stops being
theoretical once the app shell has real task stacks.

To turn it on, install it and name it as the linker for the device target:

```sh
cargo install flip-link
```

```toml
# firmware-rs/<binary>/.cargo/config.toml
[target.thumbv8m.main-none-eabihf]
linker = "flip-link"
```

It inverts the memory layout so the stack sits *below* the statics and an
overflow runs off the bottom of RAM into a fault. Enabling it changes symbol
addresses, so re-measure BUDGET.md in the same PR. CI would also need the
`cargo install`. Scheduled for Phase 3 with the app shell.

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
cd firmware-rs/hub75-diag
cargo build --release --locked --target thumbv8m.main-none-eabihf
arm-none-eabi-size -B -d target/thumbv8m.main-none-eabihf/release/hub75-diag
```

Needs `binutils-arm-none-eabi` (the Arm GNU toolchain ships it on Windows).
`cargo size` from `cargo-binutils` works equally well; CI uses the GNU tool so
its numbers and BUDGET.md's cannot drift apart. `.github/workflows/rust.yml`
runs this on every push and echoes the section table plus the largest RAM
symbols into the job summary.

The release profile keeps `debug = 2` for probe-rs, so `size -A` also lists
`.debug_*` and a `Total` that counts them — neither is on the device. Read the
`-B` output, or the filtered table CI prints.

**Any PR adding a static ≥ 1 KB updates BUDGET.md in the same PR.**
