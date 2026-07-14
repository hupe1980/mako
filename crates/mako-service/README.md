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
             │  config   shutdown  oidc      mcp_auth  telemetry            │
             │  health   http      cedar     metrics   event_bus            │
             │  webhook  builder   rate_limit           mako-plugin          │
             └──────────────────────────────────────────────────────────────┘
                  ↑            ↑             ↑           ↑
               makod        marktd        invoicd     processd  … (all 16)
```

---

## Module overview

| Module | Key exports | Purpose |
|---|---|---|
| `config` | `load_config`, `DatabaseConfig`, `HttpConfig` | Layered TOML + env-var config loading |
| `shutdown` | `token()`, `serve()` | Graceful shutdown — SIGINT **and** SIGTERM |
| `oidc` | `OidcConfig`, `OidcVerifier`, `Claims` | OIDC/JWT verification + `build_verifier()` factory |
| `mcp_auth` | `McpAuth`, `McpAuthConfig`, `McpApiKey`, `McpIdentity` | Unified MCP server authentication |
| `telemetry` | `init_tracing`, `init_tracing_from_env`, `OtelConfig` | Structured JSON logging + OTel OTLP |
| `cedar` | `CedarEnforcer` | Cedar ABAC policy enforcement |
| `health` | `health_routes` | `/health/live` + `/health/ready` endpoints |
| `http` | `default_client` | `reqwest::Client` with connect + request timeouts |
| `webhook` | `verify_signature` | Constant-time HMAC-SHA256 webhook verification |
| `builder` | `ServiceBuilder` | Composable Axum router with health, metrics, rate-limit |
| `event_bus` | `EventBus`, `WebhookBus` | CloudEvent fan-out (webhook + optional Kafka) |
| `metrics` | Prometheus handler | Real `GET /metrics` when feature `metrics` is enabled |
| `rate_limit` | `RateLimitConfig` | GCRA rate limiting via `governor` |

---

## Quick-start: `main` skeleton

Every mako service `main` follows this pattern — no boilerplate, no copy-paste:

```rust,no_run
use mako_service::{load_config, init_tracing_from_env, shutdown};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _guard = init_tracing_from_env("my-service");   // structured JSON + OTel
    let cfg: MyConfig = load_config("my-service")?;
    let ct = shutdown::token();                          // SIGINT + SIGTERM → cancel

    // … build pool, assemble Axum router …

    let listener = tokio::net::TcpListener::bind(&cfg.http.addr).await?;
    shutdown::serve(listener, app, ct).await             // graceful drain on signal
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

### Webhook verification

```rust,no_run
use mako_service::webhook::verify_signature;

let ok = verify_signature(secret, &body, provided_signature);
// Accepts "sha256=…" and bare hex; constant-time comparison
```

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
| `plugins` | `PluginRegistry` and extension-point traits |
| `wasm-plugins` | WASM plugin loading via Extism/Wasmtime sandbox |

Typical production config:

```toml
[dependencies]
mako-service = { workspace = true, features = ["oidc", "cedar", "otel"] }
```
