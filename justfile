# Justfile for edi-energy-rs / mako
# Install just: https://github.com/casey/just
#
# Usage:
#   just          → list all recipes
#   just check    → minimum gate before every commit
#   just ci       → full CI suite (check + test + lint + deny)

set shell := ["zsh", "-eu", "-o", "pipefail", "-c"]

# ── Default: list all recipes ──────────────────────────────────────────────────

[private]
default:
    @just --list

# ── Core gates ────────────────────────────────────────────────────────────────

# Minimum gate before every commit: type-check all targets
check:
    cargo check --all-targets --all-features

# Run all tests
test:
    cargo test --all-features

# Run tests for a specific crate (e.g. `just test-crate mako-engine`)
test-crate crate:
    cargo test -p {{ crate }} --all-features

# Run a specific integration test (e.g. `just test-integration smoke`)
test-integration name:
    cargo test --test {{ name }} --all-features

# ── Database integration tests ───────────────────────────────────────────────
# Every suite below self-manages its PostgreSQL via testcontainers: a throwaway
# container is started in-process (once per test binary) and torn down by the
# testcontainers reaper afterwards. The only requirement is a running Docker
# daemon — no manual `docker run`, no fixed host ports, no `*_DATABASE_URL` env
# vars. Without Docker the `#[ignore]`d tests skip gracefully.

# All database integration suites in one go.
test-db: test-edmd-db test-einsd-db test-accountingd-db test-billingd-db test-vertragd-db test-tarifbd-db test-marktd-db

# Storage integration tests for edmd (meterstore hot/cold over PostgreSQL + a
# filesystem Iceberg warehouse).
test-edmd-db:
    cargo test -p edmd --test meterstore_integration -- --include-ignored --test-threads=1

# Handler + SQL integration tests for einsd (EEG settlement).
test-einsd-db:
    cargo test -p einsd --test settlement_integration -- --include-ignored --test-threads=1

# Ledger integration tests for accountingd — the doubleentry-backed
# Massenkontokorrent (idempotency, netting, reconcile, period seal, Merkle
# inclusion proof) against real PostgreSQL.
test-accountingd-db:
    cargo test -p accountingd --test db_scenarios -- --include-ignored --test-threads=1

# Records integration tests for billingd.
test-billingd-db:
    cargo test -p billingd --test records_integration -- --include-ignored --test-threads=1

# Dispatch integration tests for vertragd.
test-vertragd-db:
    cargo test -p vertragd --test dispatch_integration -- --include-ignored --test-threads=1

# Catalog integration tests for tarifbd.
test-tarifbd-db:
    cargo test -p tarifbd --test catalog_integration -- --include-ignored --test-threads=1

# All marktd integration suites (VersorgungsStatus, MeLo graph, ESA, registries,
# durable fan-out).
test-marktd-db:
    cargo test -p marktd \
        --test versorgung_integration --test melo_graph_integration \
        --test esa_integration --test registries_integration \
        --test fanout_durable_integration -- --include-ignored --test-threads=1

# Lint with warnings as errors
clippy:
    cargo clippy --all-targets --all-features -- -D warnings

# Format all code
fmt:
    cargo fmt --all

# Check formatting without making changes (for CI)
fmt-check:
    cargo fmt --all -- --check

# Dependency audit: licenses + advisories
# cargo deny does not accept --all-features; it always resolves the full
# workspace graph from Cargo.lock. Stale skip entries (previously gated on
# slatedb/energy-api features) have been cleaned up; deny check now runs clean.
deny:
    cargo deny check

# Guard: no hardcoded rubo4e schema-version aliases in business logic.
# Domain code must use rubo4e::current:: or rubo4e::identifiers:: — never
# rubo4e::v202607:: or any other pinned version path.
no-version-alias:
    @! grep -rn 'rubo4e::v[0-9]' crates/ services/ --include='*.rs' \
        || (echo "ERROR: hardcoded rubo4e version alias found — use rubo4e::current:: instead" && exit 1)

# Build and check rustdoc (--all-features, warnings as errors)
doc-check:
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features

# `just test` only runs --all-features, which cannot catch a module whose
# `#[cfg(feature = ...)]` gate is missing or misplaced — that only shows up when
# the feature is off. Mirrors the `test` matrix in .github/workflows/ci.yml.
#
# Test every edi-energy feature combination CI builds
test-features:
    #!/usr/bin/env bash
    set -euo pipefail
    for f in "--no-default-features" "--all-features" \
             "--features utilmd" "--features mscons" "--features aperak" \
             "--features contrl" "--features invoic" "--features orders" \
             "--features partin" "--features remadv" "--features reqote" \
             "--features iftsta" "--features insrpt" "--features ordchg" \
             "--features ordrsp" "--features comdis" "--features pricat" \
             "--features utilts" "--features quotes" "--features diagnostics"; do
        echo "==> cargo test -p edi-energy $f"
        cargo test -p edi-energy $f
    done

# Full CI suite (minimum gate + tests + quality + release-lifecycle checks)
# Lint every documented role-scoped deployment profile of makod.
#
# `clippy` above runs --all-features, which turns on *every* role at once. That
# is exactly the configuration in which role gating cannot be wrong: each
# `#[cfg(feature = "role-…")]` is satisfied, so nothing is excluded and no
# unused import appears. A role-scoped build is the opposite case and the one
# operators actually deploy, so it needs its own pass — the profiles below are
# the ones documented in services/makod/Cargo.toml.
clippy-roles:
    #!/usr/bin/env bash
    set -euo pipefail
    for f in "role-lf" "role-nb" "role-msb" \
             "role-lf-strom" "role-lf-gas" \
             "role-nb-strom" "role-nb-gas" \
             "role-msb-strom" "role-msb-gas" "role-esa-strom" \
             "role-lf,role-nb,role-msb"; do
        echo "==> cargo clippy -p makod --features $f"
        cargo clippy -p makod --features "$f" --all-targets -- -D warnings
    done
    # agentd carries the same role flags: it is the one service that reaches all
    # the others, so a role-scoped build must exclude the other arm's
    # specialists rather than merely decline to run them (§ 9 EnWG).
    for f in "role-lf" "role-nb" "role-msb"; do
        echo "==> cargo clippy -p agentd --features $f"
        cargo clippy -p agentd --features "$f" --all-targets -- -D warnings
        cargo test   -p agentd --features "$f" --lib role_scoped
    done

# Boot each umbrella deployment profile and assert it passes --check.
#
# `clippy-roles` proves a role-scoped build *compiles*; it cannot prove the
# binary starts. Startup runs assertions that only fire at runtime — adapter
# coverage and dispatch completeness both panic — so a role gate that excludes
# a module while leaving its PIDs registered produces a binary that lints clean
# and dies on boot. That is the failure an operator would meet in production,
# so it needs a real run.
#
# Umbrella profiles only (lf/nb/msb): the per-Sparte features are components of
# these, and a full build per feature is too slow for the default gate.
smoke-roles:
    #!/usr/bin/env bash
    set -euo pipefail
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    for pair in "role-lf:LF" "role-nb:NB" "role-msb:MSB"; do
        feat="${pair%%:*}"; role="${pair##*:}"
        echo "==> $feat (party role $role)"
        cargo build -p makod --features "$feat"
        printf '[[party]]\nmp_id = "9900001000001"\nroles = ["%s"]\nprimary = true\n' \
            "$role" > "$tmp/makod.toml"
        ./target/debug/makod --config "$tmp/makod.toml" --allow-volatile --check
    done

ci: check test test-features clippy clippy-roles smoke-roles fmt-check deny no-version-alias check-bo4e-coverage check-routes check-tool-grants doc-check codegen-check validate-profiles-strict validate-pruefids-strict-ci

# mako proves the carrier by reading its own output back (billingd's publish
# gate), and `en16931 validate` — an independent implementation — reports the
# payload valid. Neither proves **PDF/A-3 conformance**: nothing in Rust does,
# and the XMP `document::facturx::stamp` appends lands after typst-pdf's own
# enforcement has finished. That is what veraPDF is for, and it needs a file.

# Write a stamped ZUGFeRD specimen for veraPDF / the ZUGFeRD validator
zugferd-specimen out="target/zugferd-specimen.pdf":
    #!/usr/bin/env bash
    set -euo pipefail
    # Absolute, because `cargo test` runs a test with the *package* directory as
    # its working directory — a relative path lands in services/billingd/, not
    # where the caller is standing.
    out="$(cd "$(dirname "{{ out }}")" 2>/dev/null && pwd || mkdir -p "$(dirname "{{ out }}")" && cd "$(dirname "{{ out }}")" && pwd)/$(basename "{{ out }}")"
    MAKO_ZUGFERD_OUT="$out" cargo test -p billingd --test zugferd_carrier \
        -- --ignored --nocapture write_specimen_for_external_validators
    echo ""
    echo "Wrote three files: the stamped carrier, the XRechnung-profile carrier"
    echo "(\${out%.pdf}-xrechnung.pdf) and the pre-stamp control (\${out%.pdf}-unstamped.pdf)."
    echo "Verify, in order of what each proves:"
    echo "  en16931 validate $out                       # payload, 227 core rules"
    echo "  en16931 validate \${out%.pdf}-xrechnung.pdf  # 282 rules incl. BR-DE"
    echo "  verapdf -f 3b $out                          # PDF/A-3b; the control isolates stamp regressions"
    echo "  ZUGFeRD validator on $out                   # carrier metadata against the specification"

# Verify the ZUGFeRD specimens with the two external validators, containerized —
# no host Java, no host veraPDF. `verapdf/cli` is the veraPDF Foundation's own
# image; Mustang is the ZUGFeRD project's reference validator, fetched once into
# target/ and run under Temurin. Both must report every file valid/compliant.
# In-repo checks cannot replace these: the duplicate-schemas XMP defect was
# invisible to four layers of our own checking and found only by veraPDF.

# Validate the specimens with veraPDF + Mustang, containerized (needs Docker only)
zugferd-verify: zugferd-specimen
    #!/usr/bin/env bash
    set -euo pipefail
    command -v docker >/dev/null || { echo "docker required (host verapdf/java work too — see site docs)"; exit 1; }
    jar=target/Mustang-CLI-2.25.0.jar
    [ -f "$jar" ] || curl -sL -o "$jar" \
        https://github.com/ZUGFeRD/mustangproject/releases/download/core-2.25.0/Mustang-CLI-2.25.0.jar
    fail=0
    for f in zugferd-specimen zugferd-specimen-xrechnung zugferd-specimen-unstamped; do
        echo "==> veraPDF: $f"
        docker run --rm -v "$PWD/target:/data" verapdf/cli:latest -f 3b "/data/$f.pdf" \
            | grep -oE 'isCompliant="[a-z]+"' | grep -q 'true' || { echo "   NOT COMPLIANT"; fail=1; }
    done
    for f in zugferd-specimen zugferd-specimen-xrechnung; do
        echo "==> Mustang: $f"
        docker run --rm -v "$PWD/target:/data" eclipse-temurin:21-jre \
            java -jar "/data/$(basename "$jar")" --action validate --source "/data/$f.pdf" \
            | grep -cE '<summary status="valid"/>' | grep -qv '^0$' || { echo "   NOT VALID"; fail=1; }
    done
    [ "$fail" = 0 ] && echo "all specimens valid under veraPDF + Mustang" || exit 1

# ── makotest (Python toolkit) ─────────────────────────────────────────────────

# Build the PyO3 extension and run the Python suite.
#
# Creates `makotest/.venv` on first run and addresses it by path — `maturin
# develop` refuses to run without a virtualenv, and finds `.venv` on its own, so
# no activation is needed.
test-makotest:
    cd makotest && test -d .venv || python3 -m venv .venv
    cd makotest && .venv/bin/pip install -q --upgrade pip 'maturin>=1.9,<2.0' pytest hypothesis
    cd makotest && .venv/bin/maturin develop && .venv/bin/pytest -q

# Build a release wheel (abi3, one wheel for Python ≥ 3.11).
build-makotest:
    cd makotest && .venv/bin/maturin build --release

# ── Build ─────────────────────────────────────────────────────────────────────

# Build the 3 demo Docker images — makod, marktd, processd.
# Only deps needed for the Lieferbeginn smoke test; no iceberg/LanceDB.
# Expected cold build: ~8 min (debug) / ~12 min (release).
# Optional services (invoicd, netzbilanzd, obsd): build with --target <name>-runtime.
build-demo profile="dev":
    docker build --target runtime             --build-arg PROFILE={{ profile }} -t makod:dev     .
    docker build --target marktd-runtime      --build-arg PROFILE={{ profile }} -t marktd:dev    .
    docker build --target processd-runtime    --build-arg PROFILE={{ profile }} -t processd:dev  .

# Build xtask (needed after changing xtask commands)
build-xtask:
    cargo build -p xtask

# ── Local development ─────────────────────────────────────────────────────────
#
# Run infrastructure dependencies in Docker, Rust services directly with cargo.
# Requires: docker, cargo-watch  (`cargo install cargo-watch`)
#
# Typical workflow:
#   just infra-up                 # start postgres
#   just dev marktd               # hot-reload marktd (separate terminal)
#   just dev processd             # hot-reload processd (separate terminal)
#   just infra-down               # stop postgres

# Start infrastructure (postgres only) — services run as cargo processes
infra-up:
    docker compose -f dev/docker-compose.yml up -d
    @echo "Postgres ready on :5432 — connection strings in dev/docker-compose.yml"

# Stop infrastructure and remove containers (volumes are preserved)
infra-down:
    docker compose -f dev/docker-compose.yml down

# Stop infrastructure and delete all volumes (full reset)
infra-reset:
    docker compose -f dev/docker-compose.yml down -v

# Run a single service with hot-reload (requires cargo-watch).
# Example: just dev marktd
dev service:
    cargo watch -x "run -p {{ service }}"

# Run a single service once (no watch).
# Example: just run marktd
run service:
    cargo run -p {{ service }}

# Tail logs for an infra container (postgres).
# Example: just infra-logs postgres
infra-logs container="postgres":
    docker compose -f dev/docker-compose.yml logs -f {{ container }}

# ── Versioning ────────────────────────────────────────────────────────────────

# Bump workspace version (e.g. `just bump 0.2.0`)
bump version:
    cargo xtask bump-version {{ version }}

# ── Profile codegen ───────────────────────────────────────────────────────────

# Regenerate all Rust profile code from YAML/JSON schemas
codegen:
    cargo xtask codegen

# Regenerate profiles for a single message type (e.g. `just codegen-type UTILMD`)
codegen-type type:
    cargo xtask codegen --message-type {{ type }}

# Check that generated files are up-to-date (CI drift guard)
codegen-check:
    cargo xtask codegen --check

# Mark expired profiles as archived and regenerate mod.rs
codegen-prune:
    cargo xtask codegen --prune-expired

# ── Validation ────────────────────────────────────────────────────────────────

# Validate all committed profiles for consistency errors
validate-profiles:
    cargo xtask validate-profiles

# Strict profile validation — errors on any _WARNING field (F-013 CI gate)
# Run this in CI to catch incomplete or placeholder profile entries.
validate-profiles-strict:
    cargo xtask validate-profiles --strict

# Check that every AHB Prüfidentifikator has a test fixture
validate-pruefids:
    cargo xtask validate-pruefids

# Strict Prüfidentifikator validation (exits 1 on missing coverage)
validate-pruefids-strict:
    cargo xtask validate-pruefids --strict

# F-018 CI gate: strict Prüfidentifikator validation with minimum coverage ≥ 1
# Used by the `ci` recipe to ensure every registered PID has at least one test
# fixture.  Prefer `validate-pruefids-strict` for local iteration.
validate-pruefids-strict-ci:
    cargo xtask validate-pruefids --strict --min-coverage 1

# Verify release codes appear in UNH 0057 fixtures
validate-release-codes:
    cargo xtask validate-release-codes

# Verify a profile covers today's date
check-release-coverage:
    cargo xtask check-release-coverage

# Verify the rubo4e::current active-type count matches the README.md claim (delta ≤ 2).
check-bo4e-coverage:
    cargo xtask check-bo4e-coverage

# Refuse axum 0.7 `/:param` route literals. Under axum 0.8 they panic while the
# router is built — i.e. at startup — so nothing in the test suite catches them.
check-routes:
    cargo xtask check-routes

# Every agentd tool grant must name a real MCP tool and agree with that server's
# own `read_only_hint`. A read declared mutating stops for a human on every call;
# a mutation declared read-only is dispatched unattended.
check-tool-grants:
    cargo xtask check-tool-grants

# ── AHB audit ─────────────────────────────────────────────────────────────────

# Comprehensive AHB rule-coverage analysis
audit-ahb:
    cargo xtask audit-ahb

# Audit a single message type (e.g. `just audit-ahb-type INVOIC`)
audit-ahb-type type:
    cargo xtask audit-ahb --message-type {{ type }}

# ── Fixtures ──────────────────────────────────────────────────────────────────

# Regenerate EDIFACT test fixtures
generate-fixtures:
    cargo xtask generate-fixtures

# Dry-run fixture generation (show what would be created)
generate-fixtures-dry:
    cargo xtask generate-fixtures --dry-run

# ── Profile management ────────────────────────────────────────────────────────

# Scaffold a new BDEW format-version directory skeleton (e.g. `just add-release FV2027-10-01`)
add-release fv:
    cargo xtask add-release --fv {{ fv }}

# Diff two profile releases (e.g. `just release-diff UTILMD fv20251001 fv20261001`)
# Use folder-name format (fv20251001) or canonical FV format (FV2025-10-01).
# Both spellings are accepted; FV2025-10-01 is normalised to fv20251001 automatically.
release-diff type from to:
    cargo xtask release-diff --message-type {{ type }} --from {{ from }} --to {{ to }}

# ── Import / extraction ───────────────────────────────────────────────────────

# Import BDEW code lists from CSV
import-codelists file type release:
    cargo xtask import-codelists --file {{ file }} --message-type {{ type }} --release {{ release }}

# Extract MIG/AHB tables from a PDF (best-effort)
extract-pdf file type:
    cargo xtask extract-pdf --file {{ file }} --message-type {{ type }}

# Extract MIG/AHB tables from a DOCX (exact column parser)
extract-docx file type:
    cargo xtask extract-docx --file {{ file }} --message-type {{ type }}

# Import AHB from official BDEW XML (requires BDEW subscription)
import-xml-ahb file type release valid-from:
    cargo xtask import-xml-ahb --file {{ file }} --message-type {{ type }} --release {{ release }} --valid-from {{ valid-from }}

# Import MIG from official BDEW XML (requires BDEW subscription)
import-xml-mig file type release valid-from:
    cargo xtask import-xml-mig --file {{ file }} --message-type {{ type }} --release {{ release }} --valid-from {{ valid-from }}

# ── Docs ──────────────────────────────────────────────────────────────────────

# Open rustdoc for a crate in the browser (e.g. `just doc mako-engine`)
doc crate:
    cargo doc -p {{ crate }} --all-features --no-deps --open

# Build all workspace docs
doc-all:
    cargo doc --workspace --all-features --no-deps

# Validate every Mermaid diagram on the site, and the script that renders them.
#
# `zola check` validates links only — it never parses diagram source, so a
# broken diagram publishes as an error box. This also fails if the renderer
# regresses to `startOnLoad`, which stops rendering on a cold CDN cache.
# Mirrors the "Check Mermaid diagrams" step in .github/workflows/site.yml.
check-mermaid:
    cd site && npm install --no-audit --no-fund --silent && npm run --silent check:mermaid

# Link check + diagram check for the docs site.
# Fail on bare relative Markdown links in site content.
#
# `zola check` validates `@/docs/…` links only; a plain `[text](page.md)` is
# emitted verbatim, never resolved, and 404s silently. See
# site/tools/check-links.mjs.
check-links:
    cd site && node tools/check-links.mjs

check-site: check-mermaid check-links
    cd site && zola check

# ── Fuzz ──────────────────────────────────────────────────────────────────────

# Run a fuzz target (requires nightly + cargo-fuzz; e.g. `just fuzz fuzz_parse_validate`)
fuzz target:
    cd fuzz && cargo +nightly fuzz run {{ target }}


