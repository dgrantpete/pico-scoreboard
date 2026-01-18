# Pico Scoreboard - Monorepo Structure Plan

## Project Components

1. **Frontend** - Plain Svelte app, compiled to single file, gzipped, stored on Pico
2. **Firmware** - MicroPython on Pi Pico: HUB75 driver + Microdot web server
3. **Backend** - Rust/Axum cloud service that proxies ESPN API (hosted on AWS)

---

## Recommended Directory Structure

```
pico-scoreboard/
├── frontend/                    # Svelte web application
│   ├── src/
│   ├── public/
│   ├── package.json             # Created via bun create
│   ├── vite.config.ts
│   └── tsconfig.json
│
├── firmware/                    # Pi Pico MicroPython code
│   ├── lib/                     # External dependencies (gitignored, populated at build)
│   ├── src/
│   │   ├── main.py              # Entry point
│   │   ├── server.py            # Microdot routes/endpoints
│   │   ├── display.py           # Scoreboard rendering logic
│   │   ├── config.py            # Settings management
│   │   └── api_client.py        # Calls to cloud backend
│   ├── www/                     # Served static files (gitignored)
│   ├── config.toml              # Build configuration (driver version, etc.)
│   └── build.py                 # Build script
│
├── backend/                     # Rust/Axum cloud API service
│   ├── src/
│   │   ├── main.rs
│   │   ├── routes/
│   │   ├── services/
│   │   └── models/
│   ├── Cargo.toml               # Created via cargo init + cargo add
│   └── Dockerfile
│
├── scripts/                     # Top-level automation
│   ├── build-all.py
│   └── deploy-pico.py
│
├── .github/workflows/
├── .gitignore
└── README.md
```

---

## Setup Commands (CLI-Based)

### 1. Create Project Root

```bash
cd pico-scoreboard
mkdir -p frontend firmware/src firmware/www backend scripts .github/workflows
```

### 2. Frontend Setup (Bun + Svelte + Vite)

```bash
cd frontend

# Initialize with bun and create Svelte project
bun create vite . --template svelte-ts

# Add single-file plugin for embedding everything into one HTML
bun add -D vite-plugin-singlefile

# Verify installed versions
bun list
```

Then configure `vite.config.ts` for single-file output:
```bash
# Update vite.config.ts to include singlefile plugin
# (see Configuration section below for the actual config)
```

### 3. Backend Setup (Rust + Axum)

```bash
cd backend

# Initialize Cargo project
cargo init

# Add dependencies (cargo add pulls latest versions automatically)
cargo add axum
cargo add tokio --features full
cargo add reqwest --features json
cargo add serde --features derive
cargo add serde_json
cargo add tower-http --features cors
cargo add tracing
cargo add tracing-subscriber

# Verify installed versions
cargo tree --depth 1
```

### 4. Firmware Dependencies Config

Create `firmware/config.toml`:
```bash
cd firmware

# The build script will read this and download the specified versions
cat > config.toml << 'EOF'
[dependencies]
# Specify versions here - build.py will download from GitHub releases / PyPI
hub75_version = "v1.0.0"
hub75_repo = "dgrantpete/pi-pico-hub75-driver"
# Microdot version from PyPI
microdot_version = "latest"  # or pin to specific version like "2.5.1"

[build]
# RP2040 (Pico 1) or RP2350 (Pico 2)
target_arch = "armv6m"
EOF
```

---

## Current Latest Versions (as of Jan 2026)

| Package | Version | Source |
|---------|---------|--------|
| axum | 0.8.8 | [crates.io](https://crates.io/crates/axum) |
| tokio | 1.49.0 | [crates.io](https://crates.io/crates/tokio) |
| reqwest | 0.13.1 | [crates.io](https://crates.io/crates/reqwest) |
| tower-http | 0.6.6 | [crates.io](https://crates.io/crates/tower-http) |
| svelte | 5.46.0 | [npm](https://www.npmjs.com/package/svelte) |
| @sveltejs/vite-plugin-svelte | 6.2.4 | [npm](https://www.npmjs.com/package/@sveltejs/vite-plugin-svelte) |
| vite | 7.x | [npm](https://www.npmjs.com/package/vite) |
| vite-plugin-singlefile | 2.3.0 | [npm](https://www.npmjs.com/package/vite-plugin-singlefile) |
| microdot | 2.5.1 | [PyPI](https://pypi.org/project/microdot/) |

**Note**: Using `cargo add` and `bun add` ensures you get the latest compatible versions at setup time rather than hardcoded values.

---

## Configuration Files

### Frontend: vite.config.ts

After running `bun create vite`, update the config:

```ts
import { defineConfig } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'
import { viteSingleFile } from 'vite-plugin-singlefile'

export default defineConfig({
  plugins: [svelte(), viteSingleFile()],
  build: {
    target: 'esnext',
    assetsInlineLimit: 100000000,
  }
})
```

### Firmware: build.py

```python
#!/usr/bin/env python3
"""
Build script for firmware - downloads dependencies and packages for deployment.
"""
import tomllib
import subprocess
import zipfile
import io
import shutil
from pathlib import Path
from urllib.request import urlopen

def get_latest_pypi_version(package: str) -> str:
    """Fetch latest version from PyPI."""
    import json
    url = f"https://pypi.org/pypi/{package}/json"
    with urlopen(url) as response:
        data = json.loads(response.read())
        return data["info"]["version"]

def download_hub75_driver(version: str, arch: str, lib_dir: Path):
    """Download HUB75 driver from GitHub releases."""
    base_url = f"https://github.com/dgrantpete/pi-pico-hub75-driver/releases/download/{version}"

    # Asset naming based on your release structure
    asset_name = f"hub75-{arch}-release.zip"
    url = f"{base_url}/{asset_name}"

    print(f"Downloading HUB75 driver {version} for {arch}...")

    with urlopen(url) as response:
        with zipfile.ZipFile(io.BytesIO(response.read())) as zf:
            hub75_dir = lib_dir / "hub75"
            hub75_dir.mkdir(parents=True, exist_ok=True)
            zf.extractall(hub75_dir)

    print(f"  Extracted to {hub75_dir}")

def download_microdot(version: str, lib_dir: Path):
    """Download Microdot source files."""
    if version == "latest":
        version = get_latest_pypi_version("microdot")

    print(f"Downloading Microdot {version}...")

    # Download from GitHub raw files (more reliable for MicroPython)
    base_url = f"https://raw.githubusercontent.com/miguelgrinberg/microdot/v{version}/src/microdot"
    files = ["microdot.py"]  # Add more as needed: "websocket.py", etc.

    microdot_dir = lib_dir / "microdot"
    microdot_dir.mkdir(parents=True, exist_ok=True)

    for file in files:
        url = f"{base_url}/{file}"
        dest = microdot_dir / file
        with urlopen(url) as response:
            dest.write_bytes(response.read())
        print(f"  Downloaded {file}")

def copy_frontend(www_dir: Path):
    """Copy built frontend to www directory."""
    frontend_dist = Path("../frontend/dist/index.html")
    if not frontend_dist.exists():
        print("Warning: Frontend not built yet. Run 'bun run build' in frontend/ first.")
        return

    www_dir.mkdir(parents=True, exist_ok=True)

    # Compress with gzip
    import gzip
    with open(frontend_dist, 'rb') as f_in:
        with gzip.open(www_dir / "index.html.gz", 'wb') as f_out:
            f_out.write(f_in.read())

    print(f"Compressed frontend to {www_dir / 'index.html.gz'}")

def main():
    # Load config
    with open("config.toml", "rb") as f:
        config = tomllib.load(f)

    lib_dir = Path("lib")
    www_dir = Path("www")

    # Clean previous build
    if lib_dir.exists():
        shutil.rmtree(lib_dir)

    # Download dependencies
    download_hub75_driver(
        config["dependencies"]["hub75_version"],
        config["build"]["target_arch"],
        lib_dir
    )

    download_microdot(
        config["dependencies"]["microdot_version"],
        lib_dir
    )

    # Copy frontend
    copy_frontend(www_dir)

    print("\nBuild complete! Contents ready for deployment in lib/ and www/")

if __name__ == "__main__":
    main()
```

---

## .gitignore

```gitignore
# Frontend
frontend/node_modules/
frontend/dist/
frontend/.svelte-kit/

# Firmware - generated at build time
firmware/lib/
firmware/www/

# Backend
backend/target/

# General
.DS_Store
*.pyc
__pycache__/
.env
```

---

## Build & Deployment Flow

```
1. Build Frontend
   bun run build (in frontend/)
   └─> dist/index.html (single file)

2. Build Firmware Package
   python build.py (in firmware/)
   └─> Downloads HUB75 driver from GitHub releases
   └─> Downloads Microdot
   └─> Gzips frontend → www/index.html.gz

3. Deploy to Pico
   python scripts/deploy-pico.py
   └─> Uses mpremote to upload firmware/* to Pico

4. Build & Deploy Backend
   cargo build --release (in backend/)
   └─> Docker build → push to AWS (ECR/ECS or Lambda)
```

---

## Key Firmware Endpoints

```
GET  /              → Serve gzipped frontend (Content-Encoding: gzip)
GET  /api/settings  → Current display settings
POST /api/settings  → Update settings
GET  /api/games     → Current game data (cached or from backend)
POST /api/refresh   → Force refresh from cloud backend
```

---

## Summary

| Component | Directory | Technology | Init Command |
|-----------|-----------|------------|--------------|
| Frontend | `/frontend/` | Svelte + Vite + Bun | `bun create vite . --template svelte-ts` |
| Firmware | `/firmware/` | MicroPython + Microdot | `python build.py` (downloads deps) |
| Backend | `/backend/` | Rust + Axum | `cargo init && cargo add axum tokio reqwest...` |

All version management happens via CLI tools (`bun add`, `cargo add`) or the firmware build script, ensuring you always get current versions rather than stale hardcoded values.
