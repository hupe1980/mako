# ── mako — multi-stage Docker build ──────────────────────────────────────────
#
# Uses cargo-chef for proper Rust dependency layer caching.
# Builds FOUR production binaries from the same builder layer.
#
# Build targets:
#   docker build --target runtime         -t makod:dev     .   # EDIFACT process engine
#   docker build --target marktd-runtime  -t marktd:dev    .   # Market Data Hub
#   docker build --target processd-runtime -t processd:dev .   # Process Decision Engine
#   docker build --target mdmd-runtime    -t mdmd:dev      .   # Master data manager
#
# Stages:
#   chef          cargo-chef + Rust toolchain + native build deps
#   planner       Analyse workspace, emit recipe.json
#   builder       Cook deps (cached layer) → compile makod + marktd + processd
#   runtime       gcr.io/distroless/cc-debian12:nonroot — makod (default target)
#
# Build targets:
#   docker build --target runtime      -t makod:dev .   # EDIFACT process engine
#
# Runtime library notes
# ─────────────────────
#  OpenSSL (asx-rs / AS4 WS-Security)
#    Built with OPENSSL_STATIC=1 — libssl + libcrypto compiled into the binary.
#    No libssl.so needed at runtime.
#
#  aws-lc-sys / ring / lz4-sys / zstd-sys
#    All statically compiled. No runtime .so needed.
#
#  glibc + libgcc
#    Included in distroless/cc. Required by Rust's linux-gnu target.
#
#  ca-certificates
#    Included in distroless/cc. Consumed by openssl-probe (asx-rs) and rustls.
#
#  tzdata (CET/CEST deadline arithmetic)
#    distroless/cc does not ship tzdata. Required zone files are copied from
#    the builder so time::OffsetDateTime can resolve TZ=Europe/Berlin.
#
# Build arguments (override with --build-arg)
#  RUST_VERSION     Rust toolchain channel (default: matches rust-toolchain.toml)
#  DEBIAN_CODENAME  Debian release for builder base (default: bookworm)
#  PROFILE          Cargo profile: release (default) or dev
#  OCI_VERSION      Image version label
#  OCI_REVISION     Git commit SHA (set at CI time)
#  OCI_CREATED      ISO-8601 build timestamp (set at CI time)
# ──────────────────────────────────────────────────────────────────────────────

# Global ARGs — available in FROM lines; must be re-declared inside a stage
# to be visible in that stage's RUN commands.
ARG RUST_VERSION=1.89
ARG DEBIAN_CODENAME=bookworm
ARG PROFILE=release

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 1 — chef  (cargo-chef + Rust toolchain + native build deps)
# ╚══════════════════════════════════════════════════════════════════════════════
# The lukemathwalker/cargo-chef pre-built image ships cargo-chef on top of the
# official rust image. The tagging scheme is:
#   latest-rust-<rust-version>-<debian-variant>
FROM lukemathwalker/cargo-chef:latest-rust-${RUST_VERSION}-${DEBIAN_CODENAME} AS chef

# Native build dependencies required by -sys crates:
#   pkg-config      openssl-sys build script
#   libssl-dev      openssl-sys static link (headers + libssl.a)
#   libclang-dev    aws-lc-sys bindgen
#   cmake           aws-lc-sys cmake build
#   nasm            aws-lc-sys x86 assembly (also available on arm64, unused there)
RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config \
        libssl-dev \
        libclang-dev \
        cmake \
        nasm \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Statically link OpenSSL so the runtime image needs no libssl.so.
# Disable incremental compilation — it is counter-productive in Docker because
# each layer starts from a clean state; it wastes disk I/O and build time.
ENV OPENSSL_STATIC=1 \
    CARGO_INCREMENTAL=0

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 2 — planner  (generate recipe.json from workspace manifests)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM chef AS planner

# cargo chef prepare reads every Cargo.toml and Cargo.lock in the workspace
# to build a dependency recipe. The full source tree is required only so that
# cargo can resolve the workspace graph; no compilation happens here.
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 3 — builder  (cook deps + compile binary)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM chef AS builder
ARG PROFILE=release

# ── Dependency layer (cached) ──────────────────────────────────────────────────
# cargo chef cook recreates minimal source stubs from recipe.json and compiles
# all dependencies. This Docker layer is cached as long as recipe.json is
# unchanged — i.e. until Cargo.lock or any Cargo.toml changes.
#
# --mount=type=cache for the cargo registry avoids re-downloading crates on
# each build without breaking Docker layer semantics (the compiled .rlib files
# live in the Docker layer, not the mount).
COPY --from=planner /build/recipe.json recipe.json
RUN --mount=type=cache,id=cargo-registry,sharing=locked,target=/usr/local/cargo/registry \
    cargo chef cook --profile ${PROFILE} -p makod -p marktd -p processd --recipe-path recipe.json

# ── Application layer (rebuilt on every source change) ────────────────────────
# COPY real source over the stubs created by cargo chef cook, then build.
# Cargo only recompiles crates whose source has actually changed.
COPY . .
RUN --mount=type=cache,id=cargo-registry,sharing=locked,target=/usr/local/cargo/registry \
    cargo build --profile ${PROFILE} -p makod -p marktd \
    && cargo build --profile ${PROFILE} -p processd --features integrated \
    && BIN_DIR="$([ "${PROFILE}" = "release" ] && echo target/release || echo target/debug)" \
    && cp "${BIN_DIR}/makod"    /usr/local/bin/makod    && strip /usr/local/bin/makod \
    && cp "${BIN_DIR}/marktd"   /usr/local/bin/marktd   && strip /usr/local/bin/marktd \
    && cp "${BIN_DIR}/processd" /usr/local/bin/processd && strip /usr/local/bin/processd \
    && install -d -o 65532 -g 65532 -m 0700 /var/lib/makod

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 4 — runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
# gcr.io/distroless/cc-debian12:nonroot contains:
#   • glibc, libgcc (required by Rust's x86_64/aarch64-unknown-linux-gnu target)
#   • ca-certificates (system trust store for TLS)
#   • No shell, no package manager, no coreutils
#   • UID/GID 65532 "nonroot" pre-configured
FROM gcr.io/distroless/cc-debian12:nonroot AS runtime

# tzdata: distroless/cc does not ship /usr/share/zoneinfo.
# Copy only the zone data needed for TZ=Europe/Berlin (CET/CEST).
# time::OffsetDateTime reads the zone file at the path pointed to by $TZ.
COPY --from=builder /usr/share/zoneinfo/Europe      /usr/share/zoneinfo/Europe
COPY --from=builder /usr/share/zoneinfo/UTC         /usr/share/zoneinfo/UTC
COPY --from=builder /usr/share/zoneinfo/leap-seconds.list \
                    /usr/share/zoneinfo/leap-seconds.list
# Set /etc/localtime so that localtime(3) resolves correctly for code that
# does not consult $TZ directly.
COPY --from=builder /usr/share/zoneinfo/Europe/Berlin /etc/localtime

ENV TZ=Europe/Berlin

# Binary — root:root 0755, runs as uid 65532 (distroless nonroot).
COPY --from=builder --chown=root:root /usr/local/bin/makod /usr/local/bin/makod

# Persistent state directory — pre-created with uid 65532 ownership so that
# SlateDB can write here even when no volume is mounted (e.g. --check mode).
# VOLUME is declared AFTER this COPY so Docker does not discard the ownership.
COPY --from=builder /var/lib/makod /var/lib/makod

# Persistent state directory.
# Mount a named volume or PVC here; ensure it is writable by uid 65532.
VOLUME ["/var/lib/makod"]

# Default environment — override via `docker run -e` or Kubernetes envFrom.
ENV MAKOD_LOG_FORMAT=json \
    MAKOD_LOG_LEVEL=info \
    MAKOD_DATA_DIR=/var/lib/makod \
    MAKOD_HTTP_ADDR=0.0.0.0:8080

# Exposed ports:
#   8080  HTTP REST API + Swagger UI + MCP server
#   4080  AS4 inbound transport  (--as4-addr)
#   8090  API-Webdienste Strom   (--api-webdienste-addr)
EXPOSE 8080 4080 8090

# Health-check for docker / docker-compose.
# --check validates config, profiles, and adapters then exits 0.
# In Kubernetes use a httpGet probe against GET /health instead (no shell needed).
HEALTHCHECK --interval=15s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/makod", "--check"]

ENTRYPOINT ["/usr/local/bin/makod"]

# OCI image labels.
# Set OCI_REVISION and OCI_CREATED at CI time, e.g.:
#   docker build \
#     --build-arg OCI_REVISION=$(git rev-parse HEAD) \
#     --build-arg OCI_CREATED=$(date -u +%Y-%m-%dT%H:%M:%SZ) …
ARG OCI_VERSION=0.7.0
ARG OCI_REVISION=unknown
ARG OCI_CREATED=unknown
LABEL org.opencontainers.image.title="makod" \
      org.opencontainers.image.description="Mako process engine daemon — German energy market communication (MaKo/BDEW EDI@Energy)" \
      org.opencontainers.image.version="${OCI_VERSION}" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.created="${OCI_CREATED}" \
      org.opencontainers.image.source="https://github.com/hupe1980/mako" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 5 — mdmd-runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 5 — marktd-runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM gcr.io/distroless/cc-debian12:nonroot AS marktd-runtime

COPY --from=builder /usr/share/zoneinfo/Europe      /usr/share/zoneinfo/Europe
COPY --from=builder /usr/share/zoneinfo/UTC         /usr/share/zoneinfo/UTC
COPY --from=builder /usr/share/zoneinfo/leap-seconds.list \
                    /usr/share/zoneinfo/leap-seconds.list
COPY --from=builder /usr/share/zoneinfo/Europe/Berlin /etc/localtime

ENV TZ=Europe/Berlin

COPY --from=builder --chown=root:root /usr/local/bin/marktd /usr/local/bin/marktd

EXPOSE 8180

ENV MARKTD_LOG_FORMAT=json \
    MARKTD_LOG_LEVEL=info

HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=5 \
    CMD ["/usr/local/bin/marktd", "--help"]

ENTRYPOINT ["/usr/local/bin/marktd"]

ARG OCI_VERSION=0.7.0
ARG OCI_REVISION=unknown
ARG OCI_CREATED=unknown
LABEL org.opencontainers.image.title="marktd" \
      org.opencontainers.image.description="Market Data Hub daemon — German energy market (MaKo/marktd)" \
      org.opencontainers.image.version="${OCI_VERSION}" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.created="${OCI_CREATED}" \
      org.opencontainers.image.source="https://github.com/hupe1980/mako" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 6 — processd-runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM gcr.io/distroless/cc-debian12:nonroot AS processd-runtime

COPY --from=builder /usr/share/zoneinfo/Europe      /usr/share/zoneinfo/Europe
COPY --from=builder /usr/share/zoneinfo/UTC         /usr/share/zoneinfo/UTC
COPY --from=builder /usr/share/zoneinfo/leap-seconds.list \
                    /usr/share/zoneinfo/leap-seconds.list
COPY --from=builder /usr/share/zoneinfo/Europe/Berlin /etc/localtime

ENV TZ=Europe/Berlin

COPY --from=builder --chown=root:root /usr/local/bin/processd /usr/local/bin/processd

EXPOSE 8580

ENV PROCESSD_LOG_FORMAT=json \
    PROCESSD_LOG_LEVEL=info

HEALTHCHECK --interval=10s --timeout=3s --start-period=30s --retries=5 \
    CMD ["/usr/local/bin/processd", "--help"]

ENTRYPOINT ["/usr/local/bin/processd"]

ARG OCI_VERSION=0.7.0
ARG OCI_REVISION=unknown
ARG OCI_CREATED=unknown
LABEL org.opencontainers.image.title="processd" \
      org.opencontainers.image.description="Process Decision Engine — NB STP auto-responder (MaKo)" \
      org.opencontainers.image.version="${OCI_VERSION}" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.created="${OCI_CREATED}" \
      org.opencontainers.image.source="https://github.com/hupe1980/mako" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"
