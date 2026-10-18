# mako-service

The service SDK every mako daemon is built on. `run::<D>()` owns the daemon
lifecycle — tracing, layered config, the tuned PostgreSQL pool, migrations,
`/health/*` and `/metrics`, graceful SIGINT/SIGTERM shutdown and `--check` — and
the modules below supply the shared OIDC, Cedar, Standard Webhooks, CloudEvents
and transactional-outbox plumbing, so service code is domain logic only.
Workspace-internal (`publish = false`).

## Example

`main` is one line; a daemon supplies its config type, migrations and router:

```rust,no_run
use std::sync::Arc;
use axum::Router;
use mako_service::{Daemon, ServiceContext, ServiceConfig, config::DatabaseConfig};

#[derive(serde::Deserialize)]
struct MyConfig { database: DatabaseConfig, port: Option<u16> }
impl ServiceConfig for MyConfig {
    fn database(&self) -> Option<&DatabaseConfig> { Some(&self.database) }
    fn bind_addr(&self) -> String { format!("0.0.0.0:{}", self.port.unwrap_or(8080)) }
}

struct MyService;
impl Daemon for MyService {
    type Config = MyConfig;
    const NAME: &'static str = "my-service";

    async fn migrate(pool: &sqlx::PgPool) -> anyhow::Result<()> {
        sqlx::migrate!("./migrations").run(pool).await?;
        Ok(())
    }

    async fn build(_cfg: Arc<MyConfig>, _ctx: ServiceContext) -> anyhow::Result<Router> {
        // spawn workers on ctx.shutdown; build the domain router on ctx.pool()
        Ok(Router::new())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<MyService>().await
}
```

`build` must not bind a listener or add health routes — the runner mounts them,
and `build_for_test::<D>(cfg)` (feature `testing`) builds the router exactly as
`run` does so a startup test catches a route collision or a refused config.

## Modules

| Module | Key exports | Purpose |
|---|---|---|
| `service` | `run`, `Daemon`, `ServiceConfig`, `ServiceContext` | **The daemon lifecycle owner** — `main` = `run::<D>().await` |
| `error` | `ApiError`, `ApiResult`, `UpstreamFailure` | Shared HTTP error → JSON problem body (`?`-friendly); `ApiError::forbidden(reason)` is the one 403; a 500 (`ApiError::internal`) and a 502/503/504 (`ApiError::upstream_*`) carry an `error_id` the log line carries, never the error's text |
| `json` | `Json`, `Lenient`, `DETAIL_SENTINEL` | The request-body extractor — every rejection in the problem shape, and a key the type does not declare refused as a 422 with its JSON path; `Lenient` reads an externally specified schema without that refusal |
| `headers` | `single_str` | Identity/idempotency headers — a repeated header is refused, never first-wins |
| `dates` | `parse_iso_date`, `DateError` | The one request-date parser — a malformed date is a 400 naming the field, never `None` |
| `config` | `load_config`, `DatabaseConfig`, `HttpConfig` | Layered TOML + env-var config loading |
| `policy` | `policy_choice!`, `Policy`, `PolicyReport`, `router` | Operator settings under `[policy]` — each option sourced, logged at startup, exported as `mako_policy_info`, changes recorded in `service_policy_log`, served at `GET /api/v1/policy` |
| `shutdown` | `token()`, `serve()` | Graceful shutdown — SIGINT **and** SIGTERM |
| `oidc` | `OidcConfig`, `OidcVerifier`, `Claims`, `ExpectedTenant` | OIDC/JWT verification + `build_verifier()` factory |
| `mcp_auth` | `McpAuth`, `McpAuthConfig`, `McpApiKey`, `McpIdentity` | Unified MCP server authentication (needs `cedar` **and** `oidc`) |
| `telemetry` | `init_tracing`, `init_tracing_from_env`, `LogFormat`, `OtelConfig` | Structured logging (JSON by default) and the OTLP exporter |
| `cedar` | `CedarEnforcer` | Cedar ABAC policy enforcement |
| `health` | `health_routes` | `/health/live` + `/health/ready` endpoints |
| `http` | `default_client` | `reqwest::Client` with connect + request timeouts |
| `webhook` | `sign`, `headers`, `verify_request` | The one [Standard Webhooks](https://www.standardwebhooks.com/) signer/verifier (`webhook-signature: v1,<base64>`) |
| `cloudevent` | `CloudEvent`, `source`, `post_ce_with_retry` | Canonical CloudEvents 1.0 envelope + signed, retried publisher |
| `outbox` | `enqueue`, `OutboxWorker`, `ensure_schema` | Transactional outbox — persist-before-dispatch + drain worker + DLQ |
| `builder` | `ServiceBuilder` | Composable Axum router with health, metrics, rate-limit |
| `metrics` | Prometheus handler | `GET /metrics` |
| `rate_limit` | `RateLimitConfig` | GCRA rate limiting |
| `worker_lock` | Postgres advisory locks | One runner per periodic background worker across replicas |
| `schema_check` | `check_values_in`, `assert_agrees_in` | Reads a SQL `CHECK (col IN (…))` list, so a test can hold it against the Rust that writes the column |

Configuration is `<NAME>.toml` (or `<NAME>_CONFIG`), overridden per key by
`<NAME>_<SECTION>__<KEY>`; a `*_FILE` variable reads the value from a file and
`env:VAR` indirects a value inside the TOML.

## Features

| Feature | What it enables |
|---|---|
| `oidc` | `OidcVerifier`, `Claims` extractor, `ExpectedTenant`, JWKS background refresh |
| `cedar` | `CedarEnforcer`, Cedar ABAC policy evaluation |
| `otel` | OpenTelemetry OTLP/gRPC traces via `tracing-opentelemetry` |
| `metrics` | Real Prometheus `/metrics` + `mako_http_requests_total`, `mako_http_request_duration_seconds` and `mako_bo4e_decimal_from_json_number_total` |
| `rate-limit` | GCRA rate limiter via `governor` |
| `testing` | `build_for_test` / `build_for_test_on` — a daemon's router as `run` mounts it, for startup tests; a dev-dependency feature only |
