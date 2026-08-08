# Boot-spike demo transcripts — 2026-08-07, on the bench Pico 2 W

One continuous rtt_poll.py session. `session.log` is the timeline (host
timestamps, probe actions, capture events); each `*_gen*.txt` is the decoded
defmt output of one binary *generation* (one boot phase of the bootloader or
of whatever app image was active). `boot #N` counts boots in watchdog
scratch0 — host-driven probe resets clear it (measured behavior), watchdog
and SCB self-resets don't, which is why numbering restarts at 1 after every
`command: reset`.

## Step (a) — bootloader boots A

- `boot_gen000` — `state Boot, jumping to active @ 0x1000a000`
- `app_gen001` — A up, `bootloader state Boot`, idles feeding the watchdog

## Step (b) — A stages B into DFU + mark_updated (real updater API)

- `app_gen002` — mailbox command 0x01, `staging payload (26356 bytes) to
  DFU`, `payload staged`, `mark_updated()`, self-reset

## Step (c) — reboot → bootloader swaps → B runs

- `boot_gen002` — `state Swap` pass: `Swapping` → `Swapping done` → jump.
  Wall time ≈ 5 s for the 1.5 MB partition pair (mostly-erased flash; see
  PARTITIONS.md for why full images will take longer)
- `app_gen003` — B up, `bootloader state Swap`, `TRIAL boot`, health gate

## Step (d) — B mark_booted → survives reset

- `app_gen003` (tail) — `mark_booted() written — this image is now permanent`
- `boot_gen003`, `app_gen004` — after another reset: `state Boot`, B again.
  (Debug reset, not a cold power cycle — the state is all in flash, so the
  distinction doesn't affect the mechanism; a cold-boot sanity check on the
  dev unit is still listed for Phase 4.)

## Step (e) — stage a bad image → no confirm → watchdog → REVERT to B

- `app_gen005` — B stages a_bad (11264 bytes), `mark_updated()`, self-reset
- `boot_gen005` — swap pass (a_bad becomes active)
- `app_gen006` — a_bad: `TRIAL boot — playing broken image: no
  mark_booted(), no watchdog feeding` — then hangs
- `boot_gen006` — ~8 s later, watchdog reset (`boot #3`): `Reverting`.
  (Note: this pass still *reports* `state Swap` — prepare_boot returns the
  magic it read at entry; the app-visible state after a revert is `Revert`.)
- `app_gen007` — B again: `bootloader state Revert — a trial image failed
  and was ROLLED BACK to this one`, then `state cleaned back to Boot`

## ed25519 signature verification (verify_and_mark_updated)

- `app_gen008` — a_stager_verify baseline (banner shows `verify=true`)
- `app_gen009` — command 0x03: stages, `corrupting signature byte 0 on
  purpose`, `signature REJECTED (FirmwareUpdaterError::Signature(_)) — not
  marked, no swap will happen`
- `boot_gen009`, `app_gen010` — next reset: plain `state Boot`, **no swap**,
  unsigned image never became active
- `app_gen011` — command 0x02: stages, `signature VERIFIED +
  mark_updated()`, self-reset
- `boot_gen011`, `app_gen012` — swap runs, B trial-boots and confirms

## Drill: reset between mark_updated and the swap

- `app_gen013` — B stages a_bad, marks, self-reset
- `boot_gen013` — banner only: a host reset interrupted this boot before
  prepare() ran
- `boot_gen014` — the swap still happens on the next boot (SWAP magic is in
  flash, nothing was lost) → a_bad trials (`app_gen014`), watchdog reverts
  (`boot_gen015`), B restored (`app_gen015`)

## Drill: reset in the middle of the swap itself (power-fail stand-in)

- `app_gen016` — B stages a_bad, marks, self-reset
- `boot_gen017` — `Swapping` … **interrupted mid-swap by a host reset**
  (no `Swapping done` — the transcript ends inside the copy loop)
- `boot_gen018` — next boot resumes via the state partition's progress
  array: `Swapping` → `Swapping done` → jump
- `app_gen017` — the (still bad) swapped-in image trials and hangs
- `boot_gen019` — watchdog reset → `Reverting` → `app_gen018`: B restored,
  state cleaned back to Boot

## End state

- `boot_gen020` — final reset's banner (the capture session was stopped
  right after, so only the banner landed). The demo leaves the bench target
  running image B with a confirmed (`Boot`) state.
