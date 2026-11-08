#!/usr/bin/env python3
"""
Build script for pico-scoreboard.

Deploys the backend, publishes the Rust firmware for OTA, and flashes or
publishes an already-built MicroPython app.

Usage:
    python tools/build.py deploy       # Deploy backend to Fly.io
    python tools/build.py publish-fw   # Build+sign the Rust firmware for OTA
    python tools/build.py flash --no-build   # Flash an existing app build
    python tools/build.py publish-app --no-build   # Publish an existing app build

The MicroPython app itself can no longer be built from source: its sprite
sources were retired on 2026-11-08 (BACKLOG 116). Building it (the default
command, or flash/run/publish-app without --no-build) stops with the reason.
"""

import hashlib
import json
import shutil
import subprocess
import argparse
import time
import sys
from pathlib import Path

# Directory structure
root_directory = Path(__file__).parent.parent


def _load_build_config() -> dict:
    """Load default argument values from tools/build.config.json if present."""
    config_path = Path(__file__).parent / 'build.config.json'
    if not config_path.exists():
        return {}
    with open(config_path) as f:
        return json.load(f)


_build_config = _load_build_config()


def _mpremote(args: list, port: str = None, timeout: float = None, quiet: bool = False):
    """Run one mpremote invocation with a hang guard.

    Returns the CompletedProcess, or None if the command timed out (a hung
    mpremote must never strand the flash flow — see micropython#13476).

    Always prefixes `resume`: without it, mpremote SOFT-RESETS the board
    before the first command of a session. Against a running scoreboard that
    soft reset can wedge TinyUSB via the Core 1 thread (micropython#8494) —
    the historical ~50% flash failure — and against a safe-mode REPL it
    re-runs main.py, restarting the app and defeating safe mode entirely.
    """
    cmd = ['mpremote']
    if port:
        cmd.extend(['connect', port, '+'])
    cmd.append('resume')
    cmd.extend(args)
    try:
        return subprocess.run(cmd, timeout=timeout, capture_output=quiet)
    except subprocess.TimeoutExpired:
        return None


# Runs on the device to schedule a safe-mode boot: main.py consumes the
# /update flag and skips the application (no Core 1 thread, no PIO/DMA),
# leaving a REPL that mpremote can reliably talk to. machine.reset() is a
# HARD reset — it re-initializes both cores and USB, avoiding the
# soft-reset/TinyUSB-spinlock lockup (micropython#8494).
_UPDATE_MODE_SNIPPET = "f=open('/update','w'); f.close(); import machine; machine.reset()"


def enter_update_mode(port: str = None) -> bool:
    """Reboot the running firmware into safe mode and confirm it took.

    The request exec MUST be verified: a busy device can reject the raw-REPL
    handshake, and proceeding on a silent failure once erased ROMFS under
    the running app (Core 1 crashed with `NotImplementedError: opcode`).

    Confirmation is positive proof, not timing. Two proofs are accepted:
    - main.py's safe-mode branch sets a `_SAFE_MODE` global, and mpremote
      `exec` shares that namespace (requires main.py 2026-07-07+; main.py
      only updates via USB flash, so one successful flash qualifies a
      device forever);
    - no `/main.py` on the filesystem at all (wiped or factory-fresh
      board): nothing can have started, so the REPL answering is enough.
    Anything else falls to the manual Button A prompt.
    """
    print("Requesting reboot into safe (update) mode...")
    # --no-follow: machine.reset() kills the connection mid-exec; don't wait.
    for _ in range(3):
        req = _mpremote(['exec', '--no-follow', _UPDATE_MODE_SNIPPET],
                        port, timeout=15, quiet=True)
        if req is not None and req.returncode == 0:
            break
        print("  safe-mode request rejected (device busy); retrying...")
        time.sleep(2)
    else:
        return False

    # Hard reset -> USB re-enumerates; give the OS a moment, then probe.
    time.sleep(4)
    for _ in range(3):
        probe = _mpremote(['exec', "print('SAFE' if globals().get('_SAFE_MODE') else 'UNSAFE')"],
                          port, timeout=8, quiet=True)
        if probe is not None and probe.returncode == 0:
            verdict = probe.stdout.decode(errors='replace').strip()
            if not verdict.endswith('SAFE') or verdict.endswith('UNSAFE'):
                # No sentinel. One more accepted proof: a board with no
                # main.py at all cannot have started the app this boot.
                bare = _mpremote(
                    ['exec',
                     "import os; print('APP' if 'main.py' in os.listdir('/') else 'BARE')"],
                    port, timeout=8, quiet=True)
                if (bare is not None and bare.returncode == 0
                        and bare.stdout.decode(errors='replace').strip().endswith('BARE')):
                    print("Device has no main.py (wiped/fresh board) — safe to flash.")
                    # The /update flag we wrote had no main.py to consume it;
                    # clear it or the first post-flash boot lands in safe mode.
                    _mpremote(['exec',
                               "import os\ntry:\n os.remove('/update')\nexcept OSError:\n pass"],
                              port, timeout=8, quiet=True)
                else:
                    print("Device responds but did not confirm safe mode.")
                    return False
            else:
                print("Device is in safe mode.")
            # Let the serial port fully release before the next mpremote
            # invocation opens it (Windows COM reopen race).
            time.sleep(1.5)
            return True
        time.sleep(2)
    return False


# Release-mode split: these stay on littlefs (mutable / boot-critical —
# main.py must live there because ROMFS isn't bootable (micropython#17544),
# and ota.py must survive — and never run from — the ROMFS partition it
# rewrites); everything else in the build output ships inside the image.
_LITTLEFS_FILES = ('main.py', 'ota.py', 'config.json')

# App artifacts a release deploy removes from littlefs: littlefs precedes
# /rom on sys.path (that's the dev-override mechanism), so stale littlefs
# copies would silently shadow the ROMFS app.
_LITTLEFS_PURGE = (':scoreboard', ':lib', ':hardware_diagnostic.mpy', ':index.html.gz')

def _romfs_partition_bytes() -> int:
    """Read MICROPY_HW_ROMFS_BYTES from the board header — single source of
    truth, so the image-size check can't silently drift from the firmware's
    actual partition. Expects the define on one line, e.g. `(512 * 1024)`."""
    header = root_directory / 'firmware' / 'board' / 'PICO2W_SCOREBOARD' / 'mpconfigboard.h'
    for line in header.read_text().splitlines():
        if 'MICROPY_HW_ROMFS_BYTES' in line and line.strip().startswith('#define'):
            expr = line.split('MICROPY_HW_ROMFS_BYTES', 1)[1].strip()
            # Evaluate the arithmetic C expression, e.g. "(512 * 1024)"
            if not all(c in '0123456789*+() \t' for c in expr):
                break
            return eval(expr)  # noqa: S307 - digits and arithmetic only
    raise SystemExit(f"Could not parse MICROPY_HW_ROMFS_BYTES from {header}")


def build_romfs_image(source_dir: Path) -> tuple[Path, str]:
    """Build a ROMFS image from the app portion of the build output.

    The staging tree mirrors the littlefs layout (scoreboard/, lib/, ...) so
    imports resolve identically from /rom. Files are already .mpy-compiled
    by the build, hence --no-mpy. Returns (image_path, sha256_hex) — the
    sha is the app's OTA identity (stored on-device as /app_version and
    served by the backend manifest).
    """
    stage_dir = root_directory / 'romfs_stage'
    image_path = root_directory / 'pico.romfs'
    if stage_dir.exists():
        shutil.rmtree(stage_dir)
    stage_dir.mkdir()

    for entry in source_dir.iterdir():
        if entry.name in _LITTLEFS_FILES:
            continue
        if entry.is_dir():
            shutil.copytree(entry, stage_dir / entry.name)
        else:
            shutil.copy2(entry, stage_dir / entry.name)

    result = subprocess.run(
        ['mpremote', 'romfs', '--no-mpy', '--output', str(image_path),
         'build', str(stage_dir)],
        capture_output=True, text=True,
    )
    shutil.rmtree(stage_dir)
    if result.returncode != 0:
        raise SystemExit(f"romfs image build failed:\n{result.stdout}\n{result.stderr}")

    partition_bytes = _romfs_partition_bytes()
    size = image_path.stat().st_size
    sha = hashlib.sha256(image_path.read_bytes()).hexdigest()
    print(f"ROMFS image: {image_path.name} ({size} bytes, "
          f"{size * 100 // partition_bytes}% of the {partition_bytes // 1024} KB partition, "
          f"sha256 {sha[:12]}...)")
    if size > partition_bytes:
        raise SystemExit(
            f"ROMFS image ({size} B) exceeds the partition "
            f"({partition_bytes} B). Grow MICROPY_HW_ROMFS_BYTES in "
            "firmware/board/PICO2W_SCOREBOARD/mpconfigboard.h (multiple of 4096) "
            "and rebuild + reflash the firmware."
        )
    return image_path, sha


_OTA_DEV_WRITE = "f=open('/ota_dev','w'); f.close()"
_OTA_DEV_REMOVE = "import os\ntry:\n os.remove('/ota_dev')\nexcept OSError:\n pass"


def _sync_ota_dev_marker(source_dir: Path, image_sha: str, port: str = None):
    """Align the device's /ota_dev marker with the published manifest.

    A release flash of an image whose sha isn't the published one would be
    rolled back by the next OTA check — with littlefs main.py/ota.py staying
    new while the ROMFS app goes old, which has crashed boot before
    (2026-07-11). Mismatch -> write /ota_dev (ota.py skips checks while it
    exists), match -> remove it, manifest unreachable -> leave it unchanged
    and say so.
    """
    import urllib.request

    try:
        cfg = json.loads((source_dir / 'config.json').read_text())
        api_url = cfg['api']['url'].rstrip('/')
        api_key = cfg['api']['key']
    except (OSError, KeyError, ValueError) as e:
        print(f"  WARNING: no api url/key in deployed config.json ({e});")
        print("  cannot verify the published manifest — /ota_dev left unchanged.")
        return

    try:
        req = urllib.request.Request(api_url + '/app/manifest',
                                     headers={'X-Api-Key': api_key})
        with urllib.request.urlopen(req, timeout=15) as resp:
            published = json.loads(resp.read())['sha256']
    except Exception as e:
        print(f"  WARNING: could not fetch published manifest ({e});")
        print("  /ota_dev left unchanged — verify OTA state manually.")
        return

    if published == image_sha:
        _mpremote(['exec', _OTA_DEV_REMOVE], port, timeout=30, quiet=True)
        print("  Deployed sha matches the published manifest; OTA checks active.")
    else:
        _mpremote(['exec', _OTA_DEV_WRITE], port, timeout=30, quiet=True)
        print(f"  WARNING: deployed sha {image_sha[:12]} != published manifest "
              f"{published[:12]}.")
        print("  Wrote /ota_dev — the device will SKIP OTA checks (a check would")
        print("  roll this build back). To publish this exact build and re-arm OTA:")
        print("    python tools/build.py publish-app --no-build --deploy")
        print("    python tools/build.py flash --no-build --release   (clears /ota_dev)")


def flash_device(source_dir: Path, port: str = None, repl: bool = False, release: bool = False):
    """Flash files to Pico using mpremote.

    Flashing into a *running* scoreboard is unreliable by design: with the
    Core 1 display thread up, mpremote fs commands hang (micropython#13476)
    and soft resets can wedge USB (micropython#8494). So: reboot into safe
    mode first, copy with the app stopped, then hard-reset into the new
    firmware.

    Dev mode: everything goes to littlefs — fast iteration, and littlefs
    shadows any ROMFS app. Release mode: the app ships as a ROMFS image
    (bytecode executes in place, ~100 KB less heap) with only
    main.py/config.json on littlefs. The caller resolves the default from
    build.config.json "flash_release" (see _resolve_deploy_release).
    """
    mode = 'release (ROMFS)' if release else 'dev (littlefs)'
    print(f"Flashing {source_dir.relative_to(root_directory)}/ to device [{mode}]...")
    if not release:
        print("  NOTE: dev deploy — littlefs shadows any ROMFS image and the app")
        print("  runs as RAM bytecode (~100 KB heap cost; frequent GC on a full")
        print("  heap). Use --release or build.config.json \"flash_release\": true.")

    image_path, image_sha = build_romfs_image(source_dir) if release else (None, None)

    if not enter_update_mode(port):
        print("")
        print("Could not confirm safe mode automatically. Manual fallback:")
        print("  1. Hold Button A (GPIO 10) while power-cycling or resetting the Pico")
        print("  2. Keep holding for ~2 seconds after power-up")
        print("  3. The app skips startup and the REPL is free")
        try:
            input("Press Enter once the device has rebooted in safe mode... ")
        except EOFError:
            print("No interactive stdin — aborting without flashing. Put the")
            print("device in safe mode (Button A) and re-run from a terminal.")
            return

    if release:
        print("Deploying ROMFS image...")
        deploy = _mpremote(['romfs', 'deploy', str(image_path)], port, timeout=600)
        if deploy is None or deploy.returncode != 0:
            raise SystemExit(
                "romfs deploy failed or hung. Use the Button A fallback above, "
                "then re-run: python tools/build.py flash --no-build --release"
            )

        # Remove littlefs copies so the ROMFS app actually runs (missing
        # paths are fine — already-release devices won't have them).
        print("Removing app files from littlefs (littlefs shadows /rom)...")
        for target in _LITTLEFS_PURGE:
            _mpremote(['rm', '-r', target], port, timeout=120, quiet=True)

        for name in _LITTLEFS_FILES:
            src = source_dir / name
            if src.exists():
                copy = _mpremote(['cp', str(src), f':{name}'], port, timeout=60)
                if copy is None or copy.returncode != 0:
                    raise SystemExit(f"copying {name} to littlefs failed")

        # Record the app's OTA identity and clear any stale staging so the
        # daily check compares against what was actually just deployed.
        _mpremote(['exec',
                   f"f=open('/app_version','w'); f.write('{image_sha}'); f.close()"],
                  port, timeout=30, quiet=True)
        _mpremote(['rm', ':ota_staging'], port, timeout=30, quiet=True)
        _mpremote(['rm', ':ota_pending'], port, timeout=30, quiet=True)

        # OTA rollback guard: suspend or re-arm the daily check depending on
        # whether this exact image is what the backend serves.
        _sync_ota_dev_marker(source_dir, image_sha, port)
    else:
        copy = _mpremote(['cp', '-r', f'{source_dir.as_posix()}/.', ':'], port, timeout=600)
        if copy is None or copy.returncode != 0:
            raise SystemExit(
                "mpremote copy failed or hung. Use the Button A fallback above, "
                "then re-run: python tools/build.py flash --no-build"
            )

        # Dev deploys always run unpublished code (littlefs shadows ROMFS);
        # suspend OTA checks so the daily check can't rewrite ROMFS and
        # reboot underneath the shadowed app.
        _mpremote(['exec', _OTA_DEV_WRITE], port, timeout=30, quiet=True)
        print("Wrote /ota_dev (dev deploy): OTA checks suspended until a "
              "release flash matches the published manifest.")

    # Hard reset into the (new) firmware; the /update flag was consumed at
    # the safe-mode boot, so this boot starts the app normally.
    _mpremote(['reset'], port, timeout=15, quiet=True)

    if repl:
        # Wait for device to reconnect after reset
        print("Waiting for device to reconnect...")
        time.sleep(2)

        # Start REPL in separate command
        repl_cmd = ['mpremote']
        if port:
            repl_cmd.extend(['connect', port, '+'])
        repl_cmd.append('repl')
        subprocess.run(repl_cmd)
    else:
        print("Flash complete!")


# Why the MicroPython app no longer builds. Its scoreboard/layout/ modules were
# generated from firmware/assets/layout/ by tools/compile_layout.py, and both
# are gone, so a build could only produce an image without the sprites.
APP_BUILD_RETIRED = (
    "The MicroPython app can no longer be built from source: its sprite sources "
    "(firmware/assets/layout/ and tools/compile_layout.py, which generated the "
    "app's scoreboard/layout/ modules) were retired on 2026-11-08 (BACKLOG 116).\n"
    "The last published app image is backend/app_dist/pico.romfs; "
    "'python tools/build.py deploy' still serves it, and flash/publish-app "
    "--no-build still take an existing build directory."
)


def do_build() -> bool:
    """The MicroPython app build: it needs the sprite layout modules nothing
    can generate any more, so it refuses rather than build an image without
    them."""
    print(f"Error: {APP_BUILD_RETIRED}", file=sys.stderr)
    return False


def add_common_args(parser):
    """Add common arguments to a parser.

    Defaults are sourced from tools/build.config.json when present, with
    hardcoded fallbacks if the file is missing or a key is absent.
    """
    parser.add_argument(
        '-o', '--output',
        type=Path,
        default=Path(_build_config.get('output', 'pico')),
        help='Output directory (default from build.config.json, else "pico")'
    )


# --------------------------------------------------------------------------
# publish-fw: the Rust firmware's release pipeline
# --------------------------------------------------------------------------

firmware_rs_directory = root_directory / 'firmware-rs'

# The active partition, from the ONE constants file both binaries link against
# (firmware-rs/layout/src/lib.rs). Parsed rather than hardcoded so this check
# cannot drift from the linker script.
_LAYOUT_SOURCE = firmware_rs_directory / 'layout' / 'src' / 'lib.rs'


def _layout_consts() -> dict:
    """Evaluate the `pub const NAME: u32 = expr;` table in the layout crate."""
    import re

    consts: dict[str, int] = {}
    source = _LAYOUT_SOURCE.read_text()
    for name, expr in re.findall(r'pub const (\w+): u32 = ([^;]+);', source):
        try:
            consts[name] = eval(expr, {'__builtins__': {}}, dict(consts))  # noqa: S307
        except (NameError, TypeError, SyntaxError):
            pass
    return consts


def _objcopy() -> Path:
    """llvm-objcopy from the rust toolchain — no cargo-binutils needed."""
    sysroot = subprocess.run(
        ['rustc', '--print', 'sysroot'], check=True, capture_output=True, text=True
    ).stdout.strip()
    matches = list(Path(sysroot).glob('lib/rustlib/*/bin/rust-objcopy*'))
    if not matches:
        raise SystemExit('rust-objcopy not found in the toolchain sysroot; '
                         'install the llvm-tools component')
    return matches[0]


def _fw_version() -> str:
    """`<UTC date>-<git short sha>`, with `-dirty` if the tree is not clean.

    The version IS the OTA identity, and `build.rs` refuses anything starting
    with `dev` — that prefix is what tells a device its image was never
    published. A dirty tree still publishes, but says so: an artifact nobody
    can reproduce from a commit should be visibly that.
    """
    stamp = time.strftime('%Y.%m.%d', time.gmtime())
    try:
        sha = subprocess.run(
            ['git', 'rev-parse', '--short=7', 'HEAD'],
            check=True, capture_output=True, text=True, cwd=root_directory,
        ).stdout.strip()
        dirty = subprocess.run(
            ['git', 'status', '--porcelain'],
            check=True, capture_output=True, text=True, cwd=root_directory,
        ).stdout.strip()
    except (subprocess.CalledProcessError, FileNotFoundError):
        return f'{stamp}-nogit'
    return f'{stamp}-{sha}' + ('-dirty' if dirty else '')


def build_firmware_image(version: str) -> tuple[Path, int]:
    """Build the boot-integrated app and extract its raw image.

    Returns (path, size). Raises SystemExit on anything that would produce an
    artifact a device cannot install.
    """
    import os

    app_directory = firmware_rs_directory / 'app'
    env = dict(os.environ, SCOREBOARD_FW_VERSION=version)
    print(f'Building the boot-integrated firmware as {version}...')
    result = subprocess.run(
        ['cargo', 'build', '--release', '--locked',
         '--no-default-features', '--features', 'link-boot-integrated'],
        cwd=app_directory, env=env, check=False,
    )
    if result.returncode != 0:
        raise SystemExit('firmware build failed')

    elf = app_directory / 'target' / 'thumbv8m.main-none-eabihf' / 'release' / 'scoreboard-app'
    image_path = firmware_rs_directory / 'fw.bin'
    subprocess.run([str(_objcopy()), '-O', 'binary', str(elf), str(image_path)], check=True)

    layout = _layout_consts()
    active = layout['ACTIVE_SIZE']
    size = image_path.stat().st_size
    print(f'Firmware image: {size:,} bytes '
          f'({size * 100 // active}% of the {active // 1024} KB active partition)')
    if size > active:
        raise SystemExit(
            f'image ({size:,} B) does not fit the active partition ({active:,} B). '
            'It cannot be installed; shrink it or revisit the partition table in '
            'firmware-rs/layout/src/lib.rs.'
        )

    _assert_boot_integrated(image_path, layout)
    return image_path, size


def _assert_boot_integrated(image_path: Path, layout: dict) -> None:
    """Refuse to publish an image linked for the wrong address.

    A standalone image links at flash offset 0. Swapped into the active
    partition it would hard-fault on the first vector fetch, and every device
    that took it would roll back — a fleet-wide outage from one wrong build
    flag. The check is cheap and reads the bytes rather than trusting the flag:
    the second word of a Cortex-M vector table is the reset handler's address,
    and it has to point inside the active partition.
    """
    import struct

    header = image_path.read_bytes()[:8]
    if len(header) < 8:
        raise SystemExit('image is too short to contain a vector table')
    _stack, reset = struct.unpack('<II', header)
    low = layout['FLASH_BASE'] + layout['ACTIVE_OFFSET']
    high = low + layout['ACTIVE_SIZE']
    if not low <= reset < high:
        raise SystemExit(
            f'image reset vector is {reset:#010x}, outside the active partition '
            f'[{low:#010x}, {high:#010x}). This image was linked STANDALONE and '
            'would hard-fault the moment the bootloader jumped into it. Build it '
            'with --no-default-features --features link-boot-integrated.'
        )
    print(f'  reset vector {reset:#010x} is inside the active partition')


def publish_fw(channel: str, deploy: bool) -> bool:
    """Build, verify, sign and stage a Rust firmware image for OTA.

    The signing key never leaves the machine this runs on: `fwsign.py` reads
    backend/.fw-signing-key (gitignored) and only the 128-hex signature is
    written into the artifact directory.
    """
    from fwsign import PRIVATE_KEY_PATH, sign_bytes, verify_bytes, _load_private

    if not PRIVATE_KEY_PATH.exists():
        print(f'Error: no signing key at {PRIVATE_KEY_PATH.relative_to(root_directory)}.')
        print('  Generate one:  python tools/fwsign.py keygen')
        print('  A published image that nothing can verify is worse than no image.')
        return False

    version = _fw_version()
    if version.endswith('-dirty'):
        print('WARNING: the working tree is dirty; this artifact cannot be')
        print('  reproduced from a commit. The version says so.')

    image_path, size = build_firmware_image(version)
    image = image_path.read_bytes()

    private = _load_private()
    signature = sign_bytes(private, image)
    if not verify_bytes(private.public_key(), image, signature):
        print('Error: the signature did not verify against its own public key.')
        return False

    dist_dir = root_directory / 'backend' / 'fw_dist' / channel
    dist_dir.mkdir(parents=True, exist_ok=True)
    shutil.copy2(image_path, dist_dir / 'image.bin')
    (dist_dir / 'manifest.json').write_text(json.dumps({
        'version': version,
        'signature': signature.hex(),
    }, indent=2) + '\n')

    sha = hashlib.sha256(image).hexdigest()
    print(f'Published {version} to backend/fw_dist/{channel}/')
    print(f'  {size:,} bytes, sha256 {sha[:16]}...')
    print(f'  signed with {private.public_key().public_bytes_raw().hex()[:16]}...')
    print('  (the backend recomputes the sha256 from the bytes it serves, so the')
    print('   manifest cannot describe anything but this file)')

    if deploy:
        return deploy_backend(staging=(channel == 'dev'))
    target = 'deploy --staging' if channel == 'dev' else 'deploy'
    print(f"Run 'python tools/build.py {target}' (or re-run with --deploy) to ship it.")
    return True


def publish_app(output_dir: Path, deploy: bool) -> bool:
    """Publish the device app for OTA: build the ROMFS image into
    backend/app_dist/ (baked into the next backend deploy and served at
    /app/image + /app/manifest), optionally deploying right away.
    """
    image_path, sha = build_romfs_image(output_dir)
    dist_dir = root_directory / 'backend' / 'app_dist'
    dist_dir.mkdir(exist_ok=True)
    shutil.copy2(image_path, dist_dir / 'pico.romfs')
    print(f"Published to backend/app_dist/pico.romfs (sha256 {sha})")

    if deploy:
        return deploy_backend()
    print("Run 'python tools/build.py deploy' (or re-run with --deploy) to ship it.")
    return True


def deploy_backend(staging: bool = False):
    """Deploy the Rust backend to Fly.io (prod, or the staging mock pair's
    backend with --staging: same image, fly.staging.toml config)."""
    # The Dockerfile COPYs app_dist/ (the OTA app image); guarantee the dir
    # exists so a deploy without a published image still builds.
    (root_directory / 'backend' / 'app_dist').mkdir(exist_ok=True)

    key_file = root_directory / 'backend' / '.maxmind-key'
    if not key_file.exists():
        print(f"Error: {key_file.relative_to(root_directory)} not found.")
        print("Create it with your MaxMind license key:")
        print(f'  echo "your-key" > {key_file.relative_to(root_directory)}')
        return False

    license_key = key_file.read_text().strip()
    if not license_key:
        print(f"Error: {key_file.relative_to(root_directory)} is empty.")
        return False

    # Build context is the repo root: the backend is a Cargo workspace member
    # and depends on crates/scoreboard-wire by path, which a backend/ context
    # can't reach. The Dockerfile/ignorefile paths inside the fly config resolve
    # relative to the config file, so they stay backend-relative.
    config = 'fly.staging.toml' if staging else 'fly.toml'
    command = [
        'fly', 'deploy', '.',
        '--config', f'backend/{config}',
        '--build-secret', f'MAXMIND_LICENSE_KEY={license_key}',
    ]
    print(f"Deploying {'staging' if staging else 'prod'} backend to Fly.io...")
    result = subprocess.run(command, cwd=root_directory, check=False)
    if result.returncode != 0:
        print("Deploy failed!")
        return False

    print("Deploy complete!")
    return True


def main():
    parser = argparse.ArgumentParser(
        description="Build script for pico-scoreboard"
    )

    subparsers = parser.add_subparsers(dest='command')

    # deploy subcommand
    deploy_parser = subparsers.add_parser('deploy', help='Deploy backend to Fly.io')
    deploy_parser.add_argument(
        '--staging',
        action='store_true',
        help='deploy the staging backend (fly.staging.toml; pointed at mock-ESPN)',
    )

    # publish-app subcommand (OTA)
    publish_parser = subparsers.add_parser(
        'publish-app',
        help='Package an existing app build (--no-build) as the ROMFS image in '
             'backend/app_dist for OTA'
    )
    publish_parser.add_argument(
        '--deploy',
        action='store_true',
        help='Also deploy the backend (ships the update to the fleet)'
    )
    publish_parser.add_argument(
        '--no-build',
        action='store_true',
        help='Skip build step, package existing output'
    )
    add_common_args(publish_parser)

    # publish-fw subcommand (Rust firmware OTA)
    publish_fw_parser = subparsers.add_parser(
        'publish-fw',
        help='Build, sign and stage the Rust firmware image for OTA'
    )
    publish_fw_parser.add_argument(
        '--channel',
        choices=['stable', 'dev'],
        default='stable',
        help='which artifact to publish (default: stable). Devices follow the '
             'channel their ota.channel config names.',
    )
    publish_fw_parser.add_argument(
        '--deploy',
        action='store_true',
        help='deploy the backend afterwards (--channel dev deploys staging)',
    )

    def _add_deploy_mode_args(sub) -> None:
        """--release / --dev pair shared by flash and run.

        The DEPLOY mode is separate from the -c/--configuration BUILD mode
        (mpy-cross vs plain .py) despite the shared word "release". Default
        comes from build.config.json `flash_release`; --dev forces a littlefs
        deploy for one-off iteration.
        """
        group = sub.add_mutually_exclusive_group()
        group.add_argument(
            '--release',
            action='store_true',
            help='Deploy the app as a ROMFS image (in-place execution, ~100 KB '
                 'less heap) instead of littlefs files. Requires the custom '
                 'firmware with a ROMFS partition. Default comes from '
                 'build.config.json "flash_release".'
        )
        group.add_argument(
            '--dev',
            action='store_true',
            help='Force a littlefs deploy even when build.config.json sets '
                 '"flash_release": true. Littlefs files shadow any deployed '
                 'ROMFS image and load as RAM bytecode (~100 KB heap cost).'
        )

    # flash subcommand
    flash_parser = subparsers.add_parser(
        'flash', help='Flash an existing app build (--no-build) to the device'
    )
    flash_parser.add_argument(
        '--no-build',
        action='store_true',
        help='Skip build step, flash existing output'
    )
    flash_parser.add_argument(
        '--port',
        help='Serial port for flashing (auto-detect if not specified)'
    )
    _add_deploy_mode_args(flash_parser)
    add_common_args(flash_parser)

    # run subcommand
    run_parser = subparsers.add_parser(
        'run', help='Flash an existing app build (--no-build) and open the REPL'
    )
    run_parser.add_argument(
        '--no-build',
        action='store_true',
        help='Skip build step, flash existing output'
    )
    run_parser.add_argument(
        '--port',
        help='Serial port for flashing (auto-detect if not specified)'
    )
    _add_deploy_mode_args(run_parser)
    add_common_args(run_parser)

    # Global arguments (for default build command)
    add_common_args(parser)

    args = parser.parse_args()

    def _resolve_deploy_release(a) -> bool:
        """Deploy mode: --dev > --release > build.config.json flash_release."""
        if a.dev:
            return False
        return a.release or bool(_build_config.get('flash_release', False))

    # deploy command (doesn't use output_dir or firmware args)
    if args.command == 'deploy':
        return 0 if deploy_backend(staging=args.staging) else 1

    # publish-fw command (Rust firmware OTA)
    if args.command == 'publish-fw':
        return 0 if publish_fw(args.channel, args.deploy) else 1

    # publish-app command (OTA)
    if args.command == 'publish-app':
        output_dir = args.output if args.output.is_absolute() else root_directory / args.output
        if not args.no_build:
            if not do_build():
                return 1
        elif not output_dir.exists():
            print(f"Error: {output_dir} does not exist, and the app can no longer be built "
                  "(BACKLOG 116).")
            return 1
        return 0 if publish_app(output_dir, args.deploy) else 1

    # Resolve output directory (only needed for firmware commands)
    output_dir = args.output if args.output.is_absolute() else root_directory / args.output

    # Default command (no subcommand) = build only
    if args.command is None:
        if not do_build():
            return 1
        return 0

    # flash command
    elif args.command == 'flash':
        if not args.no_build:
            if not do_build():
                return 1
        elif not output_dir.exists():
            print(f"Error: {output_dir} does not exist, and the app can no longer be built "
                  "(BACKLOG 116).")
            return 1

        flash_device(output_dir, args.port, repl=False, release=_resolve_deploy_release(args))
        return 0

    # run command
    elif args.command == 'run':
        if not args.no_build:
            if not do_build():
                return 1
        elif not output_dir.exists():
            print(f"Error: {output_dir} does not exist, and the app can no longer be built "
                  "(BACKLOG 116).")
            return 1

        flash_device(output_dir, args.port, repl=True, release=_resolve_deploy_release(args))
        return 0

    return 0


if __name__ == '__main__':
    exit(main())
