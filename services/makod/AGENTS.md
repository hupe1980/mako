<!-- Nested AGENTS.md: the closest one to the file being edited wins.
     Workspace-wide rules are in the root AGENTS.md and are not repeated here. -->

# makod Service Instructions

## Role

`makod` is the only binary crate that assembles the domain workflow crates (`mako-gpke`, `mako-wim`, `mako-geli-gas`, `mako-gabi-gas`, `mako-mabis`, `mako-emob`, `mako-redispatch`) into a single `EngineContext`. `production_modules()` in `src/startup/mod.rs` is the list, and each entry is `#[cfg]`-gated on the Marktrolle features the build selected. It owns:
- Persistence backend selection (in-memory vs. SlateDB)
- Object store configuration (local / S3 / GCS)
- OTLP instrumentation setup
- HTTP server(s) and health check endpoints
- Graceful shutdown handling

## Running a command

Every door — ingest, ERP command, API-Webdienste, deadline — reaches a process
through `Process::run(cmd, RunOptions)`, or `Process::run_timeout(&deadline,
RunOptions)` for a fired deadline. A spawn passes its Frist and its business keys
as `RunOptions::deadlines` / `RunOptions::correlations`; a resume that indexes the
process under a further key (a Belegnummer) passes it the same way. Never call
`register_correlated` beside a run — `cargo xtask check-atomic-spawn` refuses it.

A workflow reaches the deadline scheduler and `POST /admin/migrations` through
its `WorkflowDriver`, registered once in its crate's `EngineModule::drivers()`.
`orchestrator/deadline_dispatch.rs` builds its name→driver map from
`production_modules()`; do not add a second name→type table — look the driver up
by name.

## Deadlines

A fired deadline takes one path: `deadline_dispatch` → `Process::run_timeout` →
the workflow's `on_deadline`. Each workflow owns its labels in
`Workflow::DEADLINE_LABELS`; the engine refuses to persist any other label
except a delivery window (`mako_fristen::is_delivery_window`), which the outbox
worker retires on delivery and the dispatcher only alerts on. Deadlines are never
cancelled, so every one fires on the healthy path too — `on_deadline` and the
`TimeoutExpired` arm both ask the same `window_open` guard. A lapse with no
sourced default outcome ends in `FristAbgelaufen`, not an invented answer.
`tests/it/deadline_labels.rs` pins all of this.

## Feature Flags

The `slatedb` feature is enabled at the `makod` level via its dependency on
`mako-engine = { ..., features = ["slatedb"] }` in `services/makod/Cargo.toml`.
Do **not** pass `--features slatedb` to `cargo build -p makod` — makod has no
such feature of its own:

```bash
# Correct: slatedb already activated via Cargo.toml dep declaration
cargo build -p makod --release
```

The `testing` feature must **never** appear in production builds. Use it only in `makod` integration tests.

## Configuration

The primary surface is **`makod.toml`**, deserialized into
`makod::core::config::ConfigFile` (`src/core/config.rs`, `deny_unknown_fields`)
and selected with `-c` / `--config <FILE>` or `MAKOD_CONFIG`. Several blocks
exist only there and have no CLI equivalent: `[[party]]` (operator identity —
at least one entry is required), `[storage.s3]`, `[authz]`, `[engine]`,
`[webdienste]`, `[marktd]`, `[maloid]`, and the `as4.*_pem_file` paths that keep
key material off the command line and out of the environment.

A subset of settings also has a CLI flag with a `MAKOD_*` environment override.
Precedence is **CLI flag → environment variable → config file → default**.

| Flag | Env var | Notes |
|---|---|---|
| `-c`, `--config <FILE>` | `MAKOD_CONFIG` | Path to `makod.toml` |
| `--data-dir <DIR>` | `MAKOD_DATA_DIR` | Omitting enables volatile in-memory mode — not for production |
| `--object-store <BACKEND>` | `MAKOD_OBJECT_STORE` | `local` / `s3` / `gcs` / `azure` (default `local`) |
| `--s3-bucket` / `--s3-endpoint` | `MAKOD_S3_BUCKET` / `MAKOD_S3_ENDPOINT` | S3/MinIO |
| `--gcs-bucket` | `MAKOD_GCS_BUCKET` | GCS |
| `--azure-container` / `--azure-account` | `MAKOD_AZURE_CONTAINER` / `MAKOD_AZURE_ACCOUNT` | Azure Blob Storage |
| `--log-level <LEVEL>` | `MAKOD_LOG_LEVEL` | default `info` |
| `--log-format <FORMAT>` | `MAKOD_LOG_FORMAT` | `pretty` / `json` |

Every CLI field must be reachable from the TOML file: the
`cli_fields_are_reachable_from_toml` guard test in `src/main.rs` fails the build
for a flag `apply_config_file` never reads.

## Health Checks

`/health/live`, `/health/ready` and `/health` (alias of ready) are mounted on every enabled port by `src/core/health.rs`. Keep them shallow: liveness is "process responds", readiness is "store reachable and no worker heartbeat stale". No business-logic checks.

## Error Handling

`anyhow` is acceptable in `makod`. Use it for startup/configuration errors. Domain logic errors (workflow errors, store errors) are typed via `thiserror` in the engine/domain crates — surface them as structured log events, not panics.

## Observability

- OTLP **traces** are pushed to the exporter named by `OTEL_EXPORTER_OTLP_ENDPOINT` (standard OpenTelemetry env vars) when one is configured.
- Prometheus **metrics** are **scraped**: `makod` mounts `GET /metrics` itself (`src/api/metrics_api.rs`, Prometheus text format, authenticated through Cedar).
- Structured logs: JSON format (`--log-format json`) for production; `pretty` for local development.

## Integration Tests

Every integration test is a module of the single binary `tests/it/` (`main.rs`
declares them). They run against `InMemoryEventStore` through the `testing`
feature, which the dev-dependencies enable:

```bash
cargo test -p makod --test it                 # the whole suite
cargo test -p makod --test it -- deadline_labels published_counts
```
