# Drill day: taking the Rust OTA path onto hardware

Task #16's runbook. **Run in full on 2026-08-16 — all eight steps, on the seated
unit.** What follows is kept as the procedure, not as a to-do list: it is what to
re-run before any gift unit migrates, and the notes marked *(drill day)* are what
the hardware said the first time.

**Outcome.** The Rust firmware is the shipping firmware on the living-room unit,
boot-integrated behind embassy-boot, on published `2026.08.16-108bc85` off the
`stable` channel, with a two-week soak started that day. The measured numbers
are in BUDGET.md's "Drill day 2026-08-16" section. Five firmware bugs were found
and fixed in the running — the watchdog contract under a bootloader (8303b79),
the blocking DFU hash (705c761), the breadcrumb's address (7f41099), a mid-trial
`/api/status` repaint (df70e8a), and the two-connection HTTP pool (108bc85) —
which is roughly one per two steps, and the argument for having done it before
the gift fleet rather than after.

**Read this first, still.** The living-room unit at `192.168.50.57` is *in
service*. Step 2 deliberately bricks an image and step 5 deliberately starves a
watchdog; both were run on a spare first, and should be again.

---

## What you need

| | |
|---|---|
| A spare Pico 2 W + HUB75 panel | The bad-NMOS spare is fine — none of this looks at the panel closely |
| A debug probe on the spare | `probe-rs` flashes the bootloader, and RTT is where every measurement below is read |
| The signing key | `backend/.fw-signing-key`. `python tools/fwsign.py pubkey` must print the same bytes as `firmware-rs/app/src/ota/key.rs` |
| A staging backend | `python tools/build.py publish-fw --channel dev --deploy` |

Set the spare's config to the dev channel before anything else, or it will
follow `stable` and ignore everything you publish:

```json
{ "ota": { "enabled": true, "channel": "dev" } }
```

---

## 0. Flash the pair, once

The bootloader is **not** OTA-updatable by design, so it goes on by probe and
stays there.

```sh
cd firmware-rs/boot && cargo run --release      # 14,208 B into the 32 KB partition
cd ../app && cargo run --release --no-default-features --features link-boot-integrated
```

- [ ] The bootloader logs `boot: state Boot, jumping to the active partition at 0x1000a000`
- [ ] The app logs `scoreboard-app dev up (BootIntegrated image)`
- [ ] `GET /api/status` reports `"app_version": "dev+BootIntegrated"` and `"ota_state": "idle"`

> A probe-flashed image carries version `dev`, so it will **refuse to update** —
> that is the rollback guard working. To drill the update path you must install
> a `publish-fw` image (step 1), not a `cargo run` one.

**Migrating a device that has ever held another image? Erase the state
partition first** (learned 2026-08-16, the hard way): a standalone image spans
`0x10008000`, so the bootloader pair lands with garbage where embassy-boot's
state machine lives. Write 8 KB of `0xFF` before the first boot of the pair:

```sh
probe-rs download --chip RP235x --binary-format bin --base-address 0x10008000 <8KB-of-FF file>
```

Two more traps from the same day:

- **Both link profiles build to the same ELF path.** A `cargo build --release`
  (standalone) after the boot-integrated build silently replaces the artifact,
  and flashing it writes a standalone image over the bootloader. Check the
  entry point (`0x1000Axxx`, not `0x10000xxx`) before any probe flash of the
  app.
- **A running bootloader watchdog can kill a 40 s probe flash mid-write** —
  the flash loader runs on the core, so the pause-on-debug bits do not help.
  If the pair is live and wedged (no feeder), clear `WATCHDOG.CTRL.ENABLE`
  over SWD first: `probe-rs write --chip RP235x b32 0x400db000 0x40000000`.

---

## 1. The happy path

```sh
python tools/build.py publish-fw --channel dev --deploy
```

Flash the *published* image by probe so the device starts from a real version,
then publish a second one (any trivial change) and force a check:

```sh
curl -X POST http://<device>/api/check-update
```

Two operational notes from the first run of this playbook (2026-08-16):

- **The staging edge lies for ~20 s after every deploy.** The staging app has
  two machines and Fly's rolling deploy leaves one serving the previous
  manifest briefly; the device's check can land there and answer `current`
  against a version you just published. It cost three separate drill runs before
  it was recognised. Wait half a minute and ask again before suspecting the
  firmware. Production has the same topology and the same window — **BACKLOG 88**
  is the fix.
- **`{"status":"updating","message":"The check is still running"}` is the
  handler's 20 s patience expiring, not a verdict** — the check is a phase of
  the poll loop, whose tick can be up to 30 s away. POST again for the real
  answer. The SPA renders it as though an update were in flight, which is
  **BACKLOG 89**.

- [ ] The handler answers `{"status":"updating", ...}` within 20 s
- [ ] The panel switches to the progress screen and the bar advances
- [ ] `ota: installing <version> (N bytes) over <old>` in the ring log
- [ ] **Record `ota: hashed N bytes of DFU in M ms`.** This is the number the
      whole verify design turns on — see `scoreboard_ota::verify`.
      *(Drill day: **10,635 ms at 1,094,232 B**, past the 8 s bootloader
      watchdog, and the single blocking call reset the device at the end of
      every install — three times, until the attempt record refused the version.
      The fix is 705c761: the hash is a chunk loop that feeds the watchdog and
      yields between 4 KB reads, so M no longer has to be under 8,000 for the
      install to survive. It is still worth recording, because it is the only
      measurement of the flash read path there is — BACKLOG 87.)*
- [ ] The 5→1 countdown, then the panel goes dark
- [ ] **Time the dark period.** Budgeted 35–70 s; it is the swap.
      *(Drill day: **41 s** of dark reset-to-HTTP, of which ~12 s is boot plus
      Wi-Fi rejoin, so the swap itself is ~29 s. Download-to-confirmed end to
      end was ~132 s.)*
- [ ] The bootloader logs `Swapping` at trace
- [ ] The app comes back and logs `ota: TRIAL boot of <new version>`
- [ ] `/api/status` shows `"ota_state": "trial"`
- [ ] Within ~30 s: `ota: health gate passed at N s; image confirmed`, and
      `ota_state` returns to `idle`
- [ ] `app_version` is the new one

## 2. The rollback — the drill that matters most

Publish an image that boots and then wedges without feeding the watchdog. The
cheapest way is a `loop {}` early in `main`, after `embassy_rp::init` and
before the executor starts.

- [ ] It installs and swaps exactly as above
- [ ] It never confirms; the bootloader's 8 s watchdog resets it
- [ ] The bootloader logs `Reverting` at trace
- [ ] The **old** image comes back and logs
      `ota: ROLLED BACK to <old>; <bad version> failed its health gate and will not be retried`
- [ ] `/api/status` shows `"ota_state": "rolled_back"`
- [ ] The health gate then confirms, and `ota_state` returns to `idle` —
      a `Revert` that is never cleaned blocks the *next* update
- [ ] **`POST /api/check-update` now answers `error`**, not `updating`, with
      "this version was installed once and rolled back". This is the attempt
      record doing its job; without it the device would reinstall the bad image
      tomorrow, and the day after, forever.
- [ ] Publishing a *different* version unblocks it immediately

## 3. Power-loss, three times

Pull power (not the probe's reset — a real cut) at each of:

- [ ] **Mid-download.** Next boot runs the old image. The attempt count has
      already incremented, which is the point of writing it first: three of
      these and the device gives up on that version rather than retrying
      forever.
- [ ] **Between `mark_updated` and the reset.** The magic is in flash, so the
      next boot swaps anyway. Nothing is lost.
- [ ] **Mid-swap.** The next boot resumes from the state partition's progress
      array. The spike demonstrated this; confirm it still holds at 1.06 MB,
      where the swap is doing real work rather than moving erased pages.

## 4. The dev-build guard

- [ ] Probe-flash a `cargo run` image (version `dev`) onto a device whose
      channel has a published image waiting
- [ ] `POST /api/check-update` answers `dev_deploy`, and the ring log says
      "this is a dev build; a check would roll it back"
- [ ] Nothing downloads

## 5. The watchdog, deliberately starved

`--features induce-panic`, then `POST /api/induce-panic`.

- [ ] The device resets and `/api/logs/previous` has the breadcrumb
- [ ] It does **not** roll back — a confirmed image stays confirmed through a
      crash, which is the difference between "this image is broken" and "this
      image had a bad afternoon"

## 6. The one that had never been exercised at all

- [ ] Let the device sit with the backend unreachable through a whole trial
      window. At 600 s it should confirm anyway and log
      `ota: confirming at N s with no backend answer`. The alternative — an
      image that stays armed for days and reverts at the next power cut — is
      the failure this deadline exists to prevent.
      *(Drill day: it fired at **exactly 600 s**. This step also found the
      bug in df70e8a — the settle-delay check landing inside the trial window
      repainted `/api/status` as `idle` while the image was still armed and
      unconfirmed, which is precisely the state an operator has to be able to
      see. Watch `ota_state` through the whole window, not just at the ends.)*

## 7. mDNS

The device answers for its configured `device_name` — the production identity
is plain `scoreboard.local` (the `scoreboard-rs` name retired with the old
bench split).

- [ ] `dig -p 5353 @224.0.0.251 scoreboard.local` answers
- [ ] `http://scoreboard.local/` opens the settings page from a phone and
      from a laptop (they use different resolvers; both matter)
- [ ] Same in setup mode, against the AP

*(Drill day: the responder answers correctly and promptly — unicast, QU-flagged
and group-addressed queries all returned instantly — and **resolution still dies
minutes after boot** on both Windows and iOS while unicast keeps working. The
diagnosis points at the device-outward multicast leg the AP stops distributing
rather than at the firmware, and the symptom predates the Rust firmware. This is
**BACKLOG 90**, open; it did not block the migration because the IP works and
the setup flow never depends on the name.)*

---

## Only after all of the above

Move to the living-room unit, and note that it is a **migration**, not an
update: it is running a standalone image at flash offset 0, so it needs the
bootloader flashed by probe and its storage region is at the same address
either way (the layout crate's `neither profile reaches into storage` test is
what guarantees the configuration survives).

- [x] Bootloader + published boot-integrated image, by probe
- [x] Confirm the stored wifi credentials and league selection survived
- [x] Switch it to `channel: stable` and publish there
- [ ] Two-week soak before any gift unit is considered — **started 2026-08-16**,
      running now. The gift fleet stays on MicroPython and on the backend's
      `/app/*` surface until it finishes.

## What to write down

Drill day's output is four numbers and one verdict. They belong in BUDGET.md's
"Drill day" section. What 2026-08-16 recorded:

1. **DFU hash time** — 10,635 ms at 1,094,232 B (~103 KB/s)
2. **Swap time, first install** — ~29 s (41 s of dark panel, reset to HTTP)
3. **Swap time, revert** — ~86 s of dark, for the whole revert cycle
4. **Total dark-panel time, download to confirmed** — ~132 s end to end
5. **Did the 8 s bootloader watchdog come close to firing outside step 2?**
   **Yes — it fired.** The DFU hash was a single blocking call that took
   10.6 s, so every install ended in a reset; and separately, the app under
   `link-boot-integrated` was not running its feeder at all unless
   `watchdog.enabled` was set, which is the flag meaning something different
   under a bootloader than it did standalone. Both are fixed (705c761,
   8303b79). The lesson generalises and is worth carrying into the next drill:
   **every blocking step of the boot and the install is on an 8 s clock that
   cannot be disarmed.** `firmware-rs/boot`'s module docs list the ones that
   are known; the storage region's one-time erase was the other near miss, and
   it now feeds between sectors.
