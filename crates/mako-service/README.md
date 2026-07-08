# mako-service

**Shared service infrastructure for mako daemons.**

`mako-service` provides the cross-cutting plumbing that every mako microservice
needs: a composable Axum router builder, a typed TOML configuration loader,
health-check routes, and HMAC-SHA256 webhook signature verification.

All production daemons (`invoicd`, `edmd`, `obsd`, …) build on this crate to
avoid duplicating boilerplate and to guarantee a consistent operational surface.

---

## Modules

| Module | What it provides |
|---|---|
| [`ServiceBuilder`] | Composable Axum router with health routes, Prometheus metrics stub, and domain route injection |
| [`load_config`] | Type-safe TOML config loader with environment-variable value interpolation |
| [`health_routes`] | `/health/live` (always 200) and `/health/ready` (caller-supplied readiness check) |
| [`verify_hmac`] | Constant-time HMAC-SHA256 webhook signature verification (accepts bare hex or `sha256=` prefix) |

---

## Quick start

### ServiceBuilder

```rust,no_run
use axum::{Router, routing::post};
use axum::http::StatusCode;
use mako_service::ServiceBuilder;

async fn my_handler() -> StatusCode { StatusCode::NO_CONTENT }

let app: Router = ServiceBuilder::new()
    .with_health(|| async { true })     // readiness check (e.g. DB ping)
    .with_metrics()                     // GET /metrics — Prometheus text format
    .merge(Router::new().route("/webhook", post(my_handler)))
    .build();
```

### Configuration loader

The loader reads a TOML file and resolves `"env:VAR_NAME"` string values from
the process environment at startup, keeping secrets out of config files:

```toml
# invoicd.toml
[server]
listen = "0.0.0.0:8280"

[makod]
url = "env:INVOICD_MAKOD_URL"   # resolved from environment at startup
```

```rust,no_run
use mako_service::load_config;
use serde::Deserialize;

#[derive(Deserialize)]
struct Config {
    server: ServerConfig,
    makod:  MakodConfig,
}
#[derive(Deserialize)]
struct ServerConfig { listen: String }
#[derive(Deserialize)]
struct MakodConfig  { url: String }

let cfg: Config = load_config("invoicd")?;
// Reads invoicd.toml (or the path in $INVOICD_CONFIG) and resolves env: refs.
```

### Webhook HMAC verification

```rust,no_run
use mako_service::webhook::verify_hmac;

let secret = b"my-webhook-secret";
let body   = request.body_bytes();
let sig    = request.header("X-Mako-Signature").unwrap_or_default();

if !verify_hmac(secret, &body, sig) {
    return Err(StatusCode::UNAUTHORIZED);
}
```

`verify_hmac` uses constant-time comparison (`subtle::ConstantTimeEq`) and
accepts both bare hex strings and `sha256=<hex>` prefixed signatures.

---

## Health endpoints

`health_routes(ready_fn)` wires two routes into the provided `Router`:

| Path | Behaviour |
|---|---|
| `GET /health/live` | Always returns `200 OK` — used by Kubernetes liveness probes |
| `GET /health/ready` | Calls `ready_fn`; returns `200 OK` when `true`, `503 Service Unavailable` when `false` |

The readiness check is typically a lightweight database ping:

```rust,no_run
use mako_service::health::health_routes;

let routes = health_routes(|| async {
    sqlx::query("SELECT 1").fetch_one(&pool).await.is_ok()
});
```

---

## Design notes

- **No tokio dependency in config/health/webhook** — the config loader and HMAC
  helper are synchronous; only `ServiceBuilder` has an async dependency via Axum.
- **`secrecy::SecretString`** — webhook secrets are held as `SecretString` to
  prevent accidental logging.
- **RustCrypto only** — HMAC uses `hmac 0.12` + `sha2 0.10`; no OpenSSL
  dependency.
- **MSRV 1.89** — matches the workspace minimum.
