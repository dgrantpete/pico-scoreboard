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

# Set the working directory inside the container
WORKDIR /app

# =============================================================================
# Dependency Caching Optimization
# =============================================================================
# Docker caches each "layer" (each RUN/COPY command creates a layer).
# If files don't change, Docker reuses the cached layer.
#
# Strategy: Copy only Cargo.toml/Cargo.lock first, build dependencies,
# THEN copy source code. This way, if only your source code changes,
# Docker reuses the cached dependency compilation (saving ~5 minutes).

COPY Cargo.toml Cargo.lock ./

# Create a dummy main.rs to build dependencies
RUN mkdir src && \
    echo "fn main() {}" > src/main.rs && \
    cargo build --release && \
    rm -rf src

# Now copy the actual source code
COPY src/ src/

# Copy the config directory (contains default.toml with non-secret defaults)
COPY config/ config/

# Touch main.rs to update its timestamp so Cargo knows to recompile it
# (otherwise Cargo might think the dummy main.rs is still current)
RUN touch src/main.rs && \
    cargo build --release

# =============================================================================
# Stage 2: Runtime Stage
# =============================================================================
# Start fresh with a minimal image - we don't need Rust/gcc/cmake anymore!
# debian:bookworm-slim is ~80MB vs ~2GB for the build image.
FROM debian:bookworm-slim AS runtime

# Install runtime dependencies:
# - ca-certificates: Required for HTTPS connections (verifying ESPN's SSL cert)
# - libssl3: OpenSSL runtime library
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
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
COPY --from=builder /app/config /app/config

# Change ownership to our non-root user
RUN chown -R appuser:appuser /app

# Switch to non-root user
USER appuser

# Document that this container listens on port 3000
# (This doesn't actually open the port - it's documentation for humans and tools)
EXPOSE 3000

# The command to run when the container starts
CMD ["./backend"]
