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

# ── Default: list all recipes ─────────────────────────────────────────────────

[private]
default:
    @just --list

# ── Start ─────────────────────────────────────────────────────────────────────
#
# `just dev` is the estate on this machine, `just demo` the released images in
# Docker; both run Stadtwerke Musterstadt, which `just seed` fills through the
# services' public APIs. The configurations are deploy/demo/<service>.toml, the
# ports deploy/ports.toml.

# PostgreSQL in Docker, the fourteen services built by cargo and run here —
# their logs in this terminal, one colour each, and in target/dev/<service>.log
# — the Desk under Vite against them, and the seeded Stadtwerk. Checks the
# machine first. Ctrl-C stops the services; the data stays until `just
# dev-down`.
#
# The estate on this machine, seeded, with the Desk
[group('start')]
dev *args:
    deploy/dev/up.sh {{ args }}

# Stop `just dev`'s PostgreSQL and forget its data
[group('start')]
dev-down:
    docker compose -p mako-dev -f deploy/dev/compose.yaml down -v
    rm -rf deploy/dev/.data

# The Desk on its recorded answers (desk/fixtures) — no Rust, no Docker
[group('start')]
dev-ui:
    #!/usr/bin/env bash
    set -euo pipefail
    [ -d desk/node_modules ] || (cd desk && npm ci --no-audit --no-fund)
    cd desk && npm run dev:fixtures

# Each miss is named with the command that fixes it; then one line per daemon's
# configuration, ready or blocked.
#
# Can this machine run `just dev`?
[group('start')]
doctor:
    deploy/dev/doctor.sh
    {{ just_executable() }} setup --tabelle

# The deployment checklist: every demo daemon's `--check-config --json`
# (deploy/demo/<service>.toml), folded across daemons — identities,
# certificates, Bilanzkreise, access, reachability, format coverage. Exits
# non-zero while anything is red. `just setup --url http://makod:8080
# --token-env MAKOD_TOKEN` asks a running makod's
# /admin/einrichtung/deployment instead; `--tabelle` prints one line per
# daemon; `--vorschlag antworten.toml [--schreiben dir]` prints the
# configuration the setup interview's answers propose, as the Desk shows it.
#
# The deployment checklist in the terminal
[group('start')]
setup *args:
    #!/usr/bin/env bash
    set -euo pipefail
    case " {{ args }} " in
        *" --url "*) exec cargo xtask setup {{ args }} ;;
    esac
    services=$(ls deploy/demo/*.toml | xargs -n1 basename | sed 's/\.toml$//')
    cargo build --quiet $(for s in $services; do printf -- '-p %s ' "$s"; done)
    out="$(mktemp -d)"
    trap 'rm -rf "$out"' EXIT
    target="${CARGO_TARGET_DIR:-target}"
    for s in $services; do
        var="$(echo "$s" | tr 'a-z-' 'A-Z_')_CONFIG"
        ( export "$var=$PWD/deploy/demo/$s.toml"; "$target/debug/$s" --check-config --json >"$out/$s.json" 2>"$out/$s.err" ) || true
    done
    cargo xtask setup --berichte "$out" {{ args }}

# From an empty makod to a green check and a passing Probelauf for the
# fixture interview „Lieferant Strom und Gas“, each step timed; fails over 30
# minutes. No Docker, no database.
#
# The first run, scripted and timed
[group('start')]
erstlauf:
    cargo build --quiet -p makod
    cargo xtask erstlauf

# Fill the running estate with Stadtwerke Musterstadt (idempotent); `--verify`
# then asserts the outcomes and that every Desk view has content
#
# Seed Stadtwerke Musterstadt through the public APIs; --verify asserts the outcomes
[group('start')]
seed *args:
    cargo run -q -p mako-seed -- seed {{ args }}

# The released images of every service and the Desk in Docker, a seed
# container, one port: http://localhost:8000. `just demo --build` builds the
# images from this tree instead of pulling them. No Rust needed.
#
# The demo: the released images, seeded, behind the Desk
[group('start')]
demo *args:
    deploy/demo/up.sh {{ args }}

# Stop the demo and forget its data
[group('start')]
demo-down:
    docker compose -p mako-demo down -v

# Subscribes to marktd's fan-out and prints each delivery verified; Ctrl-C
# removes the subscription. Under `just demo` it reaches marktd through the
# Desk's edge and marktd reaches it as host.docker.internal; under `just dev`
# marktd is on :8180 and reaches it by this machine's host name.
#
# Every CloudEvent of the running estate, as it arrives
[group('start')]
listen *args:
    #!/usr/bin/env bash
    set -euo pipefail
    extra=()
    if docker compose -p mako-demo ps -q marktd 2>/dev/null | grep -q .; then
        extra=(--marktd "http://localhost:${MAKO_PORT:-8000}/svc/marktd" --host host.docker.internal)
    fi
    cargo run -q -p mako-seed -- listen ${extra[@]+"${extra[@]}"} {{ args }}

# Prints a token for a persona of the demo sign-in — nb, lf, admin or agent —
# for `curl -H "Authorization: Bearer $TOKEN" …`. Accepted only by a service
# whose `[oidc]` says `mode = "demo"`:
#
#     TOKEN=$(just token lf)
#
# A demo token for a persona
[group('start')]
token persona="admin":
    @python3 -c 'import json,sys; p={x["id"]:x for x in json.load(open("deploy/demo/personas.json"))["personas"]}; sys.exit("unknown persona {!r} — one of: {}".format(sys.argv[1], ", ".join(p))) if sys.argv[1] not in p else print(p[sys.argv[1]]["token"])' {{ persona }}

# ── The gate ──────────────────────────────────────────────────────────────────
#
# `just check` before every commit, `just ci` before every push: the whole suite
# and every guard of the Guards section.

# Minimum gate before every commit: type-check all targets
[group('gate')]
check:
    cargo check --workspace --all-targets --all-features

# Warnings are denied exactly as CI denies them (`RUSTFLAGS` in
# .github/workflows/ci.yml), so a warning that only one feature set raises fails
# here and not first on CI. An explicit `RUSTFLAGS` in the environment wins.
#
# The gate: every check CI runs, warnings denied
[group('gate')]
ci:
    RUSTFLAGS="${RUSTFLAGS:--D warnings}" {{ just_executable() }} _ci

[private]
[group('gate')]
_ci: check check-fuzz test test-doc check-features machete examples regulatories check-publishable check-publish-order clippy clippy-roles smoke-roles fmt-check deny no-version-alias check-bo4e-coverage check-bo4e-discriminants check-bo4e-examples check-routes check-crate-lints check-db-suites check-expected-tenant check-vorlauf-consulted check-behg-prices check-referenzreihen check-runner-disk check-docker-build check-runner-routes check-wire-timestamps check-business-dates check-workflow-purity check-workflow-interpolation check-outbox-notify check-openapi-routes api-catalog-check event-catalog-check check-atomic-spawn check-command-constructors check-approval-commands check-occupancy check-event-payloads check-event-verlauf check-cedar-roles check-tenant-predicate check-as4-controls check-citations check-known-wrong check-quotations check-policy-settings check-frist-literals check-controls-consulted check-rounding check-pid-coverage check-release-coverage check-action-pins check-ci-guards check-clippy-bans check-vex check-malo-ids check-bo4e-attributes check-ledger-drafts check-dynamic-sql check-request-bodies check-thin-main check-config-findings check-sts-codes check-desk desk-check check-error-tuples check-process-verdict check-event-durability check-prompt-tools check-tool-grants check-eval-cases check-translations check-mcp-surface check-answer-commands doc-check validate-profiles import-profiles-check validate-ebd-codes lint-makotest test-makotest

# `--all-targets` so bench targets are built and their `main` runs in test mode;
# without it a panicking Criterion bench is invisible to `just ci`. Under
# cargo-nextest each test runs in its own process; without it, `cargo test`.
# Neither runs doctests — `just test-doc` does.
#
# Run all tests
[group('gate')]
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
#
# Run the doctests, which nextest and --all-targets skip
[group('gate')]
test-doc:
    cargo test --workspace --all-features --doc

# Lint with warnings as errors
[group('gate')]
clippy:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

# Lint every documented role-scoped build profile.
#
# `clippy` above runs --all-features, which turns on *every* role at once. That
# is exactly the configuration in which role gating cannot be wrong: each
# `#[cfg(feature = "role-…")]` is satisfied, so nothing is excluded and no
# unused import appears. A role-scoped build is the opposite case and the one
# operators actually deploy, so it needs its own pass. makod has no role
# features — it selects its modules from `[deployment] rollen` at runtime, and
# `smoke-roles` boots each declaration.
#
# Lint every role-scoped build profile, warnings denied
[group('gate')]
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
#
# Boot makod once per deployment role and check each passes --check
[group('gate')]
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
               "msb-gas:GMSB:9800001000001" "bkv-strom:BKV:9900001000001" \
               "eiv-strom:EIV:9900001000001"; do
        IFS=: read -r rolle role mp_id <<< "$row"
        echo "==> rollen = [\"$rolle\"] (party role $role)"
        printf '[deployment]\nrollen = ["%s"]\n\n[[party]]\nmp_id = "%s"\nroles = ["%s"]\nprimary = true\n' \
            "$rolle" "$mp_id" "$role" > "$tmp/makod.toml"
        # The EIV's answer window has no published value and no default.
        if [ "$rolle" = "eiv-strom" ]; then
            printf '\n[redispatch_eiv]\nabruf_antwortfenster_minuten = 10\n' >> "$tmp/makod.toml"
        fi
        "$out" --config "$tmp/makod.toml" --allow-volatile \
            --http-addr 127.0.0.1:18080 --auth-key smoke=0123456789abcdef \
            --cedar-policy-dir "$tmp/cedar" \
            --allow-no-as4-signing --check
    done

# Format all code
[group('gate')]
fmt:
    cargo fmt --all

# Check formatting without making changes (for CI)
[group('gate')]
fmt-check:
    cargo fmt --all -- --check

# Dependency audit: licenses, bans, sources and advisories over the workspace
# graph with every feature on — an optional dependency is still in a build that
# turns its feature on. `fuzz/` is outside the workspace and carries its own
# `Cargo.lock`, so its advisories are checked by a run of their own under the
# same `deny.toml` (cargo-deny finds it in the parent directory); the VEX
# statements cover both locks (`check-vex`).
#
# Licences, bans, advisories and sources of both lockfiles (cargo-deny)
[group('gate')]
deny:
    cargo deny --all-features check
    cargo deny --manifest-path fuzz/Cargo.toml --all-features check advisories

# A dependency no source names still compiles and links, and still carries its
# advisories; `[package.metadata.cargo-machete] ignored` names, with a reason,
# the ones used only through a macro or a feature.
#
# Refuse a dependency no source file uses (needs cargo-machete)
[group('gate')]
machete:
    cargo machete

# Build and check rustdoc (--all-features, warnings as errors)
[group('gate')]
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
[group('gate')]
check-features *args:
    cargo hack check --each-feature --workspace {{ args }}

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
#
# Run every shipped example to completion
[group('gate')]
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

# Check the regulatory mirror against its committed manifest — no network.
#
# The PDFs behind every profile are not in the repository, but the manifest is,
# so this verifies that each recorded document is present and its bytes
# unchanged. `cargo xtask sync-regulatories` (no flag) reconciles against the
# live BDEW catalogue instead, and `--download` fetches what is missing.
#
# Check the regulatory mirror against its committed manifest, no network
[group('gate')]
regulatories:
    cargo xtask sync-regulatories --offline

# Guard: no hardcoded rubo4e schema-version aliases in business logic.
# Domain code must use rubo4e::current:: or rubo4e::identifiers:: — never
# rubo4e::v202607:: or any other pinned version path.
#
# Refuse a pinned rubo4e::v… path; domain code uses rubo4e::current
[group('gate')]
no-version-alias:
    @! grep -rn 'rubo4e::v[0-9]' crates/ services/ --include='*.rs' \
        || (echo "ERROR: hardcoded rubo4e version alias found — use rubo4e::current:: instead" && exit 1)

# Local, not in `ci`: every spec's Status agrees with its tasks (or is Parked,
# with nothing open and the roadmap's parked list naming it), and specs/ and
# concepts/ROADMAP.md name each other. Both trees are untracked, so CI skips it.
#
# Local only: every spec's Status agrees with its tasks and the roadmap
[group('gate')]
specs:
    cargo xtask check-specs

# ── Tests ─────────────────────────────────────────────────────────────────────

# Run tests for a specific crate (e.g. `just test-crate mako-engine`)
[group('tests')]
test-crate crate:
    cargo test -p {{ crate }} --all-features

# A crate's integration tests are one `tests/it` binary, so a suite is a module
# filter on it.
#
# Run one integration suite of one crate (e.g. `just test-integration edmd demo_config`)
[group('tests')]
test-integration crate name:
    CARGO_ARGS="--all-features" {{ just_executable() }} suite {{ crate }} {{ name }}

# One integration suite of one crate, with test-binary arguments after `--`.
# A crate whose integration tests are one `tests/it` binary selects the suite as
# a module (`--test it -- <suite>::`); one still built a file per binary selects
# it as its own target (`--test <suite>`). Extra cargo flags go in CARGO_ARGS.
#
# One integration suite of one crate, with test-binary arguments
[private]
[group('tests')]
suite crate name *test_args:
    #!/usr/bin/env bash
    set -euo pipefail
    dir="services/{{ crate }}"; [ -d "crates/{{ crate }}" ] && dir="crates/{{ crate }}"
    if [ -f "$dir/tests/it/main.rs" ]; then
        cargo test -p {{ crate }} ${CARGO_ARGS:-} --test it -- {{ name }}:: {{ test_args }}
    else
        cargo test -p {{ crate }} ${CARGO_ARGS:-} --test {{ name }} -- {{ test_args }}
    fi

# A demo configuration is shipped surface: it is what an operator copies. Each
# service's `demo_config` suite loads deploy/demo/<service>.toml through its own
# config type and refuses a key the type does not declare (`figment` would drop
# it in silence). The services are read from the directory, so a new config is
# covered as soon as it exists.
#
# Every deploy/demo/<service>.toml loads and names no unknown key
[group('tests')]
test-demo-configs:
    for f in deploy/demo/*.toml; do {{ just_executable() }} suite "$(basename "$f" .toml)" demo_config; done

# A seeded request body is shipped API surface — run by whoever evaluates the
# platform and copied into tickets as "this is what a request looks like" — so
# each one under tools/mako-seed/world/ is deserialised into the real request
# type. Ordinary `#[test]`s, so `just test` covers them; this runs them alone.
#
# Every request body the seeder posts is one the service accepts
[group('tests')]
test-seed-payloads:
    for c in marktd vertragd productd billingd accountingd einsd edmd makod outputd; do {{ just_executable() }} suite "$c" seed_payloads; done

# ── Database suites ───────────────────────────────────────────────────────────
# Every suite below self-manages its PostgreSQL via testcontainers: a throwaway
# container is started in-process (once per test binary) and torn down by the
# testcontainers reaper afterwards. The only requirement is a running Docker
# daemon — no manual `docker run`, no fixed host ports, no `*_DATABASE_URL` env
# vars. Without Docker the `#[ignore]`d tests skip gracefully.

# All database integration suites in one go.
[group('database')]
test-db: check-sql test-conformance-db test-edmd-db test-einsd-db test-accountingd-db test-billingd-db test-outputd-db test-vertragd-db test-productd-db test-marktd-db test-processd-db test-invoicd-db test-netzbilanzd-db test-outbox-db test-policy-db

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
#
# Prepare every service's SQL literals against its own schema
[group('database')]
check-sql:
    cargo xtask check-sql

# Storage integration tests for edmd (meterstore hot/cold over PostgreSQL + a
# filesystem Iceberg warehouse).
#
# Storage suites for edmd: meterstore hot/cold and the Iceberg warehouse
[group('database')]
test-edmd-db:
    {{ just_executable() }} suite edmd meterstore_integration --include-ignored --test-threads=1
    {{ just_executable() }} suite edmd router_builds --include-ignored
    {{ just_executable() }} suite edmd mcp_round_trips --include-ignored
    {{ just_executable() }} suite edmd kafka_ingest --include-ignored --test-threads=1
    {{ just_executable() }} suite edmd sharing_participation --include-ignored
    {{ just_executable() }} suite edmd openapi_coverage --include-ignored

# Handler + SQL integration tests for einsd (EEG settlement).
[group('database')]
test-einsd-db:
    {{ just_executable() }} suite einsd settlement_integration --include-ignored --test-threads=1

# Ledger integration tests for accountingd — the doubleentry-backed
# Massenkontokorrent (idempotency, netting, reconcile, period seal, Merkle
# inclusion proof) and the EEG payout chains against real PostgreSQL.
#
# Ledger suites for accountingd and the EEG payout chains
[group('database')]
test-accountingd-db:
    {{ just_executable() }} suite accountingd db_scenarios --include-ignored --test-threads=1
    {{ just_executable() }} suite accountingd eeg_payout --include-ignored --test-threads=1
    {{ just_executable() }} suite accountingd receivables --include-ignored --test-threads=1
    {{ just_executable() }} suite accountingd sharing_booking --include-ignored
    {{ just_executable() }} suite accountingd router_builds --include-ignored --test-threads=1
    {{ just_executable() }} suite accountingd mcp_round_trips --include-ignored
    {{ just_executable() }} suite accountingd desk_money --include-ignored --test-threads=1
    {{ just_executable() }} suite accountingd openapi_coverage --include-ignored --test-threads=1

# Records integration tests for billingd.
[group('database')]
test-billingd-db:
    {{ just_executable() }} suite billingd records_integration --include-ignored --test-threads=1

# Template-store integration tests for outputd.
[group('database')]
test-outputd-db:
    {{ just_executable() }} suite outputd store_integration --include-ignored --test-threads=1

# The conformance kit against its reference services (vertragd on PostgreSQL).
[group('database')]
test-conformance-db:
    cargo test -p mako-conformance ${CARGO_ARGS:-} --test it -- reference:: --include-ignored --test-threads=1

# Dispatch integration tests for vertragd.
[group('database')]
test-vertragd-db:
    {{ just_executable() }} suite vertragd dispatch_integration --include-ignored --test-threads=1
    {{ just_executable() }} suite vertragd bo4e_bodies --include-ignored --test-threads=1
    {{ just_executable() }} suite vertragd inbound_marker --include-ignored --test-threads=1
    {{ just_executable() }} suite vertragd online_vertragsschluss --include-ignored --test-threads=1
    {{ just_executable() }} suite vertragd webhooks --include-ignored --test-threads=1
    {{ just_executable() }} suite vertragd sales_edge --include-ignored --test-threads=1

# Catalog integration tests for productd.
[group('database')]
test-productd-db:
    {{ just_executable() }} suite productd catalog_integration --include-ignored --test-threads=1
    {{ just_executable() }} suite productd angebot_acceptance --include-ignored --test-threads=1
    {{ just_executable() }} suite productd endkunden_angebot --include-ignored --test-threads=1
    {{ just_executable() }} suite productd webhooks --include-ignored --test-threads=1

# All marktd integration suites (VersorgungsStatus, MeLo graph, ESA, registries,
# durable fan-out, MaBiS-Zählpunkt, temporal constraints, tenant isolation, and
# the 23P01 → 409 translation).
#
# Every marktd integration suite
[group('database')]
test-marktd-db:
    cargo test -p marktd --test it -- --include-ignored --test-threads=1 \
        versorgung_integration:: melo_graph_integration:: \
        esa_integration:: registries_integration:: \
        webhooks_integration:: mabis_zp_integration:: \
        zuordnungsermaechtigung_integration:: \
        temporal_constraints_integration:: tenant_isolation_integration:: \
        overlap_is_a_client_error:: konfigurationsprodukte_body:: \
        utilmd_stammdaten_integration:: lokationsbuendel_integration:: \
        write_and_event_commit_together:: referenzreihen_integration::

# invoicd's receipt store — the § 147 AO / GoBD audit trail, its per-tenant
# uniqueness and the outbox delivery of the receipt event — and the ESA's
# second round (`E_0266`) through the webhook.
#
# invoicd's receipt store, its outbox and the ESA's second round
[group('database')]
test-invoicd-db:
    {{ just_executable() }} suite invoicd receipts_pg --include-ignored --test-threads=1
    {{ just_executable() }} suite invoicd zweite_runde --include-ignored --test-threads=1

# netzbilanzd's invoice-draft store — numbering series, Abschlag deduction,
# the Storno chain and the tenant scoping of every read and transition — and
# its MaBiS submission schema: the per-run series ledger the BIKO acks against,
# and the constraints that stop a retry re-filing an acked series.
#
# netzbilanzd's invoice drafts and its MaBiS submission ledger
[group('database')]
test-netzbilanzd-db:
    cargo test -p netzbilanzd --test it -- --include-ignored --test-threads=1 \
        invoice_drafts_pg:: schema_pg::

# The shared transactional outbox: atomicity with the caller's write, the retry
# schedule and the dead-letter hand-off. Every service that emits depends on it.
# And the webhook subscriptions on it: fan-out, ordered signed delivery,
# rotation, partner binding, replay and the event feed.
#
# The shared transactional outbox and the webhook subscriptions on it
[group('database')]
test-outbox-db:
    {{ just_executable() }} suite mako-service outbox_integration --include-ignored --test-threads=1
    CARGO_ARGS="--features cedar,oidc" {{ just_executable() }} suite mako-service subscriptions --include-ignored --test-threads=4

# The runner's `service_policy_log`: a changed operator setting appends a row,
# an unchanged boot appends none. And `[database] migration_url`: the owner
# migrates, the runtime role serves with data access and no CREATE.
#
# The runner's service_policy_log and the migration/runtime role split
[group('database')]
test-policy-db:
    {{ just_executable() }} suite mako-service policy_integration --include-ignored --test-threads=1
    {{ just_executable() }} suite mako-service migration_role --include-ignored --test-threads=1

# processd's SQL suites: the approval queue claim/dispatch and decision audit
# log; the process read model's sweeps (the deadline alert and the § 7a parity
# report, both on a timer with no caller to notice a wrong predicate), its
# rebuild from marktd's replay log and the one webhook's fan-out; and the
# Sperrauftrag queue (ORDERS 17115/17117 ingest, the claim guard, the IFTSTA
# 21039 retry queue) — the only suite covering the §41f disconnection path.
#
# `--no-default-features --features integrated` because these tests drive the
# NB, the LF and the read-model module together.
#
# processd's SQL suites for the NB, the LF and the read model together
[group('database')]
test-processd-db:
    cargo test -p processd --no-default-features --features integrated --test it -- \
        --include-ignored --test-threads=1 \
        sql_integration:: worker_integration:: rebuild:: webhook_fanout:: db_scenarios:: webhook_delivery:: klaerfall::

# Every real-PostgreSQL suite is `#[ignore]`d and named by a `test-*-db` recipe.
#
# An un-ignored container suite prints `ok` on a machine with no Docker daemon,
# having returned early from every test; an unnamed one is reached by neither
# `just ci` nor `just test-db`. Both read as coverage.
#
# Every real-PostgreSQL suite is #[ignore]d and named by a test-*-db recipe
[group('database')]
check-db-suites:
    cargo xtask check-db-suites

# ── Guards ────────────────────────────────────────────────────────────────────
#
# One defect class each, one `cargo xtask` call each, all in `just ci`;
# `cargo xtask evidence-pack --xtask-recipes` lists them.

# A service that reads `Claims` also pins the tenant it expects.
#
# Without `ExpectedTenant`, a token signed by the same realm for a different
# operator extracts cleanly, and only the tenant condition in every Cedar rule
# stands between it and the data.
#
# A service that reads Claims pins the tenant it expects
[group('guards')]
check-expected-tenant:
    cargo xtask check-expected-tenant

# Every catalogued Mindestvorlaufzeit is read by production code
[group('guards')]
check-vorlauf-consulted:
    cargo xtask check-vorlauf-consulted

# No BEHG price table carries a year no enacted statute prices (from 2027)
[group('guards')]
check-behg-prices:
    cargo xtask check-behg-prices

# Every catalogued Referenzreihe is sourced, licensed and read; no value ships
# for a series mako may not distribute (no EPEX value in the repository)
#
# Every catalogued Referenzreihe is sourced, licensed and read
[group('guards')]
check-referenzreihen:
    cargo xtask check-referenzreihen

# The committed profiles are consistent: sources.json, dates and continuity
# per track, Prüfidentifikatoren, AHB rows against the MIG structure, and
# every Bedingung a status expression cites has its text.
#
# The committed AHB/MIG profiles are consistent with each other
[group('guards')]
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
#
# Hold the Antwortcode catalogue against the EBD PDF (skips without the mirror)
[group('guards')]
validate-ebd-codes:
    cargo xtask validate-ebd-codes

# Verify a profile covers today's date
[group('guards')]
check-release-coverage:
    cargo xtask check-release-coverage

# Verify the rubo4e::current active-type count matches the README.md claim exactly.
[group('guards')]
check-bo4e-coverage:
    cargo xtask check-bo4e-coverage

# A BO4E `_typ` is the type's own fact — never a literal in a struct or a `json!`.
[group('guards')]
check-bo4e-discriminants:
    cargo xtask check-bo4e-discriminants

# Every BO4E example in the docs uses fields BO4E defines. An example is copied,
# and an undefined field is absorbed into `_additional` rather than refused.
#
# Every BO4E example in the docs uses only fields BO4E defines
[group('guards')]
check-bo4e-examples:
    cargo xtask check-bo4e-examples

# Refuse a business date read in UTC. A Lieferbeginn, a Rechnungsdatum and the
# day a Frist starts counting are German calendar dates; `now_utc().date()` and
# SQL `current_date` answer the UTC resp. session date, which is the previous
# day for an hour every night. Use `mako_fristen::heute()` and the schema's
# `heute()` function.
#
# Refuse a business date read in UTC
[group('guards')]
check-business-dates:
    cargo xtask check-business-dates

# Refuse a clock, randomness, environment or filesystem read inside an
# `impl … Workflow for …` block. A workflow is a pure function of
# (state, command): `Process::execute_with_retry` re-runs `handle` after a
# version conflict and every state is re-folded from the event log, so a value
# taken from the environment makes a retry emit a different message than the one
# already sent. The instant belongs on the command (`received_at`/`gesendet_am`),
# resolved at the makod boundary.
#
# Refuse a clock, randomness, environment or file read inside a Workflow
[group('guards')]
check-workflow-purity:
    cargo xtask check-workflow-purity

# Refuse a `${{ }}` value interpolated into a `run:` script. GitHub substitutes
# it as text before bash parses the line, so one `'` in the value ends the
# quoted string and the remainder is executed. Pass it through `env:`.
#
# Refuse a ${{ }} value interpolated into a workflow run: script
[group('guards')]
check-workflow-interpolation:
    cargo xtask check-workflow-interpolation

# Hold each durable outbox's wake-up in the database, where it belongs. Postgres
# queues a NOTIFY until the raising transaction commits, so an AFTER INSERT
# trigger cannot wake the worker onto a snapshot without the row — and reaches
# every replica, which an in-process handle never can.
#
# Each durable outbox wakes from its database trigger
[group('guards')]
check-outbox-notify:
    cargo xtask check-outbox-notify

# Refuse a bare `.route("/api/v1…")` beyond each service's ratchet: an /api/v1
# route is mounted through `OpenApiRouter`, so the OpenAPI document lists it.
#
# Refuse an /api/v1 route mounted outside the OpenAPI router
[group('guards')]
check-openapi-routes:
    cargo xtask check-openapi-routes

# Every committed `services/<svc>/openapi.json` is published byte for byte under
# site/static/api/, with one reference page linking each; refuse a stale one.
#
# Refuse a stale published OpenAPI document or reference page
[group('guards')]
api-catalog-check:
    cargo xtask api-catalog --check

# The event catalog page and the per-type JSON Schemas are generated from
# mako_events and the committed `events/*.json`; refuse a stale one, and a type
# that left the untyped ratchet without a schema.
#
# Refuse a stale event catalog page or event schema
[group('guards')]
event-catalog-check:
    cargo xtask event-catalog --check

# A correlation key is written in the batch of its events
[group('guards')]
check-atomic-spawn:
    cargo xtask check-atomic-spawn

# Every workflow command variant is constructed by shipped code
[group('guards')]
check-command-constructors:
    cargo xtask check-command-constructors

# Every command processd can queue for an operator has an answer contract
[group('guards')]
check-approval-commands:
    cargo xtask check-approval-commands

# Ingest arms deciding resume-vs-spawn on index presence alone only become fewer
[group('guards')]
check-occupancy:
    cargo xtask check-occupancy

# Refuse `deny_unknown_fields` on a workflow event payload — an event is read
# back years later, so a removed field must not make a § 147 AO record unfoldable.
#
# Refuse deny_unknown_fields on a workflow event payload
[group('guards')]
check-event-payloads:
    cargo xtask check-event-payloads

# Refuse an event that is a market message and states none of it — the Desk's
# Verlauf reads direction and PID off the envelope, never off a type name.
#
# Refuse a market-message event that states no direction or PID
[group('guards')]
check-event-verlauf:
    cargo xtask check-event-verlauf

# Refuse an unreachable control at the AS4 boundary.
#
# Refuse an unreachable control at the AS4 boundary
[group('guards')]
check-as4-controls:
    cargo xtask check-as4-controls

# Refuse a statement against a tenant-scoped table with no tenant bound.
[group('guards')]
check-tenant-predicate:
    cargo xtask check-tenant-predicate

# Refuse a Cedar role literal the platform never mints.
[group('guards')]
check-cedar-roles:
    cargo xtask check-cedar-roles

# Refuse a Festlegung cited in a form it does not publish. BK6-22-024 numbers its
# operative part in Tenorziffern and carries its substance in Anlagen, so
# „BK6-22-024 § 4" names nothing that can be looked up — and an uncheckable
# citation shields whatever Frist stands beside it from review.
#
# Refuse a Festlegung cited in a form it does not publish
[group('guards')]
check-citations:
    cargo xtask check-citations

# Every operator policy setting: each option sourced (and passing the citation
# and known-wrong rules), its docs anchor resolving, the generated
# operator-policy page in sync, each key read in production code — and no
# service config defaulting an enum outside a policy.
#
# Every operator policy setting is documented, read and not defaulted elsewhere
[group('guards')]
check-policy-settings:
    cargo xtask check-policy-settings

# Refuse a claim AGENTS.md lists as known-wrong, and a tracked file that points
# into the untracked roadmap or specifications.
#
# Refuse a known-wrong claim and a pointer into the untracked planning trees
[group('guards')]
check-known-wrong:
    cargo xtask check-known-wrong

# A „…" quotation attributed to a mirrored Festlegung occurs in that document.
# SKIPS without the gitignored `regulatories/` mirror or poppler's `pdftotext`.
#
# A „…" quotation occurs in the mirrored document it cites (skips without it)
[group('guards')]
check-quotations:
    cargo xtask check-quotations

# Refuse a Werktage or clock-time literal in a string that also names a
# Festlegung, an AHB or an AWH — the Frist renders from mako_fristen.
#
# Refuse a Frist literal beside its Festlegung; Fristen render from mako_fristen
[group('guards')]
check-frist-literals:
    cargo xtask check-frist-literals

# Every declared control — Frist constants, Meldepflicht rows, credential
# fields, role columns, feature-gated security modules — is consulted, or
# listed with a reason and the milestone that removes it.
#
# Every declared control is consulted, or listed with its reason
[group('guards')]
check-controls-consulted:
    cargo xtask check-controls-consulted

# Refuse banker's rounding. `Decimal::round_dp` rounds half to even; German
# commercial practice, EN 16931/XRechnung and every BDEW settlement figure round
# half away from zero (DIN 1333). The modes differ only on exact midpoints — the
# values a ct price with three decimals produces — so the wrong one misstates a
# cent without failing a test written against ordinary numbers.
#
# Refuse banker's rounding; money rounds kaufmännisch (DIN 1333)
[group('guards')]
check-rounding:
    cargo xtask check-rounding

# How much of the published Prüfidentifikator inventory the AHB profiles carry,
# and whether the PID reference names all of it. `validate-profiles` compares one
# release against the previous one, so it can prove nothing was lost and is blind
# to a PID that was never imported. The inventory is extracted into
# `crates/edi-energy/profiles/pid-overview.json`
# (`cargo xtask import-pid-overview <Anwendungsübersicht.xlsx>`) so this runs
# without the source documents — a guard that only runs where they are is a
# guard that reports green from a skip.
#
# How much of the published PID inventory the profiles and the PID reference carry
[group('guards')]
check-pid-coverage:
    cargo xtask check-pid-coverage

# Refuse an action not pinned by commit SHA, a `FROM` without a digest, workflow-wide write permissions, and an unpinned pip/npm install
[group('guards')]
check-action-pins:
    cargo xtask check-action-pins

# Refuse a `just ci` xtask recipe no `ci.yml` job runs — the pull-request gate
# runs the list `evidence-pack --xtask-recipes` derives from `_ci`, never a copy
#
# Refuse a just-ci xtask recipe no ci.yml job runs
[group('guards')]
check-ci-guards:
    cargo xtask check-ci-guards

# A ban whose path stopped resolving is ignored by clippy in silence.
#
# Every clippy.toml ban fires on its planted use in xtask/clippy-fixtures
[group('guards')]
check-clippy-bans:
    cargo xtask check-clippy-bans

# Hold release/vex.json's OpenVEX statements equal to deny.toml's ignored advisories
[group('guards')]
check-vex:
    cargo xtask check-vex

# Refuse a raw `time` value on a JSON wire: `OffsetDateTime` and `Date` derive
# `Serialize` as their component array ([y, ordinal, h, m, s, ns, ±h, ±m, ±s]),
# which is `time`'s internal layout and readable by nothing — least of all by an
# agent asked to decide whether a `deadline_at` has passed.
#
# Refuse a raw time value on a JSON wire
[group('guards')]
check-wire-timestamps:
    cargo xtask check-wire-timestamps

# Refuse a MaLo-ID literal whose BDEW check digit is wrong (metering/meterstore
# validate it at the parse, so a bad fixture is refused by the storage layer)
#
# Refuse a MaLo-ID literal with a wrong check digit
[group('guards')]
check-malo-ids:
    cargo xtask check-malo-ids

# Refuse a ZusatzAttribut that is not `mako:`-namespaced and registered
[group('guards')]
check-bo4e-attributes:
    cargo xtask check-bo4e-attributes

# accountingd writes the ledger only through booking drafts; no cents through f64
[group('guards')]
check-ledger-drafts:
    cargo xtask check-ledger-drafts

# SQL built at runtime is asserted safe in one place, each site listed with its reason
[group('guards')]
check-dynamic-sql:
    cargo xtask check-dynamic-sql

# Rules about the moment an untrusted JSON body becomes a Rust value:
# `mako_service::Json` refuses an undeclared key at runtime, so the guard names
# what it cannot see — a flattened field, an untagged or internally tagged enum,
# a `Lenient` or `axum::Json` extraction — unless listed with a reason; a BO4E
# document in a body is a `Bo4e<T>` rather than an ungated `serde_json::Value`;
# and no comment carries a `\uXXXX` escape.
#
# Every JSON request body is read strictly and gated
[group('guards')]
check-request-bodies:
    cargo xtask check-request-bodies

# A service is a library: `main.rs` is `mako_service::run::<D>().await` and
# nothing else, so a test can build the router the binary ships. makod, which
# drives its own `main`, is the one exemption the guard names.
#
# A service's main is one run call; the Daemon lives in lib.rs
[group('guards')]
check-thin-main:
    cargo xtask check-thin-main

# Every German page of the site names the English page it was translated from
# and that page's sha256, so a changed English page fails here until the
# translation follows (`cargo xtask check-translations --rehash <file>`) — and
# states none of the figures the English pages' guards hold.
#
# Every German page is current with the English page it translates
[group('guards')]
check-translations:
    cargo xtask check-translations

# A configuration check returns every finding as a `Befund` with its path; a
# `bail!`/`anyhow!`/`ensure!` in a `Daemon::validate`, or in any function that
# collects findings, stops at the first defect and names no setting.
#
# A configuration check returns every finding as a Befund, never the first error
[group('guards')]
check-config-findings:
    cargo xtask check-config-findings

# An adapter reads a UTILMD STS code from C556 DE 9013 — `transaktionsgrund()`,
# `antwort()` — never from DE 4405, which a conformant message leaves empty.
#
# Read a UTILMD STS code from DE 9013, never DE 4405
[group('guards')]
check-sts-codes:
    cargo xtask check-sts-codes

# A handler fails with an `ApiError`, never a hand-built `(StatusCode, …)`
# tuple — that is a body shape the caller cannot rely on and, for a database
# error, the schema's own text handed to the caller. Absolute: any tuple fails.
#
# A handler fails with an ApiError, never a hand-built status tuple
[group('guards')]
check-error-tuples:
    cargo xtask check-error-tuples

# A consumer of `de.mako.engine.process.completed` reads the verdict. One type carries
# the Bestätigung and the Ablehnung; branching on the type alone reads every
# refused Anmeldung as a supply that started.
#
# A consumer of process.completed reads the verdict
[group('guards')]
check-process-verdict:
    cargo xtask check-process-verdict

# An event commits with the work it reports. Refuse a hand-built CloudEvent
# envelope, a publish with nothing durable behind it and no stated retry, and a
# marker written on a pool beside an outbox enqueue — committed alone, it says
# „done" with no event, and every later pass skips the row it marked.
#
# An event commits with the work it reports
[group('guards')]
check-event-durability:
    cargo xtask check-event-durability

# Refuse a specialist procedure that tells a model to call a tool the manifest
# does not grant. `check-tool-grants` validates the grant list; this validates
# the prompt. An unknown tool name is reported back to the model as a failed
# call rather than ending the run, so the step silently does not happen.
#
# Refuse a specialist procedure naming a tool the manifest does not grant
[group('guards')]
check-prompt-tools:
    cargo xtask check-prompt-tools

# Refuse axum 0.7 `/:param` route literals. Under axum 0.8 they panic while the
# router is built — i.e. at startup — so nothing in the test suite catches them.
#
# Refuse an axum 0.7 /:param route literal, which panics at startup
[group('guards')]
check-routes:
    cargo xtask check-routes

# Every crate root denies `unsafe_code`. `#![deny(...)]` is a crate attribute and
# cannot be inherited: a service with a lib.rs *and* a main.rs is two crates, and
# denying it in one leaves the other outside the lint.
#
# Every crate root denies unsafe_code
[group('guards')]
check-crate-lints:
    cargo xtask check-crate-lints

# Refuse a workflow job that compiles this workspace without first reclaiming
# runner disk. A hosted runner has ~14 GB free; the overflow arrives as
# `No space left on device` mid-compile or as a linker SIGBUS, and neither reads
# as a disk problem.
#
# Refuse a workflow job that compiles without reclaiming runner disk
[group('guards')]
check-runner-disk:
    cargo xtask check-runner-disk

# Refuse a Dockerfile builder stage that compiles the services in more than one
# `cargo build`: Cargo unifies features per invocation, so a second build
# recompiles the shared dependencies and the release builder outruns its timeout.
#
# Refuse a Dockerfile that compiles the services in more than one cargo build
[group('guards')]
check-docker-build:
    cargo xtask check-docker-build

# No daemon claims a route `mako_service::run` already mounts.
#
# The runner merges the daemon's router onto one that already carries
# `/health/*` and `/metrics`, and `Router::merge` panics on an overlapping
# method route — while the router is assembled, so the daemon does not boot.
#
# No daemon claims a route mako_service::run already mounts
[group('guards')]
check-runner-routes:
    cargo xtask check-runner-routes

# Every agentd tool grant must name a real MCP tool and agree with that server's
# own `read_only_hint`. A read declared mutating stops for a human on every call;
# a mutation declared read-only is dispatched unattended.
#
# Every agentd tool grant names a real MCP tool and agrees on read-only
[group('guards')]
check-tool-grants:
    cargo xtask check-tool-grants

# Every tool-calling agentd specialist has labelled eval cases (or a shrink-only
# ratchet entry), and no case carries personal data. The scores the cases yield
# are reported, never a gate.
#
# Every tool-calling specialist has eval cases, none with personal data
[group('guards')]
check-eval-cases:
    cargo xtask check-eval-cases

# Every MCP server speaks the declared revisions (`mako_service::mcp`), holds no
# session and enables no capability MCP 2026-07-28 deprecates; agentd's client
# connects on 2026-07-28 and pins the servers' set to the one it speaks.
#
# Every MCP server speaks the declared revisions and holds no session
[group('guards')]
check-mcp-surface:
    cargo xtask check-mcp-surface

# Refuse an invoicd PID route naming a makod command that does not exist.
#
# The command name is a string on both sides. A route naming one makod never
# registered fails only when a real invoice arrives: the check runs, the verdict
# is persisted, the dispatch 404s, and the Antwortfrist expires on a process that
# looked healthy the whole way.
#
# Refuse an invoicd PID route naming a makod command that does not exist
[group('guards')]
check-answer-commands:
    cargo xtask check-answer-commands

# ── Generated files ───────────────────────────────────────────────────────────
#
# Regenerate after the change that moves them; the matching `-check` guard
# refuses a stale copy.

# Republish site/static/api/ and the HTTP APIs reference page.
[group('generated')]
api-catalog:
    cargo xtask api-catalog

# Regenerate the event catalog page and site/static/schemas/events/.
[group('generated')]
event-catalog:
    cargo xtask event-catalog

# ── Release ───────────────────────────────────────────────────────────────────

# Bump workspace version (e.g. `just bump 0.2.0`)
[group('release')]
bump version:
    cargo xtask bump-version {{ version }}

# Every crate under crates/ is published; a consumer finds a breaking change
# when their build fails unless this does first. The baseline is each crate's
# newest version on crates.io — what consumers actually resolve — never a git
# tag: a tag whose release stopped before publishing would otherwise become
# the baseline and let the next patch carry the break onto crates.io. A change
# the current version number does not admit (a minor bump, under 0.x) fails.
# It runs in the release workflow before anything is published, not in `ci`:
# between releases the API may break freely, and the bump that admits a break
# is made when the release is cut.
# The crate set is the release workflow's publish list, as for
# check-publishable. Needs cargo-semver-checks; name crates to check a subset.
#
# Refuse a public-API break the version number does not admit
[group('release')]
semver *crates:
    #!/usr/bin/env bash
    set -euo pipefail
    crates="{{ crates }}"
    if [ -z "$crates" ]; then
        crates=$(grep -oE 'cargo publish -p [a-z0-9-]+' .github/workflows/release.yml \
                 | awk '{print $NF}' | sort -u)
    fi
    args=()
    for c in $crates; do args+=(-p "$c"); done
    echo "semver: ${#args[@]} flags against the versions published on crates.io"
    cargo semver-checks check-release "${args[@]}"

# `cargo publish` verifies each crate **alone, with default features**, where
# workspace feature unification no longer supplies an optional dependency. A
# `#[cfg]` gate that slipped off its item compiles all through `just ci` and
# fails in the release job.
#
# Build every publishable crate the way crates.io will
[group('release')]
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

# `cargo publish` resolves workspace dependencies against crates.io, not the
# working tree, so a crate published before one it depends on fails outright.
# `check-publishable` sorts the list before compiling it, which leaves the order
# — the whole contract of the release job — unchecked.
#
# The release job publishes every crate after the crates it depends on
[group('release')]
check-publish-order:
    cargo xtask check-publish-order

# A report, not a gate: a pin behind the registry is often a decision, and
# `just ci` must not need the network. Pass `--recorded <file>` to read a saved
# crates.io response instead of querying it.
#
# Report each direct dependency's pin against crates.io (network)
[group('release')]
dep-age *args:
    cargo xtask dep-age {{ args }}

# Assemble the release evidence pack; a section without its input reads `not measured`
[group('release')]
evidence-pack inputs out:
    cargo xtask evidence-pack --inputs {{inputs}} --out {{out}}

# ── Profiles ──────────────────────────────────────────────────────────────────

# Regenerate every profile from its BDEW MIG/AHB PDF (profiles/sources.json).
# Needs the document mirror (`just regulatories-download`) and poppler's
# `pdftotext`.
#
# Regenerate every profile from its BDEW MIG/AHB PDF
[group('profiles')]
import-profiles:
    cargo xtask import-profiles

# One profile, e.g. `just import-profile utilmd/fv20261001`
[group('profiles')]
import-profile dir:
    cargo xtask import-profiles --profile {{ dir }}

# Fail when a committed profile no longer matches its source PDF; SKIPs
# without the mirror or poppler.
#
# Fail when a committed profile no longer matches its PDF (skips without it)
[group('profiles')]
import-profiles-check:
    cargo xtask import-profiles --check

# Per Prüfidentifikator, what changed between two Formatversionen of one
# message type, e.g. `just profile-diff utilmd fv20251001 fv20261001`.
# Reads the committed profiles; no document mirror needed.
#
# Per PID, what changed between two Formatversionen of one message type
[group('profiles')]
profile-diff type from to:
    cargo xtask profile-diff {{ type }} {{ from }} {{ to }}

# Dump a BDEW PDF as the character grid the importer reads
[group('profiles')]
pdf-grid file:
    cargo xtask pdf-grid {{ file }}

# ── The Desk (desk/) ──────────────────────────────────────────────────────────
#
# The operator UI: a React app in desk/ that reads the services' MCP tools
# through its Vite server, which proxies /svc/<name> to the port
# deploy/ports.toml assigns. `just dev` runs it against the estate, `just
# dev-ui` against its recorded answers.

# Every service with an mcp.snapshot.json rewrites it from its tool router; then
# the Desk's types are regenerated from the snapshots.
#
# Refresh the MCP tool snapshots and regenerate the Desk's types
[group('desk')]
desk-gen:
    #!/usr/bin/env bash
    set -euo pipefail
    for snap in services/*/mcp.snapshot.json; do
        svc="$(basename "$(dirname "$snap")")"
        BLESS_MCP_SNAPSHOT=1 cargo test -q -p "$svc" --lib -- the_tool_contract_matches_the_snapshot
    done
    cd desk && npm run gen

# Needs Node (desk/.node-version) and no browser or Docker.
#
# The Desk's own gate: generated types current, typecheck, lint, unit tests,
# the production build within its size budget (scripts/umfang.ts)
#
# The Desk's own gate: types, typecheck, lint, tests, size budget
[group('desk')]
desk-check:
    cd desk && npm ci --no-audit --no-fund && npm run check

# No desk/src file outside src/format/ builds or parses a date or a number —
# the Desk renders Fristen and amounts, it never derives them — and the Desk's
# npm licences are in deny.toml's allow list
#
# The Desk derives no date or number, and its npm licences are allowed
[group('desk')]
check-desk:
    cargo xtask check-desk

# Vite serves the Desk, makod's answers are recorded, the clock is fixed —
# no service runs. Needs Playwright's Chromium (`npx playwright install chromium`).
#
# Regenerate the Desk's screenshots in site/content/docs/guide/desk/
[group('desk')]
desk-screenshots:
    cd desk && npm run screenshots

# Playwright against the production build under `vite preview`, agentd's
# stream played from desk/fixtures/: the first streamed step ≤ 1 s, an entry's
# ⌘K actions ≤ 100 ms, a keyboard decision's INP ≤ 100 ms, each at the 95th
# percentile, every measurement printed. Proves first that a late fixture
# fails. Needs Playwright's Chromium, so it is not part of `desk-check`.
#
# The Desk's interaction budgets, measured in a browser
[group('desk')]
desk-budgets:
    cd desk && npm run budgets

# axe-core over the assistant's trail, receipt, pill and ⌘K action panel, light
# and dark, under reduced motion, each reached from the keyboard. Needs
# Playwright's Chromium, so it is not part of `desk-check`.
#
# The assistant's surfaces scanned for access in a browser
[group('desk')]
desk-axe:
    cd desk && npm run zugang

# Every route's accessibility tree (the ARIA snapshot of `main`) held to its
# baseline in desk/aria/; a change fails until it is blessed (`just desk-aria
# --bless`). Text, so the same on every OS. Needs Playwright's Chromium, so it
# is not part of `desk-check`.
#
# Every route's accessibility tree compared with its baseline
[group('desk')]
desk-aria *args:
    cd desk && node scripts/aria.ts {{ args }}

# The Einrichtung view in light, dark and at a phone's width, held to its
# baselines in desk/vergleich/ with a pixel tolerance; a change fails until it
# is blessed (`just desk-vergleich --bless`). The baselines are Linux
# renderings: on Linux (CI) it runs directly, elsewhere in Playwright's Linux
# image (Docker), which writes back the baselines or the renderings that differ.
#
# The Einrichtung view's screenshots compared with their baselines
[group('desk')]
desk-vergleich *args:
    #!/usr/bin/env bash
    set -euo pipefail
    # Always in the pinned Playwright image: the baselines are rendered there,
    # and a host browser's fonts and rasteriser differ from it.
    version="$(node -p 'require("./desk/package.json").devDependencies.playwright')"
    docker run --rm -v "$PWD/desk:/repo/desk" -w /work "mcr.microsoft.com/playwright:v${version}-noble" bash -c '
        set -euo pipefail
        tar -C /repo --exclude=desk/node_modules --exclude=desk/dist -cf - desk | tar -xf -
        cd desk && npm ci --no-audit --no-fund >/dev/null
        status=0; node scripts/vergleich.ts {{ args }} || status=$?
        cp -r vergleich/. /repo/desk/vergleich/
        exit $status'

# ── Site and docs ─────────────────────────────────────────────────────────────

# Open rustdoc for a crate in the browser (e.g. `just doc mako-engine`)
[group('site')]
doc crate:
    cargo doc -p {{ crate }} --all-features --no-deps --open

# Build all workspace docs
[group('site')]
doc-all:
    cargo doc --workspace --all-features --no-deps

# The validator's WebAssembly module (site/static/validator/mako.wasm), held
# to the native build's answers and its size budget.
#
# Build the validator's WebAssembly module and hold it to the native answers
[group('site')]
wasm:
    rustup target add wasm32-unknown-unknown >/dev/null
    cargo build -q -p mako-wasm --profile wasm --target wasm32-unknown-unknown
    mkdir -p site/static/validator
    cp target/wasm32-unknown-unknown/wasm/mako_wasm.wasm site/static/validator/mako.wasm
    node tools/mako-wasm/parity.mjs site/static/validator/mako.wasm

# Live reload on http://127.0.0.1:1111, the validator's module built first.
# Search and the Markdown twins come from the post-build step, so they are
# absent here; `just build-site` produces the site as deployed.
#
# Serve the docs site locally with live reload
[group('site')]
site-serve: wasm
    cd site && zola serve

# Validate every Mermaid diagram on the site, and the script that renders them.
#
# `zola check` validates links only — it never parses diagram source, so a
# broken diagram publishes as an error box. This also fails if the renderer
# regresses to `startOnLoad`, which stops rendering on a cold CDN cache.
# Mirrors the "Check Mermaid diagrams" step in .github/workflows/site.yml.
#
# Validate every Mermaid diagram on the site and the script that renders it
[group('site')]
check-mermaid:
    cd site && npm install --no-audit --no-fund --silent && npm run --silent check:mermaid

# Link check + diagram check for the docs site.
# Fail on bare relative Markdown links in site content.
#
# `zola check` validates `@/docs/…` links only; a plain `[text](page.md)` is
# emitted verbatim, never resolved, and 404s silently. See
# site/tools/check-links.mjs.
#
# Refuse a relative docs link Zola would publish unresolved
[group('site')]
check-links:
    cd site && node tools/check-links.mjs

# Build the docs site as deployed: `zola build`, then the post-build step —
# the Pagefind search index, a Markdown twin beside every page, `llms.txt` and
# `llms-full.txt` (site/tools/postbuild.mjs). Output in site/public/. Mirrors
# the "Build" steps in .github/workflows/site.yml.
#
# Build the docs site as deployed, with its Markdown twins and search index
[group('site')]
build-site: wasm
    cd site && npm install --no-audit --no-fund --silent && zola build && node tools/postbuild.mjs

# `build-site` is part of the check: the post-build step refuses a content
# page with no built HTML, an `llms.txt` group missing for a section, and a
# search index that holds a different number of pages than the content tree.
#
# Mermaid, links, the full build and zola check: the site's gate
[group('site')]
check-site: check-mermaid check-links build-site
    cd site && zola check

# ── Integrations ──────────────────────────────────────────────────────────────
#
# What a counterpart system meets: the conformance kit and the ZUGFeRD carrier.

# Check a service — mako's own or a replacement — against its published
# contract: `just conformance vertragd http://localhost:9780 --token …`.
#
# Check a service, mako's own or a replacement, against its published contract
[group('integrations')]
conformance service url *args:
    cargo run -q -p mako-conformance -- {{ service }} {{ url }} {{ args }}

# mako proves the carrier by reading its own output back (outputd's publish
# gate), and `en16931 validate` — an independent implementation — reports the
# payload valid. Neither proves **PDF/A-3 conformance**: nothing in Rust does,
# and the XMP `document::facturx::stamp` appends lands after typst-pdf's own
# enforcement has finished. That is what veraPDF is for, and it needs a file.

# Write a stamped ZUGFeRD specimen for veraPDF / the ZUGFeRD validator
[group('integrations')]
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
[group('integrations')]
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
#
# Build the PyO3 extension and run the Python suite
[group('makotest')]
test-makotest:
    cd makotest && test -d .venv || python3 -m venv .venv
    cd makotest && .venv/bin/pip install -q --upgrade pip 'maturin>=1.9,<2.0' pytest hypothesis ruff
    cd makotest && .venv/bin/maturin develop && .venv/bin/pytest -q

# Lint and format-check the Python layer (ruff).
#
# `makotest` is a published PyPI artifact, so its Python surface gets the same
# gate the Rust side does — a consumer reads these files.
#
# Lint and format-check makotest's Python layer (ruff)
[group('makotest')]
lint-makotest:
    cd makotest && test -d .venv || python3 -m venv .venv
    cd makotest && .venv/bin/pip install -q --upgrade pip ruff
    cd makotest && .venv/bin/python -m ruff check .
    cd makotest && .venv/bin/python -m ruff format --check .

# Build a release wheel (abi3, one wheel for Python ≥ 3.11).
[group('makotest')]
build-makotest:
    cd makotest && .venv/bin/maturin build --release

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
#
# Compile every fuzz target against the current dependency graph
[group('fuzz')]
check-fuzz:
    cd fuzz && cargo check --all-targets

# Run a fuzz target (requires nightly + cargo-fuzz; e.g. `just fuzz fuzz_parse_validate`)
[group('fuzz')]
fuzz target:
    cd fuzz && cargo +nightly fuzz run {{ target }}
