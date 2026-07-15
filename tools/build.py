#!/usr/bin/env python3
"""
Build script for pico-scoreboard.

Builds the SvelteKit frontend and prepares firmware files for deployment
to a Raspberry Pi Pico running MicroPython.

Usage:
    python tools/build.py              # Build to pico/ (release mode)
    python tools/build.py -c dev       # Build without .mpy compilation
    python tools/build.py flash        # Build and flash to device
    python tools/build.py run          # Build, flash, and open REPL
    python tools/build.py flash --no-build   # Flash without rebuilding
    python tools/build.py deploy       # Deploy backend to Fly.io

Prerequisites:
    pip install mpy-cross    # For .mpy compilation
"""

import hashlib
import json
import shutil
import subprocess
import argparse
import time
from pathlib import Path

from compile_layout import compile_all as compile_layout_all
from compile_fonts import compile_all as compile_fonts_all

# Directory structure
root_directory = Path(__file__).parent.parent
firmware_source = root_directory / 'firmware' / 'src'
frontend_directory = root_directory / 'frontend'
frontend_build = frontend_directory / 'build'

# Files to always copy without compilation (glob patterns)
COPY_ONLY_FILES = [
    '**/main.py',      # Entry point - keep as .py for debugging
    '**/ota.py',       # OTA/recovery - boot-critical, field-readable
    '**/config.json',  # Configuration file
    '**/index.html.gz',           # Binary assets (index.html.gz)
    '**/*.mpy',          # Already compiled (hub75, miqro deps)
]

# Files/directories to skip entirely (glob patterns)
# Note: Path.full_match matches against the whole path and `*` does not cross
# separators, so these patterns need a `**/` prefix to match at any depth.
SKIP_FILES = [
    '**/__pycache__/**',  # Python cache directories
    '**/*.pyc',           # Compiled Python cache
]

def _load_build_config() -> dict:
    """Load default argument values from tools/build.config.json if present."""
    config_path = Path(__file__).parent / 'build.config.json'
    if not config_path.exists():
        return {}
    with open(config_path) as f:
        return json.load(f)


_build_config = _load_build_config()


def build_frontend() -> bool:
    """Build the SvelteKit frontend using Bun."""
    print("Building frontend...")
    result = subprocess.run(
        ['bun', 'run', 'build'],
        cwd=frontend_directory,
        check=False
    )
    if result.returncode != 0:
        print("Frontend build failed!")
        return False
    print("Frontend build complete.")
    return True


def process_firmware_files(output_dir: Path, configuration: str, arch: str):
    """
    Process firmware files - compile .py to .mpy or copy.

    In release mode, .py files are compiled to .mpy using mpy-cross.
    In dev mode, all files are copied without compilation.

    Args:
        output_dir: Destination directory for processed files
        configuration: 'dev' or 'release'
        arch: Target architecture ('armv6m', 'armv7emsp', or 'all')
    """
    print(f"Processing firmware files ({configuration} mode)...")

    compiled_count = 0
    copied_count = 0

    for file in firmware_source.rglob('*'):
        if file.is_dir():
            continue

        relative_path = file.relative_to(firmware_source)
        output_path = output_dir / relative_path

        # Skip files that should never be included
        if any(file.full_match(p) for p in SKIP_FILES):
            continue

        output_path.parent.mkdir(parents=True, exist_ok=True)

        # Copy non-.py files and copy-only patterns
        if file.suffix != '.py' or any(file.full_match(p) for p in COPY_ONLY_FILES):
            shutil.copy2(file, output_path)
            copied_count += 1
            print(f"  Copied {relative_path}")
            continue

        # In dev mode, copy .py files without compilation
        if configuration == 'dev':
            shutil.copy2(file, output_path)
            copied_count += 1
            print(f"  Copied {relative_path} (dev mode)")
            continue

        # Try to compile .py to .mpy
        try:
            mpy_path = output_path.with_suffix('.mpy')
            cmd = ['mpy-cross', '-o', str(mpy_path), str(file)]
            if arch != 'all':
                cmd.append(f'-march={arch}')

            result = subprocess.run(cmd, capture_output=True, check=True, text=True)
            compiled_count += 1
            print(f"  Compiled {relative_path} -> {relative_path.with_suffix('.mpy')}")

        except subprocess.CalledProcessError as e:
            # If compilation fails due to arch requirements, fall back to copying
            if 'invalid arch' in (e.stderr or ''):
                shutil.copy2(file, output_path)
                copied_count += 1
                print(f"  Copied {relative_path} (multi-arch required)")
            else:
                print(f"  Error compiling {relative_path}:")
                print(f"    {e.stderr or e.stdout}")
                raise

    print(f"  Compiled: {compiled_count}, Copied: {copied_count}")


def copy_frontend_build(output_dir: Path) -> bool:
    """Copy built frontend to output directory."""
    src = frontend_build / 'index.html.gz'
    dst = output_dir / 'index.html.gz'

    if not src.exists():
        print(f"Warning: {src} not found. Run frontend build first.")
        return False

    shutil.copy2(src, dst)
    print(f"  Copied index.html.gz from frontend build")
    return True


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


def do_build(output_dir: Path, configuration: str, arch: str, no_assets: bool = False) -> bool:
    """Execute the build pipeline."""
    # Regenerate layout/font modules from firmware/assets/ (source of truth)
    if not no_assets:
        print("Compiling layout modules...")
        compile_layout_all()
        print("Compiling font modules...")
        compile_fonts_all()

    # Build frontend
    if not build_frontend():
        return False

    # Clean and recreate output directory
    if output_dir.exists():
        shutil.rmtree(output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)

    # Process firmware files (compile or copy)
    process_firmware_files(output_dir, configuration, arch)

    # Copy frontend build output (overwrites source index.html.gz)
    copy_frontend_build(output_dir)

    print(f"\nBuild complete: {output_dir.relative_to(root_directory)}/")
    return True


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
    parser.add_argument(
        '-c', '--configuration',
        choices=['dev', 'release'],
        default=_build_config.get('configuration', 'release'),
        help='Build configuration: dev copies .py files, release compiles to .mpy '
             '(default from build.config.json, else "release")'
    )
    parser.add_argument(
        '-a', '--arch',
        choices=['armv7emsp', 'armv6m', 'all'],
        default=_build_config.get('arch', 'armv7emsp'),
        help='Target architecture for mpy-cross: RP2040=armv6m, RP2350=armv7emsp '
             '(default from build.config.json, else "armv7emsp")'
    )
    parser.add_argument(
        '--no-assets',
        action='store_true',
        help='Skip sprite/font regeneration (fast iteration when only firmware .py changed)'
    )


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


def deploy_backend():
    """Deploy the Rust backend to Fly.io."""
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

    print("Deploying backend to Fly.io...")
    result = subprocess.run(
        ['fly', 'deploy', '--build-secret', f'MAXMIND_LICENSE_KEY={license_key}'],
        cwd=root_directory / 'backend',
        check=False
    )
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
    subparsers.add_parser('deploy', help='Deploy backend to Fly.io')

    # publish-app subcommand (OTA)
    publish_parser = subparsers.add_parser(
        'publish-app',
        help='Build the app ROMFS image into backend/app_dist for OTA'
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
    flash_parser = subparsers.add_parser('flash', help='Build and flash to device')
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
    run_parser = subparsers.add_parser('run', help='Build, flash, and open REPL')
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
        return 0 if deploy_backend() else 1

    # publish-app command (OTA)
    if args.command == 'publish-app':
        output_dir = args.output if args.output.is_absolute() else root_directory / args.output
        if not args.no_build:
            if not do_build(output_dir, args.configuration, args.arch, args.no_assets):
                return 1
        elif not output_dir.exists():
            print(f"Error: {output_dir} does not exist. Run build first or remove --no-build.")
            return 1
        return 0 if publish_app(output_dir, args.deploy) else 1

    # Resolve output directory (only needed for firmware commands)
    output_dir = args.output if args.output.is_absolute() else root_directory / args.output

    # Default command (no subcommand) = build only
    if args.command is None:
        if not do_build(output_dir, args.configuration, args.arch, args.no_assets):
            return 1
        return 0

    # flash command
    elif args.command == 'flash':
        if not args.no_build:
            if not do_build(output_dir, args.configuration, args.arch, args.no_assets):
                return 1
        elif not output_dir.exists():
            print(f"Error: {output_dir} does not exist. Run build first or remove --no-build.")
            return 1

        flash_device(output_dir, args.port, repl=False, release=_resolve_deploy_release(args))
        return 0

    # run command
    elif args.command == 'run':
        if not args.no_build:
            if not do_build(output_dir, args.configuration, args.arch, args.no_assets):
                return 1
        elif not output_dir.exists():
            print(f"Error: {output_dir} does not exist. Run build first or remove --no-build.")
            return 1

        flash_device(output_dir, args.port, repl=True, release=_resolve_deploy_release(args))
        return 0

    return 0


if __name__ == '__main__':
    exit(main())
