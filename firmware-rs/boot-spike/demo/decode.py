#!/usr/bin/env python3
"""Decode the raw defmt captures from a rtt_poll.py session into text.

Boot generations decode against boot.elf. App generations are decoded
against every app ELF variant and the cleanest decode wins (0 skipped
frames beats any skips; more lines breaks ties) — the winner is printed in
the header, and the in-message identity strings ("app A:", "app B:") make a
wrong pick self-evident anyway.

Writes <raw>.txt next to each .raw and prints everything, generation by
generation, interleaved with session.log for the timeline.
"""

import subprocess
import sys
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

SPIKE = Path(__file__).resolve().parent.parent
OUT = SPIKE / "demo" / "out"
SESSION = OUT / "session"
DEFMT_PRINT = str(Path.home() / ".cargo" / "bin" / "defmt-print.exe")

APP_ELVES = ["a_stager", "a_stager_verify", "b_confirm", "a_bad"]


def decode(raw: bytes, elf: Path) -> tuple[list[str], int]:
    r = subprocess.run(
        [DEFMT_PRINT, "-e", str(elf), "--show-skipped-frames", "stdin"],
        input=raw,
        capture_output=True,
    )
    all_lines = r.stdout.decode(errors="replace").splitlines()
    # defmt-print reports skips on stdout as "(HOST) malformed frame skipped"
    # plus a location line; count them as skips, not as decoded output.
    skipped = sum("malformed frame" in l for l in all_lines)
    lines = [l for l in all_lines if "malformed frame" not in l and "defmt-print @" not in l]
    return lines, skipped


def main() -> None:
    for raw_path in sorted(SESSION.glob("*_gen*.raw")):
        raw = raw_path.read_bytes()
        if not raw:
            continue
        if raw_path.name.startswith("boot_"):
            candidates = [OUT / "boot.elf"]
        else:
            candidates = [OUT / f"{n}.elf" for n in APP_ELVES if (OUT / f"{n}.elf").exists()]
        decodes = [(elf, *decode(raw, elf)) for elf in candidates]
        # The banner's feature flags are runtime args, so they read true in
        # any decode that renders the banner at all — use them to split the
        # verify build from its siblings, whose tables largely coincide.
        if len(decodes) > 1:
            is_verify = any("verify=true" in l for _, lines, _ in decodes for l in lines)
            decodes = [d for d in decodes if ("verify" in d[0].name) == is_verify] or decodes
        best = None
        for elf, lines, skipped in decodes:
            # A wrong sibling table often still "decodes" (shared format
            # strings), but tends to produce spurious ERROR lines; prefer
            # clean decodes over long ones.
            errors = sum(l.startswith("ERROR") for l in lines)
            score = (skipped == 0, -errors, len(lines), -skipped)
            if best is None or score > best[0]:
                best = (score, elf, lines, skipped)
        _, elf, lines, skipped = best
        header = f"=== {raw_path.name} ({len(raw)} bytes, decoded with {elf.name}" + (
            f", {skipped} skipped!)" if skipped else ")"
        )
        text = "\n".join([header, *lines, ""])
        raw_path.with_suffix(".txt").write_text(text, encoding="utf-8")
        print(text)


if __name__ == "__main__":
    main()
