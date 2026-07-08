"""Desktop preview pipeline: run the real firmware render code on CPython.

Loads `scoreboard.{state,display,mlb,fonts,config}` under MicroPython shims,
drives named scenarios through the real state machinery, and renders the
intercepted RGB565 buffers to PNGs/GIFs plus an HTML gallery -- so screen
designs can be iterated without flashing a device.

Entry point: ``python -m tools.preview`` (see ``__main__.py``).
"""
