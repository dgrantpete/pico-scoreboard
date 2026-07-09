"""Drive the firmware renderer over a scenario and capture raw RGB565 frames.

For each frame the loop replicates the Core 1 display tick exactly: set the
virtual clock, latch the published state via `acquire_display_state()`, call the
renderer with the real `render_frame` signature
`(display, writer, regions, state, colors, now_ms, view_elapsed_ms,
play_elapsed_ms)` — the two frame-rail elapsed values latched exactly the way
the Core 1 thread latches them — and snapshot the RGB565
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
        # Frame-rail latching, mirroring run_display_thread: the rail advances
        # one FRAME_MS per frame and epochs re-latch when their stamp changes.
        anim_ms = 0
        view_stamp = None
        view_epoch = 0
        play_stamp = None
        play_epoch = 0
        for i in range(frame_count):
            now = base + i * 50
            ctx.clock.set(now)
            state, _seq = ctx.state.acquire_display_state()
            anim_ms += 50
            if state.animation_start_ms != view_stamp:
                view_stamp = state.animation_start_ms
                view_epoch = anim_ms
            if state.game.play.updated_ms != play_stamp:
                play_stamp = state.game.play.updated_ms
                play_epoch = anim_ms
            renderer(display, writer, regions, state, state.ui_colors, now,
                     anim_ms - view_epoch, anim_ms - play_epoch)
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
        # Under ideal pacing the frame rail equals the wall rail, so the fixed
        # golden offset serves as both elapsed values.
        renderer(display, writer, regions, state, state.ui_colors, now,
                 elapsed_ms, elapsed_ms)
    return bytes(display.buffer)
