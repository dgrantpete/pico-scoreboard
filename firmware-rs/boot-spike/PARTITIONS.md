# Confirmed RP2350 flash layout for embassy-boot A/B OTA

Status: **hardware-verified 2026-08-07** on the bench Pico 2 W (RP2350A,
4 MB flash) with embassy-boot 0.7.0 / embassy-boot-rp 0.10.0 /
embassy-rp 0.10.0 — swap, trial-boot confirm, watchdog revert, mid-swap
reset resume, and ed25519 verify all demonstrated on this exact layout
(transcripts: `demo/transcripts/`).

The machine-readable source of truth is `layout/src/lib.rs`
(`spike-layout`): both binaries' `memory.x` are generated from it in their
build scripts, and its `const` asserts re-check every rule below on every
build. Phase 4 lifts that crate (or its constants) wholesale — SPEC §8's
"one constants file" is prototyped here.

## The table

| offset      | size               | region  | notes |
|-------------|--------------------|---------|-------|
| `0x00_0000` | 32 KB              | boot    | bootloader; bootrom scans its first 4 KB for IMAGE_DEF |
| `0x00_8000` | 8 KB               | state   | embassy-boot magic byte + swap/revert progress |
| `0x00_A000` | 1536 KB            | active  | app links here (XIP `0x1000_A000`) |
| `0x18_A000` | 1536 KB + **4 KB** | dfu     | staging; must exceed active by ≥ one erase page |
| `0x30_B000` | 980 KB             | storage | sequential-storage region (SPEC §9); first sector doubles as the spike's demo mailbox |

All boundaries 4 KB-aligned (flash erase size). Sums to exactly 4 MB.

## Deviations from the SPEC §8 draft, and why

The draft said "boot 32 KB · state 8 KB · active 1.5 MB · DFU 1.5 MB ·
remainder storage". Two corrections, both forced by
`embassy_boot::BootLoader`'s `assert_partitions`
(embassy-boot-0.7.0, `src/boot_loader.rs`):

1. **DFU = active + one erase page (4 KB), not DFU = active.** The swap
   algorithm needs one scratch page in DFU. Equal-size partitions panic the
   bootloader at its first `prepare_boot()`. Storage shrinks by the same
   4 KB (984 → 980 KB).
2. **State sizing is byte-cheap on RP2350 — 8 KB stands, and even 4 KB
   would work.** The rule is
   `2 + 4 × (active / erase_size) ≤ state_size / WRITE_SIZE`. embassy-rp's
   flash driver exposes `WRITE_SIZE = 1` (it page-buffers 256-byte programs
   internally), so a 1536 KB active needs only 1,538 state *bytes*. Keep
   8 KB for margin — the constraint flips hard if a future embassy-rp
   raises `WRITE_SIZE` (at 256 it would demand ~394 KB, i.e. a redesign).
   The const assert catches that at build time.

## Measured sizes (this spike; sizes to re-confirm in Phase 3 per SPEC)

- Bootloader binary: **12.2 KB of 32 KB** (37%), `opt-level = "s"`, with
  defmt + trace logging. Comfortable; do not shrink the partition — a
  future signature-verifying bootloader variant or extra logging fits.
- Active 1536 KB is generous: the demo apps are 11–26 KB; the real app
  (fonts + SPA + logic) was budgeted ~600–800 KB. Room to spare, and the
  active size directly drives swap time and state-partition math, so
  revisit only with the const asserts in the loop.

## Behavior confirmed on hardware (what Phase 4 can rely on)

- **IMAGE_DEF:** embassy-rp 0.10.0 auto-emits `ImageDef::secure_exe()`
  into `.start_block` for every rp235x binary (`imagedef-none` opts out).
  The bootrom only ever reads the *bootloader's* block; the bootloader
  reaches the app by `SCB.VTOR = active; asm::bootload` with no bootrom
  involvement, so the app's own IMAGE_DEF is inert at runtime (kept for
  picotool). The block is a single self-looped entry (link offset 0) —
  position-independent by construction, so images survive active↔DFU
  relocation unchanged. Verified: swapped-in images boot; `picotool`-style
  metadata walking was not needed anywhere.
- **Swap/revert timing:** ~5 s per swap pass with mostly-erased 1.5 MB
  partitions. Time scales with *content*: erase of an already-erased 4 KB
  sector is fast, a programmed one costs ~45 ms + reprogram. Budget
  **~35–70 s of dark screen** for a full-sized image apply (and the same
  again for a revert) until measured with real images in Phase 4.
- **Power-fail safety:** a reset in the middle of a swap resumes from the
  state partition's progress array on the next boot (demonstrated); a
  reset between `mark_updated()` and the swap loses nothing (magic is in
  flash; demonstrated).
- **Watchdog revert:** the bootloader arms an 8 s watchdog
  (`WatchdogFlash`) that stays armed across the jump; an app that never
  feeds it gets reset and the unconfirmed trial image is reverted
  (demonstrated end-to-end). `WDSEL`/PSM reset routing via embassy-rp is
  correct on RP2350 — the watchdog reset lands back in the bootrom → boot
  path cleanly.
- **State semantics quirk:** on the boot that performs a revert,
  `prepare_boot` *returns* `State::Swap` (the magic read at entry); the
  app sees `State::Revert` from then on and should health-gate +
  `mark_booted()` to return the state machine to `Boot` (the spike app
  does; port this into the production supervision flow).
- **Watchdog scratch:** survives watchdog and `SCB::sys_reset` resets but
  **not probe-rs (debug) resets**. Fine for the production boot-fail
  counter intent (§12) since embassy-boot keeps everything that matters in
  flash — just don't put anything load-bearing in scratch registers.

## Signature verification (ed25519) — audited + demonstrated

- Feature: `embassy-boot/ed25519-dalek` (or `ed25519-salty`). Enabling it
  **removes `mark_updated()`** and replaces it with
  `verify_and_mark_updated(&pubkey32, &sig64, len)` — verification gates
  the *swap request*, in the app, after the image is fully staged to DFU.
  The bootloader binary is unchanged and does no verification; **no layout
  impact** (the signature travels out-of-band, via the manifest in
  production, `include_bytes!` in the spike).
- Scheme: `sig = Ed25519_sign(SHA512(image))` — plain Ed25519 over the
  64-byte digest, not Ed25519ph. `demo/sign.py` implements it (throwaway
  dev key; production key lives in backend deploy secrets per SPEC §8).
- Demonstrated on hardware: corrupted signature →
  `FirmwareUpdaterError::Signature`, no mark, no swap on subsequent boots;
  good signature → verified, marked, swapped, trial-confirmed.
- ed25519-dalek builds `no_std` without `alloc` on thumbv8m (no-alloc
  policy §10 holds). Verify cost at 26 KB was sub-second despite
  embassy-boot's internal 2-byte hashing chunk buffer; at ~800 KB expect
  tens of seconds of SHA-512 through that 2-byte buffer — measure in
  Phase 4 and, if painful, hash via `BlockingFirmwareUpdater::hash` with a
  bigger chunk is not an option (the buffer is hardcoded in
  `verify_and_mark_updated`) — either accept it, feed the watchdog first,
  or carry a small patch. Flagged as the one upstream wart.
