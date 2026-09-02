# mako-service

**The shared SDK for all mako microservices.**

Every mako daemon is built on `mako-service`. It solves the cross-cutting
concerns that every service needs — configuration, authentication, structured
logging, graceful shutdown, health endpoints, metrics, and more — so service
code focuses on domain logic instead of plumbing.

```
             ┌──────────────────────────────────────────────────────────────┐
             │                     mako-service SDK                          │
             │                                                              │
             │  config   shutdown  oidc       mcp_auth   telemetry           │
             │  health   http      cedar      metrics    outbox              │
             │  webhook  builder   rate_limit cloudevent                     │
             └──────────────────────────────────────────────────────────────┘
                  ↑            ↑             ↑           ↑
               makod        marktd        invoicd     processd  … (all 16)
```

---

## Module overview

| Module | Key exports | Purpose |
|---|---|---|
| `service` | `run`, `Daemon`, `ServiceConfig`, `ServiceContext` | **The daemon lifecycle owner** — `main` = `run::<D>().await` |
| `error` | `ApiError`, `ApiResult` | Shared HTTP error → JSON problem body (`?`-friendly) |
| `config` | `load_config`, `DatabaseConfig`, `HttpConfig` | Layered TOML + env-var config loading |
| `shutdown` | `token()`, `serve()` | Graceful shutdown — SIGINT **and** SIGTERM |
| `oidc` | `OidcConfig`, `OidcVerifier`, `Claims` | OIDC/JWT verification + `build_verifier()` factory |
| `mcp_auth` | `McpAuth`, `McpAuthConfig`, `McpApiKey`, `McpIdentity` | Unified MCP server authentication |
| `telemetry` | `init_tracing`, `init_tracing_from_env`, `OtelConfig` | Structured JSON logging + OTel OTLP |
| `cedar` | `CedarEnforcer` | Cedar ABAC policy enforcement |
| `health` | `health_routes` | `/health/live` + `/health/ready` endpoints |
| `http` | `default_client` | `reqwest::Client` with connect + request timeouts |
| `webhook` | `sign`, `verify_hmac`, `hmac_hex` | The one canonical HMAC-SHA256 signer/verifier (`sha256=<hex>`) |
| `cloudevent` | `CloudEvent`, `source`, `post_ce_with_retry` | Canonical CloudEvents 1.0 envelope + signed, retried publisher |
| `outbox` | `enqueue`, `OutboxWorker`, `ensure_schema` | Transactional outbox — persist-before-dispatch + drain worker + DLQ |
| `builder` | `ServiceBuilder` | Composable Axum router with health, metrics, rate-limit |
| `metrics` | Prometheus handler | Real `GET /metrics` when feature `metrics` is enabled |
| `rate_limit` | `RateLimitConfig` | GCRA rate limiting via `governor` |

---

## Quick-start: `main` is one line

`run::<D>()` owns the whole lifecycle — tracing, the **tuned** pool (with
`application_name`), migrations, a **real** `/health/ready` (a `SELECT 1` DB
ping, not `|| true`), the infra routes (health / metrics / tracing), bind, and
graceful SIGINT/SIGTERM shutdown. A service supplies only its config type,
migrations, and domain router + workers:

```rust,no_run
use std::sync::Arc;
use axum::Router;
use mako_service::{Daemon, ServiceContext, ServiceConfig, config::DatabaseConfig};

#[derive(serde::Deserialize)]
struct MyConfig { database: DatabaseConfig, port: Option<u16> }
impl ServiceConfig for MyConfig {
    fn database(&self) -> &DatabaseConfig { &self.database }
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

    async fn build(cfg: Arc<MyConfig>, ctx: ServiceContext) -> anyhow::Result<Router> {
        // spawn workers on ctx.shutdown; build the domain router with ctx.pool …
        Ok(Router::new())
    }
}
```

### One optional hook: `tracing_layer`

`Daemon::tracing_layer() -> Option<ExtraLayer>` installs one extra `tracing`
layer on the registry, ahead of the filter and the formatter. It exists for a
daemon embedding a library that **emits** metrics without choosing an exporter:
`agentplane` publishes its whole instrument catalogue as `tracing` events on a
dedicated target and leaves the bridge to whoever embeds it, so `agentd` returns
a layer that turns those events into Prometheus series on the registry
`GET /metrics` already serves. Without a seam like this the counters are emitted
and collected by nobody — which, on a dashboard, reads exactly like a service
where nothing happens.

It is called once, before the config is loaded, so it takes no arguments. The
layer is typed against the bare `Registry` because it is added first; a layer
added later would be typed against a `Layered<…>` stack no caller can name.

```rust,ignore
fn tracing_layer() -> Option<mako_service::ExtraLayer> {
    Some(Box::new(MyMetricBridge))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<MyService>().await
}
```

---

## Configuration

### Shared config structs

Services use shared structs from `mako-service` instead of defining their own:

```rust,no_run
use mako_service::{DatabaseConfig, HttpConfig};
use mako_service::mcp_auth::McpAuthConfig;
use mako_service::oidc::OidcConfig;
use mako_service::telemetry::OtelConfig;

#[derive(serde::Deserialize)]
struct MyConfig {
    pub database: DatabaseConfig,       // [database] section — url + pool_size
    pub http:     HttpConfig,           // [http] section — listen addr
    pub mcp:      McpAuthConfig,        // [mcp] section — api_key + named keys
    pub oidc:     Option<OidcConfig>,   // [oidc] section — omit for dev mode
    pub otel:     OtelConfig,           // [otel] section — omit to disable tracing
}
```

### TOML example

```toml
[database]
url       = "env:DATABASE_URL"   # defer to env at runtime
pool_size = 10

[http]
addr = "0.0.0.0:9080"

[mcp]
api_key = "env:MY_SERVICE_MCP_API_KEY"   # Bearer token for agentd LLM client

# Optional named keys for per-caller audit:
[[mcp.named_keys]]
name    = "billing-bot"
api_key = "env:BILLING_BOT_KEY"

[oidc]                   # omit section → dev mode (no auth required)
issuer   = "https://login.microsoftonline.com/{tid}/v2.0"
audience = "api://my-service"

[otel]                   # omit section → disable distributed tracing
endpoint = "http://otel-collector:4317"
```

### Environment-variable overrides

Every TOML key is overridable via a `SERVICE_SECTION__KEY` env var (double-underscore = section separator):

```bash
MY_SERVICE_DATABASE__URL=postgres://prod/my-service
MY_SERVICE_MCP__API_KEY=agentd-secret
```

### Kubernetes Secret files (`_FILE` suffix)

```bash
MY_SERVICE_DATABASE__URL_FILE=/run/secrets/db-url        # contents → url
MY_SERVICE_MCP__API_KEY_FILE=/run/secrets/mcp-api-key    # contents → api_key
```

---

## OIDC + MCP authentication

### Build a verifier from config

```rust,no_run
use mako_service::http::default_client;
use mako_service::oidc::OidcConfig;

let http = default_client();
// Builds OidcVerifier (with background JWKS refresh) OR disabled dev-mode verifier:
let oidc = OidcConfig::build_verifier(cfg.oidc.as_ref(), &http, &cfg.tenant, ct.clone()).await?;
```

### MCP server authentication

`McpAuth` covers every deployment scenario with a single type and handles JWT routing,
constant-time API-key comparison, and Cedar policy checks:

```rust,no_run
use mako_service::mcp_auth::McpAuth;

// OIDC + Cedar + agentd API-key fallback (production):
let auth = McpAuth::from_auth_config_oidc(&cfg.mcp, oidc, Some(cedar), &tenant);

// API-key only (services without an IdP):
let auth = McpAuth::from_auth_config(&cfg.mcp, &tenant);
```

In every service, `mcp_auth_middleware` is a single line:

```rust,no_run
async fn mcp_auth_middleware(
    axum::extract::State(s): axum::extract::State<std::sync::Arc<MyMcpState>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    s.auth.authenticate(req, next).await
}
```

On success, `McpAuth` injects `McpIdentity { name, method }` as an Axum extension
so handlers can audit which caller (OIDC `sub`, API-key name, or `"dev-mode"`) made the request.

---

## Graceful shutdown

`shutdown::token()` creates a `CancellationToken` wired to both `SIGINT` (Ctrl-C)
and `SIGTERM` (Kubernetes pod eviction). Pass it to background tasks and the final
`serve()` call:

```rust,no_run
use mako_service::shutdown;

let ct = shutdown::token();
tokio::spawn(background_worker(ct.clone()));
let listener = tokio::net::TcpListener::bind("0.0.0.0:9080").await?;
shutdown::serve(listener, app, ct).await  // waits for signal, drains connections
```

Plain `tokio::signal::ctrl_c().await` misses `SIGTERM` — pods evicted by Kubernetes
get `SIGTERM` first.

---

## Telemetry

```rust,no_run
// One-liner: reads LOG_LEVEL/RUST_LOG and OTEL_EXPORTER_OTLP_ENDPOINT from env
let _guard = mako_service::init_tracing_from_env("my-service");

// Explicit control:
let _guard = mako_service::init_tracing("my-service", "debug", Some(&cfg.otel));
```

> **Keep `_guard` alive** until process exit — dropping it flushes OTel spans.
> Use `let _guard = …` (not `let _ = …`).

---

## Other utilities

### Error handling

Return `ApiResult<T>` from handlers and use `?` — every error renders as the same
JSON problem body with the right status, and internal errors are logged, never
leaked:

```rust,no_run
use mako_service::{ApiError, ApiResult};
use axum::Json;

async fn get_order(id: String, pool: sqlx::PgPool) -> ApiResult<Json<String>> {
    if id.is_empty() {
        return Err(ApiError::bad_request("id required"));   // → 400
    }
    // `?` maps sqlx RowNotFound → 404, any other DB error → 500 (logged)
    let row: (String,) = sqlx::query_as("SELECT name FROM orders WHERE id = $1")
        .bind(&id).fetch_one(&pool).await?;
    Ok(Json(row.0))
}
```

**A 422 can carry structure, not just a sentence.** `unprocessable_with` merges
an object's keys into the problem body alongside `error` and `detail`, so a
caller can branch on *why* a payload was refused rather than parsing prose. The
BO4E gate is what this exists for — its rejection names the stage that refused
and the JSON-path or rule that stopped it:

```rust,no_run
# use mako_service::{ApiError, ApiResult};
# fn demo(e: impl std::fmt::Display, detail: serde_json::Map<String, serde_json::Value>) -> ApiError {
ApiError::unprocessable_with(e.to_string(), detail.into())
# }
```

```json
{
  "error":  "Unprocessable Entity",
  "detail": "MARKTLOKATION carries 1 out-of-schema enum value(s) at: sparte",
  "code":   "bo4e.unknown_enum",
  "paths":  ["sparte"]
}
```

`error` and `detail` are never overwritten by the extra keys: the shape of a
problem body is this type's contract, not the caller's.

### Health endpoints

```rust,no_run
use mako_service::health::health_routes;

let app = Router::new()
    .merge(health_routes(|| async { pool.acquire().await.is_ok() }));
// GET /health/live  → 200 always
// GET /health/ready → 200 when ready_fn returns true, 503 otherwise
```

### HTTP client

```rust,no_run
// Never use reqwest::Client::new() — no connect timeout → startup hangs
let http = mako_service::http::default_client();
// 5 s connect timeout · 30 s request timeout · pool_max_idle_per_host = 4
```

---

## CloudEvents transport

`mako-service` owns the whole CloudEvents *transport* layer — the envelope, the
outbound publisher, and the one HMAC signer/verifier — so every daemon puts the
same bytes on the wire. The event *type* names (and the glob matcher) live in the
zero-dependency [`mako-events`](../mako-events/) catalog; everything about how an
event is built, signed, and delivered lives here.

### Emit an event

```rust,no_run
use mako_service::{CloudEvent, source, post_ce_with_retry, http::default_client};

let ce = CloudEvent::new(
    source("billingd", &tenant),                 // urn:mako:billingd:tenant:<tenant>
    mako_service::cloud_events::billing::RECHNUNG_ERSTELLT, // type from the catalog
    &malo_id,                                     // subject
    serde_json::json!({ "betrag": "42.00" }),     // data
);
// Signs (webhook-signature: v1,<base64>) when the secret is Some, sends
// Content-Type: application/cloudevents+json, retries transient failures 3×,
// and returns immediately on a permanent 4xx.
post_ce_with_retry(&default_client(), &webhook_url, &ce, secret.map(str::as_bytes)).await?;
```

`CloudEvent::new` fixes the whole envelope by construction: `specversion = "1.0"`,
`id` = UUID v4 (override with `.with_id` to carry an idempotency key), `time` =
now in **RFC3339**, `datacontenttype = "application/json"`. Extension attributes
(`makopid`, `traceparent`, …) chain via `.extension(k, v)` / `.extension_opt`.

### Sign / verify

```rust,no_run
use mako_service::webhook::{sign, verify_hmac};

let header = sign(secret, &body);               // "sha256=<hex>" — the canonical form
let ok = verify_hmac(secret, &body, provided);  // constant-time; tolerates bare hex too
```

There is exactly **one** signer and **one** verifier in the workspace. `sign`
always emits the `sha256=` prefix; `verify_hmac` accepts it or a bare hex digest,
so producer and consumer can never disagree on the format.

### Never lose an event: the transactional outbox

An emitter must never drop a domain event because the HTTP POST failed *after*
the business row committed. `outbox` is the fix — *persist-before-dispatch*:
write the event to a table **in the same transaction as the business write**,
then a background worker delivers it (at-least-once, retried, dead-lettered).

```rust,no_run
# async fn ex(pool: sqlx::PgPool, ce: mako_service::CloudEvent, ct: tokio_util::sync::CancellationToken) -> Result<(), sqlx::Error> {
// One-time, at startup:
mako_service::outbox::ensure_schema(&pool).await?;
tokio::spawn(mako_service::outbox::OutboxWorker::new(pool.clone(), "https://erp/events", None).run(ct));

// In a handler — event and domain write commit together, or not at all:
let mut tx = pool.begin().await?;
// … the business INSERT/UPDATE on &mut *tx …
mako_service::outbox::enqueue(&mut tx, &ce).await?;
tx.commit().await?;
# Ok(()) }
```

Delivery reuses `post_ce_with_retry` (signing, `X-Idempotency-Key`,
permanent-vs-transient), so a receiver dedups the at-least-once duplicates on the
CloudEvent `id`. The worker claims batches with a lease (`FOR UPDATE SKIP LOCKED`
+ a forward-pushed `next_attempt_at`), so it holds no long locks and recovers
in-flight events after a crash. Dead-letters are a status column on the same row —
inspect and requeue with `outbox::list_dead_letters` / `outbox::requeue`.
Delivered rows are pruned hourly past `OutboxConfig::retention` (default 30 days),
so the table stays small.

---

## Feature flags

| Feature | What it enables |
|---|---|
| `oidc` | `OidcVerifier`, `Claims` extractor, JWKS background refresh |
| `cedar` | `CedarEnforcer`, Cedar ABAC policy evaluation |
| `otel` | OpenTelemetry OTLP/gRPC traces via `tracing-opentelemetry` |
| `metrics` | Real Prometheus `/metrics` + `mako_http_requests_total` counter |
| `rate-limit` | GCRA rate limiter via `governor` |
| `kafka` | `KafkaBus` for high-throughput CloudEvent fan-out |

Typical production config:

```toml
[dependencies]
mako-service = { workspace = true, features = ["oidc", "cedar", "otel"] }
```
