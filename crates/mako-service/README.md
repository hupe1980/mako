# mako-service

The service SDK every mako daemon except `makod` is built on. `run::<D>()` owns
the lifecycle — tracing, layered config, the tuned PostgreSQL pool, migrations,
`/health/*` and `/metrics`, graceful SIGINT/SIGTERM shutdown and the `--check`
flags — and the modules supply the shared OIDC, Cedar, Standard Webhooks,
CloudEvents and transactional-outbox plumbing, so a service's own code is domain
logic only. Workspace-internal (`publish = false`).

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

    async fn migrate(_cfg: &MyConfig, pool: &sqlx::PgPool) -> anyhow::Result<()> {
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

What an implementer needs beyond the snippet:

- **`build` binds nothing.** It must not bind a listener or add health routes;
  further ports come from `Daemon::listeners`, and the runner merges `/health/*`
  into each.
- **`validate` returns every finding.** `Daemon::validate` returns all
  `Befund`s, never the first; a blocking one refuses the boot, and
  `--check-config`, `--print-config` and `GET /admin/einrichtung` read the same
  list.
- **Two database roles.** `migrate` runs as `[database] migration_url` when set
  (one connection, closed after), `url` otherwise; with two roles the runner
  then grants `url`'s role `SELECT`, `INSERT`, `UPDATE`, `DELETE` on `public`
  and no DDL.
- **Config** is `<NAME>.toml` (or `<NAME>_CONFIG`), overridden per key by
  `<NAME>_<SECTION>__<KEY>`; a `*_FILE` variable reads the value from a file
  and `env:VAR` indirects a value inside the TOML.
- **Startup tests.** `build_for_test::<D>(cfg)` (feature `testing`) builds the
  router exactly as `run` does, so a test catches a route collision or a
  refused config.

## What it covers

| Module | Key items | Purpose |
|---|---|---|
| `service` | `run`, `Daemon`, `ServiceConfig`, `ServiceContext`, `Listener` | the daemon lifecycle |
| `befund` | `Befund`, `Schwere` | config findings with TOML path and source |
| `einrichtung` | `router`, `checkliste`, `befragung::vorschlag` | readiness report, deployment checklist, setup interview (`befragung::Rolle`, `lf-strom` … `bkv-strom`) |
| `policy` | `policy_choice!`, `Policy`, `PolicyReport`, `router` | operator settings under `[policy]`, logged, exported, served at `GET /api/v1/policy` |
| `error` | `ApiError`, `ApiResult` | problem body; a 500 or upstream failure carries an `error_id`, never the error text |
| `json` | `Json`, `Lenient` | request bodies; an undeclared key is a 422 naming its JSON path |
| `headers`, `dates` | `single_str`, `parse_iso_date` | a repeated header and a malformed date are refused by name |
| `config` | `load_config`, `DatabaseConfig`, `HttpConfig` | layered TOML and environment config |
| `webhook` | `sign`, `headers`, `verify_request` | [Standard Webhooks](https://www.standardwebhooks.com/): `webhook-signature: v1,<base64>` over `{id}.{timestamp}.{body}` |
| `cloudevent` | `CloudEvent`, `source`, `post_ce_with_retry` | CloudEvents 1.0 envelope and the signed, retried publisher |
| `outbox` | `enqueue`, `OutboxWorker`, `ensure_schema` | transactional outbox, drained by a worker |
| `oidc`, `cedar`, `mcp_auth` | `OidcVerifier`, `Claims`, `CedarEnforcer`, `McpAuth` | authentication and Cedar authorisation |
| `mcp`, `mcp_snapshot` | `REVISIONS` | the MCP revisions served; the pinned tool contract |
| `telemetry`, `metrics` | `init_tracing`, `init_metrics` | JSON logs, OTLP export, Prometheus |
| `builder`, `health`, `http`, `shutdown` | `ServiceBuilder`, `health_routes`, `default_client` | router, probes, HTTP client, SIGINT/SIGTERM |
| `rate_limit`, `worker_lock` | `RateLimitConfig` | GCRA limiting; one runner per periodic worker across replicas |
| `schema_check`, `sql` | `check_values_in`, `dynamic` | hold a SQL `CHECK` list against Rust; the one runtime-SQL assertion |

## Features

| Feature | Enables |
|---|---|
| `oidc` | `OidcVerifier`, the `Claims` extractor, JWKS refresh; implies `cedar` |
| `cedar` | `CedarEnforcer` |
| `otel` | OpenTelemetry OTLP/gRPC traces |
| `metrics` | Prometheus `GET /metrics` with the request counters and histogram |
| `mcp` | `mcp::REVISIONS` and the stateless Streamable HTTP service |
| `rate-limit` | the `governor` GCRA rate limiter |
| `testing` | `build_for_test` / `build_for_test_on` — dev-dependencies only |

Webhook contract: [ERP Integration](https://hupe1980.github.io/mako/docs/architecture/erp-integration/) ·
Operator settings: [Operator policy](https://hupe1980.github.io/mako/docs/reference/operator-policy/)

License: MIT OR Apache-2.0
