# `index.html.gz` — the settings web app, committed as a build artifact

`index.html.gz` is the compiled Svelte SPA from `frontend/`, gzipped. It is
`include_bytes!`d into the firmware by `src/http/spa.rs` and hashed into its
ETag by `build.rs`.

## Why it is committed

This is the **committed-generated** pattern, and it is a deliberate exception
to CLAUDE.md's "generated files are gitignored" rule. The alternative — having
`build.rs` shell out to `bun run build` — would put a JavaScript toolchain in
the cargo dependency graph: every `cargo build` would need bun and a populated
`frontend/node_modules`, CI would install both to compile a firmware image, and
a network hiccup in `bun install` would look like a Rust build failure. The
artifact is 54 KB and changes when the frontend changes, which is rarely.

The MicroPython firmware made the same call for the same reason —
`tools/build.py` copies `frontend/build/index.html.gz` into the deploy
directory rather than building it as part of the firmware build.

## Regenerating it

```sh
cd frontend
bun install          # first time only
bun run build        # writes frontend/build/index.html.gz
cp build/index.html.gz ../firmware-rs/app/assets/index.html.gz
```

Then rebuild the firmware. `build.rs` has a `rerun-if-changed` on this file, so
the ETag follows the bytes automatically — there is nothing else to update.

**Commit the new artifact in the same commit as the frontend change that
produced it.** A bundle that does not correspond to any commit of `frontend/`
is the failure mode this pattern trades for, and the only defence is discipline.

## It is reproducible

The gzip is byte-identical across runs on the same source: `svelte.config.js`
pins `version.name = 'v1'` so the build carries no timestamp, and the gzip
header's MTIME field is zeroed. Two consecutive `bun run build` invocations were
verified to produce the same SHA-1 (2026-08-08). So a diff on this file means
the frontend actually changed, not that somebody rebuilt it.

## Provenance of the current artifact

| | |
|---|---|
| SHA-1 | `80e23203b05a816bf9c980859ab20f7c97f06ff1` |
| ETag (first 8 bytes) | `80e23203b05a816b` |
| Size | 56,617 B |
| Built | 2026-10-15, `bun 1.3.6`, in the same commit as the frontend change it carries |

The settings page gains the NHL: a switch on the Sports card and an "NHL
Live" layout picker (A Period ledger / B Rink), each shown only when the
device's `GET /api/config` carries its key, so a unit on the frozen
MicroPython never sees either (NHL.md D7). The artifact before it, 56,517 B
(`610bb724…`), was the status card showing the whole release version
(`2026.10.12-03942fd`, with the link profile in its tooltip) instead of its
first eight characters, which for a date-shaped version was `2026.10.` for
every build of a month; before that, 56,607 B (`496aa1b6…`), the Advanced
card's watchdog row reading the device's `watchdog_always_on`; and before
that, 56,305 B (`e436e643…`), built 2026-08-19 from the `frontend/` of
commit `3259128` — a bake that followed its frontend change in a commit of
its own, the failure mode the rule above warns about. The tree before this
change rebuilt to `610bb724…` byte for byte, and this one rebuilds to the
same SHA-1 twice running.
