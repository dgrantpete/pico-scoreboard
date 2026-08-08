# =============================================================================
# Stage 1: Build Stage
# =============================================================================
# We use the official Rust image which includes cargo, rustc, and common tools.
# "bookworm" is Debian 12, a stable Linux distribution.
FROM rust:1.91-bookworm AS builder

# Install build dependencies required by aws-lc-sys (your TLS library):
# - build-essential: C compiler (gcc) and basic build tools
# - cmake: Required by aws-lc-sys for building cryptographic libraries
# - pkg-config: Helps find installed libraries
# - libssl-dev: OpenSSL headers (some crates may need this)
#
# The "rm -rf /var/lib/apt/lists/*" cleans up package lists to reduce image size.
RUN apt-get update && apt-get install -y \
    build-essential \
    cmake \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Set the working directory inside the container. The build context is the
# REPO ROOT (the backend is a Cargo workspace member and depends on
# crates/scoreboard-wire by path, which a backend-only context can't reach), so
# this is the workspace root and paths below are repo-relative.
WORKDIR /app

# =============================================================================
# Dependency Caching Optimization
# =============================================================================
# Docker caches each "layer" (each RUN/COPY command creates a layer).
# If files don't change, Docker reuses the cached layer.
#
# Strategy: copy only the manifests first and build against stub sources, so
# the third-party dependency compilation (~5 minutes) is reused whenever only
# our own code changed. Every workspace member the backend pulls in needs its
# manifest here; a new one shows up as a "failed to load manifest" build error.

COPY Cargo.toml Cargo.lock ./
COPY backend/Cargo.toml backend/
COPY crates/scoreboard-wire/Cargo.toml crates/scoreboard-wire/

# Stub sources: enough for cargo to resolve the workspace and compile the
# dependency graph. `-p backend` keeps future firmware-only members (which
# don't build for this host) out of the picture.
RUN mkdir -p backend/src crates/scoreboard-wire/src && \
    echo "fn main() {}" > backend/src/main.rs && \
    : > crates/scoreboard-wire/src/lib.rs && \
    cargo build --release -p backend && \
    rm -rf backend/src crates/scoreboard-wire/src

# Now copy the actual source code
COPY crates/scoreboard-wire/src/ crates/scoreboard-wire/src/
COPY backend/src/ backend/src/

# Copy the config directory (contains default.toml with non-secret defaults)
COPY backend/config/ backend/config/

# Touch the stubbed-over roots so Cargo recompiles them (otherwise it might
# think the stubs are still current)
RUN touch backend/src/main.rs crates/scoreboard-wire/src/lib.rs && \
    cargo build --release -p backend

# =============================================================================
# Stage 2: Runtime Stage
# =============================================================================
# Start fresh with a minimal image - we don't need Rust/gcc/cmake anymore!
# debian:bookworm-slim is ~80MB vs ~2GB for the build image.
FROM debian:bookworm-slim AS runtime

# Install runtime dependencies:
# - ca-certificates: Required for HTTPS connections (verifying ESPN's SSL cert)
# - libssl3: OpenSSL runtime library
# - curl: Used at build time to download the MaxMind GeoIP database
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create a non-root user for security.
# Running as root inside containers is a security risk - if someone
# exploits your app, they'd have root access to the container.
RUN useradd --create-home --shell /bin/bash appuser

WORKDIR /app

# Copy ONLY the compiled binary from the build stage (not all the build tools!)
# The "--from=builder" references the first stage by its name.
COPY --from=builder /app/target/release/backend /app/backend

# Copy the configuration file (provides defaults; secrets come from env vars)
COPY --from=builder /app/backend/config /app/config

# Device app ROMFS image for OTA (produced by `tools/build.py publish-app`).
# Own layer, after the binary, so app-image-only publishes reuse the cached
# cargo build. The endpoints 404 gracefully if the image is absent.
COPY backend/app_dist/ /app/app_dist/
# The Rust firmware's signed images, per channel. The .gitkeep files are what
# make this COPY succeed on a checkout where nothing has been published yet.
COPY backend/fw_dist/ /app/fw_dist/

# Download MaxMind GeoLite2-City database for IP-to-timezone resolution.
# This executes at build time — the .mmdb file is baked into the image.
# The license key is mounted as a build secret and never appears in any layer.
RUN --mount=type=secret,id=MAXMIND_LICENSE_KEY \
    curl -sSL "https://download.maxmind.com/app/geoip_download?edition_id=GeoLite2-City&license_key=$(cat /run/secrets/MAXMIND_LICENSE_KEY)&suffix=tar.gz" \
    | tar -xz --strip-components=1 --wildcards "*.mmdb" -C /app/

# Change ownership to our non-root user
RUN chown -R appuser:appuser /app

# Switch to non-root user
USER appuser

# Document that this container listens on port 3000
# (This doesn't actually open the port - it's documentation for humans and tools)
EXPOSE 3000

# The command to run when the container starts
CMD ["./backend"]
