"""Static HTML gallery: scenario rows x variant columns.

Emits a single self-contained `out/index.html` (inline CSS + JS, no external
assets) so it opens straight off the filesystem. Images are lazy-loaded and
click-to-zoom into a fullscreen overlay. Regenerated on every run.
"""

import html
from pathlib import Path


_PAGE_CSS = """
:root { color-scheme: dark; }
* { box-sizing: border-box; }
body {
  margin: 0; padding: 24px;
  background: #0b0d12; color: #d7dbe0;
  font: 14px/1.5 -apple-system, Segoe UI, Roboto, sans-serif;
}
h1 { font-size: 18px; margin: 0 0 4px; }
p.sub { margin: 0 0 20px; color: #7d8592; }
table { border-collapse: collapse; }
th, td {
  border: 1px solid #232833; padding: 10px;
  text-align: center; vertical-align: middle;
}
th { background: #151922; position: sticky; top: 0; }
th.scenario, td.scenario {
  text-align: left; font-weight: 600; white-space: nowrap;
  background: #151922; position: sticky; left: 0;
}
td img {
  image-rendering: pixelated; max-width: 100%; height: auto;
  cursor: zoom-in; border-radius: 3px; display: block; margin: 0 auto;
}
.tag {
  display: inline-block; font-size: 11px; color: #0b0d12;
  background: #6ea8fe; border-radius: 3px; padding: 0 5px; margin-left: 6px;
}
.empty { color: #545b68; font-style: italic; }
#overlay {
  position: fixed; inset: 0; background: rgba(0,0,0,.9);
  display: none; align-items: center; justify-content: center; cursor: zoom-out;
  z-index: 10;
}
#overlay img { image-rendering: pixelated; max-width: 96vw; max-height: 96vh; }
"""

_ZOOM_JS = """
(function () {
  var overlay = document.getElementById('overlay');
  var big = document.getElementById('overlay-img');
  document.addEventListener('click', function (e) {
    var t = e.target;
    if (t.tagName === 'IMG' && t.dataset.zoom) {
      big.src = t.dataset.zoom;
      overlay.style.display = 'flex';
    } else if (overlay.style.display === 'flex') {
      overlay.style.display = 'none';
      big.src = '';
    }
  });
})();
"""


def write_gallery(out_dir: Path, rows: list, variant_names: list, subtitle: str = "") -> Path:
    """Write `out/index.html`. `rows` = [{scenario, cells:{variant:{src,animated}}}]."""
    parts = [
        "<!doctype html><html><head><meta charset='utf-8'>",
        "<meta name='viewport' content='width=device-width, initial-scale=1'>",
        "<title>Scoreboard Preview</title>",
        f"<style>{_PAGE_CSS}</style></head><body>",
        "<h1>Pico Scoreboard - Desktop Preview</h1>",
        f"<p class='sub'>{html.escape(subtitle)}</p>",
        "<table><thead><tr><th class='scenario'>scenario</th>",
    ]
    for vname in variant_names:
        parts.append(f"<th>{html.escape(vname)}</th>")
    parts.append("</tr></thead><tbody>")

    for row in rows:
        parts.append(
            f"<tr><td class='scenario'>{html.escape(row['scenario'])}</td>"
        )
        for vname in variant_names:
            cell = row["cells"].get(vname)
            if not cell:
                parts.append("<td><span class='empty'>-</span></td>")
                continue
            src = html.escape(cell["src"])
            tag = "<span class='tag'>gif</span>" if cell.get("animated") else ""
            parts.append(
                f"<td><img loading='lazy' src='{src}' data-zoom='{src}' "
                f"alt='{html.escape(row['scenario'])} / {html.escape(vname)}'>{tag}</td>"
            )
        parts.append("</tr>")

    parts.append("</tbody></table>")
    parts.append("<div id='overlay'><img id='overlay-img' alt='zoom'></div>")
    parts.append(f"<script>{_ZOOM_JS}</script>")
    parts.append("</body></html>")

    out_path = out_dir / "index.html"
    out_path.write_text("".join(parts), encoding="utf-8")
    return out_path
