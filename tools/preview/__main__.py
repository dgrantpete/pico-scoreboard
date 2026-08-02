"""CLI entry point: render scenarios x variants into PNGs/GIFs + an HTML gallery.

    python -m tools.preview                      # all scenarios, placeholder logos
    python -m tools.preview --list               # list scenarios + variants
    python -m tools.preview --scenario live-basic --open
    python -m tools.preview --backend-url https://... --api-key KEY --refresh-logos
"""

import argparse
import os
import sys
import webbrowser
from pathlib import Path

from . import gallery, panel, render
from .firmware_env import load_firmware
from .shims.time_shim import VirtualClock

_DEFAULT_OUT = Path(__file__).resolve().parent / "out"


def _parse_args(argv):
    p = argparse.ArgumentParser(prog="python -m tools.preview")
    p.add_argument("--scenario", action="append", default=[],
                   help="render only this scenario (repeatable)")
    p.add_argument("--variant", action="append", default=[],
                   help="render only this variant (repeatable)")
    p.add_argument("--backend-url", default=None,
                   help="fetch real logos from this backend base URL")
    p.add_argument("--api-key", default=None,
                   help="API key (else $SCOREBOARD_API_KEY)")
    p.add_argument("--refresh-logos", action="store_true",
                   help="ignore cached logos and re-fetch")
    p.add_argument("--scale", type=int, default=panel.DEFAULT_SCALE,
                   help="LED-look upscale factor (default 8)")
    p.add_argument("--flat", action="store_true",
                   help="gallery shows the flat panel image, not the LED look")
    p.add_argument("--out", default=None, help="output directory")
    p.add_argument("--open", action="store_true", help="open the gallery when done")
    p.add_argument("--list", action="store_true", help="list scenarios/variants and exit")
    return p.parse_args(argv)


def _do_list():
    from . import scenarios, variants  # scenarios import needs no firmware env
    print("Scenarios:")
    for name, sc in scenarios.REGISTRY.items():
        kind = f"animated {sc.duration_ms}ms" if sc.duration_ms > 0 else "static"
        print(f"  {name:22s} {kind}")
    print("\nVariants:")
    for name, v in variants.REGISTRY.items():
        print(f"  {name:22s} -> {v.renderer}")


def _select(names, registry_keys, kind):
    if not names:
        return list(registry_keys)
    unknown = [n for n in names if n not in registry_keys]
    if unknown:
        raise SystemExit(f"unknown {kind}(s): {', '.join(unknown)}\n"
                         f"available: {', '.join(registry_keys)}")
    return names


def main(argv=None) -> int:
    args = _parse_args(sys.argv[1:] if argv is None else argv)

    if args.list:
        _do_list()
        return 0

    clock = VirtualClock()
    env = load_firmware(clock)

    # Imported after the firmware env is live (renderers resolve firmware modules).
    from . import scenarios, variants
    from .logos import LogoProvider
    from .scenarios import ScenarioContext

    scenario_names = _select(args.scenario, list(scenarios.REGISTRY.keys()), "scenario")
    variant_names = _select(args.variant, list(variants.REGISTRY.keys()), "variant")

    api_key = args.api_key or os.environ.get("SCOREBOARD_API_KEY")
    logos = LogoProvider(backend_url=args.backend_url, api_key=api_key,
                         refresh=args.refresh_logos)

    display, writer, regions = render.build_render_targets(env)
    ctx = ScenarioContext(env, logos)

    out_dir = Path(args.out) if args.out else _DEFAULT_OUT
    out_dir.mkdir(parents=True, exist_ok=True)

    rows = []
    for sname in scenario_names:
        scenario = scenarios.REGISTRY[sname]
        compatible = {v.name for v in variants.compatible_variants(scenario)}
        cells = {}
        for vname in variant_names:
            if vname not in compatible:
                continue
            variant = variants.REGISTRY[vname]
            frames = render.render_scenario(ctx, scenario, variant, display, writer, regions)
            cell = _emit_cell(out_dir, sname, vname, frames, args.scale, args.flat)
            cells[vname] = cell
            print(f"  rendered {sname} / {vname} -> {cell['src']} "
                  f"({len(frames)} frame{'s' if len(frames) != 1 else ''})")
        rows.append({"scenario": sname, "cells": cells})

    subtitle = (f"{len(scenario_names)} scenarios x {len(variant_names)} variant(s) - "
                f"{'flat panel' if args.flat else f'LED look x{args.scale}'}"
                f"{' - real logos' if args.backend_url else ' - placeholder logos'}")
    index = gallery.write_gallery(out_dir, rows, variant_names, subtitle)
    print(f"\nGallery: {index}")

    if args.open:
        webbrowser.open(index.resolve().as_uri())
    return 0


def _emit_cell(out_dir, sname, vname, frames, scale, flat):
    """Write the image(s) for one scenario/variant cell; return its gallery record."""
    base = f"{sname}__{vname}"
    natives = [panel.buffer_to_image(f) for f in frames]
    animated = len(natives) > 1

    # Flat 128x64 ground-truth PNG (frame 0) is always written.
    flat_name = f"{base}_flat.png"
    panel.save_png(natives[0], out_dir / flat_name)

    if flat:
        if animated:
            gif_name = f"{base}.gif"
            panel.save_gif([panel.nearest_upscale(im, scale) for im in natives],
                           out_dir / gif_name)
            return {"src": gif_name, "animated": True}
        return {"src": flat_name, "animated": False}

    if animated:
        gif_name = f"{base}.gif"
        panel.save_gif([panel.led_image(im, scale) for im in natives], out_dir / gif_name)
        return {"src": gif_name, "animated": True}

    png_name = f"{base}.png"
    panel.save_png(panel.led_image(natives[0], scale), out_dir / png_name)
    return {"src": png_name, "animated": False}


if __name__ == "__main__":
    raise SystemExit(main())
