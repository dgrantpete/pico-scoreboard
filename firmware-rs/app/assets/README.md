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
| SHA-1 | `8f3da1a8a347aabe343947c357f73f4d3fa4d264` |
| ETag (first 8 bytes) | `8f3da1a8a347aabe` |
| Size | 54,641 B |
| Built | 2026-08-08, `bun 1.3.6`, from the `frontend/` of the commit that carries it |

The provenance line names no commit hash because it cannot: the rule above puts
the bundle and the source change in one commit, so the hash would have to
reference the commit it is inside. The previous artifact was 54,528 B with SHA-1
`6547ed61…`, built from `frontend/` at commit `18a05c8`; the 113-byte growth is
the 60 FPS scroll-speed options and the stale-value hint in
`ScreenLayoutsCard.svelte`.
