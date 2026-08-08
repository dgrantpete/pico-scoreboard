# boot-spike — embassy-boot A/B trial-boot on RP2350, de-risked

The SPEC §13 Phase 4 / §14 risk-table spike: prove embassy-boot's whole-image
A/B model (swap, trial boot, watchdog revert, signature gate) on real RP2350
hardware *before* the app exists. **Outcome: it works** — see
`PARTITIONS.md` for the confirmed layout + findings and
`demo/transcripts/` for the captured on-hardware evidence.

Standalone cargo workspace (device-only targets, same arrangement as
`hub75-diag`):

- `layout/` — `spike-layout`: partition constants + `memory.x` generation,
  the single source of truth (Phase 4 lifts this).
- `boot/` — `spike-boot`: the embassy-boot bootloader (`#[entry]`, no
  executor). Arms an 8 s watchdog that stays armed across the jump.
- `app/` — `spike-app`: one binary, three demo personalities via features:
  `identity-a`/`identity-b` (who it says it is), `confirm` (health-gate +
  `mark_booted()`; a build *without* it plays the broken image), `stage`
  (embed a payload `.bin` from env `SPIKE_PAYLOAD` and stage it on mailbox
  command), `verify` (ed25519-gate staging; env `SPIKE_PAYLOAD_SIG`).
- `demo/` — host tooling: `build.py` (builds every image variant into
  `demo/out/`), `sign.py` (throwaway dev keypair + image signing),
  `rtt_poll.py` (SWD-side defmt capture + probe action driver),
  `decode.py` (raw captures → text transcripts).

## Build

```
python demo/sign.py keygen     # once; dev_priv.bin is gitignored
python demo/build.py
```

Requires the repo toolchain (stable + thumbv8m target via
`rust-toolchain.toml`), python3 with `cryptography` + `pyelftools`, and
`defmt-print` (`cargo install defmt-print`) for decoding.

Payload chain (breaks the A-embeds-B-embeds-A cycle): `a_bad` embeds
nothing → `b_confirm` embeds `a_bad.bin` → `a_stager`/`a_stager_verify`
embed `b_confirm.bin`.

## Why the demo is driven over SWD instead of `probe-rs run`

Three bench realities shaped the tooling; all are workarounds, not
firmware requirements:

1. **probe-rs 0.32 prints no RTT text when stdout is not a terminal**, so
   scripted `probe-rs run`/`attach` sessions capture nothing. Instead,
   `rtt_poll.py` reads the defmt rings straight out of RAM over SWD
   (`probe-rs read`) and `decode.py` runs them through `defmt-print`.
2. Each binary pins its defmt-rtt ring to a dedicated region at the top of
   RAM (see `spike-layout`), so bootloader and app frames are attributed
   by construction — across swaps, self-resets and watchdog reboots, in
   one continuous session. Boot-counter + ROSC nonce + boot-time-jitter
   lines make consecutive boots' bytes unique so the poller can detect
   ring re-initialization. (The ROSC `RANDOMBIT` samples are taken
   back-to-back and visibly correlated — fine for uniqueness here, not a
   real RNG.)
3. A killed probe-rs session can leave the core halted (watchdog paused —
   RP2350 `PAUSE_DBG*` defaults), producing a "hang" that is pure
   observation artifact. The poller never halts the core; if you
   fall back to manual probe-rs, `probe-rs reset` recovers.

The generous `asm::delay` pauses in the bootloader and the pre-reset
"drain" delay in the app exist only so the ~1 Hz poller never misses a
boot phase; production trims them to ~100 ms.

## Running the demo

Start the driver, then feed it commands by writing one line to
`demo/out/poller_cmd.txt` (it executes on the next tick and logs it):

```
python demo/rtt_poll.py            # leave running; Ctrl+C or `stop` to end
```

| command | effect |
|---|---|
| `reset` | probe-rs reset |
| `download <file>` | flash an ELF |
| `download <file> <addr>` | write a raw bin at an address |
| `stop` | end the session |

Full choreography (what produced `demo/transcripts/2026-08-07-full-demo/`,
whose `INDEX.md` maps every step to its transcript):

```
# baseline: bootloader + A in active, state erased
download demo/out/boot.elf
download demo/out/a_stager.elf
download demo/out/erase_byte.bin 0x10008000    # state sector 1
download demo/out/erase_byte.bin 0x10009000    # state sector 2
reset                                          # (a) boot A

download demo/out/cmd_stage.bin 0x1030B000     # mailbox: stage
reset                                          # (b,c,d) A stages B → swap → B trial → confirm
reset                                          # (d) B survives reset

download demo/out/cmd_stage.bin 0x1030B000
reset                                          # (e) B stages a_bad → swap → hang → watchdog → REVERT

download demo/out/a_stager_verify.elf          # verify phase
reset
download demo/out/cmd_stage_badsig.bin 0x1030B000
reset                                          # rejected, no swap
download demo/out/cmd_stage_verified.bin 0x1030B000
reset                                          # verified → swap → B

python demo/decode.py                          # transcripts from the captures
```

The mailbox is the first storage sector (`0x1030B000`): the app reads one
command byte at boot and erases it before acting. `0x01` stage (plain),
`0x02` stage + verify, `0x03` stage + deliberately corrupted signature.

## Recovery

Any weird state: `probe-rs erase --chip RP235x` (≈5 min) and re-run the
baseline block. Nothing here touches OTP.
