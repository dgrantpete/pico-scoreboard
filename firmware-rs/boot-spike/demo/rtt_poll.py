#!/usr/bin/env python3
"""SWD-side defmt capture + probe action driver for the boot-spike demo.

Why this exists: probe-rs 0.32 prints no RTT text when stdout is not a
terminal, so scripted `probe-rs run` sessions capture nothing. Instead of
streaming, this driver polls target RAM over SWD (`probe-rs read`) and reads
the defmt-rtt rings directly. Each binary pins its ring to a well-known
region (see spike-layout): bootloader at RTT_BOOT_BUF, app at RTT_APP_BUF —
so every captured byte is attributed to its source binary by construction,
across resets, swaps and watchdog reboots, in one continuous session.

The driver also executes probe actions (reset / download / flash) itself,
between polls, so nothing else ever contends for the probe. Actions come
from a command file (out/poller_cmd.txt): the orchestrating shell writes one
line, the driver picks it up on the next tick, executes it, and logs it in
the session log. Commands:

    reset                        probe-rs reset
    download <file> [<addr>]     probe-rs download (bin at addr, else ELF)
    stop                         end the session

Capture model: for every known _SEGGER_RTT header address (union over the
demo ELFs; both binaries currently place it at the same spot), read the
header, key the ring by its buffer pointer, and append bytes my cursor
hasn't seen. A write-cursor regression means the binary re-initialized the
header (new boot / jump into app): close the current generation file and
start the next. Generations land in out/session/<source>_genNNN.raw plus a
timestamped session.log; decode.py turns them into text transcripts.

Frames are never consumed device-side (RdOff untouched): one boot phase's
frames must fit in the 4 KB ring, which they comfortably do, and the
bootloader idles ~1.5 s at boot so a ~1 Hz poll can't miss a generation.
"""

import re
import struct
import subprocess
import sys
import time
from pathlib import Path

from elftools.elf.elffile import ELFFile

SPIKE = Path(__file__).resolve().parent.parent
OUT = SPIKE / "demo" / "out"
SESSION = OUT / "session"
CMD_FILE = OUT / "poller_cmd.txt"
PROBE_RS = str(Path.home() / ".cargo" / "bin" / "probe-rs.exe")
CHIP = ["--chip", "RP235x"]

HEADER_SIZE = 48  # id[16] + counts[8] + up[0]{name,buf,size,wr,rd,flags}


def layout_consts() -> dict[str, int]:
    """Rust u32 const expressions are valid Python (underscored literals
    included); multi-pass because consts may reference later ones."""
    src = (SPIKE / "layout" / "src" / "lib.rs").read_text()
    pending = re.findall(r"pub const (\w+): u32 = ([^;]+);", src)
    consts: dict[str, int] = {}
    while pending:
        still = []
        for name, expr in pending:
            try:
                consts[name] = eval(expr, {}, dict(consts))
            except NameError:
                still.append((name, expr))
        if len(still) == len(pending):
            break
        pending = still
    return consts


L = layout_consts()
SOURCES = {L["RTT_BOOT_BUF"]: "boot", L["RTT_APP_BUF"]: "app"}
RING = L["RTT_BUF_SIZE"]


def probe(args: list[str], tries: int = 3) -> str | None:
    for i in range(tries):
        r = subprocess.run([PROBE_RS, *args, *CHIP], capture_output=True, text=True)
        if r.returncode == 0:
            return r.stdout
        time.sleep(0.3)
    log(f"probe-rs {' '.join(args[:2])} failed: {r.stderr.strip().splitlines()[-1] if r.stderr else '?'}")
    return None


def read_mem(addr: int, n: int) -> bytes | None:
    out = probe(["read", "b8", f"{addr:#x}", str(n)])
    if out is None:
        return None
    data = b""
    for line in out.splitlines():
        if ":" in line:
            data += bytes.fromhex(line.split(":", 1)[1].replace(" ", ""))
    return data if len(data) == n else None


def header_addrs() -> set[int]:
    addrs = set()
    for elf_path in OUT.glob("*.elf"):
        with open(elf_path, "rb") as f:
            symtab = ELFFile(f).get_section_by_name(".symtab")
            for sym in symtab.iter_symbols():
                if sym.name == "_SEGGER_RTT":
                    addrs.add(sym["st_value"])
    return addrs


LOG_FH = None


def log(msg: str) -> None:
    line = f"[{time.strftime('%H:%M:%S')}] {msg}"
    print(line, flush=True)
    LOG_FH.write(line + "\n")
    LOG_FH.flush()


HEAD_CMP = 256  # bytes of each generation's start re-checked for reinit


class Source:
    def __init__(self, name: str):
        self.name = name
        self.cursor = 0
        self.gen = -1
        self.fh = None
        self.head = b""
        self._next_gen()

    def _next_gen(self):
        if self.fh:
            self.fh.close()
        self.gen += 1
        self.cursor = 0
        self.head = b""
        self.fh = open(SESSION / f"{self.name}_gen{self.gen:03d}.raw", "wb")
        log(f"{self.name}: generation {self.gen}")

    def advance(self, wroff: int, buf_addr: int):
        # Reinit detection must not depend on catching an intermediate
        # WrOff: a fresh boot rewrites the ring from 0, and every boot's
        # first frames carry a per-boot nonce, so comparing the stored start
        # of this generation against live memory catches a reboot even when
        # the new WrOff already passed the old cursor between polls.
        if self.cursor > 0 and self.head:
            live = read_mem(buf_addr, min(len(self.head), wroff) if wroff else len(self.head))
            if live is not None and live != self.head[: len(live)]:
                self._next_gen()
        elif wroff < self.cursor:
            self._next_gen()

        if wroff > self.cursor:
            chunk = read_mem(buf_addr + self.cursor, wroff - self.cursor)
            if chunk is None:
                return
            self.fh.write(chunk)
            self.fh.flush()
            log(f"{self.name}: +{len(chunk)} bytes (gen {self.gen}, {self.cursor}->{wroff})")
            self.cursor = wroff
            if len(self.head) < HEAD_CMP:
                self.head += chunk[: HEAD_CMP - len(self.head)]


def run_command(line: str) -> bool:
    """Returns False when the session should stop."""
    parts = line.split()
    match parts:
        case ["stop"]:
            log("command: stop")
            return False
        case ["reset"]:
            log("command: reset")
            probe(["reset"])
        case ["download", path]:
            log(f"command: download {path}")
            probe(["download", path])
        case ["download", path, addr]:
            log(f"command: download {path} @ {addr}")
            probe(["download", path, "--binary-format", "bin", "--base-address", addr])
        case _:
            log(f"command: UNRECOGNIZED {line!r}")
    return True


def main() -> None:
    SESSION.mkdir(parents=True, exist_ok=True)
    global LOG_FH
    LOG_FH = open(SESSION / "session.log", "a", encoding="utf-8")
    CMD_FILE.write_text("")

    addrs = header_addrs()
    log(f"session start; header addrs {[hex(a) for a in addrs]}, "
        f"rings boot={L['RTT_BOOT_BUF']:#x} app={L['RTT_APP_BUF']:#x}")

    sources = {name: Source(name) for name in SOURCES.values()}

    while True:
        cmd = CMD_FILE.read_text().strip()
        if cmd:
            CMD_FILE.write_text("")
            if not run_command(cmd):
                break

        seen: dict[int, tuple[int, int]] = {}  # buf_addr -> (wroff, header_addr)
        for haddr in addrs:
            hdr = read_mem(haddr, HEADER_SIZE)
            if hdr is None or not hdr.startswith(b"SEGGER RTT"):
                continue
            _name, buf, size, wr, _rd, _fl = struct.unpack_from("<IIIIII", hdr, 24)
            if buf in SOURCES and size == RING and wr <= RING:
                seen[buf] = (wr, haddr)

        for buf, (wr, _haddr) in seen.items():
            sources[SOURCES[buf]].advance(wr, buf)

        time.sleep(0.35)

    for s in sources.values():
        s.fh.close()
    log("session end")


if __name__ == "__main__":
    main()
