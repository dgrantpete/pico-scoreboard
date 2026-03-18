# Pico Scoreboard

Monorepo: MicroPython firmware (Pi Pico W + HUB75 LED matrix) | Svelte frontend | Rust/Axum backend

See `.claude/ARCHITECTURE.md` for full component details.

## Agent Teams

For any task spanning multiple components or with parallel workstreams, use **native agent teams** (TeamCreate) rather than individual subagents. When the user says "team" or "swarm", always use TeamCreate. Be eager about using teams — prefer them whenever there are multiple competing objectives or parallel work opportunities.

## Conventions

- Firmware: MicroPython, runs on Pi Pico W with HUB75 LED matrix driver
- Frontend: Svelte + Vite, compiled to single-file HTML, gzipped, served from Pico
- Backend: Rust + Axum, deployed on Fly.io, proxies ESPN API
