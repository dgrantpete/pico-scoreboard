#!/usr/bin/env python3
"""Build every image the boot-spike demo needs, into demo/out/.

Artifacts (see README.md for the demo choreography that consumes them):

    boot.elf                the bootloader
    a_bad.elf/.bin          identity A, does NOT confirm trial boots (the
                            deliberately-broken image the revert demo ships)
    b_confirm.elf/.bin      identity B, confirms, can stage a_bad
    b_confirm.bin.sig       dev signature over b_confirm.bin (if keys exist)
    a_stager.elf            identity A, confirms, can stage b_confirm
    a_stager_verify.elf     same but staging goes through ed25519
                            verify_and_mark_updated
    cmd_stage.bin           1-byte mailbox payloads for probe-rs download
    cmd_stage_verified.bin
    cmd_stage_badsig.bin

Build order breaks the "A embeds B embeds A" cycle: a_bad embeds nothing,
b_confirm embeds a_bad, the stagers embed b_confirm.

The payload .bins are extracted with llvm-objcopy from the rust toolchain
(no cargo-binutils needed): -O binary over the ELF's loadable segments gives
exactly the bytes the active partition will hold.
"""

import re
import shutil
import subprocess
import sys
from pathlib import Path

SPIKE = Path(__file__).resolve().parent.parent
OUT = SPIKE / "demo" / "out"
TARGET_ELF = SPIKE / "target" / "thumbv8m.main-none-eabihf" / "release" / "spike-app"
BOOT_ELF = SPIKE / "target" / "thumbv8m.main-none-eabihf" / "release" / "spike-boot"

CMD_BYTES = {
    "cmd_stage.bin": 0x01,
    "cmd_stage_verified.bin": 0x02,
    "cmd_stage_badsig.bin": 0x03,
}


def run(args: list[str], **kw) -> None:
    print("+", " ".join(str(a) for a in args))
    subprocess.run(args, check=True, cwd=SPIKE, **kw)


def objcopy_path() -> Path:
    sysroot = subprocess.run(
        ["rustc", "--print", "sysroot"], check=True, capture_output=True, text=True
    ).stdout.strip()
    matches = list(Path(sysroot).glob("lib/rustlib/*/bin/rust-objcopy*"))
    if not matches:
        sys.exit("rust-objcopy not found in the toolchain sysroot")
    return matches[0]


def layout_consts() -> dict[str, int]:
    """Read partition sizes out of the single source of truth."""
    src = (SPIKE / "layout" / "src" / "lib.rs").read_text()
    consts = {}
    for name, expr in re.findall(r"pub const (\w+): u32 = ([^;]+);", src):
        try:
            consts[name] = eval(expr, {}, dict(consts))  # trusted repo file
        except NameError:
            pass
    return consts


def build_app(name: str, features: str, env_extra: dict[str, str], objcopy: Path, want_bin: bool) -> None:
    import os

    env = dict(os.environ, **env_extra)
    run(
        ["cargo", "build", "--release", "-p", "spike-app", "--no-default-features", "--features", features],
        env=env,
    )
    shutil.copy2(TARGET_ELF, OUT / f"{name}.elf")
    if want_bin:
        run([str(objcopy), "-O", "binary", str(TARGET_ELF), str(OUT / f"{name}.bin")])


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    objcopy = objcopy_path()
    consts = layout_consts()

    run(["cargo", "build", "--release", "-p", "spike-boot"])
    shutil.copy2(BOOT_ELF, OUT / "boot.elf")
    run([str(objcopy), "-O", "binary", str(BOOT_ELF), str(OUT / "boot.bin")])

    build_app("a_bad", "identity-a", {}, objcopy, want_bin=True)
    build_app(
        "b_confirm",
        "identity-b,confirm,stage",
        {"SPIKE_PAYLOAD": str(OUT / "a_bad.bin")},
        objcopy,
        want_bin=True,
    )
    build_app(
        "a_stager",
        "identity-a,confirm,stage",
        {"SPIKE_PAYLOAD": str(OUT / "b_confirm.bin")},
        objcopy,
        want_bin=False,
    )

    pub = SPIKE / "demo" / "keys" / "dev_pub.bin"
    priv = SPIKE / "demo" / "keys" / "dev_priv.bin"
    if priv.exists() and pub.exists():
        run([sys.executable, str(SPIKE / "demo" / "sign.py"), "sign", str(OUT / "b_confirm.bin")])
        build_app(
            "a_stager_verify",
            "identity-a,confirm,stage,verify",
            {
                "SPIKE_PAYLOAD": str(OUT / "b_confirm.bin"),
                "SPIKE_PAYLOAD_SIG": str(OUT / "b_confirm.bin.sig"),
            },
            objcopy,
            want_bin=False,
        )
    else:
        print("NOTE: demo/keys/dev_priv.bin missing - skipping a_stager_verify")
        print("      (run `python demo/sign.py keygen`, then re-run this script)")

    for fname, byte in CMD_BYTES.items():
        (OUT / fname).write_bytes(bytes([byte]))
    # Downloading this erases the containing 4 KB sector back to 0xFF —
    # used to reset the embassy-boot state sectors between demo phases.
    (OUT / "erase_byte.bin").write_bytes(b"\xff")

    print()
    boot_size = (OUT / "boot.bin").stat().st_size
    print(f"boot.bin      {boot_size:>8} bytes  (partition {consts['BOOT_SIZE']}, "
          f"{100 * boot_size / consts['BOOT_SIZE']:.0f}% full)")
    for name in ["a_bad", "b_confirm"]:
        size = (OUT / f"{name}.bin").stat().st_size
        print(f"{name + '.bin':<13} {size:>8} bytes  (active partition {consts['ACTIVE_SIZE']}, "
              f"{100 * size / consts['ACTIVE_SIZE']:.0f}% full)")
    if boot_size > consts["BOOT_SIZE"]:
        sys.exit("FATAL: bootloader does not fit its partition")


if __name__ == "__main__":
    main()
