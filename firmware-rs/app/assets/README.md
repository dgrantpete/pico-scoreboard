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
| SHA-1 | `e436e6431b630e7b48cbbbe3ce851ca51b6b77b3` |
| ETag (first 8 bytes) | `e436e6431b630e7b` |
| Size | 56,305 B |
| Built | 2026-08-19, `bun 1.3.6`, from the `frontend/` of commit `3259128` |

This artifact names a source commit, which the rule above says it should not
need to: the timezone lane's frontend change (`3259128`) landed without its
bake, so the bundle follows in a commit of its own — the failure mode the rule
warns about, caught at the next flash boundary rather than on a device. The
1,664-byte growth is the timezone card, its store, and the offset-schedule API
surface. The previous artifact was 54,641 B with SHA-1 `8f3da1a8…`, built
2026-08-08.
