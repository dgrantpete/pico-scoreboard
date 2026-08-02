#!/usr/bin/env bash
# Build the custom MicroPython firmware for the scoreboard (Pico 2 W).
#
# Produces firmware/dist/scoreboard-fw-<git>-mp<version>.uf2 from the pinned
# micropython submodule plus the out-of-tree board definition in
# firmware/board/PICO2W_SCOREBOARD (ROMFS enabled, Bluetooth off, lwip TCP
# pools raised — see that directory's files for rationale).
#
# Runs on Linux and WSL; CI (.github/workflows/firmware.yml) calls this same
# script. First build fetches the rp2 port's submodules (pico-sdk etc.) and
# takes several minutes; incremental rebuilds take seconds.
#
# Environment overrides:
#   FW_BUILD_DIR   out-of-tree cmake build directory. Defaults to the
#                  in-tree ports/rp2/build-PICO2W_SCOREBOARD, EXCEPT under
#                  WSL on a /mnt/* checkout, where object files default to
#                  ~/.cache/pico-scoreboard-fw (building onto the Windows
#                  filesystem through 9P is ~10x slower).
#   JOBS           parallel build jobs (default: nproc)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MPY_DIR="$REPO_ROOT/firmware/micropython"
BOARD=PICO2W_SCOREBOARD
BOARD_DIR="$REPO_ROOT/firmware/board/$BOARD"
DIST_DIR="$REPO_ROOT/firmware/dist"
JOBS="${JOBS:-$(nproc)}"

# pip-installed user-local cmake (WSL without sudo) lives here
export PATH="$HOME/.local/bin:$PATH"

# WSL + Windows-filesystem checkout: keep object files on the Linux side.
if [ -z "${FW_BUILD_DIR:-}" ] && grep -qi microsoft /proc/version 2>/dev/null \
        && [[ "$REPO_ROOT" == /mnt/* ]]; then
    FW_BUILD_DIR="$HOME/.cache/pico-scoreboard-fw"
    echo "WSL on /mnt detected: building into $FW_BUILD_DIR (override with FW_BUILD_DIR)"
fi

MAKE_ARGS=(BOARD="$BOARD" BOARD_DIR="$BOARD_DIR")
if [ -n "${FW_BUILD_DIR:-}" ]; then
    mkdir -p "$FW_BUILD_DIR"
    MAKE_ARGS+=(BUILD="$FW_BUILD_DIR")
    BUILD_OUT="$FW_BUILD_DIR"
else
    BUILD_OUT="$MPY_DIR/ports/rp2/build-$BOARD"
fi

echo "==> Checking micropython submodule"
git -C "$REPO_ROOT" submodule update --init firmware/micropython

echo "==> Fetching rp2 port submodules (pico-sdk etc.; no-op when present)"
make -C "$MPY_DIR/ports/rp2" "${MAKE_ARGS[@]}" submodules

echo "==> Building mpy-cross"
make -C "$MPY_DIR/mpy-cross" -j "$JOBS"

echo "==> Building rp2 firmware ($BOARD)"
make -C "$MPY_DIR/ports/rp2" "${MAKE_ARGS[@]}" -j "$JOBS"

MP_VERSION="$(git -C "$MPY_DIR" describe --tags --always)"
APP_VERSION="$(git -C "$REPO_ROOT" describe --always --dirty)"
UF2_NAME="scoreboard-fw-$APP_VERSION-$MP_VERSION.uf2"

mkdir -p "$DIST_DIR"
cp "$BUILD_OUT/firmware.uf2" "$DIST_DIR/$UF2_NAME"

echo
echo "==> Done: firmware/dist/$UF2_NAME"
ls -la "$DIST_DIR/$UF2_NAME"
echo "Flash: hold BOOTSEL while plugging in, then copy the UF2 to the RP2350 drive"
echo "(or: picotool load -x $DIST_DIR/$UF2_NAME)"
