// Scoreboard custom board: Raspberry Pi Pico 2 W (RP2350) with ROMFS and
// networking tuned for the scoreboard workload. Based on the stock
// RPI_PICO2_W board config from the pinned micropython submodule; see
// firmware/board/README.md for what differs and why.

#define MICROPY_HW_BOARD_NAME                   "Pico Scoreboard (Pico 2 W)"

// Flash layout (4 MB total):
//   [ firmware ~1.0 MB | ROMFS 512 KB | littlefs 2.5 MB ]
// littlefs size matches stock RPI_PICO2_W (PICO_FLASH_SIZE_BYTES - 1.5 MB)
// so existing devices keep their filesystem intact across the firmware
// swap. The ROMFS partition is carved from the top of the remaining code
// region (rp2_flash.c: MICROPY_HW_ROMFS_BASE = storage base - ROMFS bytes)
// and holds the app's .mpy files, executed in place.
//
// NOTE: changing ROMFS_BYTES moves the partition BASE, orphaning existing
// contents — after a firmware reflash the device self-heals via
// ota.recover() (re-downloads the app from the backend), or redeploy with
// `build.py flash --release`. tools/build.py parses this value for its
// image-size check; keep the define on one line.
#define MICROPY_HW_FLASH_STORAGE_BYTES          (PICO_FLASH_SIZE_BYTES - 1536 * 1024)
#define MICROPY_HW_ROMFS_BYTES                  (512 * 1024)

// Enable networking.
#define MICROPY_PY_NETWORK 1
#define MICROPY_PY_NETWORK_HOSTNAME_DEFAULT     "scoreboard"

// CYW43 driver configuration.
#define CYW43_USE_SPI (1)
#define CYW43_LWIP (1)
#define CYW43_GPIO (1)
#define CYW43_SPI_PIO (1)

// For debugging mbedtls - also set
// Debug level (0-4) 1=warning, 2=info, 3=debug, 4=verbose
// #define MODUSSL_MBEDTLS_DEBUG_LEVEL 1

#define MICROPY_HW_PIN_EXT_COUNT    CYW43_WL_GPIO_COUNT

int mp_hal_is_pin_reserved(int n);
#define MICROPY_HW_PIN_RESERVED(i) mp_hal_is_pin_reserved(i)
