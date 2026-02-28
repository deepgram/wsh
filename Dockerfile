# ── Build stage ──────────────────────────────────────────────────
FROM rust:1-bookworm AS builder

# Install Bun for the web frontend build
RUN curl -fsSL https://bun.sh/install | bash
ENV PATH="/root/.bun/bin:${PATH}"

WORKDIR /src

# 1) Cache web dependencies
COPY web/package.json web/bun.lock* web/
RUN cd web && bun install

# 2) Cache Rust dependencies (build a dummy first)
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
    && mkdir -p web-dist && touch web-dist/index.html \
    && WSH_SKIP_WEB_BUILD=1 cargo build --release 2>/dev/null || true \
    && rm -rf src web-dist

# 3) Full build with web frontend + Rust binary
COPY . .
RUN cargo build --release

# ── Runtime stage ────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
       bash \
       ca-certificates \
       curl \
       git \
       procps \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /src/target/release/wsh /usr/local/bin/wsh

# Default terminal type
ENV TERM=xterm-256color
ENV SHELL=/bin/bash

EXPOSE 8080

ENTRYPOINT ["wsh", "server"]
CMD ["--bind", "0.0.0.0:8080", "--no-auth"]
