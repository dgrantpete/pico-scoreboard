"""Design variants: a renderer + a set of firmware-constant overrides.

A variant names the renderer to call (`"module:function"`, default
`scoreboard.display:render_frame`) and an optional map of module constants to
override for the duration of the render:

    Variant("scroll-fast", overrides={
        "scoreboard.screen_geometry": {"GAME_SCROLL_PX_PER_SEC": 40},
    })

`apply()` is a context manager that setattr's each override onto the imported
module and restores the previous value on exit -- so a tuning sweep can render
the same scenario at several constant values without editing firmware.

Only the "default" variant exists today; screen-geometry variants (pregame /
final A/B/C) and tuning sweeps are registered here in a later phase.
"""

import importlib
from contextlib import contextmanager

DEFAULT_RENDERER = "scoreboard.display:render_frame"

REGISTRY: "dict[str, Variant]" = {}


class Variant:
    def __init__(self, name, renderer=DEFAULT_RENDERER, overrides=None):
        self.name = name
        self.renderer = renderer
        self.overrides = overrides or {}

    def resolve_renderer(self):
        module_path, _, func_name = self.renderer.partition(":")
        module = importlib.import_module(module_path)
        return getattr(module, func_name)

    @contextmanager
    def apply(self):
        """Temporarily install this variant's constant overrides.

        A dict-valued override onto an existing dict attr MERGES (the module
        keeps its object identity semantics: a fresh merged dict is
        installed, the original restored on exit) — this is how per-key
        selections like screen_geometry._ACTIVE are partially overridden.
        """
        saved = []  # (module, attr, had_it, old_value)
        try:
            for module_path, consts in self.overrides.items():
                module = importlib.import_module(module_path)
                for attr, value in consts.items():
                    had_it = hasattr(module, attr)
                    old = getattr(module, attr, None)
                    saved.append((module, attr, had_it, old))
                    if had_it and isinstance(old, dict) and isinstance(value, dict):
                        merged = dict(old)
                        merged.update(value)
                        setattr(module, attr, merged)
                    else:
                        setattr(module, attr, value)
            yield
        finally:
            for module, attr, had_it, old in reversed(saved):
                if had_it:
                    setattr(module, attr, old)
                else:
                    delattr(module, attr)


def register(variant: Variant) -> Variant:
    REGISTRY[variant.name] = variant
    return variant


def compatible_variants(scenario) -> "list[Variant]":
    """Variants a scenario opts into (all of them when it declares None)."""
    allowed = scenario.compatible_variants
    result = []
    for variant in REGISTRY.values():
        if allowed is None or variant.name in allowed:
            result.append(variant)
    return result


# Default: no overrides, whatever screen_geometry ships as the active variant.
register(Variant("default"))

# Screen-geometry variants: flip the active table for every sport sharing
# that screen (partial merges into screen_geometry._ACTIVE). The renderer is
# the normal render_frame (it dispatches by mode); scenarios opt in via
# compatible_variants so final variants only pair with final scenarios etc.
# (Pregame has no variants since 2026-07-15 — "Big time" is the one design.)
_SG = "scoreboard.screen_geometry"
for _letter in ("A", "B", "C"):
    register(Variant(f"final-{_letter}", overrides={_SG: {"_ACTIVE": {
        "mlb_final": _letter, "nba_final": _letter}}}))
    register(Variant(f"soccer-{_letter}", overrides={_SG: {"_ACTIVE": {
        "soccer_live": _letter}}}))

# Divider on/off comparison (config display.show_dividers); scenarios opt in.
register(Variant("no-dividers", overrides={_SG: {"SHOW_DIVIDERS": False}}))

# Critical-count red-tint saturation sweep (paired with critical-red-tint only).
for _s_max in (0, 48, 80, 128):
    register(Variant(f"red-tint-{_s_max}",
                     overrides={"scoreboard.display": {"CRITICAL_PULSE_S_MAX": _s_max}}))
