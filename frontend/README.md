# Frontend

The scoreboard's settings web app: SvelteKit, built to a single inlined,
gzipped HTML file that the firmware serves off the device itself. It talks to
the firmware's `/api/*` endpoints (config, network status, WiFi setup, logs) —
there is no separate host to deploy it to.

## Developing

```sh
bun install
bun run dev            # or: bun run dev -- --open
```

API calls are relative (`/api/config`, …), so the dev server has no device
behind them unless you add a Vite proxy. Pages are hash-routed (`/#/settings`),
which is what the device serves.

## Building

```sh
bun run build          # writes build/index.html.gz
```

`adapter-static` with `bundleStrategy: 'inline'` and `precompress` produces the
single file; `tools/build.py` then copies it into the MicroPython firmware
(`firmware/src/index.html.gz`), and the Rust firmware embeds its own committed
copy at `firmware-rs/app/assets/index.html.gz` (see that directory's README).
