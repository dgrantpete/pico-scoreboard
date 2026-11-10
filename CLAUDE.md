# Pico Scoreboard

Monorepo: two firmwares (Rust/Embassy shipping, MicroPython legacy) | Svelte frontend | Rust/Axum backend

See `.claude/ARCHITECTURE.md` for full component details.

## Conventions

- Firmware (shipping): Rust + Embassy in `firmware-rs/` and `crates/*`, runs on
  a Pi Pico 2 W (RP2350) with HUB75 LED matrix driver, boot-integrated behind
  embassy-boot and OTA-updated from the backend. Spec: `firmware-rs/SPEC.md`
- Firmware (legacy): MicroPython in `firmware/`, still deployed on the gift-fleet
  units and still served by the backend's `/app/*` OTA surface. **Frozen since
  2026-10-01:** no feature or visual work lands there and it is not kept in
  sync with Rust. The Rust renderer is the visual source of truth; its golden
  frames are its own (`crates/scoreboard-render/tests/golden_frames.rs`,
  re-bless with `SCOREBOARD_BLESS_FRAMES=1`)
- Data path: the backend is the only data path. Phase S's direct-to-ESPN
  (no-backend) mode was deleted from main on 2026-11-08; its code is preserved
  at the git tag `archive/phase-s`, and `firmware-rs/PHASE-S*.md`,
  `S3-DESIGN.md` and `PARSE-PERF.md` are history
- Frontend: Svelte + Vite, compiled to single-file HTML, gzipped, served from the
  device — by both firmwares
- Backend: Rust + Axum, deployed on Fly.io, proxies ESPN API; the Docker build
  context is the repo root, not `backend/`
- Art (Rust): code wherever the art is geometry — the rink, court, star, count
  dot, inning arrows and toast icons are modules in `crates/scoreboard-render/src/`,
  pinned to exact pixels by their tests. The four hand-drawn sprites are 8-bit
  PNGs in `crates/scoreboard-render/art/`, the source of truth, compiled into
  the committed `src/generated/art.rs` by the `tests/art_compile.rs` test
  (re-generate with `SCOREBOARD_BLESS_ART=1`; it fails when stale). The
  court's static pixels are its code run on the host by
  `tests/court_compile.rs` into the committed `src/generated/court.rs`
  (`SCOREBOARD_BLESS_COURT=1`, same staleness check). No Aseprite, no Python
- Fonts: `firmware/assets/fonts/` is the source of truth; `tools/compile_fonts.py`
  emits the committed Rust glyph tables and the MicroPython modules in
  `firmware/src/scoreboard/fonts/` (everything except `__init__.py`, gitignored)
- MicroPython layout output is retired (BACKLOG 116, 2026-11-08): the
  `.aseprite` sources and `tools/compile_layout.py`, which generated
  `firmware/src/scoreboard/layout/`, are gone, so the MicroPython app cannot be
  rebuilt; `tools/build.py` says so, and the last published image is
  `backend/app_dist/pico.romfs`
