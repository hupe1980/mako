# ── mako — multi-stage Docker build ──────────────────────────────────────────
#
# Uses cargo-chef for proper Rust dependency layer caching. One runtime stage
# per service binary, `runtime` (makod) being the default target.
#
# Stages:
#   chef          cargo-chef + Rust toolchain + native build deps
#   planner       Analyse workspace, emit recipe.json
#   demo-builder  makod, marktd, processd — what `just build-demo` builds
#   builder       Every other service
#   *-runtime     gcr.io/distroless/cc-debian12:nonroot, one per binary
#
# Build one image:
#   docker build --target runtime             -t makod:dev       .
#   docker build --target marktd-runtime      -t marktd:dev      .
#   docker build --target processd-runtime    -t processd:dev    .
#   docker build --target invoicd-runtime     -t invoicd:dev     .
#   docker build --target edmd-runtime        -t edmd:dev        .
#   docker build --target netzbilanzd-runtime -t netzbilanzd:dev .
#   docker build --target einsd-runtime       -t einsd:dev       .
#   docker build --target productd-runtime    -t productd:dev    .
#   docker build --target billingd-runtime    -t billingd:dev    .
#   docker build --target outputd-runtime     -t outputd:dev     .
#   docker build --target accountingd-runtime -t accountingd:dev .
#   docker build --target vertragd-runtime    -t vertragd:dev    .
#   docker build --target portald-runtime     -t portald:dev     .
#   docker build --target agentd-runtime      -t agentd:dev      .
#
# `docker-bake.hcl` has a target per service and builds them in groups;
# `just build-demo` builds the three `demos/nb-stp` starts.
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
# Base images are pinned by digest, so a tag moved upstream changes nothing
# here until the digest in this file changes; `cargo xtask check-action-pins`
# refuses a `FROM` without one. The compiler is `rust-toolchain.toml`'s channel,
# installed into the `chef` stage and asserted there.
#
# Every binary is built with `cargo auditable`, which embeds its dependency
# manifest in a `.dep-v0` section. GNU `strip` keeps that section, and each
# builder stage reads the manifest back from the stripped binary with
# `rust-audit-info` — a binary without one fails the image build.
#
# Build arguments (override with --build-arg)
#  PROFILE          Cargo profile: release (default) or dev
#  OCI_VERSION      Image version label. Defaults to `dev`, matching
#                   `docker-bake.hcl`: a plain `docker build` produces an
#                   unreleased image and must not label it with a released
#                   version. CI passes the workspace version.
#  OCI_REVISION     Git commit SHA (set at CI time)
#  OCI_CREATED      ISO-8601 build timestamp (set at CI time)
# ──────────────────────────────────────────────────────────────────────────────

# Global ARGs — available in FROM lines; must be re-declared inside a stage
# to be visible in that stage's RUN commands.
ARG PROFILE=release

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 1 — chef  (cargo-chef + Rust toolchain + native build deps)
# ╚══════════════════════════════════════════════════════════════════════════════
# The lukemathwalker/cargo-chef pre-built image ships cargo-chef on top of the
# official rust image. The tagging scheme is:
#   latest-rust-<rust-version>-<debian-variant>
# The tag is for the reader; the digest is what is pulled.
FROM lukemathwalker/cargo-chef:latest-rust-1.94-bookworm@sha256:7e550e59b8c14e74aa86ca9e5950b75d937c9d51e7f5a4c840e726a33b4b8455 AS chef

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

# The compiler is `rust-toolchain.toml`'s channel. The base image's default
# toolchain carries a full version name (`1.94.1`), which rustup does not take
# for the file's `1.94`, so the file's toolchain is installed here — in a cached
# layer, rather than by the first `cargo` call after `COPY . .` on every source
# change. The assertion fails the build when anything (a `RUSTUP_TOOLCHAIN`
# override, a changed file) makes the active compiler disagree with the file.
COPY rust-toolchain.toml rust-toolchain.toml
RUN rustup toolchain install \
    && want="$(sed -n 's/^channel *= *"\([^"]*\)".*/\1/p' rust-toolchain.toml)" \
    && have="$(rustc --version | cut -d' ' -f2)" \
    && case "$have" in "$want"|"$want".*) echo "rustc $have = rust-toolchain.toml $want" ;; \
         *) echo "rustc $have, but rust-toolchain.toml pins $want" >&2; exit 1 ;; esac

# `cargo auditable` embeds each binary's dependency manifest; `rust-audit-info`
# reads it back. Both pinned, so the builder stages run the tools this file names.
RUN cargo install --locked cargo-auditable@0.7.7 rust-audit-info@0.5.4

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
# ║ Stage 3a — demo-builder  (makod, marktd, processd)
# ╚══════════════════════════════════════════════════════════════════════════════
# The three binaries `demos/nb-stp` starts, and the only ones whose runtime
# stage copies from here. They pull in neither iceberg/datafusion (edmd, einsd)
# nor LanceDB (agentd), which is what keeps this stage at a few minutes cold
# while `builder` takes tens. A demo that needs a service from `builder` —
# `demos/eeg-billing` needs edmd and einsd — builds that image on its own.
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
    CARGO_INCREMENTAL=1 mold -run cargo auditable build --profile ${PROFILE} \
                -p makod -p marktd \
    && CARGO_INCREMENTAL=1 mold -run cargo auditable build --profile ${PROFILE} -p processd --features integrated \
    && BIN_DIR="$([ "${PROFILE}" = "release" ] && echo target/release || echo target/debug)" \
    && for bin in makod marktd processd; do \
         cp "${BIN_DIR}/${bin}" "/usr/local/bin/${bin}" && strip "/usr/local/bin/${bin}" || exit 1; \
         rust-audit-info "/usr/local/bin/${bin}" > /dev/null \
           || { echo "${bin}: no cargo-auditable manifest after strip" >&2; exit 1; }; \
       done \
    && install -d -o 65532 -g 65532 -m 0700 /var/lib/makod

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 3b — builder  (every other service — production / CI release pipeline)
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
                    -p makod -p marktd -p processd -p invoicd -p edmd \
                    -p netzbilanzd -p einsd \
                    -p productd -p billingd -p outputd -p accountingd -p vertragd \
                    -p portald -p agentd \
                    --recipe-path recipe.json

# ── Application layer (rebuilt on every source change) ────────────────────────
COPY . .
RUN --mount=type=cache,id=cargo-registry,sharing=locked,target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-target-full,sharing=locked,target=/build/target \
    CARGO_INCREMENTAL=1 mold -run cargo auditable build --profile ${PROFILE} -p makod -p marktd -p invoicd -p edmd \
                                     -p netzbilanzd -p einsd \
                                     -p productd -p billingd -p outputd -p accountingd \
                                     -p vertragd -p portald -p agentd \
    && CARGO_INCREMENTAL=1 mold -run cargo auditable build --profile ${PROFILE} -p processd --features integrated \
    && BIN_DIR="$([ "${PROFILE}" = "release" ] && echo target/release || echo target/debug)" \
    && for bin in makod marktd processd invoicd edmd netzbilanzd einsd \
                  productd billingd outputd accountingd vertragd portald agentd; do \
         cp "${BIN_DIR}/${bin}" "/usr/local/bin/${bin}" && strip "/usr/local/bin/${bin}" || exit 1; \
         rust-audit-info "/usr/local/bin/${bin}" > /dev/null \
           || { echo "${bin}: no cargo-auditable manifest after strip" >&2; exit 1; }; \
       done \
    && install -d -o 65532 -g 65532 -m 0700 /var/lib/makod

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 4 — runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
# gcr.io/distroless/cc-debian12:nonroot contains:
#   • glibc, libgcc (required by Rust's x86_64/aarch64-unknown-linux-gnu target)
#   • ca-certificates (system trust store for TLS)
#   • No shell, no package manager, no coreutils
#   • UID/GID 65532 "nonroot" pre-configured
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f AS runtime

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
# In Kubernetes use httpGet probes against /health/live (liveness) and
# /health/ready (readiness) instead — no shell needed.
HEALTHCHECK --interval=15s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/makod", "--check"]

ENTRYPOINT ["/usr/local/bin/makod"]

# OCI image labels.
# Set OCI_REVISION and OCI_CREATED at CI time, e.g.:
#   docker build \
#     --build-arg OCI_REVISION=$(git rev-parse HEAD) \
#     --build-arg OCI_CREATED=$(date -u +%Y-%m-%dT%H:%M:%SZ) …
ARG OCI_VERSION=dev
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
# ║ Stage 5 — marktd-runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f AS marktd-runtime

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

# `--check` probes this instance's own /health/ready over loopback (a DB ping
# plus the daemon's readiness), which is what a distroless image can do without
# a shell or curl. It does not open a second pool or re-run migrations.
HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=5 \
    CMD ["/usr/local/bin/marktd", "--check"]

ENTRYPOINT ["/usr/local/bin/marktd"]

ARG OCI_VERSION=dev
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
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f AS processd-runtime

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

ARG OCI_VERSION=dev
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
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f AS invoicd-runtime

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

ARG OCI_VERSION=dev
ARG OCI_REVISION=unknown
ARG OCI_CREATED=unknown
LABEL org.opencontainers.image.title="invoicd" \
      org.opencontainers.image.description="INVOIC plausibility-check daemon — LF role, § 147 AO / GoBD (MaKo)" \
      org.opencontainers.image.version="${OCI_VERSION}" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.created="${OCI_CREATED}" \
      org.opencontainers.image.source="https://github.com/hupe1980/mako" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 8 — edmd-runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f AS edmd-runtime

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

ARG OCI_VERSION=dev
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
# ║ Stage 9 — netzbilanzd-runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f AS netzbilanzd-runtime

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
ARG OCI_VERSION=dev
ARG OCI_REVISION=unknown
ARG OCI_CREATED=unknown
LABEL org.opencontainers.image.title="netzbilanzd" \
      org.opencontainers.image.description="NNE/KA/MMM/MSB/AWH billing and MaBiS Summenzeitreihen (MSCONS 13003) — NB role, GridSettlement, CalculationTrace (MaKo)" \
      org.opencontainers.image.version="${OCI_VERSION}" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.created="${OCI_CREATED}" \
      org.opencontainers.image.source="https://github.com/hupe1980/mako" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 10 — einsd-runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f AS einsd-runtime

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
ARG OCI_VERSION=dev
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
# ║ Stage 11 — productd-runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f AS productd-runtime

COPY --from=builder /usr/share/zoneinfo/Europe      /usr/share/zoneinfo/Europe
COPY --from=builder /usr/share/zoneinfo/UTC         /usr/share/zoneinfo/UTC
COPY --from=builder /usr/share/zoneinfo/leap-seconds.list \
                    /usr/share/zoneinfo/leap-seconds.list
COPY --from=builder /usr/share/zoneinfo/Europe/Berlin /etc/localtime
ENV TZ=Europe/Berlin
COPY --from=builder --chown=root:root /usr/local/bin/productd /usr/local/bin/productd
EXPOSE 9080
ENV PRODUCTD_LOG_FORMAT=json \
    PRODUCTD_LOG_LEVEL=info
HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=5 \
    CMD ["/usr/local/bin/productd", "--check"]
ENTRYPOINT ["/usr/local/bin/productd"]
ARG OCI_VERSION=dev
ARG OCI_REVISION=unknown
ARG OCI_CREATED=unknown
LABEL org.opencontainers.image.title="productd" \
      org.opencontainers.image.description="Product & Tariff Catalog daemon — LF role, EPEX §41a, B2B Angebote (MaKo)" \
      org.opencontainers.image.version="${OCI_VERSION}" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.created="${OCI_CREATED}" \
      org.opencontainers.image.source="https://github.com/hupe1980/mako" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 12 — billingd-runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f AS billingd-runtime

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
ARG OCI_VERSION=dev
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
# ║ Stage 13 — outputd-runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f AS outputd-runtime

COPY --from=builder /usr/share/zoneinfo/Europe      /usr/share/zoneinfo/Europe
COPY --from=builder /usr/share/zoneinfo/UTC         /usr/share/zoneinfo/UTC
COPY --from=builder /usr/share/zoneinfo/leap-seconds.list \
                    /usr/share/zoneinfo/leap-seconds.list
COPY --from=builder /usr/share/zoneinfo/Europe/Berlin /etc/localtime
ENV TZ=Europe/Berlin
COPY --from=builder --chown=root:root /usr/local/bin/outputd /usr/local/bin/outputd
EXPOSE 9880
ENV OUTPUTD_LOG_FORMAT=json \
    OUTPUTD_LOG_LEVEL=info
HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=5 \
    CMD ["/usr/local/bin/outputd", "--check"]
ENTRYPOINT ["/usr/local/bin/outputd"]
ARG OCI_VERSION=dev
ARG OCI_REVISION=unknown
ARG OCI_CREATED=unknown
LABEL org.opencontainers.image.title="outputd" \
      org.opencontainers.image.description="Customer Communications daemon — operator Typst templates, ZUGFeRD PDF/A-3 carrier, publish gates (MaKo)" \
      org.opencontainers.image.version="${OCI_VERSION}" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.created="${OCI_CREATED}" \
      org.opencontainers.image.source="https://github.com/hupe1980/mako" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 14 — accountingd-runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f AS accountingd-runtime

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
ARG OCI_VERSION=dev
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
# ║ Stage 15 — vertragd-runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f AS vertragd-runtime

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
ARG OCI_VERSION=dev
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
# ║ Stage 16 — portald-runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f AS portald-runtime

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
ARG OCI_VERSION=dev
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
# ║ Stage 17 — agentd-runtime (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f AS agentd-runtime

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
ARG OCI_VERSION=dev
ARG OCI_REVISION=unknown
ARG OCI_CREATED=unknown
LABEL org.opencontainers.image.title="agentd" \
      org.opencontainers.image.description="Multi-agent LLM orchestration daemon — 28 specialists, LanceDB RAG, MCP tools (MaKo)" \
      org.opencontainers.image.version="${OCI_VERSION}" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.created="${OCI_CREATED}" \
      org.opencontainers.image.source="https://github.com/hupe1980/mako" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"
