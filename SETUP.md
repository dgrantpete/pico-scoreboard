# Project Setup Guide

This document explains the commands used to initialize the pico-scoreboard monorepo.

---

## Prerequisites

Before starting, ensure you have the following installed:

| Tool | Version | Check Command |
|------|---------|---------------|
| Bun | 1.0+ | `bun --version` |
| Rust | 1.70+ | `rustc --version` |
| Cargo | 1.70+ | `cargo --version` |

---

## Directory Structure

```bash
mkdir -p frontend backend scripts
```

Creates the three main directories:
- `frontend/` - Svelte web application
- `backend/` - Rust API server
- `scripts/` - Build and deployment automation

---

## Frontend Setup (Svelte + Vite + Bun)

### 1. Create Vite Project

```bash
cd frontend
bun create vite . --template svelte-ts
```

**What this does:**
- `bun create vite` - Uses Bun to scaffold a new Vite project
- `.` - Creates the project in the current directory (frontend/)
- `--template svelte-ts` - Uses the Svelte + TypeScript template

**Files created:**
- `package.json` - Project manifest and scripts
- `tsconfig.json` - TypeScript configuration
- `vite.config.ts` - Vite build configuration
- `svelte.config.js` - Svelte compiler options
- `src/` - Source code directory
  - `main.ts` - Application entry point
  - `App.svelte` - Root Svelte component
  - `app.css` - Global styles

### 2. Install Dependencies

```bash
bun install
```

**What this does:**
- Reads `package.json` and installs all listed dependencies
- Creates `bun.lockb` - Bun's lockfile (binary format, faster than JSON)
- Creates `node_modules/` - Installed packages

**Dependencies installed:**
- `svelte` - The Svelte framework
- `vite` - Build tool and dev server
- `@sveltejs/vite-plugin-svelte` - Vite plugin for Svelte compilation
- `typescript` - TypeScript compiler
- `svelte-check` - Type checking for Svelte files

### 3. Add Single-File Plugin

```bash
bun add -D vite-plugin-singlefile
```

**What this does:**
- `bun add` - Adds a new dependency to the project
- `-D` - Adds as a devDependency (only needed for building, not runtime)
- `vite-plugin-singlefile` - Inlines all JS/CSS into a single HTML file

**Why we need this:**
The Pico has limited storage. By inlining everything into one HTML file, we can gzip it and store it efficiently on the device.

### 4. Configure Vite for Single-File Output

The `vite.config.ts` is updated to:

```typescript
import { defineConfig } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'
import { viteSingleFile } from 'vite-plugin-singlefile'

export default defineConfig({
  plugins: [svelte(), viteSingleFile()],
  build: {
    target: 'esnext',
    assetsInlineLimit: 100000000,
  },
})
```

**Configuration explained:**
- `plugins: [svelte(), viteSingleFile()]` - Enable both Svelte compilation and single-file output
- `build.target: 'esnext'` - Use modern JavaScript (smaller output, modern browsers only)
- `build.assetsInlineLimit: 100000000` - Inline assets up to ~100MB (effectively everything)

### 5. Development Commands

```bash
# Start development server with hot reload
bun dev

# Build for production (creates dist/index.html)
bun run build

# Preview production build locally
bun run preview

# Type-check the project
bun run check
```

---

## Backend Setup (Rust + Axum)

### 1. Initialize Cargo Project

```bash
cd backend
cargo init
```

**What this does:**
- Creates a new Rust binary project in the current directory
- Creates `Cargo.toml` - Rust's package manifest (like package.json)
- Creates `src/main.rs` - Entry point with a "Hello, world!" template

**Cargo.toml structure:**
```toml
[package]
name = "backend"      # Package name
version = "0.1.0"     # Semantic version
edition = "2024"      # Rust edition (language version)

[dependencies]        # Runtime dependencies go here
```

### 2. Add Dependencies

```bash
cargo add axum
cargo add tokio --features full
cargo add reqwest --features json
cargo add serde --features derive
cargo add serde_json
cargo add tower-http --features cors
cargo add tracing
cargo add tracing-subscriber
```

**What `cargo add` does:**
- Fetches the latest compatible version from crates.io
- Adds the dependency to `Cargo.toml`
- Updates `Cargo.lock` with exact resolved versions

**Dependencies explained:**

| Crate | Purpose |
|-------|---------|
| `axum` | Web framework for routing and request handling |
| `tokio` | Async runtime (required for async Rust) |
| `reqwest` | HTTP client for calling ESPN API |
| `serde` | Serialization/deserialization framework |
| `serde_json` | JSON support for serde |
| `tower-http` | HTTP middleware (CORS, compression, etc.) |
| `tracing` | Structured logging/diagnostics |
| `tracing-subscriber` | Log output formatting |

**Feature flags:**
- `--features full` (tokio) - Enables all tokio features (networking, timers, etc.)
- `--features json` (reqwest) - Enables JSON request/response bodies
- `--features derive` (serde) - Enables `#[derive(Serialize, Deserialize)]` macros
- `--features cors` (tower-http) - Enables only CORS middleware, not the full crate

### 3. Verify Dependencies

```bash
cargo tree --depth 1
```

**Output shows direct dependencies:**
```
backend v0.1.0
├── axum v0.8.8
├── reqwest v0.13.1
├── serde v1.0.228
├── serde_json v1.0.149
├── tokio v1.49.0
├── tower-http v0.6.8
├── tracing v0.1.44
└── tracing-subscriber v0.3.22
```

### 4. Development Commands

```bash
# Check code compiles without building
cargo check

# Build debug binary
cargo build

# Build optimized release binary
cargo build --release

# Run the server
cargo run

# Run with auto-reload (requires cargo-watch)
cargo watch -x run
```

---

## Dependency Management Philosophy

### Bun (JavaScript)

```bash
bun add <package>        # Add runtime dependency
bun add -D <package>     # Add dev dependency
bun remove <package>     # Remove dependency
bun update               # Update all dependencies
bun list                 # Show installed packages
```

### Cargo (Rust)

```bash
cargo add <crate>                    # Add dependency (latest version)
cargo add <crate>@1.2.3              # Add specific version
cargo add <crate> --features foo     # Add with feature flags
cargo remove <crate>                 # Remove dependency
cargo update                         # Update within semver constraints
cargo tree                           # Show dependency tree
```

---

## Version Pinning

Both tools handle versions differently:

**Bun/npm** - Uses semver ranges in `package.json`:
```json
"dependencies": {
  "svelte": "^5.47.0"    // ^5.47.0 means >=5.47.0 <6.0.0
}
```

**Cargo** - Uses semver ranges in `Cargo.toml`:
```toml
[dependencies]
axum = "0.8.8"           # "0.8.8" means >=0.8.8 <0.9.0
```

Both create lockfiles (`bun.lockb`, `Cargo.lock`) that pin exact versions for reproducible builds.

---

## Quick Reference

| Task | Frontend (Bun) | Backend (Cargo) |
|------|----------------|-----------------|
| Initialize | `bun create vite . --template svelte-ts` | `cargo init` |
| Add dependency | `bun add <pkg>` | `cargo add <crate>` |
| Install all | `bun install` | `cargo build` |
| Dev server | `bun dev` | `cargo run` |
| Production build | `bun run build` | `cargo build --release` |
| Check types | `bun run check` | `cargo check` |
