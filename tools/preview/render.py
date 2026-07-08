"""Drive the firmware renderer over a scenario and capture raw RGB565 frames.

For each frame the loop replicates the Core 1 display tick exactly: set the
virtual clock, latch the published state via `acquire_display_state()`, call the
renderer with the real `render_frame` signature
`(display, writer, regions, state, colors, now_ms)`, and snapshot the RGB565
buffer. A static scenario yields one frame; an animated one yields
`duration_ms // 50` frames spaced 50 ms apart in virtual time.
"""

from .shims.time_shim import CLOCK_START_MS


def build_render_targets(env):
    """Construct the (display, writer, regions) trio the renderer draws into."""
    from hub75 import PreviewDisplay

    display = PreviewDisplay(128, 64)
    writer = env.fonts.FontWriter(display, default_font=env.fonts.unscii_8)
    regions = env.display.Regions(display)
    return display, writer, regions


def render_scenario(ctx, scenario, variant, display, writer, regions) -> "list[bytes]":
    """Render one scenario x variant into a list of raw RGB565 frame buffers."""
    ctx.clock.set(CLOCK_START_MS)
    ctx.reset()
    ctx.clock.set(CLOCK_START_MS)
    scenario.setup(ctx)

    base = ctx.clock.now
    frame_count = scenario.frame_count()
    renderer = variant.resolve_renderer()

    frames = []
    with variant.apply():
        for i in range(frame_count):
            now = base + i * 50
            ctx.clock.set(now)
            state, _seq = ctx.state.acquire_display_state()
            renderer(display, writer, regions, state, state.ui_colors, now)
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

    ctx.clock.set(CLOCK_START_MS)
    ctx.reset()
    ctx.clock.set(CLOCK_START_MS)
    scenario.setup(ctx)

    now = ctx.clock.now + elapsed_ms
    ctx.clock.set(now)
    renderer = variant.resolve_renderer()
    with variant.apply():
        state, _seq = ctx.state.acquire_display_state()
        renderer(display, writer, regions, state, state.ui_colors, now)
    return bytes(display.buffer)
