"""Fold text to the display fonts' ASCII + Latin-1 repertoire.

The glyph tables (scoreboard.fonts) cover codepoints 32..255, so Latin-1
names — Suárez, Peña, Muñoz — render natively. Anything above 0xFF is
folded here to its closest in-repertoire equivalent at wire ingest
(wire.read_str): base letters for the Latin Extended diacritics that show
up in NBA/soccer names (Jokić -> Jokic, Şengün -> Sengün), ASCII for
typographic punctuation. Unmapped codepoints pass through and render as
the fonts' '?' fallback glyph.

Runs on Core 0 at commit time only, and only for strings that actually
contain a non-ASCII char (the caller's fast path skips pure-ASCII strings
in O(1)); the render path never sees this module.

The 1:1 table is two parallel const strings rather than a dict: string
literals in a ROMFS-deployed .mpy are memory-mapped in place, so the table
costs no heap, and lookups are a rare-path linear scan.
"""

# Latin Extended-A (0x100..0x17F), complete, in codepoint order, followed by
# the Latin Extended-B / General Punctuation strays worth having: Romanian
# comma-below Ș ș Ț ț (0x218..0x21B — what ESPN actually sends for Romanian
# names), hyphen/dash forms, curly quotes, prime marks, and the bullet
# (folded to Latin-1 middle dot). Multi-char expansions live in _MULTI.
_SRC = (
    "ĀāĂăĄąĆćĈĉĊċČčĎďĐđ"
    "ĒēĔĕĖėĘęĚěĜĝĞğĠġĢģ"
    "ĤĥĦħĨĩĪīĬĭĮįİı"
    "ĴĵĶķĸĹĺĻļĽľĿŀŁł"
    "ŃńŅņŇňŊŋŌōŎŏŐő"
    "ŔŕŖŗŘřŚśŜŝŞşŠš"
    "ŢţŤťŦŧŨũŪūŬŭŮůŰűŲų"
    "ŴŵŶŷŸŹźŻżŽžſ"
    "ȘșȚț"
    "‐‑‒–—―"
    "‘’‚“”„"
    "•′″⁄"
)
_DST = (
    "AaAaAaCcCcCcCcDdDd"
    "EeEeEeEeEeGgGgGgGg"
    "HhHhIiIiIiIiIi"
    "JjKkkLlLlLlLlLl"
    "NnNnNnNnOoOoOo"
    "RrRrRrSsSsSsSs"
    "TtTtTtUuUuUuUuUuUu"
    "WwYyYZzZzZzs"
    "SsTt"
    "------"
    "'',\"\"\""
    "\xb7'\"/"
)

# The few folds that widen: ligatures and the ellipsis.
_MULTI = {
    "Ĳ": "IJ",
    "ĳ": "ij",
    "ŉ": "'n",
    "Œ": "OE",
    "œ": "oe",
    "…": "...",
}

# The tables are data, not code — a length drift would silently misfold
# everything after the drift point, so fail the import instead.
assert len(_SRC) == len(_DST)


def fold_text(s: str) -> str:
    """Fold codepoints above 0xFF to renderable equivalents.

    Returns `s` itself (no allocation) when nothing needs folding; unmapped
    high codepoints are kept and render as the '?' fallback glyph.
    """
    for ch in s:
        if ch >= "Ā":
            break
    else:
        return s
    out = []
    for ch in s:
        if ch >= "Ā":
            i = _SRC.find(ch)
            if i >= 0:
                ch = _DST[i]
            else:
                ch = _MULTI.get(ch, ch)
        out.append(ch)
    return "".join(out)
