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

Prerequisites:
    pip install mpy-cross    # For .mpy compilation
"""

import shutil
import subprocess
import argparse
import time
from pathlib import Path

# Directory structure
root_directory = Path(__file__).parent.parent
firmware_source = root_directory / 'firmware' / 'src'
frontend_directory = root_directory / 'frontend'
frontend_build = frontend_directory / 'build'

# Files to always copy without compilation (glob patterns)
COPY_ONLY_FILES = [
    '**/main.py',      # Entry point - keep as .py for debugging
    '**/config.json',  # Configuration file
    '**/index.html.gz',           # Binary assets (index.html.gz)
    '**/*.mpy',          # Already compiled (hub75, miqro deps)
]

# Files/directories to skip entirely (glob patterns)
SKIP_FILES = [
    '*/__pycache__/*',  # Python cache files
    '*.pyc',            # Compiled Python cache
]

# Files to skip in release builds (glob patterns)
DEV_ONLY_FILES = []  # None currently, but available for future use


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

        # Skip dev-only files in release
        if configuration == 'release' and any(file.full_match(p) for p in DEV_ONLY_FILES):
            print(f"  Skipping {relative_path} (dev-only)")
            continue

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


def flash_device(source_dir: Path, port: str = None, repl: bool = False):
    """Flash files to Pico using mpremote."""
    print(f"Flashing {source_dir.relative_to(root_directory)}/ to device...")

    # Build command for copy + reset
    cmd_parts = ['mpremote']
    if port:
        cmd_parts.extend(['connect', port, '+'])
    cmd_parts.extend(['cp', '-r', f'{source_dir.as_posix()}/.', ':', '+', 'reset'])

    subprocess.run(cmd_parts, check=True)

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


def do_build(output_dir: Path, configuration: str, arch: str) -> bool:
    """Execute the build pipeline."""
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
    """Add common arguments to a parser."""
    parser.add_argument(
        '-o', '--output',
        type=Path,
        default='pico',
        help='Output directory (default: pico)'
    )
    parser.add_argument(
        '-c', '--configuration',
        choices=['dev', 'release'],
        default='release',
        help='Build configuration: dev copies .py files, release compiles to .mpy (default: release)'
    )
    parser.add_argument(
        '-a', '--arch',
        choices=['armv7emsp', 'armv6m', 'all'],
        default='all',
        help='Target architecture for mpy-cross: RP2040=armv6m, RP2350=armv7emsp (default: all)'
    )


def main():
    parser = argparse.ArgumentParser(
        description="Build script for pico-scoreboard"
    )

    subparsers = parser.add_subparsers(dest='command')

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
    add_common_args(run_parser)

    # Global arguments (for default build command)
    add_common_args(parser)

    args = parser.parse_args()

    # Resolve output directory
    output_dir = args.output if args.output.is_absolute() else root_directory / args.output

    # Default command (no subcommand) = build only
    if args.command is None:
        if not do_build(output_dir, args.configuration, args.arch):
            return 1
        return 0

    # flash command
    elif args.command == 'flash':
        if not args.no_build:
            if not do_build(output_dir, args.configuration, args.arch):
                return 1
        elif not output_dir.exists():
            print(f"Error: {output_dir} does not exist. Run build first or remove --no-build.")
            return 1

        flash_device(output_dir, args.port, repl=False)
        return 0

    # run command
    elif args.command == 'run':
        if not args.no_build:
            if not do_build(output_dir, args.configuration, args.arch):
                return 1
        elif not output_dir.exists():
            print(f"Error: {output_dir} does not exist. Run build first or remove --no-build.")
            return 1

        flash_device(output_dir, args.port, repl=True)
        return 0

    return 0


if __name__ == '__main__':
    exit(main())
