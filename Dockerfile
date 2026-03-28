# Multi-stage build for mm (Witch server) + mm-web + mm-tui
#
# Build:  docker build -t mm .
# Run:    docker run -v /path/to/music:/music -v mm_data:/data -v mm_config:/config mm
# TUI:    docker exec -it <container> mm-tui

# ── Builder (Rust) ──────────────────────────────────────────────────────────
FROM rust:1-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    cmake pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src

# ── Dependency cache layer ──────────────────────────────────────────────────
# Copy only manifests + lockfile, create dummy source files, and build deps.
# This layer is cached until Cargo.toml/Cargo.lock change.
COPY Cargo.toml Cargo.lock ./
COPY crates/mm-utils/Cargo.toml    crates/mm-utils/Cargo.toml
COPY crates/mm-derive/Cargo.toml   crates/mm-derive/Cargo.toml
COPY crates/mm-meta/Cargo.toml     crates/mm-meta/Cargo.toml
COPY crates/mm-ui/Cargo.toml       crates/mm-ui/Cargo.toml
COPY crates/mm-tui/Cargo.toml      crates/mm-tui/Cargo.toml
COPY crates/mm-web/Cargo.toml      crates/mm-web/Cargo.toml

# Dummy source files so cargo can resolve the workspace and compile deps
RUN mkdir -p src && echo 'fn main() {}' > src/main.rs \
    && mkdir -p crates/mm-utils/src   && echo '' > crates/mm-utils/src/lib.rs \
    && mkdir -p crates/mm-derive/src  && echo '' > crates/mm-derive/src/lib.rs \
    && mkdir -p crates/mm-meta/src    && echo '' > crates/mm-meta/src/lib.rs \
    && mkdir -p crates/mm-ui/src      && echo '' > crates/mm-ui/src/lib.rs \
    && mkdir -p crates/mm-tui/src     && echo '' > crates/mm-tui/src/lib.rs \
    && echo 'fn main() {}' > crates/mm-tui/src/main.rs \
    && mkdir -p crates/mm-web/src     && echo '' > crates/mm-web/src/lib.rs \
    && echo 'fn main() {}' > crates/mm-web/src/main.rs

# Build deps
RUN cargo build --release -p mm -p mm-web -p mm-tui 2>/dev/null || true

# Remove dummy source and workspace crate fingerprints (but keep compiled
# deps in target/). Fingerprints must go because COPY preserves host mtimes
# which predate the dep-cache artifacts — cargo would skip recompilation.
RUN rm -rf src crates \
    && rm -rf target/release/.fingerprint/mm-* \
    && rm -rf target/release/.fingerprint/mm_*

# ── Frontend build ──────────────────────────────────────────────────────────
FROM node:22-bookworm-slim AS frontend

WORKDIR /src/crates/mm-web/frontend
COPY crates/mm-web/frontend/package.json crates/mm-web/frontend/package-lock.json* ./
RUN npm ci
COPY crates/mm-web/frontend/ .
RUN npm run build

# ── Rust build ──────────────────────────────────────────────────────────────
FROM builder AS rust-build

COPY . .
RUN cargo build --release -p mm -p mm-web -p mm-tui

# ── Runtime ─────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    bash ca-certificates tini sqlite3 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=rust-build /src/target/release/mm /usr/local/bin/mm
COPY --from=rust-build /src/target/release/mm-tui /usr/local/bin/mm-tui
COPY --from=rust-build /src/target/release/mm-web /usr/local/bin/mm-web
COPY --from=frontend /src/crates/mm-web/frontend/dist /srv/mm-web/static

COPY docker/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

# XDG directories inside the container
ENV XDG_CONFIG_HOME=/config
ENV XDG_DATA_HOME=/data
ENV XDG_RUNTIME_DIR=/tmp
ENV MM_ROOT=/music
ENV MM_WEB_LISTEN=0.0.0.0:3313
ENV MM_WEB_STATIC_DIR=/srv/mm-web/static

RUN mkdir -p /config/mm /data/mm && chown -R 1000:1000 /config /data

VOLUME ["/music", "/config", "/data"]
EXPOSE 3313

ENTRYPOINT ["tini", "--"]
CMD ["/entrypoint.sh"]
