"""Drive the firmware renderer over a scenario and capture raw RGB565 frames.

For each frame the loop replicates the Core 1 display tick exactly: set the
virtual clock, latch the published state via `acquire_display_state()`,
advance the REAL `LoopState` (the firmware's one cross-frame state object —
no hand-mirrored latch arithmetic here), poison all registered scratch, call
the renderer with the real `render_frame` signature
`(display, writer, regions, state, colors, now_ms, view_elapsed_ms,
play_elapsed_ms)`, and snapshot the RGB565 buffer. A static scenario yields
one frame; an animated one yields `duration_ms // 50` frames spaced 50 ms
apart in virtual time.

Scratch poisoning: before EVERY rendered frame, every object registered in
`display.scratch_buffers()` / `display.SCRATCH_PALETTE_ENTRIES` is filled
with sentinels. The Core 1 mutation contract (see scoreboard/display.py)
says scratch is write-before-read within one draw call, so correct code
overwrites the sentinels before they can reach a pixel; code that reads a
leftover value — scratch silently promoted to cross-frame state — renders
garbage and fails the golden test deterministically.
"""

from .shims.time_shim import CLOCK_START_MS

# Sentinels: valid-but-garish values a correct frame can never surface.
_POISON_BYTE = 0xAA
_POISON_WORD = 0x2AAA


def poison_scratch(display_mod, writer) -> None:
    """Fill every registered scratch object with sentinels (see module doc)."""
    for buf in display_mod.scratch_buffers(writer):
        if isinstance(buf, bytearray):
            for i in range(len(buf)):
                buf[i] = _POISON_BYTE
        else:  # list of small ints
            for i in range(len(buf)):
                buf[i] = _POISON_WORD
    for pal, first, count in display_mod.SCRATCH_PALETTE_ENTRIES:
        for e in range(first, first + count):
            pal.pixel(e, 0, _POISON_WORD)


def build_render_targets(env):
    """Construct the (display, writer, regions) trio the renderer draws into."""
    from hub75 import PreviewDisplay

    display = PreviewDisplay(128, 64)
    writer = env.fonts.FontWriter(display, default_font=env.fonts.unscii_8)
    regions = env.display.Regions(display)
    return display, writer, regions


def render_scenario(ctx, scenario, variant, display, writer, regions) -> "list[bytes]":
    """Render one scenario x variant into a list of raw RGB565 frame buffers.

    Scenario setup AND the Regions build both run inside `variant.apply()` so a
    screen-geometry variant (which flips `screen_geometry.PREGAME_VARIANT` etc.)
    governs both the strings the setter pre-builds (per-phase scroll dwell is
    sized against the active variant's region width) and the Regions the
    renderer draws into. Building either before the override would freeze them
    at the default variant. The `regions` passed in is rebuilt here for that
    reason.
    """
    frame_count = scenario.frame_count()
    renderer = variant.resolve_renderer()

    frames = []
    with variant.apply():
        ctx.clock.set(CLOCK_START_MS)
        ctx.reset()
        ctx.clock.set(CLOCK_START_MS)
        scenario.setup(ctx)
        regions = ctx.display.Regions(display)

        base = ctx.clock.now
        # The firmware's own cross-frame state object and latch arithmetic
        # (LoopState.advance_and_latch) — the golden test exercises the real
        # code, so preview and firmware cannot drift.
        ls = ctx.display.LoopState(base)
        for i in range(frame_count):
            now = base + i * 50
            ctx.clock.set(now)
            state, _seq = ctx.state.acquire_display_state()
            ls.advance_and_latch(state)
            poison_scratch(ctx.display, writer)
            renderer(display, writer, regions, state, state.ui_colors, now,
                     ls.view_elapsed, ls.play_elapsed)
            frames.append(bytes(display.buffer))
    return frames


def render_golden_frame(ctx, display, writer, regions, scenario_name, elapsed_ms):
    """Render a single deterministic frame of a scenario at a fixed elapsed time.

    Used by the golden test: same scenario, same virtual offset, same bytes.
    """
    from . import scenarios
    from . import variants

    scenario = scenarios.REGISTRY[scenario_name]
    variant = variants.REGISTRY["default"]

    renderer = variant.resolve_renderer()
    with variant.apply():
        ctx.clock.set(CLOCK_START_MS)
        ctx.reset()
        ctx.clock.set(CLOCK_START_MS)
        scenario.setup(ctx)
        regions = ctx.display.Regions(display)

        now = ctx.clock.now + elapsed_ms
        ctx.clock.set(now)
        state, _seq = ctx.state.acquire_display_state()
        poison_scratch(ctx.display, writer)
        # Under ideal pacing the frame rail equals the wall rail, so the fixed
        # golden offset serves as both elapsed values.
        renderer(display, writer, regions, state, state.ui_colors, now,
                 elapsed_ms, elapsed_ms)
    return bytes(display.buffer)
