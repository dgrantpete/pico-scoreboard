# Custom MicroPython firmware

The scoreboard runs a custom-compiled MicroPython (pinned as the
`firmware/micropython` submodule) with an out-of-tree board definition in
`PICO2W_SCOREBOARD/`. No fork: the board dir is passed to the stock build
via `BOARD_DIR`, and upstream version bumps are a submodule pin change.

## What differs from stock RPI_PICO2_W, and why

| Change | Where | Why |
|---|---|---|
| ROMFS partition, 256 KB (`MICROPY_HW_ROMFS_BYTES`) | `mpconfigboard.h` | App `.mpy` files will execute in place from flash instead of occupying ~100+ KB of heap, while staying OTA-rewritable via `vfs.rom_ioctl` (unlike frozen modules). Partition sits between firmware and littlefs; littlefs size/location unchanged, so existing device filesystems survive the firmware swap. |
| Bluetooth off | `mpconfigboard.cmake` | Unused; btstack costs ~20 KB RAM (static pools) + ~80 KB flash. |
| `MEMP_NUM_TCP_PCB` 5 → 16, `PBUF_POOL_SIZE` 16 → 24 | `mpconfigboard.cmake` | Stock lwip allows only ~5 simultaneous TCP connections (established + TIME_WAIT share the pool); a browser plus the score poller exhausts it and inbound SYNs are silently dropped ("site won't load, device fine"). ~+2 KB static RAM. |
| Minimal frozen manifest | `manifest.py` | Stock freezes bundle-networking (mip/ntptime/requests/webrepl) + aioble; the app uses none of them. App code is deliberately NOT frozen — frozen can't be OTA-updated; ROMFS is the plan (see BACKLOG). |
| Hostname default "scoreboard" | `mpconfigboard.h` | Cosmetic; runtime hostname still comes from config.json. |

## Building

Canonical: **GitHub Actions** (`.github/workflows/firmware.yml`) builds on
every push touching `firmware/board/**`, the submodule pin, or the build
script, and uploads the UF2 as an artifact.

Local (Linux or WSL):

```bash
bash tools/build_firmware.sh
# → firmware/dist/scoreboard-fw-<git>-mp<ver>.uf2
```

First build initializes the rp2 port submodules (pico-sdk etc.) and takes
several minutes; incremental rebuilds are fast. Under WSL with the repo on
`/mnt/*`, object files go to `~/.cache/pico-scoreboard-fw` automatically
(building through 9P is ~10x slower); override with `FW_BUILD_DIR`.

## Flashing the firmware image (not the app)

Firmware images are flashed over USB: hold **BOOTSEL** while plugging the
Pico in, then copy the UF2 onto the `RP2350` drive (or
`picotool load -x <uf2>`). This is only needed for interpreter/board-config
changes — app code keeps deploying via `python tools/build.py flash`
(and, later, OTA). The littlefs filesystem (config.json, logs, app code)
survives a firmware reflash.

## Version bumps

```bash
cd firmware/micropython && git fetch --tags && git checkout v1.XX.0 && cd ../..
git add firmware/micropython && git commit -m "firmware: micropython v1.XX.0"
```

CI rebuilds automatically; check the release notes for rp2 port changes
(lwipopts defaults, board config renames) before bumping.
