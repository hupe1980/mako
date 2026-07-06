# mdmd — Master Data Manager daemon

**Production HTTP service for Marktlokation, Messlokation, and trading-partner master data
in German energy market communication (MaKo/MDM).**

`mdmd` is the companion service to [`makod`](../../services/makod): while `makod` handles
EDIFACT protocol processing, `mdmd` owns the master data records that sit below the protocol
layer — market locations, metering locations, contracts, subscriptions, and trading-partner
channels. The two services communicate via CloudEvents 1.0 webhooks.

---

## At a glance

| Feature | Detail |
|---|---|
| **HTTP port** | `:8180` |
| **Database** | PostgreSQL 15+ (sqlx 0.8, compile-time query-free) |
| **Auth** | OIDC/JWT (RS256 / ES256 / PS256), JWKS background refresh |
| **API spec** | OpenAPI 3.1 at `/swagger-ui/` and `/api-docs/openapi.json` |
| **Events** | Outbound CloudEvents 1.0 (`application/cloudevents+json`) + HMAC-SHA256 |
| **Idempotency** | Inbound `POST /api/v1/events` uses `INSERT … ON CONFLICT DO NOTHING` |
| **Body limit** | 2 MiB per request |
| **Errors** | RFC 7807 Problem Details (`application/problem+json`) on all error responses |

---

## REST API

| Method | Path | Description |
|---|---|---|
| `GET` | `/health` | Liveness probe |
| `GET` | `/ready` | Readiness probe (PostgreSQL connectivity) |
| `GET` | `/api/v1/malos` | List Marktlokationen (paginated, filterable) |
| `GET` | `/api/v1/malos/{malo_id}` | Fetch single MaLo (at German reference date) |
| `PUT` | `/api/v1/malos/{malo_id}` | Upsert MaLo + Lokationszuordnung |
| `GET` | `/api/v1/melos` | List Messlokationen (paginated) |
| `GET` | `/api/v1/melos/{melo_id}` | Fetch single MeLo |
| `PUT` | `/api/v1/melos/{melo_id}` | Upsert MeLo |
| `GET` | `/api/v1/contracts` | List contracts (paginated) |
| `GET` | `/api/v1/contracts/{id}` | Fetch single contract |
| `PUT` | `/api/v1/contracts/{id}` | Upsert contract |
| `GET` | `/api/v1/partners` | List trading partners (paginated) |
| `GET` | `/api/v1/partners/{gln}` | Fetch partner by GLN |
| `PUT` | `/api/v1/partners/{gln}` | Upsert partner |
| `GET` | `/api/v1/subscriptions` | List event subscriptions |
| `POST` | `/api/v1/subscriptions` | Register webhook subscription |
| `DELETE` | `/api/v1/subscriptions/{id}` | Remove subscription |
| `POST` | `/api/v1/subscriptions/{id}/test` | Send test event to one subscription endpoint |
| `POST` | `/api/v1/events` | Ingest inbound CloudEvent (idempotent) |
| `GET` | `/api/v1/correlations/{malo_id}` | Query active process correlations for a MaLo |

---

## Quick start

### With Docker Compose

```yaml
services:
  postgres:
    image: postgres:17
    environment:
      POSTGRES_DB: mdmd
      POSTGRES_USER: mdmd
      POSTGRES_PASSWORD: secret
    ports: ["5432:5432"]

  mdmd:
    image: ghcr.io/hupe1980/mdmd:0.7.0
    depends_on: [postgres]
    environment:
      DATABASE_URL: postgres://mdmd:secret@postgres/mdmd
      MDMD_TENANT_GLN: "9900357000004"
      MDMD_AUTH_ISSUER: https://auth.example.com
      MDMD_AUTH_AUDIENCE: mdmd
    ports: ["8180:8180"]
```

### Binary

```bash
mdmd \
  --database-url postgres://mdmd:secret@localhost/mdmd \
  --tenant-gln 9900357000004 \
  --auth-issuer https://auth.example.com \
  --auth-audience mdmd \
  --addr 0.0.0.0:8180
```

### Environment variables

Every CLI flag has a corresponding environment variable with the `MDMD_` prefix:

| Variable | CLI flag | Default |
|---|---|---|
| `DATABASE_URL` | `--database-url` | required |
| `MDMD_ADDR` | `--addr` | `0.0.0.0:8180` |
| `MDMD_TENANT_GLN` | `--tenant-gln` | required |
| `MDMD_AUTH_ISSUER` | `--auth-issuer` | required |
| `MDMD_AUTH_AUDIENCE` | `--auth-audience` | required |
| `MDMD_AUTH_JWKS_REFRESH_SECS` | `--auth-jwks-refresh-secs` | `3600` |

---

## Configuration file (`mdmd.toml`)

```toml
addr       = "0.0.0.0:8180"
tenant_gln = "9900357000004"

[database]
url          = "postgres://mdmd:secret@localhost/mdmd"
max_conn     = 20

[auth]
issuer       = "https://auth.example.com"
audience     = "mdmd"
jwks_refresh = 3600   # seconds
```

---

## Database migrations

Migrations run automatically at startup. The schema is managed via sqlx migrations
under `migrations/`:

| Migration | Description |
|---|---|
| `0001_initial_schema.sql` | Core tables: `malos`, `melos`, `lokationszuordnung`, `contracts`, `partners`, `subscriptions`, `processed_events` |
| `0002_improvements.sql` | `contracts.updated_at`, GIN indexes on JSONB columns, partial index on running processes |

### PostgreSQL requirements

- PostgreSQL 15+
- Extensions: `pgcrypto` (UUID generation), `btree_gin` (GIN index support)

```sql
CREATE EXTENSION IF NOT EXISTS pgcrypto;
CREATE EXTENSION IF NOT EXISTS btree_gin;
```

---

## Authentication and authorization

`mdmd` uses OIDC/JWT bearer tokens. The JWKS endpoint is discovered from the issuer URL
(`<issuer>/.well-known/jwks.json`) and refreshed in the background every `jwks_refresh` seconds.

Supported signing algorithms: **RS256, ES256, PS256** only.
HS256 and HS512 are rejected — symmetric keys are not acceptable for public OIDC issuers.

All error responses (including `401 Unauthorized` and `403 Forbidden`) return RFC 7807
Problem Details:

```json
{
  "type": "https://docs.rs/mako-mdm/latest/mako_mdm/error/enum.MdmError.html#variant.Forbidden",
  "title": "Forbidden",
  "status": 403,
  "detail": "insufficient scope: requires mdm:malo:write"
}
```

---

## Events

### Outbound webhooks (push)

Register a subscription to receive CloudEvents 1.0 webhooks:

```bash
curl -s -X POST http://localhost:8180/api/v1/subscriptions \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "endpoint_url": "https://erp.example.com/mdm/events",
    "secret":       "mysecret",
    "event_types":  ["de.mdm.malo.updated", "de.mdm.contract.updated"],
    "sparte":       "STROM",
    "role":         "NB"
  }'
```

The webhook is delivered as `application/cloudevents+json`. When `secret` is set, an
`X-Mdm-Signature: <hmac-sha256-hex>` header is included for verification:

```python
import hmac, hashlib

def verify(secret: str, body: bytes, signature: str) -> bool:
    expected = hmac.new(secret.encode(), body, hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected, signature)
```

### Inbound events (pull from makod)

`mdmd` can receive process lifecycle events from `makod` via `POST /api/v1/events`.
Delivery is idempotent — duplicate CloudEvent IDs are silently ignored.

Configure `makod` to push events to `mdmd`:

```toml
# makod.toml
[erp]
webhook_url    = "http://mdmd:8180/api/v1/events"
webhook_secret = "shared-hmac-secret"
```

---

## Temporal location assignments (Lokationszuordnung)

Every `MaLo` carries a list of role assignments (`lokationszuordnung`) with validity
date ranges in German local time (CET/CEST):

```json
{
  "malo_id": "51238696780",
  "sparte": "STROM",
  "lokationszuordnung": [
    {
      "zuordnungstyp":    "NB",
      "rollencodenummer": "9900357000004",
      "valid_from":       "2025-10-01",
      "valid_to":         null
    },
    {
      "zuordnungstyp":    "LF",
      "rollencodenummer": "9900000000001",
      "valid_from":       "2026-01-01",
      "valid_to":         "2026-09-30"
    }
  ]
}
```

`GET /api/v1/malos/{malo_id}` returns only the assignments valid on the current German
calendar date (evaluated in CET/CEST, not UTC). Historical assignments are stored in the
database but filtered server-side by the query.

---

## Process correlations

`mdmd` tracks which MaKo processes are currently running against a MaLo, enabling ERP
systems to detect contention before initiating a new process:

```
GET /api/v1/correlations/51238696780

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

## Building from source

```bash
# Full workspace CI gate (includes mdmd):
just ci

# Build only mdmd:
cargo build -p mdmd --release

# Run tests (requires PostgreSQL on localhost):
cargo test -p mdmd --all-features
```

Requires a running PostgreSQL instance for integration tests. Set `DATABASE_URL` before
running tests:

```bash
export DATABASE_URL=postgres://mdmd:secret@localhost/mdmd
cargo test -p mdmd
```
