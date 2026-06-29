---
description: "Use when working in services/makod: the production daemon that assembles all domain modules, configures persistence, handles startup validation, and wires up HTTP/health endpoints."
applyTo: "services/makod/**"
---

# makod Service Instructions

## Role

`makod` is the only binary crate that assembles all domain workflow crates (`mako-gpke`, `mako-wim`, `mako-geli-gas`, `mako-mabis`) into a single `EngineContext`. It owns:
- Persistence backend selection (in-memory vs. SlateDB)
- Object store configuration (local / S3 / GCS)
- OTLP instrumentation setup
- HTTP server(s) and health check endpoints
- Graceful shutdown handling

## Feature Flags

The `slatedb` feature **must only be enabled at this level** — never in library crate defaults:

```bash
cargo build -p makod --release --features slatedb
```

The `testing` feature must **never** appear in production builds. Use it only in `makod` integration tests.

## Configuration

All configuration is driven by CLI flags with `MAKOD_*` environment variable overrides. Key flags:

| Flag | Env var | Notes |
|---|---|---|
| `--data-dir <DIR>` | `MAKOD_DATA_DIR` | Omitting enables volatile in-memory mode — not for production |
| `--object-store <BACKEND>` | `MAKOD_OBJECT_STORE` | `local` / `s3` / `gcs` |
| `--s3-bucket` / `--s3-endpoint` | `MAKOD_S3_BUCKET` / `MAKOD_S3_ENDPOINT` | S3/MinIO |
| `--log-level <LEVEL>` | `MAKOD_LOG_LEVEL` | default `info` |
| `--log-format <FORMAT>` | `MAKOD_LOG_FORMAT` | `pretty` / `json` |

## Health Checks

`GET /health` is mounted on every enabled server port. It must return `200 OK` before the process is considered ready. Do not add business-logic checks to the health endpoint — keep it shallow (process alive + stores reachable).

## Error Handling

`anyhow` is acceptable in `makod`. Use it for startup/configuration errors. Domain logic errors (workflow errors, store errors) are typed via `thiserror` in the engine/domain crates — surface them as structured log events, not panics.

## Observability

- OTLP traces and metrics: configure via `OTEL_EXPORTER_OTLP_ENDPOINT` (standard OpenTelemetry env vars).
- Structured logs: JSON format (`--log-format json`) for production; `pretty` for local development.
- Metrics are exposed via OTLP push, not a scrape endpoint.

## Integration Tests

`services/makod/tests/` contains integration tests that build and run the daemon binary. These require the `testing` feature and use `InMemoryEventStore`. Run:

```bash
cargo test -p makod --all-features
```
