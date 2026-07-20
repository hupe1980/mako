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

# Run the edmd database-backed tests against a throwaway PostgreSQL
test-edmd-db:
    #!/usr/bin/env bash
    set -euo pipefail
    docker rm -f edmd-test >/dev/null 2>&1 || true
    docker run -d --name edmd-test -e POSTGRES_PASSWORD=test -e POSTGRES_DB=edmd \
        -p 55432:5432 postgres:17-alpine >/dev/null
    trap 'docker rm -f edmd-test >/dev/null 2>&1 || true' EXIT
    for _ in $(seq 1 30); do
        docker exec edmd-test pg_isready -U postgres >/dev/null 2>&1 && break
        sleep 1
    done
    EDMD_TEST_DATABASE_URL="postgres://postgres:test@localhost:55432/edmd" \
        cargo test -p edmd --test ingest_integration -- --include-ignored --test-threads=1

# Integration tests for einsd against a throwaway PostgreSQL.
test-einsd-db:
    #!/usr/bin/env bash
    set -euo pipefail
    docker rm -f einsd-test >/dev/null 2>&1 || true
    docker run -d --name einsd-test -e POSTGRES_PASSWORD=test -e POSTGRES_DB=einsd \
        -p 55434:5432 postgres:17-alpine >/dev/null
    trap 'docker rm -f einsd-test >/dev/null 2>&1 || true' EXIT
    for _ in $(seq 1 30); do
        docker exec einsd-test pg_isready -U postgres >/dev/null 2>&1 && break
        sleep 1
    done
    EINSD_TEST_DATABASE_URL="postgres://postgres:test@localhost:55434/einsd" \
        cargo test -p einsd --test settlement_integration -- --include-ignored --test-threads=1

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

# Full CI suite (minimum gate + tests + quality + release-lifecycle checks)
ci: check test clippy fmt-check deny no-version-alias doc-check codegen-check validate-profiles-strict validate-pruefids-strict-ci

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

# ── Fuzz ──────────────────────────────────────────────────────────────────────

# Run a fuzz target (requires nightly + cargo-fuzz; e.g. `just fuzz fuzz_parse_validate`)
fuzz target:
    cd fuzz && cargo +nightly fuzz run {{ target }}


