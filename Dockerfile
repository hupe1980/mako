# ── mako — multi-stage Docker build ──────────────────────────────────────────
#
# Uses cargo-chef for proper Rust dependency layer caching.
# Builds SIX production binaries from the same builder layer.
#
# Build targets:
#   docker build --target runtime          -t makod:dev     .   # EDIFACT process engine
#   docker build --target marktd-runtime   -t marktd:dev    .   # Market Data Hub
#   docker build --target processd-runtime -t processd:dev  .   # Process Decision Engine
#   docker build --target invoicd-runtime  -t invoicd:dev   .   # INVOIC plausibility daemon
#   docker build --target edmd-runtime     -t edmd:dev      .   # Energy Data Management daemon
#   docker build --target obsd-runtime     -t obsd:dev      .   # Observability daemon
#
# Stages:
#   chef          cargo-chef + Rust toolchain + native build deps
#   planner       Analyse workspace, emit recipe.json
#   builder       Cook deps (cached layer) → compile all six binaries
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
ARG RUST_VERSION=1.94
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
#   protobuf-compiler  lance-encoding (LanceDB / agentd) — protoc binary
#   libprotobuf-dev    lance-encoding — google/protobuf/*.proto well-known types in /usr/include
RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config \
        libssl-dev \
        libclang-dev \
        cmake \
        nasm \
        protobuf-compiler \
        libprotobuf-dev \
        mold \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Statically link OpenSSL so the runtime image needs no libssl.so.
# mold: use via mold -run in cargo build steps (no clang wrapper needed).
# CARGO_INCREMENTAL=0 globally; application layers override to 1 with target/ cache.
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
# ║ Stage 3a — demo-builder  (8 demo services — NO LanceDB/agentd/einsd/tarifbd)
# ╚══════════════════════════════════════════════════════════════════════════════
# Compiles only the 7 services used by demo/docker-compose.yml.
# edmd is excluded: its iceberg + datafusion deps add ~12 min to the cold build.
# edmd can be built separately: docker build --target edmd-runtime -t edmd:dev .
# Skips agentd (LanceDB ~800 extra crates), einsd (iceberg), and 7 LF services.
# Expected build time: ~6-8 min cold (3 services, no iceberg/LanceDB).
# Demo runtime targets (makod, marktd, processd, invoicd, obsd,
# netzbilanzd, nis-syncd) all use --from=demo-builder.
FROM chef AS demo-builder
ARG PROFILE=release

COPY --from=planner /build/recipe.json recipe.json
RUN --mount=type=cache,id=cargo-registry-demo,sharing=locked,target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-target-demo,sharing=locked,target=/build/target \
    cargo chef cook --profile ${PROFILE} \
                    -p makod -p marktd -p processd \
                    --recipe-path recipe.json

COPY . .
RUN --mount=type=cache,id=cargo-registry-demo,sharing=locked,target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-target-demo,sharing=locked,target=/build/target \
    CARGO_INCREMENTAL=1 mold -run cargo build --profile ${PROFILE} \
                -p makod -p marktd \
    && CARGO_INCREMENTAL=1 mold -run cargo build --profile ${PROFILE} -p processd --features integrated \
    && BIN_DIR="$([ "${PROFILE}" = "release" ] && echo target/release || echo target/debug)" \
    && cp "${BIN_DIR}/makod"       /usr/local/bin/makod       && strip /usr/local/bin/makod \
    && cp "${BIN_DIR}/marktd"      /usr/local/bin/marktd      && strip /usr/local/bin/marktd \
    && cp "${BIN_DIR}/processd"    /usr/local/bin/processd    && strip /usr/local/bin/processd \
    && install -d -o 65532 -g 65532 -m 0700 /var/lib/makod

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 3b — builder  (all 17 services — production / CI release pipeline)
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
    --mount=type=cache,id=cargo-target-full,sharing=locked,target=/build/target \
    cargo chef cook --profile ${PROFILE} \
                    -p makod -p marktd -p processd -p invoicd -p edmd -p obsd \
                    -p netzbilanzd -p sperrd -p nis-syncd -p einsd \
                    -p tarifbd -p billingd -p accountingd -p vertragd \
                    -p portald -p agentd -p mabis-syncd \
                    --recipe-path recipe.json

# ── Application layer (rebuilt on every source change) ────────────────────────
COPY . .
RUN --mount=type=cache,id=cargo-registry,sharing=locked,target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-target-full,sharing=locked,target=/build/target \
    CARGO_INCREMENTAL=1 mold -run cargo build --profile ${PROFILE} -p makod -p marktd -p invoicd -p edmd -p obsd \
                                     -p netzbilanzd -p sperrd -p nis-syncd -p einsd \
                                     -p tarifbd -p billingd -p accountingd \
                                     -p vertragd -p portald -p agentd -p mabis-syncd \
    && CARGO_INCREMENTAL=1 mold -run cargo build --profile ${PROFILE} -p processd --features integrated \
    && BIN_DIR="$([ "${PROFILE}" = "release" ] && echo target/release || echo target/debug)" \
    && cp "${BIN_DIR}/makod"       /usr/local/bin/makod       && strip /usr/local/bin/makod \
    && cp "${BIN_DIR}/marktd"      /usr/local/bin/marktd      && strip /usr/local/bin/marktd \
    && cp "${BIN_DIR}/processd"    /usr/local/bin/processd    && strip /usr/local/bin/processd \
    && cp "${BIN_DIR}/invoicd"     /usr/local/bin/invoicd     && strip /usr/local/bin/invoicd \
    && cp "${BIN_DIR}/edmd"        /usr/local/bin/edmd        && strip /usr/local/bin/edmd \
    && cp "${BIN_DIR}/obsd"        /usr/local/bin/obsd        && strip /usr/local/bin/obsd \
    && cp "${BIN_DIR}/netzbilanzd" /usr/local/bin/netzbilanzd && strip /usr/local/bin/netzbilanzd \
    && cp "${BIN_DIR}/sperrd"      /usr/local/bin/sperrd      && strip /usr/local/bin/sperrd \
    && cp "${BIN_DIR}/einsd"       /usr/local/bin/einsd       && strip /usr/local/bin/einsd \
    && cp "${BIN_DIR}/tarifbd"     /usr/local/bin/tarifbd     && strip /usr/local/bin/tarifbd \
    && cp "${BIN_DIR}/billingd"    /usr/local/bin/billingd    && strip /usr/local/bin/billingd \
    && cp "${BIN_DIR}/accountingd" /usr/local/bin/accountingd && strip /usr/local/bin/accountingd \
    && cp "${BIN_DIR}/vertragd"    /usr/local/bin/vertragd    && strip /usr/local/bin/vertragd \
    && cp "${BIN_DIR}/portald"     /usr/local/bin/portald     && strip /usr/local/bin/portald \
    && cp "${BIN_DIR}/agentd"      /usr/local/bin/agentd      && strip /usr/local/bin/agentd \
    && cp "${BIN_DIR}/mabis-syncd" /usr/local/bin/mabis-syncd && strip /usr/local/bin/mabis-syncd \
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
COPY --from=demo-builder /usr/share/zoneinfo/Europe      /usr/share/zoneinfo/Europe
COPY --from=demo-builder /usr/share/zoneinfo/UTC         /usr/share/zoneinfo/UTC
COPY --from=demo-builder /usr/share/zoneinfo/leap-seconds.list \
                    /usr/share/zoneinfo/leap-seconds.list
# Set /etc/localtime so that localtime(3) resolves correctly for code that
# does not consult $TZ directly.
COPY --from=demo-builder /usr/share/zoneinfo/Europe/Berlin /etc/localtime

ENV TZ=Europe/Berlin

# Binary — root:root 0755, runs as uid 65532 (distroless nonroot).
COPY --from=demo-builder --chown=root:root /usr/local/bin/makod /usr/local/bin/makod

# Persistent state directory — pre-created with uid 65532 ownership so that
# SlateDB can write here even when no volume is mounted (e.g. --check mode).
# VOLUME is declared AFTER this COPY so Docker does not discard the ownership.
COPY --from=demo-builder /var/lib/makod /var/lib/makod

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
ARG OCI_VERSION=0.8.0
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

COPY --from=demo-builder /usr/share/zoneinfo/Europe      /usr/share/zoneinfo/Europe
COPY --from=demo-builder /usr/share/zoneinfo/UTC         /usr/share/zoneinfo/UTC
COPY --from=demo-builder /usr/share/zoneinfo/leap-seconds.list \
                    /usr/share/zoneinfo/leap-seconds.list
COPY --from=demo-builder /usr/share/zoneinfo/Europe/Berlin /etc/localtime

ENV TZ=Europe/Berlin

COPY --from=demo-builder --chown=root:root /usr/local/bin/marktd /usr/local/bin/marktd

EXPOSE 8180

ENV MARKTD_LOG_FORMAT=json \
    MARKTD_LOG_LEVEL=info

HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=5 \
    CMD ["/usr/local/bin/marktd", "--check"]

ENTRYPOINT ["/usr/local/bin/marktd"]

ARG OCI_VERSION=0.8.0
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

COPY --from=demo-builder /usr/share/zoneinfo/Europe      /usr/share/zoneinfo/Europe
COPY --from=demo-builder /usr/share/zoneinfo/UTC         /usr/share/zoneinfo/UTC
COPY --from=demo-builder /usr/share/zoneinfo/leap-seconds.list \
                    /usr/share/zoneinfo/leap-seconds.list
COPY --from=demo-builder /usr/share/zoneinfo/Europe/Berlin /etc/localtime

ENV TZ=Europe/Berlin

COPY --from=demo-builder --chown=root:root /usr/local/bin/processd /usr/local/bin/processd

EXPOSE 8580

ENV PROCESSD_LOG_FORMAT=json \
    PROCESSD_LOG_LEVEL=info

HEALTHCHECK --interval=10s --timeout=3s --start-period=30s --retries=5 \
    CMD ["/usr/local/bin/processd", "--check"]

ENTRYPOINT ["/usr/local/bin/processd"]

ARG OCI_VERSION=0.8.0
ARG OCI_REVISION=unknown
ARG OCI_CREATED=unknown
LABEL org.opencontainers.image.title="processd" \
      org.opencontainers.image.description="Process Decision Engine — NB STP auto-responder (MaKo)" \
      org.opencontainers.image.version="${OCI_VERSION}" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.created="${OCI_CREATED}" \
      org.opencontainers.image.source="https://github.com/hupe1980/mako" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 7 — invoicd-runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM gcr.io/distroless/cc-debian12:nonroot AS invoicd-runtime

COPY --from=builder /usr/share/zoneinfo/Europe      /usr/share/zoneinfo/Europe
COPY --from=builder /usr/share/zoneinfo/UTC         /usr/share/zoneinfo/UTC
COPY --from=builder /usr/share/zoneinfo/leap-seconds.list \
                    /usr/share/zoneinfo/leap-seconds.list
COPY --from=builder /usr/share/zoneinfo/Europe/Berlin /etc/localtime

ENV TZ=Europe/Berlin

COPY --from=builder --chown=root:root /usr/local/bin/invoicd /usr/local/bin/invoicd

EXPOSE 8280

ENV INVOICD_LOG_FORMAT=json \
    INVOICD_LOG_LEVEL=info

HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=5 \
    CMD ["/usr/local/bin/invoicd", "--check"]

ENTRYPOINT ["/usr/local/bin/invoicd"]

ARG OCI_VERSION=0.8.0
ARG OCI_REVISION=unknown
ARG OCI_CREATED=unknown
LABEL org.opencontainers.image.title="invoicd" \
      org.opencontainers.image.description="INVOIC plausibility-check daemon — LF role, §22 MessZV (MaKo)" \
      org.opencontainers.image.version="${OCI_VERSION}" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.created="${OCI_CREATED}" \
      org.opencontainers.image.source="https://github.com/hupe1980/mako" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 8 — edmd-runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM gcr.io/distroless/cc-debian12:nonroot AS edmd-runtime

COPY --from=builder /usr/share/zoneinfo/Europe      /usr/share/zoneinfo/Europe
COPY --from=builder /usr/share/zoneinfo/UTC         /usr/share/zoneinfo/UTC
COPY --from=builder /usr/share/zoneinfo/leap-seconds.list \
                    /usr/share/zoneinfo/leap-seconds.list
COPY --from=builder /usr/share/zoneinfo/Europe/Berlin /etc/localtime

ENV TZ=Europe/Berlin

COPY --from=builder --chown=root:root /usr/local/bin/edmd /usr/local/bin/edmd

EXPOSE 8380

ENV EDMD_LOG_FORMAT=json \
    EDMD_LOG_LEVEL=info

HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=5 \
    CMD ["/usr/local/bin/edmd", "--check"]

ENTRYPOINT ["/usr/local/bin/edmd"]

ARG OCI_VERSION=0.8.0
ARG OCI_REVISION=unknown
ARG OCI_CREATED=unknown
LABEL org.opencontainers.image.title="edmd" \
      org.opencontainers.image.description="Energy Data Management daemon — MSCONS meter readings, MeterBillingPeriod (MaKo)" \
      org.opencontainers.image.version="${OCI_VERSION}" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.created="${OCI_CREATED}" \
      org.opencontainers.image.source="https://github.com/hupe1980/mako" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 9 — obsd-runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM gcr.io/distroless/cc-debian12:nonroot AS obsd-runtime

COPY --from=builder /usr/share/zoneinfo/Europe      /usr/share/zoneinfo/Europe
COPY --from=builder /usr/share/zoneinfo/UTC         /usr/share/zoneinfo/UTC
COPY --from=builder /usr/share/zoneinfo/leap-seconds.list \
                    /usr/share/zoneinfo/leap-seconds.list
COPY --from=builder /usr/share/zoneinfo/Europe/Berlin /etc/localtime

ENV TZ=Europe/Berlin

COPY --from=builder --chown=root:root /usr/local/bin/obsd /usr/local/bin/obsd

EXPOSE 8480

ENV OBSD_LOG_FORMAT=json \
    OBSD_LOG_LEVEL=info

HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=5 \
    CMD ["/usr/local/bin/obsd", "--check"]

ENTRYPOINT ["/usr/local/bin/obsd"]

ARG OCI_VERSION=0.8.0
ARG OCI_REVISION=unknown
ARG OCI_CREATED=unknown
LABEL org.opencontainers.image.title="obsd" \
      org.opencontainers.image.description="Business-process observability daemon — KPI reports, §20 EnWG parity (MaKo)" \
      org.opencontainers.image.version="${OCI_VERSION}" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.created="${OCI_CREATED}" \
      org.opencontainers.image.source="https://github.com/hupe1980/mako" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 10 — netzbilanzd-runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM gcr.io/distroless/cc-debian12:nonroot AS netzbilanzd-runtime

COPY --from=builder /usr/share/zoneinfo/Europe      /usr/share/zoneinfo/Europe
COPY --from=builder /usr/share/zoneinfo/UTC         /usr/share/zoneinfo/UTC
COPY --from=builder /usr/share/zoneinfo/leap-seconds.list \
                    /usr/share/zoneinfo/leap-seconds.list
COPY --from=builder /usr/share/zoneinfo/Europe/Berlin /etc/localtime
ENV TZ=Europe/Berlin
COPY --from=builder --chown=root:root /usr/local/bin/netzbilanzd /usr/local/bin/netzbilanzd
EXPOSE 8680
ENV NETZBILANZD_LOG_FORMAT=json \
    NETZBILANZD_LOG_LEVEL=info
HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=5 \
    CMD ["/usr/local/bin/netzbilanzd", "--check"]
ENTRYPOINT ["/usr/local/bin/netzbilanzd"]
ARG OCI_VERSION=0.11.0
ARG OCI_REVISION=unknown
ARG OCI_CREATED=unknown
LABEL org.opencontainers.image.title="netzbilanzd" \
      org.opencontainers.image.description="NNE/KA/MMM/MSB/AWH billing daemon — NB role, GridSettlement, CalculationTrace (MaKo)" \
      org.opencontainers.image.version="${OCI_VERSION}" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.created="${OCI_CREATED}" \
      org.opencontainers.image.source="https://github.com/hupe1980/mako" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 11 — sperrd-runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM gcr.io/distroless/cc-debian12:nonroot AS sperrd-runtime

COPY --from=builder /usr/share/zoneinfo/Europe      /usr/share/zoneinfo/Europe
COPY --from=builder /usr/share/zoneinfo/UTC         /usr/share/zoneinfo/UTC
COPY --from=builder /usr/share/zoneinfo/leap-seconds.list \
                    /usr/share/zoneinfo/leap-seconds.list
COPY --from=builder /usr/share/zoneinfo/Europe/Berlin /etc/localtime
ENV TZ=Europe/Berlin
COPY --from=builder --chown=root:root /usr/local/bin/sperrd /usr/local/bin/sperrd
EXPOSE 8780
ENV SPERRD_LOG_FORMAT=json \
    SPERRD_LOG_LEVEL=info
HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=5 \
    CMD ["/usr/local/bin/sperrd", "--check"]
ENTRYPOINT ["/usr/local/bin/sperrd"]
ARG OCI_VERSION=0.11.0
ARG OCI_REVISION=unknown
ARG OCI_CREATED=unknown
LABEL org.opencontainers.image.title="sperrd" \
      org.opencontainers.image.description="Sperrung execution tracking daemon — NB role, IFTSTA 21039 auto-dispatch (MaKo)" \
      org.opencontainers.image.version="${OCI_VERSION}" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.created="${OCI_CREATED}" \
      org.opencontainers.image.source="https://github.com/hupe1980/mako" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 12 — nis-syncd-runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM gcr.io/distroless/cc-debian12:nonroot AS nis-syncd-runtime

COPY --from=builder /usr/share/zoneinfo/Europe      /usr/share/zoneinfo/Europe
COPY --from=builder /usr/share/zoneinfo/UTC         /usr/share/zoneinfo/UTC
COPY --from=builder /usr/share/zoneinfo/leap-seconds.list \
                    /usr/share/zoneinfo/leap-seconds.list
COPY --from=builder /usr/share/zoneinfo/Europe/Berlin /etc/localtime
ENV TZ=Europe/Berlin
COPY --from=builder --chown=root:root /usr/local/bin/nis-syncd /usr/local/bin/nis-syncd
EXPOSE 9680
ENV NIS_SYNCD_LOG_FORMAT=json \
    NIS_SYNCD_LOG_LEVEL=info
HEALTHCHECK --interval=10s --timeout=3s --start-period=15s --retries=5 \
    CMD ["/usr/local/bin/nis-syncd", "--check"]
ENTRYPOINT ["/usr/local/bin/nis-syncd"]
ARG OCI_VERSION=0.11.0
ARG OCI_REVISION=unknown
ARG OCI_CREATED=unknown
LABEL org.opencontainers.image.title="nis-syncd" \
      org.opencontainers.image.description="NIS/GIS grid topology import adapter — NB role, stateless, lifts Anmeldung STP to ≥95% (MaKo)" \
      org.opencontainers.image.version="${OCI_VERSION}" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.created="${OCI_CREATED}" \
      org.opencontainers.image.source="https://github.com/hupe1980/mako" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 13 — einsd-runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM gcr.io/distroless/cc-debian12:nonroot AS einsd-runtime

COPY --from=builder /usr/share/zoneinfo/Europe      /usr/share/zoneinfo/Europe
COPY --from=builder /usr/share/zoneinfo/UTC         /usr/share/zoneinfo/UTC
COPY --from=builder /usr/share/zoneinfo/leap-seconds.list \
                    /usr/share/zoneinfo/leap-seconds.list
COPY --from=builder /usr/share/zoneinfo/Europe/Berlin /etc/localtime
ENV TZ=Europe/Berlin
COPY --from=builder --chown=root:root /usr/local/bin/einsd /usr/local/bin/einsd
EXPOSE 9180
ENV EINSD_LOG_FORMAT=json \
    EINSD_LOG_LEVEL=info
HEALTHCHECK --interval=10s --timeout=3s --start-period=30s --retries=5 \
    CMD ["/usr/local/bin/einsd", "--check"]
ENTRYPOINT ["/usr/local/bin/einsd"]
ARG OCI_VERSION=0.11.0
ARG OCI_REVISION=unknown
ARG OCI_CREATED=unknown
LABEL org.opencontainers.image.title="einsd" \
      org.opencontainers.image.description="Einspeiser Registry + EEG/KWKG Settlement daemon — 9 settlement schemes, 324 tests (MaKo)" \
      org.opencontainers.image.version="${OCI_VERSION}" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.created="${OCI_CREATED}" \
      org.opencontainers.image.source="https://github.com/hupe1980/mako" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 14 — tarifbd-runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM gcr.io/distroless/cc-debian12:nonroot AS tarifbd-runtime

COPY --from=builder /usr/share/zoneinfo/Europe      /usr/share/zoneinfo/Europe
COPY --from=builder /usr/share/zoneinfo/UTC         /usr/share/zoneinfo/UTC
COPY --from=builder /usr/share/zoneinfo/leap-seconds.list \
                    /usr/share/zoneinfo/leap-seconds.list
COPY --from=builder /usr/share/zoneinfo/Europe/Berlin /etc/localtime
ENV TZ=Europe/Berlin
COPY --from=builder --chown=root:root /usr/local/bin/tarifbd /usr/local/bin/tarifbd
EXPOSE 9080
ENV TARIFBD_LOG_FORMAT=json \
    TARIFBD_LOG_LEVEL=info
HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=5 \
    CMD ["/usr/local/bin/tarifbd", "--check"]
ENTRYPOINT ["/usr/local/bin/tarifbd"]
ARG OCI_VERSION=0.11.0
ARG OCI_REVISION=unknown
ARG OCI_CREATED=unknown
LABEL org.opencontainers.image.title="tarifbd" \
      org.opencontainers.image.description="Product & Tariff Catalog daemon — LF role, EPEX §41a, B2B Angebote (MaKo)" \
      org.opencontainers.image.version="${OCI_VERSION}" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.created="${OCI_CREATED}" \
      org.opencontainers.image.source="https://github.com/hupe1980/mako" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 15 — billingd-runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM gcr.io/distroless/cc-debian12:nonroot AS billingd-runtime

COPY --from=builder /usr/share/zoneinfo/Europe      /usr/share/zoneinfo/Europe
COPY --from=builder /usr/share/zoneinfo/UTC         /usr/share/zoneinfo/UTC
COPY --from=builder /usr/share/zoneinfo/leap-seconds.list \
                    /usr/share/zoneinfo/leap-seconds.list
COPY --from=builder /usr/share/zoneinfo/Europe/Berlin /etc/localtime
ENV TZ=Europe/Berlin
COPY --from=builder --chown=root:root /usr/local/bin/billingd /usr/local/bin/billingd
EXPOSE 9280
ENV BILLINGD_LOG_FORMAT=json \
    BILLINGD_LOG_LEVEL=info
HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=5 \
    CMD ["/usr/local/bin/billingd", "--check"]
ENTRYPOINT ["/usr/local/bin/billingd"]
ARG OCI_VERSION=0.11.0
ARG OCI_REVISION=unknown
ARG OCI_CREATED=unknown
LABEL org.opencontainers.image.title="billingd" \
      org.opencontainers.image.description="Energy Billing Engine daemon — LF role, 12 categories, XRechnung 3.0, §14a, §41a (MaKo)" \
      org.opencontainers.image.version="${OCI_VERSION}" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.created="${OCI_CREATED}" \
      org.opencontainers.image.source="https://github.com/hupe1980/mako" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 16 — accountingd-runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM gcr.io/distroless/cc-debian12:nonroot AS accountingd-runtime

COPY --from=builder /usr/share/zoneinfo/Europe      /usr/share/zoneinfo/Europe
COPY --from=builder /usr/share/zoneinfo/UTC         /usr/share/zoneinfo/UTC
COPY --from=builder /usr/share/zoneinfo/leap-seconds.list \
                    /usr/share/zoneinfo/leap-seconds.list
COPY --from=builder /usr/share/zoneinfo/Europe/Berlin /etc/localtime
ENV TZ=Europe/Berlin
COPY --from=builder --chown=root:root /usr/local/bin/accountingd /usr/local/bin/accountingd
EXPOSE 9380
ENV ACCOUNTINGD_LOG_FORMAT=json \
    ACCOUNTINGD_LOG_LEVEL=info
HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=5 \
    CMD ["/usr/local/bin/accountingd", "--check"]
ENTRYPOINT ["/usr/local/bin/accountingd"]
ARG OCI_VERSION=0.11.0
ARG OCI_REVISION=unknown
ARG OCI_CREATED=unknown
LABEL org.opencontainers.image.title="accountingd" \
      org.opencontainers.image.description="Customer Account Ledger daemon — LF role, SEPA pain.008/001, auto-dunning, GDPR Art.17 (MaKo)" \
      org.opencontainers.image.version="${OCI_VERSION}" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.created="${OCI_CREATED}" \
      org.opencontainers.image.source="https://github.com/hupe1980/mako" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 17 — vertragd-runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM gcr.io/distroless/cc-debian12:nonroot AS vertragd-runtime

COPY --from=builder /usr/share/zoneinfo/Europe      /usr/share/zoneinfo/Europe
COPY --from=builder /usr/share/zoneinfo/UTC         /usr/share/zoneinfo/UTC
COPY --from=builder /usr/share/zoneinfo/leap-seconds.list \
                    /usr/share/zoneinfo/leap-seconds.list
COPY --from=builder /usr/share/zoneinfo/Europe/Berlin /etc/localtime
ENV TZ=Europe/Berlin
COPY --from=builder --chown=root:root /usr/local/bin/vertragd /usr/local/bin/vertragd
EXPOSE 9780
ENV VERTRAGD_LOG_FORMAT=json \
    VERTRAGD_LOG_LEVEL=info
HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=5 \
    CMD ["/usr/local/bin/vertragd", "--check"]
ENTRYPOINT ["/usr/local/bin/vertragd"]
ARG OCI_VERSION=0.11.0
ARG OCI_REVISION=unknown
ARG OCI_CREATED=unknown
LABEL org.opencontainers.image.title="vertragd" \
      org.opencontainers.image.description="Contract & Customer Management daemon — LF role, B2C+B2B, OIDC→MaLo auth gateway (MaKo)" \
      org.opencontainers.image.version="${OCI_VERSION}" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.created="${OCI_CREATED}" \
      org.opencontainers.image.source="https://github.com/hupe1980/mako" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 18 — portald-runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM gcr.io/distroless/cc-debian12:nonroot AS portald-runtime

COPY --from=builder /usr/share/zoneinfo/Europe      /usr/share/zoneinfo/Europe
COPY --from=builder /usr/share/zoneinfo/UTC         /usr/share/zoneinfo/UTC
COPY --from=builder /usr/share/zoneinfo/leap-seconds.list \
                    /usr/share/zoneinfo/leap-seconds.list
COPY --from=builder /usr/share/zoneinfo/Europe/Berlin /etc/localtime
ENV TZ=Europe/Berlin
COPY --from=builder --chown=root:root /usr/local/bin/portald /usr/local/bin/portald
EXPOSE 9480
ENV PORTALD_LOG_FORMAT=json \
    PORTALD_LOG_LEVEL=info
HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=5 \
    CMD ["/usr/local/bin/portald", "--check"]
ENTRYPOINT ["/usr/local/bin/portald"]
ARG OCI_VERSION=0.11.0
ARG OCI_REVISION=unknown
ARG OCI_CREATED=unknown
LABEL org.opencontainers.image.title="portald" \
      org.opencontainers.image.description="Customer Portal gateway — LF role, REST+SSE, §41 EnWG self-service (MaKo)" \
      org.opencontainers.image.version="${OCI_VERSION}" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.created="${OCI_CREATED}" \
      org.opencontainers.image.source="https://github.com/hupe1980/mako" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 19 — agentd-runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM gcr.io/distroless/cc-debian12:nonroot AS agentd-runtime

COPY --from=builder /usr/share/zoneinfo/Europe      /usr/share/zoneinfo/Europe
COPY --from=builder /usr/share/zoneinfo/UTC         /usr/share/zoneinfo/UTC
COPY --from=builder /usr/share/zoneinfo/leap-seconds.list \
                    /usr/share/zoneinfo/leap-seconds.list
COPY --from=builder /usr/share/zoneinfo/Europe/Berlin /etc/localtime
ENV TZ=Europe/Berlin
COPY --from=builder --chown=root:root /usr/local/bin/agentd /usr/local/bin/agentd
EXPOSE 9580
ENV AGENTD_LOG_FORMAT=json \
    AGENTD_LOG_LEVEL=info
HEALTHCHECK --interval=10s --timeout=3s --start-period=30s --retries=5 \
    CMD ["/usr/local/bin/agentd", "--check"]
ENTRYPOINT ["/usr/local/bin/agentd"]
ARG OCI_VERSION=0.11.0
ARG OCI_REVISION=unknown
ARG OCI_CREATED=unknown
LABEL org.opencontainers.image.title="agentd" \
      org.opencontainers.image.description="Multi-agent LLM orchestration daemon — 24 specialists, LanceDB RAG, MCP tools (MaKo)" \
      org.opencontainers.image.version="${OCI_VERSION}" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.created="${OCI_CREATED}" \
      org.opencontainers.image.source="https://github.com/hupe1980/mako" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 20 — mabis-syncd-runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM gcr.io/distroless/cc-debian12:nonroot AS mabis-syncd-runtime

COPY --from=builder /usr/share/zoneinfo/Europe      /usr/share/zoneinfo/Europe
COPY --from=builder /usr/share/zoneinfo/UTC         /usr/share/zoneinfo/UTC
COPY --from=builder /usr/share/zoneinfo/leap-seconds.list \
                    /usr/share/zoneinfo/leap-seconds.list
COPY --from=builder /usr/share/zoneinfo/Europe/Berlin /etc/localtime
ENV TZ=Europe/Berlin
COPY --from=builder --chown=root:root /usr/local/bin/mabis-syncd /usr/local/bin/mabis-syncd
EXPOSE 8880
ENV MABIS_SYNCD_LOG_FORMAT=json \
    MABIS_SYNCD_LOG_LEVEL=info
HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=5 \
    CMD ["/usr/local/bin/mabis-syncd", "--check"]
ENTRYPOINT ["/usr/local/bin/mabis-syncd"]
ARG OCI_VERSION=0.11.0
ARG OCI_REVISION=unknown
ARG OCI_CREATED=unknown
LABEL org.opencontainers.image.title="mabis-syncd" \
      org.opencontainers.image.description="MaBiS UTILTS synchronisation daemon — ÜNB/NB role, Summenzeitreihe day-3/day-8 schedule (MaKo)" \
      org.opencontainers.image.version="${OCI_VERSION}" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.created="${OCI_CREATED}" \
      org.opencontainers.image.source="https://github.com/hupe1980/mako" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"
