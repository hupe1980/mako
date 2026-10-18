# Justfile for mako
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
    cargo check --workspace --all-targets --all-features

# `--all-targets` so bench targets are built and their `main` runs in test mode;
# without it a panicking Criterion bench is invisible to `just ci`. Under
# cargo-nextest each test runs in its own process; without it, `cargo test`.
# Neither runs doctests — `just test-doc` does.
#
# Run all tests
test:
    #!/usr/bin/env bash
    set -euo pipefail
    if cargo nextest --version >/dev/null 2>&1; then
        cargo nextest run --workspace --all-features --all-targets
    else
        cargo test --workspace --all-features --all-targets
    fi

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
    for c in marktd vertragd productd billingd accountingd einsd; do {{ just_executable() }} suite "$c" demo_payloads; done

# A demo *config* is shipped surface for the same reason a demo payload is: it
# is what an operator copies. `figment` merges the TOML with the service's
# environment variables, so a key the config struct does not declare is dropped
# in silence — a setting that appears configured and does nothing. Asserts no
# unknown key; a missing one is fine, since the demo stacks supply the database
# URL and the secrets through the environment.
test-demo-configs:
    for c in marktd vertragd productd billingd accountingd einsd edmd outputd processd; do \
        {{ just_executable() }} suite "$c" demo_config; done

# Run tests for a specific crate (e.g. `just test-crate mako-engine`)
test-crate crate:
    cargo test -p {{ crate }} --all-features

# A crate's integration tests are one `tests/it` binary, so a suite is a module
# filter on it.
#
# Run one integration suite of one crate (e.g. `just test-integration edmd demo_config`)
test-integration crate name:
    CARGO_ARGS="--all-features" {{ just_executable() }} suite {{ crate }} {{ name }}

# One integration suite of one crate, with test-binary arguments after `--`.
# A crate whose integration tests are one `tests/it` binary selects the suite as
# a module (`--test it -- <suite>::`); one still built a file per binary selects
# it as its own target (`--test <suite>`). Extra cargo flags go in CARGO_ARGS.
[private]
suite crate name *test_args:
    #!/usr/bin/env bash
    set -euo pipefail
    dir="services/{{ crate }}"; [ -d "crates/{{ crate }}" ] && dir="crates/{{ crate }}"
    if [ -f "$dir/tests/it/main.rs" ]; then
        cargo test -p {{ crate }} ${CARGO_ARGS:-} --test it -- {{ name }}:: {{ test_args }}
    else
        cargo test -p {{ crate }} ${CARGO_ARGS:-} --test {{ name }} -- {{ test_args }}
    fi

# ── Database integration tests ───────────────────────────────────────────────
# Every suite below self-manages its PostgreSQL via testcontainers: a throwaway
# container is started in-process (once per test binary) and torn down by the
# testcontainers reaper afterwards. The only requirement is a running Docker
# daemon — no manual `docker run`, no fixed host ports, no `*_DATABASE_URL` env
# vars. Without Docker the `#[ignore]`d tests skip gracefully.

# All database integration suites in one go.
test-db: check-sql test-edmd-db test-einsd-db test-accountingd-db test-billingd-db test-outputd-db test-vertragd-db test-productd-db test-marktd-db test-processd-db test-invoicd-db test-netzbilanzd-db test-outbox-db test-policy-db

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
    {{ just_executable() }} suite edmd meterstore_integration --include-ignored --test-threads=1
    {{ just_executable() }} suite edmd router_builds --include-ignored

# Handler + SQL integration tests for einsd (EEG settlement).
test-einsd-db:
    {{ just_executable() }} suite einsd settlement_integration --include-ignored --test-threads=1

# Ledger integration tests for accountingd — the doubleentry-backed
# Massenkontokorrent (idempotency, netting, reconcile, period seal, Merkle
# inclusion proof) against real PostgreSQL.
test-accountingd-db:
    {{ just_executable() }} suite accountingd db_scenarios --include-ignored --test-threads=1
    {{ just_executable() }} suite accountingd router_builds --include-ignored

# Records integration tests for billingd.
test-billingd-db:
    {{ just_executable() }} suite billingd records_integration --include-ignored --test-threads=1

# Template-store integration tests for outputd.
test-outputd-db:
    {{ just_executable() }} suite outputd store_integration --include-ignored --test-threads=1

# Dispatch integration tests for vertragd.
test-vertragd-db:
    {{ just_executable() }} suite vertragd dispatch_integration --include-ignored --test-threads=1

# Catalog integration tests for productd.
test-productd-db:
    {{ just_executable() }} suite productd catalog_integration --include-ignored --test-threads=1

# All marktd integration suites (VersorgungsStatus, MeLo graph, ESA, registries,
# durable fan-out, MaBiS-Zählpunkt, temporal constraints, tenant isolation, and
# the 23P01 → 409 translation).
test-marktd-db:
    cargo test -p marktd --test it -- --include-ignored --test-threads=1 \
        versorgung_integration:: melo_graph_integration:: \
        esa_integration:: registries_integration:: \
        fanout_durable_integration:: mabis_zp_integration:: \
        temporal_constraints_integration:: tenant_isolation_integration:: \
        overlap_is_a_client_error::

# invoicd's receipt store — the § 147 AO / GoBD audit trail, its per-tenant
# uniqueness and the ERP re-dispatch lease.
test-invoicd-db:
    {{ just_executable() }} suite invoicd receipts_pg --include-ignored --test-threads=1

# netzbilanzd's invoice-draft store — numbering series, Abschlag deduction,
# the Storno chain and the tenant scoping of every read and transition — and
# its MaBiS submission schema: the per-run series ledger the BIKO acks against,
# and the constraints that stop a retry re-filing an acked series.
test-netzbilanzd-db:
    cargo test -p netzbilanzd --test it -- --include-ignored --test-threads=1 \
        invoice_drafts_pg:: schema_pg::

# The shared transactional outbox: atomicity with the caller's write, the retry
# schedule and the dead-letter hand-off. Every service that emits depends on it.
test-outbox-db:
    {{ just_executable() }} suite mako-service outbox_integration --include-ignored --test-threads=1

# The runner's `service_policy_log`: a changed operator setting appends a row,
# an unchanged boot appends none.
test-policy-db:
    {{ just_executable() }} suite mako-service policy_integration --include-ignored --test-threads=1

# processd's SQL suites: the approval queue claim/dispatch and decision audit
# log; the process read model's sweeps (the deadline alert and the § 7a parity
# report, both on a timer with no caller to notice a wrong predicate), its
# rebuild from marktd's replay log and the one webhook's fan-out; and the
# Sperrauftrag queue (ORDERS 17115/17117 ingest, the claim guard, the IFTSTA
# 21039 retry queue) — the only suite covering the §41f disconnection path.
#
# `--no-default-features --features integrated` because these tests drive the
# NB, the LF and the read-model module together.
test-processd-db:
    cargo test -p processd --no-default-features --features integrated --test it -- \
        --include-ignored --test-threads=1 \
        sql_integration:: worker_integration:: rebuild:: webhook_fanout:: db_scenarios::

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

# No BEHG price table carries a year no enacted statute prices (from 2027)
check-behg-prices:
    cargo xtask check-behg-prices

# Local, not in `ci`: every spec's Status agrees with its tasks (or is Parked,
# with nothing open and the roadmap's parked list naming it), and specs/ and
# concepts/ROADMAP.md name each other. Both trees are untracked, so CI skips it.
specs:
    cargo xtask check-specs

# Lint with warnings as errors
clippy:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

# Format all code
fmt:
    cargo fmt --all

# Check formatting without making changes (for CI)
fmt-check:
    cargo fmt --all -- --check

# Dependency audit: licenses, bans, sources and advisories over the workspace
# graph with every feature on — an optional dependency is still in a build that
# turns its feature on. `fuzz/` is outside the workspace and carries its own
# `Cargo.lock`, so its advisories are checked by a run of their own under the
# same `deny.toml` (cargo-deny finds it in the parent directory); the VEX
# statements cover both locks (`check-vex`).
deny:
    cargo deny --all-features check
    cargo deny --manifest-path fuzz/Cargo.toml --all-features check advisories

# Guard: no hardcoded rubo4e schema-version aliases in business logic.
# Domain code must use rubo4e::current:: or rubo4e::identifiers:: — never
# rubo4e::v202607:: or any other pinned version path.
no-version-alias:
    @! grep -rn 'rubo4e::v[0-9]' crates/ services/ --include='*.rs' \
        || (echo "ERROR: hardcoded rubo4e version alias found — use rubo4e::current:: instead" && exit 1)

# Build and check rustdoc (--all-features, warnings as errors)
doc-check:
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features

# `just test` only runs --all-features, which cannot catch a module whose
# `#[cfg(feature = ...)]` gate is missing or misplaced — that only shows up when
# the feature is off. The feature list is read from each manifest by
# `cargo hack`, so a new feature is built with no edit here or in CI. Each crate
# is also built with its default features, the build `cargo publish` verifies.
# CI shards the runs with `just check-features --partition M/N`.
#
# Build every feature of every crate on its own (needs cargo-hack)
check-features *args:
    cargo hack check --each-feature --workspace {{ args }}

# A dependency no source names still compiles and links, and still carries its
# advisories; `[package.metadata.cargo-machete] ignored` names, with a reason,
# the ones used only through a macro or a feature.
#
# Refuse a dependency no source file uses (needs cargo-machete)
machete:
    cargo machete

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

# Every crate under crates/ is published; a consumer finds a breaking change
# when their build fails unless this does first. The baseline is the newest
# `v*` tag before `HEAD`, built from git, so no registry download is needed; a
# change the current version number does not admit (a minor bump, under 0.x)
# fails. It runs in the release workflow before anything is published, not in
# `ci`: between releases the API may break freely, and the bump that admits a
# break is made when the release is cut.
# The crate set is the release workflow's publish list, as for
# check-publishable. Needs cargo-semver-checks; name crates to check a subset.
#
# Refuse a public-API break the version number does not admit
semver *crates:
    #!/usr/bin/env bash
    set -euo pipefail
    base="$(git describe --tags --abbrev=0 --match 'v*' HEAD^)"
    crates="{{ crates }}"
    if [ -z "$crates" ]; then
        crates=$(grep -oE 'cargo publish -p [a-z0-9-]+' .github/workflows/release.yml \
                 | awk '{print $NF}' | sort -u)
    fi
    args=()
    for c in $crates; do args+=(-p "$c"); done
    echo "semver: ${#args[@]} flags against $base"
    cargo semver-checks check-release --baseline-rev "$base" "${args[@]}"

# A report, not a gate: a pin behind the registry is often a decision, and
# `just ci` must not need the network. Pass `--recorded <file>` to read a saved
# crates.io response instead of querying it.
#
# Report each direct dependency's pin against crates.io (network)
dep-age *args:
    cargo xtask dep-age {{ args }}

# Full CI suite (minimum gate + tests + quality + release-lifecycle checks)
# Lint every documented role-scoped build profile.
#
# `clippy` above runs --all-features, which turns on *every* role at once. That
# is exactly the configuration in which role gating cannot be wrong: each
# `#[cfg(feature = "role-…")]` is satisfied, so nothing is excluded and no
# unused import appears. A role-scoped build is the opposite case and the one
# operators actually deploy, so it needs its own pass. makod has no role
# features — it selects its modules from `[deployment] rollen` at runtime, and
# `smoke-roles` boots each declaration.
clippy-roles:
    #!/usr/bin/env bash
    set -euo pipefail
    # processd's role features decide which answer paths a build carries: an
    # nb-only build cannot answer as an LF or an MSB, so a misconfiguration fails
    # closed. That is not an unbundling control — no § 6a EnWG duty is
    # discharged by a build flag. Each profile is linted and its
    # `role_separation` suite run, because a module gated on the wrong role
    # feature ships one role's obligations inside another role's binary.
    for f in "lf-only" "nb-only" "msb-only" "integrated"; do
        echo "==> cargo clippy -p processd --no-default-features --features $f"
        cargo clippy -p processd --no-default-features --features "$f" --all-targets -- -D warnings
        CARGO_ARGS="--no-default-features --features $f" {{ just_executable() }} suite processd role_separation
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
    # specialists rather than merely decline to run them: it cannot answer as a
    # role it was not built for. The confidentiality duty is Cedar's, at runtime.
    for f in "role-lf" "role-nb" "role-msb"; do
        echo "==> cargo clippy -p agentd --features $f"
        cargo clippy -p agentd --features "$f" --all-targets -- -D warnings
        cargo test   -p agentd --features "$f" --lib role_scoped
    done

# Boot makod once per deployment role and assert each passes --check.
#
# The role → module table is pinned by a unit test; this proves each
# declaration *starts*. Startup runs assertions that only fire at runtime —
# adapter coverage and dispatch completeness both panic — so a role whose
# module set registers a PID without its workflow lints clean and dies on boot.
# One build serves every role: the declaration is `[deployment] rollen`.
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
    # `default.cedar` — so the least-privilege set is staged alone, the one a
    # role-limited deployment should be smoke-tested against.
    mkdir -p "$tmp/cedar"
    cp services/makod/src/cedar/conservative.cedar "$tmp/cedar/"
    cargo build -p makod
    # rollen token : [[party]] role : MP-ID (Strom 99…, Gas 98…)
    for row in "nb-strom:NB:9900001000001" "lf-strom:LF:9900001000001" \
               "msb-strom:MSB:9900001000001" "esa-strom:ESA:9900001000001" \
               "nb-gas:GNB:9800001000001" "lf-gas:LFG:9800001000001" \
               "msb-gas:GMSB:9800001000001"; do
        IFS=: read -r rolle role mp_id <<< "$row"
        echo "==> rollen = [\"$rolle\"] (party role $role)"
        printf '[deployment]\nrollen = ["%s"]\n\n[[party]]\nmp_id = "%s"\nroles = ["%s"]\nprimary = true\n' \
            "$rolle" "$mp_id" "$role" > "$tmp/makod.toml"
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

# Warnings are denied exactly as CI denies them (`RUSTFLAGS` in
# .github/workflows/ci.yml), so a warning that only one feature set raises fails
# here and not first on CI. An explicit `RUSTFLAGS` in the environment wins.
#
# The gate: every check CI runs, warnings denied
ci:
    RUSTFLAGS="${RUSTFLAGS:--D warnings}" {{ just_executable() }} _ci

[private]
_ci: check check-fuzz test test-doc check-features machete examples regulatories check-publishable check-publish-order clippy clippy-roles smoke-roles fmt-check deny check-licenses no-version-alias check-bo4e-coverage check-bo4e-discriminants check-bo4e-examples check-routes check-crate-lints check-db-suites check-expected-tenant check-vorlauf-consulted check-behg-prices check-runner-disk check-runner-routes check-wire-timestamps check-business-dates check-workflow-purity check-workflow-interpolation check-outbox-notify check-atomic-spawn check-command-constructors check-approval-commands check-occupancy check-event-payloads check-cedar-roles check-tenant-predicate check-as4-controls check-citations check-known-wrong check-policy-settings check-frist-literals check-controls-consulted check-rounding check-pid-coverage check-release-coverage check-dep-versions check-action-pins check-ci-guards check-clippy-bans check-vex check-malo-ids check-bo4e-attributes check-request-bodies check-thin-main check-error-tuples check-prompt-tools check-tool-grants check-answer-commands doc-check validate-profiles import-profiles-check validate-ebd-codes lint-makotest test-makotest

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
    MAKO_ZUGFERD_OUT="$out" cargo test -p outputd --test it \
        -- --ignored --nocapture zugferd_carrier::write_specimen_for_external_validators
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
# In-repo checks cannot replace these: an XMP defect can pass every in-repo
# layer and fail only veraPDF.
#
# Expected notices on the *core* specimen (not the XRechnung one, which is
# clean): Mustang applies XRechnung/Peppol rules informationally to a document
# that does not claim that CIUS (BR-DE-*), and `PEPPOL-EN16931-R008` on the
# empty `ram:ApplicableHeaderTradeDelivery` is a false positive — the element
# is mandatory in the D16B XSD (omitting it fails schema validation) and
# KoSIT's own Schematron carves exactly this element out of the R008
# empty-element rule; Peppol publishes no CII Schematron at all. Do not "fix" it.

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
# Optional services (invoicd, netzbilanzd): build with --target <name>-runtime.

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
# PostgreSQL runs in Docker; the services run as cargo processes, each with its
# config from dev/config/<service>.toml.
#
#   just dev marktd               # starts PostgreSQL if needed, then marktd
#   just dev processd             # a second service, in a second terminal
#   just infra-down               # stop PostgreSQL

# Start infrastructure (postgres only) and wait until it accepts connections
infra-up:
    docker compose -f dev/docker-compose.yml up -d --wait
    @echo "Postgres ready on :5432 — connection strings in dev/docker-compose.yml"

# Stop infrastructure and remove containers (volumes are preserved)
infra-down:
    docker compose -f dev/docker-compose.yml down

# Stop infrastructure and delete all volumes and dev/.data (full reset)
infra-reset:
    docker compose -f dev/docker-compose.yml down -v
    rm -rf dev/.data

# Restarts on change with watchexec or bacon when one is installed, else runs
# once with plain `cargo run`.
#
# Run one service against the dev stack with dev/config/<service>.toml (just dev marktd)
dev service:
    #!/usr/bin/env bash
    set -euo pipefail
    cfg="dev/config/{{ service }}.toml"
    if [ ! -f "$cfg" ]; then
        echo "no $cfg — one of: $(ls dev/config | sed 's/\.toml$//' | tr '\n' ' ')" >&2
        exit 1
    fi
    # makod keeps its event store in memory in dev; every other service has a
    # database in the compose stack.
    if [ "{{ service }}" != makod ]; then
        docker compose -f dev/docker-compose.yml up -d --wait >/dev/null
    fi
    . dev/env.sh {{ service }}
    if command -v watchexec >/dev/null; then
        exec watchexec --restart --exts rs,toml -- cargo run -p {{ service }}
    elif command -v bacon >/dev/null; then
        exec bacon run -- -p {{ service }}
    else
        exec cargo run -p {{ service }}
    fi

# Run a single service once against the dev stack (no watch).
# Example: just run marktd
run service:
    #!/usr/bin/env bash
    set -euo pipefail
    . dev/env.sh {{ service }}
    exec cargo run -p {{ service }}

# The services are read from the directory, so a new config is covered as soon
# as it exists.
#
# Every dev/config/<service>.toml loads through its service's config type
test-dev-configs:
    #!/usr/bin/env bash
    set -euo pipefail
    for f in dev/config/*.toml; do
        {{ just_executable() }} suite "$(basename "$f" .toml)" dev_config
    done

# What `just dev` promises a fresh clone. Needs Docker; with no argument it
# boots every service under dev/config/.
#
# Boot services with their dev configs and wait for /health/ready (just smoke-dev marktd)
smoke-dev *services:
    dev/smoke.sh {{ services }}

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

# A correlation key is written in the batch of its events
check-atomic-spawn:
    cargo xtask check-atomic-spawn

# Every workflow command variant is constructed by shipped code
check-command-constructors:
    cargo xtask check-command-constructors

# Every command processd can queue for an operator has an answer contract
check-approval-commands:
    cargo xtask check-approval-commands

# Ingest arms deciding resume-vs-spawn on index presence alone only become fewer
check-occupancy:
    cargo xtask check-occupancy

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

# Every operator policy setting: each option sourced (and passing the citation
# and known-wrong rules), its docs anchor resolving, the generated
# operator-policy page in sync, each key read in production code — and no
# service config defaulting an enum outside a policy.
check-policy-settings:
    cargo xtask check-policy-settings

# Refuse a claim AGENTS.md lists as known-wrong, and a tracked file that points
# into the untracked roadmap or specifications.
check-known-wrong:
    cargo xtask check-known-wrong

# Refuse a Werktage or clock-time literal in a string that also names a
# Festlegung, an AHB or an AWH — the Frist renders from mako_fristen.
check-frist-literals:
    cargo xtask check-frist-literals

# Every declared control — Frist constants, Meldepflicht rows, credential
# fields, role columns, feature-gated security modules — is consulted, or
# listed with a reason and the milestone that removes it.
check-controls-consulted:
    cargo xtask check-controls-consulted

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
# depend on, and instructs a maintainer to keep it in step with `deny.toml`.
# Both directions: an allowed licence
# nobody recorded a review for, and a page stating a permission `cargo deny`
# refuses.
check-licenses:
    cargo xtask check-licenses

# The architecture page lists every external crate mako's domain rests on with
# the version it is pinned to. A version in prose is a claim like any other:
# check it against the manifests.
check-dep-versions:
    cargo xtask check-dep-versions

# Refuse an action not pinned by commit SHA, a `FROM` without a digest, workflow-wide write permissions, and an unpinned pip/npm install
check-action-pins:
    cargo xtask check-action-pins

# Refuse a `just ci` xtask recipe no `ci.yml` job runs — the pull-request gate
# runs the list `evidence-pack --xtask-recipes` derives from `_ci`, never a copy
check-ci-guards:
    cargo xtask check-ci-guards

# A ban whose path stopped resolving is ignored by clippy in silence.
#
# Every clippy.toml ban fires on its planted use in xtask/clippy-fixtures
check-clippy-bans:
    cargo xtask check-clippy-bans

# Hold release/vex.json's OpenVEX statements equal to deny.toml's ignored advisories
check-vex:
    cargo xtask check-vex

# Assemble the release evidence pack; a section without its input reads `not measured`
evidence-pack inputs out:
    cargo xtask evidence-pack --inputs {{inputs}} --out {{out}}

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

# Rules about the moment an untrusted JSON body becomes a Rust value:
# `mako_service::Json` refuses an undeclared key at runtime, so the guard names
# what it cannot see — a flattened field, an untagged or internally tagged enum,
# a `Lenient` or `axum::Json` extraction — unless listed with a reason; a BO4E
# document in a body is a `Bo4e<T>` rather than an ungated `serde_json::Value`;
# and no comment carries a `\uXXXX` escape.
check-request-bodies:
    cargo xtask check-request-bodies

# A service is a library: `main.rs` is `mako_service::run::<D>().await` and
# nothing else, so a test can build the router the binary ships. makod, which
# drives its own `main`, is the one exemption the guard names.
check-thin-main:
    cargo xtask check-thin-main

# A handler fails with an `ApiError`, never a hand-built `(StatusCode, …)`
# tuple — that is a body shape the caller cannot rely on and, for a database
# error, the schema's own text handed to the caller. Absolute: any tuple fails.
check-error-tuples:
    cargo xtask check-error-tuples

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
    cargo xtask check-answer-commands

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


