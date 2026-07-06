---
layout: default
title: mdmd Operator Guide
nav_order: 25
parent: Architecture
description: >
  mdmd operator guide: Master Data Manager daemon for Marktlokation, Messlokation,
  contracts, subscriptions, and trading-partner management. PostgreSQL-backed,
  OIDC-secured, OpenAPI 3.1, CloudEvents 1.0 outbound webhooks.
---

# `mdmd` Operator Guide

`mdmd` is the Master Data Manager daemon. It stores and serves the records that
`makod` consumes at the protocol layer: Marktlokationen (MaLo), Messlokationen
(MeLo), contracts, trading-partner channels, and ERP webhook subscriptions.

The two services have a clean separation of concerns:

| Service | Responsibility |
|---|---|
| `makod` | EDIFACT parsing, BDEW process rules, AS4 delivery, regulatory deadlines |
| `mdmd` | Master data (MaLo/MeLo/contracts), ERP subscriptions, process-correlation tracking |

They communicate via CloudEvents 1.0 webhooks: `makod` can push process lifecycle
events to `mdmd`'s ingest endpoint, and `mdmd` fans out to registered ERP subscribers.

---

## Port Layout

```
┌─────────────────────────────────────────────────────────────────┐
│  mdmd                                                           │
│                                                                 │
│  :8180  ← HTTP REST API (MaLo/MeLo/contracts/subscriptions)    │
│                                                                 │
│  GET /health  — liveness                                        │
│  GET /ready   — readiness (PostgreSQL connectivity)             │
└─────────────────────────────────────────────────────────────────┘
```

`mdmd` occupies `:8180` by default to avoid port conflicts with `makod` (`:8080`,
`:4080`, `:8090`). All endpoints including health are on the same port.

---

## Quick Start

### With Docker Compose

The minimal setup needs only PostgreSQL and a configured OIDC issuer:

```yaml
# docker-compose.yml
services:
  postgres:
    image: postgres:17
    environment:
      POSTGRES_DB:       mdmd
      POSTGRES_USER:     mdmd
      POSTGRES_PASSWORD: secret
    ports: ["5432:5432"]

  mdmd:
    image: ghcr.io/hupe1980/mdmd:0.7.0
    depends_on: [postgres]
    environment:
      DATABASE_URL:    postgres://mdmd:secret@postgres/mdmd
      MDMD_TENANT_GLN: "9900357000004"
      MDMD_AUTH_ISSUER:   https://auth.example.com
      MDMD_AUTH_AUDIENCE: mdmd
    ports: ["8180:8180"]
```

```bash
docker compose up -d
curl http://localhost:8180/health
# → {"status":"ok","db":"up"}
```

### Binary

```bash
mdmd \
  --database-url postgres://mdmd:secret@localhost/mdmd \
  --tenant-gln   9900357000004 \
  --auth-issuer  https://auth.example.com \
  --auth-audience mdmd \
  --addr 0.0.0.0:8180
```

Migrations run automatically at startup. No separate `migrate` step is needed.

### Development — auth disabled

In development, pass `--auth-disabled` to skip JWT validation entirely:

> **⚠ Never use `--auth-disabled` in production.**

```bash
mdmd \
  --database-url postgres://mdmd:secret@localhost/mdmd \
  --tenant-gln   9900357000004 \
  --auth-disabled
```

---

## Configuration file (`mdmd.toml`)

`mdmd` can be configured entirely via a TOML file. CLI flags and environment
variables take precedence (see precedence order below).

```toml
addr       = "0.0.0.0:8180"
tenant_gln = "9900357000004"

[database]
url      = "postgres://mdmd:secret@localhost/mdmd"
max_conn = 20

[auth]
issuer       = "https://auth.example.com"
audience     = "mdmd"
jwks_refresh = 3600   # seconds between JWKS refresh
```

### Configuration precedence

```
CLI flags  >  environment variables  >  TOML file  >  defaults
```

### All environment variables

| Variable | CLI flag | Default |
|---|---|---|
| `DATABASE_URL` | `--database-url` | *(required)* |
| `MDMD_ADDR` | `--addr` | `0.0.0.0:8180` |
| `MDMD_TENANT_GLN` | `--tenant-gln` | *(required)* |
| `MDMD_AUTH_ISSUER` | `--auth-issuer` | *(required unless `--auth-disabled`)* |
| `MDMD_AUTH_AUDIENCE` | `--auth-audience` | *(required unless `--auth-disabled`)* |
| `MDMD_AUTH_JWKS_REFRESH_SECS` | `--auth-jwks-refresh-secs` | `3600` |
| `MDMD_AUTH_DISABLED` | `--auth-disabled` | `false` |
| `MDMD_LOG_FORMAT` | `--log-format` | `json` |
| `MDMD_LOG_LEVEL` | `--log-level` | `info` |

---

## REST API

Interactive OpenAPI documentation is available at `/swagger-ui/` when `mdmd` is running.
The raw spec can be fetched at `/api-docs/openapi.json`.

### Health and readiness

```bash
# Liveness (no DB check)
curl http://localhost:8180/health

# Readiness (checks PostgreSQL connectivity)
curl http://localhost:8180/ready
```

### Marktlokationen (MaLo)

```bash
# List — paginated, filterable by Sparte, zuordnungstyp, rollencodenummer
curl http://localhost:8180/api/v1/malos \
  -H "Authorization: Bearer $TOKEN" \
  | jq '.items[].malo_id'

# Fetch single MaLo (lokationszuordnung filtered to today CET/CEST)
curl http://localhost:8180/api/v1/malos/51238696780 \
  -H "Authorization: Bearer $TOKEN"

# Upsert — full replacement with optimistic concurrency (ETag)
curl -X PUT http://localhost:8180/api/v1/malos/51238696780 \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -H "If-Match: 3" \
  -d '{
    "sparte": "STROM",
    "data": { "_typ": "MARKTLOKATION", "marktlokationsId": "51238696780" },
    "lokationszuordnung": [
      {
        "zuordnungstyp":    "NB",
        "rollencodenummer": "9900357000004",
        "valid_from":       "2025-10-01",
        "valid_to":         null
      }
    ]
  }'
```

The `If-Match` header is optional. If supplied, it is compared against the current version
and a `412 Precondition Failed` is returned on mismatch. Omit it for an unconditional upsert.

### Lokationszuordnung — temporal role assignments

Each MaLo record carries a list of role-assignment records with explicit date validity.
`GET /api/v1/malos/{malo_id}` evaluates these against the current German calendar date
(CET/CEST, not UTC) and returns only the assignments valid today:

```json
{
  "malo_id": "51238696780",
  "sparte":  "STROM",
  "version": 3,
  "data":    { "_typ": "MARKTLOKATION", … },
  "lokationszuordnung": [
    {
      "zuordnungstyp":    "NB",
      "rollencodenummer": "9900357000004",
      "valid_from":       "2025-10-01",
      "valid_to":         null
    }
  ]
}
```

Historical assignments are retained in the database. They are filtered at query time, not
deleted, so you can audit the full history via direct SQL if needed.

### Messlokationen (MeLo)

```bash
# List MeLos for a given MaLo
curl "http://localhost:8180/api/v1/melos?malo_id=51238696780" \
  -H "Authorization: Bearer $TOKEN"

# Upsert
curl -X PUT http://localhost:8180/api/v1/melos/DE0001234567890123456789012345678 \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{ "malo_id": "51238696780", "sparte": "STROM", "data": { … } }'
```

### Contracts

```bash
# Upsert contract
curl -X PUT http://localhost:8180/api/v1/contracts/my-contract-id \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "malo_id": "51238696780",
    "sparte":  "STROM",
    "data":    { "lieferbeginn": "2026-01-01", "lieferende": null }
  }'
```

### Trading partners

```bash
# Upsert partner channels (GLN → AS4 endpoint or email)
curl -X PUT http://localhost:8180/api/v1/partners/9900000000001 \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "channels": [
      { "qualifier": "AK", "address": "https://partner.example/as4/inbox" }
    ]
  }'
```

---

## Event subscriptions and webhooks

`mdmd` provides a subscription API: ERP systems register a webhook endpoint and
receive CloudEvents 1.0 notifications when master data changes or when
`makod` pushes process lifecycle events through the ingest endpoint.

### Register a subscription

```bash
curl -X POST http://localhost:8180/api/v1/subscriptions \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "endpoint_url": "https://erp.example.com/mdm/events",
    "secret":       "mysecret64hexchars",
    "event_types":  ["de.mdm.malo.updated", "de.mdm.contract.updated"],
    "sparte":       "STROM",
    "role":         "NB"
  }'
```

Omit `sparte` and `role` to receive all events regardless of commodity or market role.
Omit `event_types` to subscribe to every event type.

### Webhook payload

Each delivery is `Content-Type: application/cloudevents+json`. When `secret` is set,
an HMAC-SHA256 signature is included:

```
POST https://erp.example.com/mdm/events
Content-Type: application/cloudevents+json
X-Mdm-Signature: <hex>
```

```json
{
  "specversion":      "1.0",
  "id":               "01932a4f-7b3e-4c5d-8f6a-9e0b1c2d3e4f",
  "source":           "urn:mdm:tenant:9900357000004",
  "type":             "de.mdm.malo.updated",
  "time":             "2026-10-01T10:15:00+02:00",
  "subject":          "51238696780",
  "datacontenttype":  "application/json",
  "mdmmaloid":        "51238696780",
  "mdmrole":          "NB",
  "data": { "_typ": "MARKTLOKATION", … }
}
```

### Verify the signature

```python
import hmac, hashlib

def verify(secret: str, body: bytes, header: str) -> bool:
    mac = hmac.new(secret.encode(), body, hashlib.sha256).hexdigest()
    return hmac.compare_digest(mac, header)
```

### Test a subscription

Send a test event directly to a single subscription's endpoint without going through
the fan-out queue. Returns the webhook's actual HTTP status:

```bash
curl -X POST http://localhost:8180/api/v1/subscriptions/SUBID/test \
  -H "Authorization: Bearer $TOKEN"
# → 200 OK  (webhook responded 200)
# → 502 Bad Gateway  (webhook returned an error)
```

---

## Inbound events from `makod`

`mdmd` can receive process lifecycle events from `makod` via `POST /api/v1/events`.
This enables a clean push model: `makod` fires a webhook on every significant state
transition (APERAK received, process completed, deadline expired) and `mdmd` correlates
it with master data and fans out to ERP subscribers.

### Configure `makod` to push events to `mdmd`

```toml
# makod.toml
[erp]
webhook_url    = "http://mdmd:8180/api/v1/events"
webhook_secret = "shared-hmac-secret"
```

### Idempotency

Inbound event delivery is idempotent. If `makod` retries a delivery, the duplicate
is detected by `event_id` and returns `202 Accepted` immediately without re-processing.

---

## Process correlations

`mdmd` tracks which MaKo processes are currently running against a given MaLo. This
lets ERP systems detect contention before initiating a new process:

```bash
curl http://localhost:8180/api/v1/correlations/51238696780 \
  -H "Authorization: Bearer $TOKEN"
```

```json
[
  {
    "malo_id":      "51238696780",
    "pid":          55001,
    "conv_id":      "018f3a2b-...",
    "initiated_at": "2026-07-01T08:00:00Z",
    "status":       "RUNNING"
  }
]
```

---

## Authentication and authorization
{: #authorization }

`mdmd` uses OIDC/JWT bearer tokens. The JWKS endpoint is auto-discovered from
`<issuer>/.well-known/jwks.json` and refreshed in the background.

**Supported signing algorithms: RS256, ES256, PS256 only.**
HS256 and HS512 are rejected — symmetric algorithms are not acceptable for OIDC
production deployments.

All error responses — including `401 Unauthorized` and `403 Forbidden` — follow
RFC 7807 Problem Details (`application/problem+json`):

```json
{
  "type":   "https://docs.rs/mako-mdm/latest/mako_mdm/error/",
  "title":  "Unauthorized",
  "status": 401,
  "detail": "missing or invalid Authorization header"
}
```

### Identity providers

| Provider | Notes |
|---|---|
| Keycloak | Issuer: `https://keycloak.example.com/realms/<realm>` |
| Azure AD / Entra ID | Issuer: `https://login.microsoftonline.com/<tenant-id>/v2.0` |
| Okta | Issuer: `https://<org>.okta.com/oauth2/default` |
| Kubernetes (OIDC) | Issuer: `https://kubernetes.default.svc` |

---

## Database

`mdmd` requires **PostgreSQL 15+**. Migrations run automatically at startup.

### PostgreSQL extensions

```sql
CREATE EXTENSION IF NOT EXISTS pgcrypto;
CREATE EXTENSION IF NOT EXISTS btree_gin;
```

`pgcrypto` provides UUID generation. `btree_gin` enables GIN indexes on JSONB columns
for fast full-text and structured data search.

### Schema

| Table | Purpose |
|---|---|
| `malos` | Marktlokation records with JSONB `data` and version counter |
| `melos` | Messlokation records with JSONB `data` |
| `lokationszuordnung` | Temporal role assignments (valid_from / valid_to) |
| `contracts` | Contract records with `created_at`, `updated_at` |
| `partners` | Trading partner channels (GLN → AS4 / email) |
| `subscriptions` | ERP webhook registrations |
| `process_correlations` | Running/completed process tracking per MaLo |
| `processed_events` | Inbound event dedup (7-day TTL, hourly cleanup) |

### Migrations

| File | Description |
|---|---|
| `0001_initial_schema.sql` | All core tables |
| `0002_improvements.sql` | `contracts.updated_at`, GIN indexes on JSONB, partial index on running correlations |

---

## Docker deployment

### Pre-built image

```bash
docker pull ghcr.io/hupe1980/mdmd:0.7.0
docker run --rm ghcr.io/hupe1980/mdmd:0.7.0 --help
```

Images are tagged with the semver version (`0.7.0`), major.minor (`0.7`), and `latest`.

### Running the container

```bash
docker run -d \
  --name mdmd \
  -p 8180:8180 \
  -e DATABASE_URL=postgres://mdmd:secret@postgres/mdmd \
  -e MDMD_TENANT_GLN=9900357000004 \
  -e MDMD_AUTH_ISSUER=https://auth.example.com \
  -e MDMD_AUTH_AUDIENCE=mdmd \
  ghcr.io/hupe1980/mdmd:0.7.0
```

### Kubernetes example

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: mdmd
spec:
  replicas: 2
  selector:
    matchLabels: { app: mdmd }
  template:
    metadata:
      labels: { app: mdmd }
    spec:
      containers:
        - name: mdmd
          image: ghcr.io/hupe1980/mdmd:0.7.0
          ports:
            - containerPort: 8180
          env:
            - name: DATABASE_URL
              valueFrom:
                secretKeyRef: { name: mdmd-secrets, key: database-url }
            - name: MDMD_TENANT_GLN
              value: "9900357000004"
            - name: MDMD_AUTH_ISSUER
              value: https://auth.example.com
            - name: MDMD_AUTH_AUDIENCE
              value: mdmd
          readinessProbe:
            httpGet: { path: /ready, port: 8180 }
            initialDelaySeconds: 5
            periodSeconds: 10
          livenessProbe:
            httpGet: { path: /health, port: 8180 }
            initialDelaySeconds: 10
            periodSeconds: 30
          resources:
            requests: { cpu: 100m, memory: 128Mi }
            limits:   { cpu: 500m, memory: 512Mi }
```

---

## Health checks

| Endpoint | Description | Use for |
|---|---|---|
| `GET /health` | Returns `{"status":"ok"}`. Never queries the DB. | Kubernetes `livenessProbe` |
| `GET /ready` | Pings the DB connection pool. Returns `503` if PostgreSQL is unreachable. | Kubernetes `readinessProbe` |

---

## Background workers

`mdmd` runs two background workers:

| Worker | Interval | Purpose |
|---|---|---|
| Fan-out worker | driven by `mpsc` channel | Delivers outbound CloudEvents to all matching webhook subscriptions |
| Cleanup worker | every 3600 s | Deletes `processed_events` rows older than 7 days (`MissedTickBehavior::Skip`) |

Both workers respect graceful shutdown via `CancellationToken` and drain cleanly
on `SIGTERM`.

---

## Logging

### Structured JSON (production)

```json
{"timestamp":"2026-07-05T10:00:00.123Z","level":"INFO","target":"mdmd","message":"upsert_malo","malo_id":"51238696780","version":4}
```

```bash
MDMD_LOG_FORMAT=json MDMD_LOG_LEVEL=info mdmd ...
```

### Human-readable (development)

```bash
MDMD_LOG_FORMAT=text MDMD_LOG_LEVEL=debug mdmd ...
```

---

## Relationship to `makod`

```
┌──────────────────────────────────────────────────────────────┐
│  makod :8080/:4080/:8090                                     │
│  EDIFACT ↔ AS4 ↔ MaKo process engine                        │
│  SlateDB event store                                         │
│          │                                                   │
│          │ POST /api/v1/events (CloudEvents 1.0 + HMAC)      │
│          ▼                                                   │
│  mdmd :8180                                                  │
│  PostgreSQL master data store                                │
│  webhook fan-out to ERP                                      │
│          │                                                   │
│          │ POST (CloudEvents 1.0 + HMAC)                     │
│          ▼                                                   │
│  ERP system (SAP, Schleupen, Wilken, …)                      │
└──────────────────────────────────────────────────────────────┘
```

`makod` and `mdmd` are independently deployable. You can run `makod` without `mdmd`
(protocol processing only) or `mdmd` without `makod` (master data API only).
The webhook integration is entirely optional and configured on the `makod` side.

---

## See Also

- [`makod` Operator Guide](./makod.md) — protocol daemon, AS4, SlateDB, Cedar ABAC
- [ERP Integration](./erp-integration.md) — CloudEvents 1.0 webhooks, BO4E schema, HMAC verification
- [Domain Model](./domain-model.md) — MaLo, MeLo, NeLo, identifier formats, Marktrollen
- [Getting Started](./getting-started.md) — first workflow in 5 minutes
