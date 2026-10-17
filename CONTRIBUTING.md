# Contributing to mako

Rust 1.94 (pinned in `rust-toolchain.toml`, installed by `rustup` on first
use), [`just`](https://github.com/casey/just), and Docker for the database.
`AGENTS.md` holds the domain rules and non-negotiables; read it before you
change domain code.

## Build

```bash
just check                 # cargo check over every target and feature
```

A bare `cargo build` or `cargo test` at the root builds every member except
`makotest` (its PyO3 bindings need Python ≥ 3.11). The `just` gate recipes pass
`--workspace` and include it, reading the interpreter from `.venv/bin/python`
(`PYO3_PYTHON` overrides it).

## Test one crate

```bash
just test                     # the whole suite, under cargo-nextest when installed
just test-crate mako-fristen   # cargo test -p mako-fristen --all-features
just test-doc                  # doctests — `--all-targets` does not run them
just test-db                   # the real-PostgreSQL suites (needs Docker)
```

A crate's integration tests are one binary, `tests/it/main.rs`, with one
module per file under `tests/it/`. Add a test file there and name it in
`main.rs`. Filter one module with `cargo test -p <crate> --test it <module>::`.

## Run one service

```bash
just dev marktd
```

That starts PostgreSQL from `dev/docker-compose.yml` if it is not running and
starts the service with `dev/config/marktd.toml` (`makod` needs no database:
its dev store is in memory). It restarts on change when `watchexec` or `bacon`
is installed, and runs once with `cargo run` otherwise. Every service has a dev
config; each one's `dev_config` test (`just test-dev-configs`) loads it through
the service's own config type, so a renamed key fails the suite rather than the
next clone.

A config problem is reported all at once — the file read, the env prefix and
every missing, unknown or invalid key:

```bash
cargo run -p marktd -- --help          # usage, without reading any config
cargo run -p marktd -- --print-config  # the merged config, secrets redacted
```

Each key can be overridden by `<SERVICE>_<KEY>`, nested keys joined with `__`
(`MARKTD_DATABASE__URL`); `just infra-down` stops PostgreSQL and
`just infra-reset` also deletes its volume.

## The service shape

Every daemon but `makod` has one shape, and a guard holds each part:

- **A library with a one-line `main`.** The `Daemon` lives in `lib.rs` (or a
  module of it); `main.rs` is `mako_service::run::<D>().await`
  (`check-thin-main`), and `tests/it/router_builds.rs` builds the router the
  binary mounts with `mako_service::build_for_test::<D>(cfg)`.
- **Every failure is an `ApiError`.** Handlers return `ApiResult<T>` and use
  `?`; a refusal is a variant written for the caller (`bad_request`,
  `not_found`, `precondition_failed`, `too_many_requests`, …), with a stable
  `code` and extra keys through `with_code` / `with_fields`. A 500 or an
  upstream failure carries an `error_id` the log line carries and never the
  error's own text. A hand-built `(StatusCode, …)` tuple fails
  `check-error-tuples`.
- **Bodies come through `mako_service::Json`**, which refuses a key the type
  does not declare. `Lenient<T>` reads an externally specified schema without
  that refusal; `check-request-bodies` names every use, and every flattened
  or buffered shape the runtime check cannot see.
- **One listen shape.** A config takes `[http] addr`; `dev/ports.toml` holds
  every dev port, and the `dev_configs` tests hold every dev config to it.

## Run the gate

```bash
just ci > ci.log 2>&1; echo EXIT:$?
grep -E 'test result: FAILED|recipe .* failed' ci.log
```

`just ci` is the whole suite plus every guard; nothing is done until it passes.
Four traps, each of which has produced a false green or a stalled run:

- **Never pipe `just ci` through `tail` or `grep` for the verdict** — the pipe
  masks the exit code. Read `EXIT:` from the log.
- **`cargo check` is not clippy.** The gate runs
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`; run
  `just clippy` over what you touched before reporting.
- **Set `CARGO_TARGET_DIR` when a `Cargo.toml` has just changed.** An editor's
  rust-analyzer rebuilds into the default `target/` and races the gate into a
  fingerprint write failure that looks like a build error.
- **`cargo clean` between long sessions**, never while a build runs. A full
  disk fails as `No space left on device` inside a compile, which reads like a
  build error and is not one.

## Build speed

`[profile.dev]` keeps line tables (a backtrace still names `file:line`) and no
debug info for dependencies; build scripts and proc macros compile optimised.
Measured on an Apple-silicon laptop, each in a fresh target directory, building
`makod` and then touching `mako-fristen` and rebuilding:

| | full DWARF | `[profile.dev]` |
|---|---|---|
| `target/` after the build | 7.4 GB | 4.3 GB |
| touch rebuild (3 runs) | 13 s, 10 s, 10 s | 14 s, 10 s, 9 s |

The rebuild is dominated by recompiling the crates above `mako-fristen`, not by
the link. The disk is what the profile buys.

On Linux, `mold` links faster than the default linker: install it and
uncomment the `[target.x86_64-unknown-linux-gnu]` section in
`.cargo/config.toml`. macOS's default linker is already fast.

## Changes

- Breaking changes are fine; the project is unreleased and the SQL schema is
  edited in place.
- Docs and comments state current truth — no history, no "used to".
- A defect class becomes a guard: the deliverable for a bug is the check that
  makes it unrepresentable, seen to fail before it is trusted.
- Every regulatory claim cites the primary document, chapter and page.
- The profiles under `crates/edi-energy/profiles/` are generated from the BDEW
  PDFs: fix the reader in `xtask/src/bdew/` and run `cargo xtask import-profiles`
  instead of editing JSON.
