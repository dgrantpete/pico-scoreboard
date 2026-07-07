# Frozen-module manifest for the scoreboard firmware.
#
# Deliberately minimal: only the rp2 port's baseline manifest (rp2 helpers,
# asyncio). The stock RPI_PICO2_W additionally freezes "bundle-networking"
# (mip, ntptime, requests, webrepl) and "aioble" — the scoreboard uses none
# of those (it vendors its own aiohttp fork and syncs time from its own
# backend), and Bluetooth is compiled out entirely.
#
# App code is NOT frozen here by design: frozen modules can't be updated
# over the air. The app ships as .mpy on littlefs today and moves to the
# ROMFS partition (in-place execution, OTA-rewritable) next — see BACKLOG.
include("$(PORT_DIR)/boards/manifest.py")
