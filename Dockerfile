# Multi-stage build for mm (Witch server) + mm-web
#
# Build:  docker build -t mm .
# Run:    docker run -v /path/to/music:/music -v mm_data:/data -v mm_config:/config mm

# ── Builder ──────────────────────────────────────────────────────────────────
FROM rust:1-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    cmake pkg-config \
    && rm -rf /var/lib/apt/lists/*

# wasm-pack + target for web client build
RUN rustup target add wasm32-unknown-unknown \
    && cargo install wasm-pack

WORKDIR /src
COPY . .

# Build WASM client first (outputs to crates/mm-web/static/pkg/)
RUN wasm-pack build crates/mm-web/client --target web --out-dir ../static/pkg --release

# Build mm (witch server) and mm-web (http api)
RUN cargo build --release -p mm -p mm-web

# ── Runtime ──────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates tini \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /src/target/release/mm /usr/local/bin/mm
COPY --from=builder /src/target/release/mm-web /usr/local/bin/mm-web
COPY --from=builder /src/crates/mm-web/static /srv/mm-web/static

COPY docker/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

# XDG directories inside the container
ENV XDG_CONFIG_HOME=/config
ENV XDG_DATA_HOME=/data
ENV XDG_RUNTIME_DIR=/run
ENV MM_ROOT=/music
ENV MM_WEB_LISTEN=0.0.0.0:3313
ENV MM_WEB_STATIC_DIR=/srv/mm-web/static

VOLUME ["/music", "/config", "/data"]
EXPOSE 3313

ENTRYPOINT ["tini", "--"]
CMD ["/entrypoint.sh"]
