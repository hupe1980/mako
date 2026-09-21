# Justfile for edi-energy-rs / mako
# Install just: https://github.com/casey/just
#
# Usage:
#   just          → list all recipes
#   just check    → minimum gate before every commit
#   just ci       → full CI suite (check + test + lint + deny)

# bash, not zsh: `ubuntu-latest` ships bash and not zsh, so a zsh pin makes every
# `just` recipe on CI fail before it runs a command. Recipes needing more than a
# single command carry their own `#!/usr/bin/env bash` shebang.
set shell := ["bash", "-euo", "pipefail", "-c"]

# ── Default: list all recipes ──────────────────────────────────────────────────

[private]
default:
    @just --list

# ── Core gates ────────────────────────────────────────────────────────────────

# Minimum gate before every commit: type-check all targets
check:
    cargo check --all-targets --all-features

# Run all tests
# `--all-targets` so bench targets are built and their `main` runs in test mode;
# without it a panicking Criterion bench is invisible to `just ci`.
test:
    cargo test --all-features --all-targets

# Doc examples. `--all-targets` selects every lib, bin, test, bench and example
# target and **excludes doctests** — the two flags do not compose, so `just test`
# compiles no `/// ```rust` block at all. Without this recipe a doc example is
# prose: it can name a function that no longer exists, or call one with the wrong
# arity, and nothing in the suite says so. A published crate's front page is the
# first thing a reader copies, so it is held to compiling and passing.
test-doc:
    cargo test --workspace --all-features --doc

# A demo payload is shipped API surface — run by whoever evaluates the platform
# and copied into tickets as "this is what a request looks like" — so each one
# is deserialised into the real request type. These are ordinary `#[test]`s, so
# `just test` covers them; the recipe runs them alone, in seconds, while a demo
# is being edited.
#
# Every request body demos/ posts is one the service accepts
test-demo-payloads:
    cargo test --test demo_payloads -p marktd -p vertragd -p productd -p billingd -p accountingd -p einsd

# A demo *config* is shipped surface for the same reason a demo payload is: it
# is what an operator copies. `figment` merges the TOML with the service's
# environment variables, so a key the config struct does not declare is dropped
# in silence — a setting that appears configured and does nothing. Asserts no
# unknown key; a missing one is fine, since the demo stacks supply the database
# URL and the secrets through the environment.
test-demo-configs:
    cargo test --test demo_config -p marktd -p vertragd -p productd -p billingd \
        -p accountingd -p einsd -p edmd -p outputd -p processd

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
test-db: check-sql test-edmd-db test-einsd-db test-accountingd-db test-billingd-db test-outputd-db test-vertragd-db test-productd-db test-marktd-db test-processd-db test-sperrd-db test-invoicd-db test-mabis-syncd-db test-netzbilanzd-db test-obsd-db test-outbox-db

# Every service's SQL literals prepare against its own schema.
#
# `sqlx::query` prepares at run time, so a statement Postgres will not accept
# compiles and fails on the first request that reaches it — a `melo` scoped by a
# `tenant` column it does not have, an INSERT naming nineteen columns against
# eighteen placeholders. One throwaway server answers for all fourteen services.
# Not in `just ci`: locally it starts a Docker container, and it fails rather
# than skipping when there is no daemon. CI runs it in the `check-sql` job
# instead, where `services: postgres` supplies the server and
# `MAKO_CHECK_SQL_URL` points the check at it — set that variable here too to
# prepare against a server you already have.
check-sql:
    cargo xtask check-sql

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

# Execution-queue integration tests for sperrd (ORDERS 17115/17117 ingest,
# the claim guard, and the IFTSTA 21039 retry queue) against real PostgreSQL —
# the only suite covering the §41f disconnection execution path.
test-sperrd-db:
    cargo test -p sperrd --test db_scenarios -- --include-ignored --test-threads=1

# Records integration tests for billingd.
test-billingd-db:
    cargo test -p billingd --test records_integration -- --include-ignored --test-threads=1

# Template-store integration tests for outputd.
test-outputd-db:
    cargo test -p outputd --test store_integration -- --include-ignored --test-threads=1

# Dispatch integration tests for vertragd.
test-vertragd-db:
    cargo test -p vertragd --test dispatch_integration -- --include-ignored --test-threads=1

# Catalog integration tests for productd.
test-productd-db:
    cargo test -p productd --test catalog_integration -- --include-ignored --test-threads=1

# All marktd integration suites (VersorgungsStatus, MeLo graph, ESA, registries,
# durable fan-out, MaBiS-Zählpunkt, temporal constraints, tenant isolation, and
# the 23P01 → 409 translation).
test-marktd-db:
    cargo test -p marktd \
        --test versorgung_integration --test melo_graph_integration \
        --test esa_integration --test registries_integration \
        --test fanout_durable_integration --test mabis_zp_integration \
        --test temporal_constraints_integration --test tenant_isolation_integration \
        --test overlap_is_a_client_error \
        -- --include-ignored --test-threads=1

# invoicd's receipt store — the § 147 AO / GoBD audit trail, its per-tenant
# uniqueness and the ERP re-dispatch lease.
test-invoicd-db:
    cargo test -p invoicd --test receipts_pg -- --include-ignored --test-threads=1

# mabis-syncd's submission schema — the per-run series ledger the BIKO acks
# against, and the constraints that stop a retry re-filing an acked series.
test-mabis-syncd-db:
    cargo test -p mabis-syncd --test schema_pg -- --include-ignored --test-threads=1

# netzbilanzd's invoice-draft store — numbering series, Abschlag deduction,
# the Storno chain and the tenant scoping of every read and transition.
test-netzbilanzd-db:
    cargo test -p netzbilanzd --test invoice_drafts_pg -- --include-ignored --test-threads=1

# obsd's sweeps — the deadline alert and the § 7a parity report, both of which
# run on a timer with no caller to notice a wrong predicate.
test-obsd-db:
    cargo test -p obsd --test worker_integration -- --include-ignored --test-threads=1

# The shared transactional outbox: atomicity with the caller's write, the retry
# schedule and the dead-letter hand-off. Every service that emits depends on it.
test-outbox-db:
    cargo test -p mako-service --test outbox_integration -- --include-ignored --test-threads=1

# processd's SQL suite (approval queue claim/dispatch, decision audit log).
#
# `--no-default-features --features integrated` because the default build is
# role-split and these tests drive both the NB and the LF module.
test-processd-db:
    cargo test -p processd --no-default-features --features integrated \
        --test sql_integration -- --include-ignored --test-threads=1

# Every real-PostgreSQL suite is `#[ignore]`d and named by a `test-*-db` recipe.
#
# An un-ignored container suite prints `ok` on a machine with no Docker daemon,
# having returned early from every test; an unnamed one is reached by neither
# `just ci` nor `just test-db`. Both read as coverage.
check-db-suites:
    cargo xtask check-db-suites

# A service that reads `Claims` also pins the tenant it expects.
#
# Without `ExpectedTenant`, a token signed by the same realm for a different
# operator extracts cleanly, and only the tenant condition in every Cedar rule
# stands between it and the data.
check-expected-tenant:
    cargo xtask check-expected-tenant

# Every catalogued Mindestvorlaufzeit is read by production code
check-vorlauf-consulted:
    cargo xtask check-vorlauf-consulted

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
# workspace graph from Cargo.lock.
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

# `cargo publish` verifies each crate **alone, with default features**, where
# workspace feature unification no longer supplies an optional dependency. A
# `#[cfg]` gate that slipped off its item compiles all through `just ci` and
# fails in the release job.
#
# Build every publishable crate the way crates.io will
check-publishable:
    #!/usr/bin/env bash
    set -euo pipefail
    # The publish order in .github/workflows/release.yml is the source of truth.
    crates=$(grep -oE 'cargo publish -p [a-z0-9-]+' .github/workflows/release.yml \
             | awk '{print $NF}' | sort -u)
    for c in $crates; do
        echo "==> cargo check -p $c (default features)"
        cargo check -q -p "$c"
    done
    echo "check-publishable: $(echo "$crates" | wc -w | tr -d ' ') crates build with default features"

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
        echo "==> cargo clippy -p makod --no-default-features --features $f"
        cargo clippy -p makod --no-default-features --features "$f" --all-targets -- -D warnings
    done
    # processd's role features enforce § 6a EnWG informatorische Entflechtung: an
    # nb-only build
    # must contain no LF or MSB answer path. Each profile is linted and its
    # `role_separation` suite run, because a module gated on the wrong role
    # feature ships one role's obligations inside another role's binary.
    for f in "lf-only" "nb-only" "msb-only" "integrated"; do
        echo "==> cargo clippy -p processd --no-default-features --features $f"
        cargo clippy -p processd --no-default-features --features "$f" --all-targets -- -D warnings
        cargo test   -p processd --no-default-features --features "$f" --test role_separation
    done
    # mako-pruefung carries role features of its own — the EBD trees are grouped
    # by prüfende Rolle, so an NB-only build holds no LF catalogue. The default
    # build stays clean whatever the role gates do, so each one needs its own
    # pass.
    for f in "role-nb" "role-lf" "role-msb" "role-mabis" "role-emob" \
             "role-nb,role-lf" "role-mabis,role-emob"; do
        echo "==> cargo clippy -p mako-pruefung --no-default-features --features $f"
        cargo clippy -p mako-pruefung --no-default-features --features "$f" --all-targets -- -D warnings
    done
    # agentd carries the same role flags: it is the one service that reaches all
    # the others, so a role-scoped build must exclude the other arm's
    # specialists rather than merely decline to run them (§ 6a EnWG).
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
    # The binary is wherever cargo put it. Hard-coding `./target` breaks under
    # `CARGO_TARGET_DIR`, which is the isolation a run worth reporting uses —
    # rust-analyzer writes to the default directory while this runs.
    out="${CARGO_TARGET_DIR:-target}/debug/makod"
    # Authorization is default-deny, so a bare start refuses every request and
    # `--check` says so. `--cedar-policy-dir` loads *every* `.cedar` in the
    # directory it is given, and `src/cedar/` also holds the permissive
    # `default.cedar` — so the least-privilege set is staged alone. That is the
    # policy set § 6a EnWG role separation is written for, and the one a role
    # build should be smoke-tested against.
    mkdir -p "$tmp/cedar"
    cp services/makod/src/cedar/conservative.cedar "$tmp/cedar/"
    for pair in "role-lf:LF" "role-nb:NB" "role-msb:MSB"; do
        feat="${pair%%:*}"; role="${pair##*:}"
        echo "==> $feat (party role $role)"
        cargo build -p makod --no-default-features --features "$feat"
        printf '[[party]]\nmp_id = "9900001000001"\nroles = ["%s"]\nprimary = true\n' \
            "$role" > "$tmp/makod.toml"
        "$out" --config "$tmp/makod.toml" --allow-volatile \
            --http-addr 127.0.0.1:18080 --auth-key smoke=0123456789abcdef \
            --cedar-policy-dir "$tmp/cedar" \
            --allow-no-as4-signing --check
    done

# Check the regulatory mirror against its committed manifest — no network.
#
# The PDFs behind every profile are not in the repository, but the manifest is,
# so this verifies that each recorded document is present and its bytes
# unchanged. `cargo xtask sync-regulatories` (no flag) reconciles against the
# live BDEW catalogue instead, and `--download` fetches what is missing.
regulatories:
    cargo xtask sync-regulatories --offline

# Run every shipped example to completion.
#
# `cargo check --all-targets` compiles examples but never runs them, so a driven
# workflow that the domain refuses, or a fixture missing a mandatory segment
# group, compiles clean. An example that exits non-zero is a broken promise to
# whoever pastes it, so the gate is a real run.
#
# The exit code is only half of it: an example that *prints* its findings and
# returns `Ok(())` passes a run gate while shipping a message no counterparty
# accepts. Every example asserts what it claims, and the output is scanned here
# as a second net: an example may not report a failure, a validation error or a
# panic on the happy path.
#
# An example whose *subject* is an invalid message — `05_validate` shows what a
# rejection looks like — opts out by declaring
# `_EXAMPLE_EXPECTS_VALIDATION_FINDINGS` in its source. Nothing else grants the
# exemption, so the opt-out is greppable and cannot be reached by accident.
#
# The list comes from `cargo metadata`, not from this file: a hand-kept list is
# a list a new example is forgotten from.
examples:
    #!/usr/bin/env bash
    set -uo pipefail
    fail=0
    while read -r crate ex; do
        src=$(cargo metadata --no-deps --format-version 1 | \
            python3 -c "import json,sys;m=json.load(sys.stdin);print(next(t['src_path'] for p in m['packages'] if p['name']=='$crate' for t in p['targets'] if t['name']=='$ex' and 'example' in t['kind']))")
        out=$(cargo run -q -p "$crate" --all-features --example "$ex" 2>&1)
        code=$?
        if grep -q '_EXAMPLE_EXPECTS_VALIDATION_FINDINGS' "$src"; then
            if [ $code -ne 0 ]; then
                echo "  FAIL $crate/$ex"
                tail -25 <<<"$out" | sed 's/^/       /'
                fail=1
            else
                echo "  ok   $crate/$ex  (findings expected)"
            fi
            continue
        fi
        # `FAILED`/`panicked` catch an assertion; a severity word or a
        # non-zero count catches a report an example prints instead of
        # asserting on. Neither the bracket around the severity nor the `(s)`
        # after the count is required: an example that prints `ERROR: …` or
        # `3 errors` is reporting the same failure as one that brackets and
        # parenthesises it.
        if [ $code -ne 0 ] \
           || grep -qE 'FAILED|panicked|\b(ERROR|FATAL|CRITICAL)\b|\[(MIG-|AHB-|SEM-)' <<<"$out" \
           || grep -qE '(^|[^0-9.])[1-9][0-9]* (error|finding)s?([^a-z]|$)' <<<"$out"; then
            echo "  FAIL $crate/$ex"
            tail -25 <<<"$out" | sed 's/^/       /'
            fail=1
        else
            echo "  ok   $crate/$ex"
        fi
    done < <(cargo metadata --no-deps --format-version 1 | \
        python3 -c "import json,sys; m=json.load(sys.stdin); [print(p['name'], t['name']) for p in m['packages'] for t in p['targets'] if 'example' in t['kind']]" | sort)
    exit $fail

ci: check check-fuzz test test-doc test-features examples regulatories check-publishable check-publish-order clippy clippy-roles smoke-roles fmt-check deny check-licenses no-version-alias check-bo4e-coverage check-bo4e-discriminants check-bo4e-examples check-routes check-crate-lints check-db-suites check-expected-tenant check-vorlauf-consulted check-runner-disk check-runner-routes check-wire-timestamps check-business-dates check-workflow-purity check-workflow-interpolation check-outbox-notify check-event-payloads check-cedar-roles check-tenant-predicate check-as4-controls check-citations check-rounding check-pid-coverage check-release-coverage check-dep-versions check-malo-ids check-bo4e-attributes check-request-bodies check-prompt-tools check-tool-grants check-answer-commands doc-check validate-profiles import-profiles-check validate-ebd-codes lint-makotest test-makotest

# mako proves the carrier by reading its own output back (outputd's publish
# gate), and `en16931 validate` — an independent implementation — reports the
# payload valid. Neither proves **PDF/A-3 conformance**: nothing in Rust does,
# and the XMP `document::facturx::stamp` appends lands after typst-pdf's own
# enforcement has finished. That is what veraPDF is for, and it needs a file.

# Write a stamped ZUGFeRD specimen for veraPDF / the ZUGFeRD validator
zugferd-specimen out="target/zugferd-specimen.pdf":
    #!/usr/bin/env bash
    set -euo pipefail
    # Absolute, because `cargo test` runs a test with the *package* directory as
    # its working directory — a relative path lands in services/outputd/, not
    # where the caller is standing.
    out="$(cd "$(dirname "{{ out }}")" 2>/dev/null && pwd || mkdir -p "$(dirname "{{ out }}")" && cd "$(dirname "{{ out }}")" && pwd)/$(basename "{{ out }}")"
    MAKO_ZUGFERD_OUT="$out" cargo test -p outputd --test zugferd_carrier \
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
#
# Expected notices on the *core* specimen (not the XRechnung one, which is
# clean): Mustang applies XRechnung/Peppol rules informationally to a document
# that does not claim that CIUS (BR-DE-*), and `PEPPOL-EN16931-R008` on the
# empty `ram:ApplicableHeaderTradeDelivery` is a false positive — the element
# is mandatory in the D16B XSD (omitting it fails schema validation) and
# KoSIT's own Schematron carves exactly this element out of the R008
# empty-element rule; Peppol publishes no CII Schematron at all. Settled with
# en16931-formats 0.5.0 (the writer documents the evidence). Do not "fix" it.

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
    cd makotest && .venv/bin/pip install -q --upgrade pip 'maturin>=1.9,<2.0' pytest hypothesis ruff
    cd makotest && .venv/bin/maturin develop && .venv/bin/pytest -q

# Lint and format-check the Python layer (ruff).
#
# `makotest` is a published PyPI artifact, so its Python surface gets the same
# gate the Rust side does — a consumer reads these files.
lint-makotest:
    cd makotest && test -d .venv || python3 -m venv .venv
    cd makotest && .venv/bin/pip install -q --upgrade pip ruff
    cd makotest && .venv/bin/python -m ruff check .
    cd makotest && .venv/bin/python -m ruff format --check .

# Build a release wheel (abi3, one wheel for Python ≥ 3.11).
build-makotest:
    cd makotest && .venv/bin/maturin build --release

# ── Build ─────────────────────────────────────────────────────────────────────

# Only deps needed for the Lieferbeginn smoke test; no iceberg/LanceDB.
# Expected cold build: ~8 min (debug) / ~12 min (release).
# Optional services (invoicd, netzbilanzd, obsd): build with --target <name>-runtime.

# Build the nb-stp demo images (makod, marktd, processd)
build-demo profile="dev":
    docker build --target runtime             --build-arg PROFILE={{ profile }} -t makod:dev     .
    docker build --target marktd-runtime      --build-arg PROFILE={{ profile }} -t marktd:dev    .
    docker build --target processd-runtime    --build-arg PROFILE={{ profile }} -t processd:dev  .

# `demos/eeg-billing` needs none of makod/processd: the EEG settlement path is
# marktd (Marktpartner) → edmd (Zählwerte) → einsd (Vergütung), with no MaKo
# message on it, so the images the Lieferbeginn demo builds are the wrong three.
# Expected cold build: ~8 min (debug) / ~12 min (release).

# Build the eeg-billing demo images (marktd, edmd, einsd)
build-demo-eeg profile="dev":
    docker build --target marktd-runtime      --build-arg PROFILE={{ profile }} -t marktd:dev    .
    docker build --target edmd-runtime        --build-arg PROFILE={{ profile }} -t edmd:dev      .
    docker build --target einsd-runtime       --build-arg PROFILE={{ profile }} -t einsd:dev     .

# `demos/o2c` runs the retail money path — productd → vertragd → billingd →
# outputd → accountingd. All five come out of the full `builder` stage, so the
# first build is the slow one and the rest are a copy.
# Expected cold build: ~20-45 min; a warm BuildKit cache, a few minutes.

# Build the order-to-cash demo images (productd, vertragd, billingd, outputd, accountingd)
build-demo-o2c profile="dev":
    docker build --target productd-runtime    --build-arg PROFILE={{ profile }} -t productd:dev    .
    docker build --target vertragd-runtime    --build-arg PROFILE={{ profile }} -t vertragd:dev    .
    docker build --target billingd-runtime    --build-arg PROFILE={{ profile }} -t billingd:dev    .
    docker build --target outputd-runtime     --build-arg PROFILE={{ profile }} -t outputd:dev     .
    docker build --target accountingd-runtime --build-arg PROFILE={{ profile }} -t accountingd:dev .

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

# ── Profiles ──────────────────────────────────────────────────────────────────

# Regenerate every profile from its BDEW MIG/AHB PDF (profiles/sources.json).
# Needs the document mirror (`just regulatories-download`) and poppler's
# `pdftotext`.
import-profiles:
    cargo xtask import-profiles

# One profile, e.g. `just import-profile utilmd/fv20261001`
import-profile dir:
    cargo xtask import-profiles --profile {{ dir }}

# Fail when a committed profile no longer matches its source PDF; SKIPs
# without the mirror or poppler.
import-profiles-check:
    cargo xtask import-profiles --check

# Per Prüfidentifikator, what changed between two Formatversionen of one
# message type, e.g. `just profile-diff utilmd fv20251001 fv20261001`.
# Reads the committed profiles; no document mirror needed.
profile-diff type from to:
    cargo xtask profile-diff {{ type }} {{ from }} {{ to }}

# Dump a BDEW PDF as the character grid the importer reads
pdf-grid file:
    cargo xtask pdf-grid {{ file }}

# ── Validation ────────────────────────────────────────────────────────────────

# The committed profiles are consistent: sources.json, dates and continuity
# per track, Prüfidentifikatoren, AHB rows against the MIG structure, and
# every Bedingung a status expression cites has its text.
validate-profiles:
    cargo xtask validate-profiles

# Hold the mako-pruefung Antwortcode catalogue against the published EBD PDF.
#
# Checks each `code!` entry's tree, code and Cluster against
# `regulatories/bdew-mako/Entscheidungsbaum-Diagramme_und_Codelisten_*.pdf`.
# The Cluster is the half nothing else guards: the same code means Zustimmung in
# one tree and Ablehnung in another, so a wrong one answers a confirmation with
# a refusal on the wire and every other check still passes.
#
# `regulatories/` is gitignored, so this SKIPS without the mirror or without
# poppler's `pdftotext`. Run `cargo xtask sync-regulatories --download` first.
validate-ebd-codes:
    cargo xtask validate-ebd-codes

# Verify a profile covers today's date
check-release-coverage:
    cargo xtask check-release-coverage

# Verify the rubo4e::current active-type count matches the README.md claim exactly.
check-bo4e-coverage:
    cargo xtask check-bo4e-coverage

# A BO4E `_typ` is the type's own fact — never a literal in a struct or a `json!`.
check-bo4e-discriminants:
    cargo xtask check-bo4e-discriminants

# Every BO4E example in the docs uses fields BO4E defines. An example is copied,
# and an undefined field is absorbed into `_additional` rather than refused.
check-bo4e-examples:
    cargo xtask check-bo4e-examples

# Refuse a business date read in UTC. A Lieferbeginn, a Rechnungsdatum and the
# day a Frist starts counting are German calendar dates; `now_utc().date()` and
# SQL `current_date` answer the UTC resp. session date, which is the previous
# day for an hour every night. Use `mako_fristen::heute()` and the schema's
# `heute()` function.
check-business-dates:
    cargo xtask check-business-dates

# Refuse a clock, randomness, environment or filesystem read inside an
# `impl … Workflow for …` block. A workflow is a pure function of
# (state, command): `Process::execute_with_retry` re-runs `handle` after a
# version conflict and every state is re-folded from the event log, so a value
# taken from the environment makes a retry emit a different message than the one
# already sent. The instant belongs on the command (`received_at`/`gesendet_am`),
# resolved at the makod boundary.
check-workflow-purity:
    cargo xtask check-workflow-purity

# Refuse a `${{ }}` value interpolated into a `run:` script. GitHub substitutes
# it as text before bash parses the line, so one `'` in the value ends the
# quoted string and the remainder is executed. Pass it through `env:`.
check-workflow-interpolation:
    cargo xtask check-workflow-interpolation

# Hold each durable outbox's wake-up in the database, where it belongs. Postgres
# queues a NOTIFY until the raising transaction commits, so an AFTER INSERT
# trigger cannot wake the worker onto a snapshot without the row — and reaches
# every replica, which an in-process handle never can.
check-outbox-notify:
    cargo xtask check-outbox-notify

# Refuse `deny_unknown_fields` on a workflow event payload — an event is read
# back years later, so a removed field must not make a § 147 AO record unfoldable.
check-event-payloads:
    cargo xtask check-event-payloads

# Refuse a Festlegung cited in a form it does not publish. BK6-22-024 numbers its
# operative part in Tenorziffern and carries its substance in Anlagen, so
# „BK6-22-024 § 4" names nothing that can be looked up — and an uncheckable
# citation shields whatever Frist stands beside it from review.
# Refuse an unreachable control at the AS4 boundary.
check-as4-controls:
    cargo xtask check-as4-controls

# Refuse a statement against a tenant-scoped table with no tenant bound.
check-tenant-predicate:
    cargo xtask check-tenant-predicate

# Refuse a Cedar role literal the platform never mints.
check-cedar-roles:
    cargo xtask check-cedar-roles

check-citations:
    cargo xtask check-citations

# How much of the published Prüfidentifikator inventory the AHB profiles carry,
# and whether the PID reference names all of it. `validate-profiles` compares one
# release against the previous one, so it can prove nothing was lost and is blind
# to a PID that was never imported. The inventory is extracted into
# `crates/edi-energy/profiles/pid-overview.json`
# (`cargo xtask import-pid-overview <Anwendungsübersicht.xlsx>`) so this runs
# without the source documents — a guard that only runs where they are is a
# guard that reports green from a skip.
# Refuse banker's rounding. `Decimal::round_dp` rounds half to even; German
# commercial practice, EN 16931/XRechnung and every BDEW settlement figure round
# half away from zero (DIN 1333). The modes differ only on exact midpoints — the
# values a ct price with three decimals produces — so the wrong one misstates a
# cent without failing a test written against ordinary numbers.
check-rounding:
    cargo xtask check-rounding

check-pid-coverage:
    cargo xtask check-pid-coverage

# The licence governance page tells a compliance reader what this workspace may
# depend on, and instructs a maintainer to keep it in step with `deny.toml` —
# a promise nothing enforced until this ran. Both directions: an allowed licence
# nobody recorded a review for, and a page stating a permission `cargo deny`
# refuses.
check-licenses:
    cargo xtask check-licenses

# The architecture page lists every external crate mako's domain rests on with
# the version it is pinned to. A version in prose is a claim like any other:
# check it against the manifests.
check-dep-versions:
    cargo xtask check-dep-versions

# Refuse a raw `time` value on a JSON wire: `OffsetDateTime` and `Date` derive
# `Serialize` as their component array ([y, ordinal, h, m, s, ns, ±h, ±m, ±s]),
# which is `time`'s internal layout and readable by nothing — least of all by an
# agent asked to decide whether a `deadline_at` has passed.
check-wire-timestamps:
    cargo xtask check-wire-timestamps

# Refuse a MaLo-ID literal whose BDEW check digit is wrong (metering/meterstore
# validate it at the parse, so a bad fixture is refused by the storage layer)
check-malo-ids:
    cargo xtask check-malo-ids

# Refuse a ZusatzAttribut that is not `mako:`-namespaced and registered
check-bo4e-attributes:
    cargo xtask check-bo4e-attributes

# Three rules about the moment an untrusted JSON body becomes a Rust value:
# every `Json<T>` body denies unknown fields (serde silently drops a key no
# field declares, so a customer can be created with no name), a BO4E document in
# a body is a `Bo4e<T>` rather than an ungated `serde_json::Value`, and no
# comment carries a `\uXXXX` escape.
check-request-bodies:
    cargo xtask check-request-bodies

# Refuse a specialist procedure that tells a model to call a tool the manifest
# does not grant. `check-tool-grants` validates the grant list; this validates
# the prompt. An unknown tool name is reported back to the model as a failed
# call rather than ending the run, so the step silently does not happen.
check-prompt-tools:
    cargo xtask check-prompt-tools

# Refuse axum 0.7 `/:param` route literals. Under axum 0.8 they panic while the
# router is built — i.e. at startup — so nothing in the test suite catches them.
check-routes:
    cargo xtask check-routes

# Every crate root denies `unsafe_code`. `#![deny(...)]` is a crate attribute and
# cannot be inherited: a service with a lib.rs *and* a main.rs is two crates, and
# denying it in one leaves the other outside the lint.
check-crate-lints:
    cargo xtask check-crate-lints

# No daemon claims a route `mako_service::run` already mounts.
#
# The runner merges the daemon's router onto one that already carries
# `/health/*` and `/metrics`, and `Router::merge` panics on an overlapping
# method route — while the router is assembled, so the daemon does not boot.
# `accountingd` shipped exactly that for its ledger gauges.
# Refuse a workflow job that compiles this workspace without first reclaiming
# runner disk. A hosted runner has ~14 GB free; the overflow arrives as
# `No space left on device` mid-compile or as a linker SIGBUS, and neither reads
# as a disk problem.
check-runner-disk:
    cargo xtask check-runner-disk

check-runner-routes:
    cargo xtask check-runner-routes

# `cargo publish` resolves workspace dependencies against crates.io, not the
# working tree, so a crate published before one it depends on fails outright.
# `check-publishable` sorts the list before compiling it, which leaves the order
# — the whole contract of the release job — unchecked.
check-publish-order:
    cargo xtask check-publish-order

# Every agentd tool grant must name a real MCP tool and agree with that server's
# own `read_only_hint`. A read declared mutating stops for a human on every call;
# a mutation declared read-only is dispatched unattended.
check-tool-grants:
    cargo xtask check-tool-grants

# Refuse an invoicd PID route naming a makod command that does not exist.
#
# The command name is a string on both sides. A route naming one makod never
# registered fails only when a real invoice arrives: the check runs, the verdict
# is persisted, the dispatch 404s, and the Antwortfrist expires on a process that
# looked healthy the whole way.
check-answer-commands:
    cargo run -q -p xtask -- check-answer-commands

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

# Compile every fuzz target against the current dependency graph.
#
# `fuzz/` is excluded from the workspace, so `just check` never sees it and its
# own `Cargo.toml` restates each version from `[workspace.dependencies]` by
# hand. This gate is what makes those two copies agree: a workspace-wide bump
# that the fuzz manifest does not follow fails here, at compile time, instead of
# the next time somebody reaches for the fuzzer.
#
# A `cargo check` rather than `cargo fuzz build`: the latter needs nightly and a
# full sanitiser-instrumented link of every target, which is minutes per run.
check-fuzz:
    cd fuzz && cargo check --all-targets

# Run a fuzz target (requires nightly + cargo-fuzz; e.g. `just fuzz fuzz_parse_validate`)
fuzz target:
    cd fuzz && cargo +nightly fuzz run {{ target }}


