# ── mako — multi-stage Docker build ──────────────────────────────────────────
#
# Uses cargo-chef for proper Rust dependency layer caching. One runtime stage
# per service binary; `runtime` (makod) is the last stage and so the default
# target.
#
# Stages:
#   chef           cargo-chef + Rust toolchain + native build deps
#   planner        Analyse workspace, emit recipe.json
#   demo-builder   makod, marktd, processd — what `just build-demo` builds
#   builder        Every other service
#   runtime-base   gcr.io/distroless/cc-debian12:nonroot + Europe/Berlin zone data
#   *-runtime      One per binary, on runtime-base
#
# Build one image (makod's stage is `runtime`, every other `<service>-runtime`):
#   docker build --target runtime        -t makod:dev  .
#   docker build --target marktd-runtime -t marktd:dev .
#
# `docker-bake.hcl` holds the list of released images, one target per service.
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
# ║ Stage 3b — builder  (every service demo-builder does not build)
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
                    -p invoicd -p edmd -p netzbilanzd -p einsd \
                    -p productd -p billingd -p outputd -p accountingd -p vertragd \
                    -p portald -p agentd \
                    --recipe-path recipe.json

# ── Application layer (rebuilt on every source change) ────────────────────────
COPY . .
RUN --mount=type=cache,id=cargo-registry,sharing=locked,target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-target-full,sharing=locked,target=/build/target \
    CARGO_INCREMENTAL=1 mold -run cargo auditable build --profile ${PROFILE} \
                -p invoicd -p edmd -p netzbilanzd -p einsd \
                -p productd -p billingd -p outputd -p accountingd -p vertragd \
                -p portald -p agentd \
    && BIN_DIR="$([ "${PROFILE}" = "release" ] && echo target/release || echo target/debug)" \
    && for bin in invoicd edmd netzbilanzd einsd \
                  productd billingd outputd accountingd vertragd portald agentd; do \
         cp "${BIN_DIR}/${bin}" "/usr/local/bin/${bin}" && strip "/usr/local/bin/${bin}" || exit 1; \
         rust-audit-info "/usr/local/bin/${bin}" > /dev/null \
           || { echo "${bin}: no cargo-auditable manifest after strip" >&2; exit 1; }; \
       done

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 4 — runtime-base (distroless)
# ╚══════════════════════════════════════════════════════════════════════════════
# gcr.io/distroless/cc-debian12:nonroot contains:
#   • glibc, libgcc (required by Rust's x86_64/aarch64-unknown-linux-gnu target)
#   • ca-certificates (system trust store for TLS)
#   • No shell, no package manager, no coreutils
#   • UID/GID 65532 "nonroot" pre-configured
#
# Every service stage below starts here and adds its binary, port, health check
# and title. A service's `--check` probes its own /health/ready over loopback,
# which is what a distroless image can do without a shell or curl.
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f AS runtime-base

# tzdata: distroless/cc does not ship /usr/share/zoneinfo. Only the zone data
# for TZ=Europe/Berlin (CET/CEST) is copied; /etc/localtime serves code that
# does not consult $TZ.
COPY --from=chef /usr/share/zoneinfo/Europe            /usr/share/zoneinfo/Europe
COPY --from=chef /usr/share/zoneinfo/UTC               /usr/share/zoneinfo/UTC
COPY --from=chef /usr/share/zoneinfo/leap-seconds.list /usr/share/zoneinfo/leap-seconds.list
COPY --from=chef /usr/share/zoneinfo/Europe/Berlin     /etc/localtime

# `LOG_FORMAT` is the fallback every mako-service daemon reads after its own
# `<SERVICE>_LOG_FORMAT`.
ENV TZ=Europe/Berlin \
    LOG_FORMAT=json

# OCI image labels. CI passes OCI_VERSION, OCI_REVISION and OCI_CREATED; each
# service stage adds its title and description.
ARG OCI_VERSION=dev
ARG OCI_REVISION=unknown
ARG OCI_CREATED=unknown
LABEL org.opencontainers.image.version="${OCI_VERSION}" \
      org.opencontainers.image.revision="${OCI_REVISION}" \
      org.opencontainers.image.created="${OCI_CREATED}" \
      org.opencontainers.image.source="https://github.com/hupe1980/mako" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"

# ╔══════════════════════════════════════════════════════════════════════════════
# ║ Stage 5 — one runtime stage per service
# ╚══════════════════════════════════════════════════════════════════════════════
# Binaries are root:root 0755 and run as uid 65532 (distroless nonroot).

FROM runtime-base AS marktd-runtime
COPY --from=demo-builder --chown=root:root /usr/local/bin/marktd /usr/local/bin/marktd
EXPOSE 8180
HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=5 \
    CMD ["/usr/local/bin/marktd", "--check"]
ENTRYPOINT ["/usr/local/bin/marktd"]
LABEL org.opencontainers.image.title="marktd" \
      org.opencontainers.image.description="Market Data Hub daemon — MaLo/MeLo master data, Versorgungsstatus, Preisblätter, event fan-out"

FROM runtime-base AS processd-runtime
COPY --from=demo-builder --chown=root:root /usr/local/bin/processd /usr/local/bin/processd
EXPOSE 8580
HEALTHCHECK --interval=10s --timeout=3s --start-period=30s --retries=5 \
    CMD ["/usr/local/bin/processd", "--check"]
ENTRYPOINT ["/usr/local/bin/processd"]
LABEL org.opencontainers.image.title="processd" \
      org.opencontainers.image.description="Process Decision Engine — NB STP (mako-pruefung) and LF auto-response, process read model, Sperrauftrag queue"

FROM runtime-base AS invoicd-runtime
COPY --from=builder --chown=root:root /usr/local/bin/invoicd /usr/local/bin/invoicd
EXPOSE 8280
HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=5 \
    CMD ["/usr/local/bin/invoicd", "--check"]
ENTRYPOINT ["/usr/local/bin/invoicd"]
LABEL org.opencontainers.image.title="invoicd" \
      org.opencontainers.image.description="INVOIC plausibility-check daemon — LF role, § 147 AO / GoBD receipt retention"

FROM runtime-base AS edmd-runtime
COPY --from=builder --chown=root:root /usr/local/bin/edmd /usr/local/bin/edmd
EXPOSE 8380
HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=5 \
    CMD ["/usr/local/bin/edmd", "--check"]
ENTRYPOINT ["/usr/local/bin/edmd"]
LABEL org.opencontainers.image.title="edmd" \
      org.opencontainers.image.description="Energy Data Management daemon — MSCONS meter readings, MeterBillingPeriod"

FROM runtime-base AS netzbilanzd-runtime
COPY --from=builder --chown=root:root /usr/local/bin/netzbilanzd /usr/local/bin/netzbilanzd
EXPOSE 8680
HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=5 \
    CMD ["/usr/local/bin/netzbilanzd", "--check"]
ENTRYPOINT ["/usr/local/bin/netzbilanzd"]
LABEL org.opencontainers.image.title="netzbilanzd" \
      org.opencontainers.image.description="NNE/KA/MMM/MSB/AWH billing and MaBiS Summenzeitreihen — NB role, GridSettlement, CalculationTrace"

FROM runtime-base AS einsd-runtime
COPY --from=builder --chown=root:root /usr/local/bin/einsd /usr/local/bin/einsd
EXPOSE 9180
HEALTHCHECK --interval=10s --timeout=3s --start-period=30s --retries=5 \
    CMD ["/usr/local/bin/einsd", "--check"]
ENTRYPOINT ["/usr/local/bin/einsd"]
LABEL org.opencontainers.image.title="einsd" \
      org.opencontainers.image.description="Einspeiser registry and EEG/KWKG settlement daemon"

FROM runtime-base AS productd-runtime
COPY --from=builder --chown=root:root /usr/local/bin/productd /usr/local/bin/productd
EXPOSE 9080
HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=5 \
    CMD ["/usr/local/bin/productd", "--check"]
ENTRYPOINT ["/usr/local/bin/productd"]
LABEL org.opencontainers.image.title="productd" \
      org.opencontainers.image.description="Product & Tariff Catalog daemon — LF role, dynamic tariffs (§ 41a EnWG), B2B Angebote"

FROM runtime-base AS billingd-runtime
COPY --from=builder --chown=root:root /usr/local/bin/billingd /usr/local/bin/billingd
EXPOSE 9280
HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=5 \
    CMD ["/usr/local/bin/billingd", "--check"]
ENTRYPOINT ["/usr/local/bin/billingd"]
LABEL org.opencontainers.image.title="billingd" \
      org.opencontainers.image.description="Energy Billing Engine daemon — LF role, XRechnung, § 14a EnWG, § 41a EnWG"

FROM runtime-base AS outputd-runtime
COPY --from=builder --chown=root:root /usr/local/bin/outputd /usr/local/bin/outputd
EXPOSE 9880
HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=5 \
    CMD ["/usr/local/bin/outputd", "--check"]
ENTRYPOINT ["/usr/local/bin/outputd"]
LABEL org.opencontainers.image.title="outputd" \
      org.opencontainers.image.description="Customer-communications daemon — Typst templates, ZUGFeRD PDF/A-3, issued-document store, delivery evidence"

FROM runtime-base AS accountingd-runtime
COPY --from=builder --chown=root:root /usr/local/bin/accountingd /usr/local/bin/accountingd
EXPOSE 9380
HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=5 \
    CMD ["/usr/local/bin/accountingd", "--check"]
ENTRYPOINT ["/usr/local/bin/accountingd"]
LABEL org.opencontainers.image.title="accountingd" \
      org.opencontainers.image.description="Customer Account Ledger daemon — LF role, SEPA pain.008/pain.001, dunning, GDPR Art. 17"

FROM runtime-base AS vertragd-runtime
COPY --from=builder --chown=root:root /usr/local/bin/vertragd /usr/local/bin/vertragd
EXPOSE 9780
HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=5 \
    CMD ["/usr/local/bin/vertragd", "--check"]
ENTRYPOINT ["/usr/local/bin/vertragd"]
LABEL org.opencontainers.image.title="vertragd" \
      org.opencontainers.image.description="Contract & Customer Management daemon — LF role, B2C + B2B, OIDC → MaLo auth gateway"

FROM runtime-base AS portald-runtime
COPY --from=builder --chown=root:root /usr/local/bin/portald /usr/local/bin/portald
EXPOSE 9480
HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=5 \
    CMD ["/usr/local/bin/portald", "--check"]
ENTRYPOINT ["/usr/local/bin/portald"]
LABEL org.opencontainers.image.title="portald" \
      org.opencontainers.image.description="Customer Portal gateway — LF role, REST + SSE customer self-service"

FROM runtime-base AS agentd-runtime
COPY --from=builder --chown=root:root /usr/local/bin/agentd /usr/local/bin/agentd
EXPOSE 9580
HEALTHCHECK --interval=10s --timeout=3s --start-period=30s --retries=5 \
    CMD ["/usr/local/bin/agentd", "--check"]
ENTRYPOINT ["/usr/local/bin/agentd"]
LABEL org.opencontainers.image.title="agentd" \
      org.opencontainers.image.description="Multi-agent LLM orchestration daemon — agentplane durable runtime, LanceDB RAG, MCP tools"

# makod — last, so a plain `docker build .` builds it.
FROM runtime-base AS runtime
COPY --from=demo-builder --chown=root:root /usr/local/bin/makod /usr/local/bin/makod

# Persistent state directory — pre-created with uid 65532 ownership so that
# SlateDB can write here even when no volume is mounted (e.g. --check mode).
# VOLUME is declared AFTER this COPY so Docker does not discard the ownership.
# Mount a named volume or PVC here; ensure it is writable by uid 65532.
COPY --from=demo-builder /var/lib/makod /var/lib/makod
VOLUME ["/var/lib/makod"]

# makod reads its own MAKOD_* variables, not the mako-service fallbacks.
ENV MAKOD_LOG_FORMAT=json \
    MAKOD_DATA_DIR=/var/lib/makod \
    MAKOD_HTTP_ADDR=0.0.0.0:8080

# Exposed ports:
#   8080  HTTP REST API + MCP server
#   4080  AS4 inbound transport  (--as4-addr)
#   8090  API-Webdienste Strom   (--api-webdienste-addr)
EXPOSE 8080 4080 8090

# --check validates config, profiles, and adapters then exits 0. In Kubernetes
# use httpGet probes against /health/live and /health/ready instead.
HEALTHCHECK --interval=15s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/makod", "--check"]

ENTRYPOINT ["/usr/local/bin/makod"]
LABEL org.opencontainers.image.title="makod" \
      org.opencontainers.image.description="Mako process engine daemon — German energy market communication (MaKo/BDEW EDI@Energy)"
