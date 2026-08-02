# Scoreboard custom board: Raspberry Pi Pico 2 W (RP2350).
# Differences from the stock RPI_PICO2_W board (see also mpconfigboard.h):
#   - Bluetooth OFF: the scoreboard never uses BLE; btstack costs ~20 KB
#     RAM (static pools) and ~80 KB flash in the stock build.
#   - lwip TCP pools raised: the stock MEMP_NUM_TCP_PCB=5 allows only ~4-5
#     simultaneous TCP connections (established + TIME_WAIT share the
#     pool), which silently drops inbound SYNs when a browser talks to the
#     device's web server while the score poller holds its HTTPS
#     connection. 16 PCBs ≈ +2 KB static RAM. PBUF pool raised to match.

set(PICO_BOARD "pico2_w")

set(MICROPY_PY_LWIP ON)
set(MICROPY_PY_NETWORK_CYW43 ON)

# Bluetooth: off (unused by the scoreboard; frees ~20 KB RAM / ~80 KB flash)
set(MICROPY_PY_BLUETOOTH OFF)
set(MICROPY_BLUETOOTH_BTSTACK OFF)
set(MICROPY_PY_BLUETOOTH_CYW43 OFF)

# Latent pico-sdk gap exposed by BT-off: cyw43_driver_picow's own interface
# source (cyw43_bus_pio_spi.c) includes pico/cyw43_driver.h but the target
# doesn't link pico_cyw43_driver_headers, so the include dir never reaches
# the firmware target. Every in-tree CYW43 board masks this because
# Bluetooth's pico_btstack_hci_transport_cyw43 pulls those headers in.
# Board cmake runs before targets exist, so defer the (headers-only,
# no-extra-sources) link until the end of the port's directory scope.
cmake_language(DEFER CALL target_link_libraries firmware pico_cyw43_driver_headers)

# lwip TCP tuning (values are #ifndef-guarded upstream, so -D wins)
list(APPEND MICROPY_DEF_BOARD
    MEMP_NUM_TCP_PCB=16
    MEMP_NUM_TCP_PCB_LISTEN=8
    PBUF_POOL_SIZE=24
)

# Board specific version of the frozen manifest
set(MICROPY_FROZEN_MANIFEST ${MICROPY_BOARD_DIR}/manifest.py)
