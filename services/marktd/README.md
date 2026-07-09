# marktd — Market Data Hub

**Pure data hub for German energy market communication (MaKo). No domain policy.**

`marktd` is the companion service to [`makod`](../../services/makod): while `makod` handles
EDIFACT protocol processing, `marktd` is the single source of truth for all market entity
state — market locations, metering locations, contracts, VersorgungsStatus (with full
history and `?at=YYYY-MM-DD` point-in-time queries), MaLo grid topology (`malo_grid`),
Netz-Element-Lokationen (NeLo) for Redispatch 2.0, trading-partner channels, price sheets,
and ERP webhook subscriptions. The two services communicate via HMAC-signed CloudEvents 1.0
webhooks.

**Architecture principle:** `marktd` contains **no domain policy**. Automated NB Anmeldung
STP decisions (PIDs 55001/55016/44001) are handled by [`processd`](../processd) via its
EventBus subscription. `marktd` emits events; `processd` reacts. This keeps `marktd`
independently testable and deployable without any decision logic.

`marktd` runs a **VersorgungsStatus derivation pipeline**: inbound `de.mako.process.completed`
events for PIDs 55003/44003 (Lieferbeginn) and 55013/44013 (Abmeldung) are automatically
converted into `LieferStatus` updates, persisted in `versorgungsstatus`, and appended to
`versorgungsstatus_history` — so ERP and `processd` always have fresh supply-state data
without manual intervention, and any historical state can be retrieved by date.

---

## At a glance

| Feature | Detail |
|---|---|
| **HTTP port** | `:8180` |
| **Database** | PostgreSQL 15+ (sqlx 0.8, compile-time query-free) |
| **Auth** | OIDC/JWT (RS256 / ES256 / PS256), JWKS background refresh |
| **Authorization** | Cedar ABAC (`policies/marktd.cedar`) — per-tenant, role-gated |
| **API spec** | OpenAPI 3.1 at `/swagger-ui/` and `/api-docs/openapi.json` |
| **Events** | Outbound CloudEvents 1.0 (`application/cloudevents+json`) + HMAC-SHA256 |
| **Event source** | `urn:markt:tenant:{tenant_gln}` |
| **CE extensions** | `marktrole`, `marktmaloid`, `marktmeloid`, `marktcontractid`, `markterpref` |
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
| `GET` | `/api/v1/nb-contracts/{id}` | Fetch NB network contract |
| `PUT` | `/api/v1/nb-contracts/{id}` | Upsert NB network contract |
| `GET` | `/api/v1/partners` | List trading partners (paginated) |
| `GET` | `/api/v1/partners/{mp_id}` | Fetch partner by GLN |
| `PUT` | `/api/v1/partners/{mp_id}` | Upsert partner |
| `GET` | `/api/v1/versorgung/{malo_id}` | Fetch VersorgungsStatus — add `?at=YYYY-MM-DD` for point-in-time |
| `GET` | `/api/v1/versorgung/{malo_id}/history` | Full supply-state change history (newest first, paged) |
| `PUT` | `/api/v1/versorgung/{malo_id}` | Admin override for VersorgungsStatus |
| `GET` | `/api/v1/malo/{id}/grid` | Fetch NB grid topology record for a MaLo (read by `processd` NB module) |
| `PUT` | `/api/v1/malo/{id}/grid` | Upsert NB grid topology (sourced from NIS/GIS; read by `processd`) |
| `GET` | `/api/v1/nelo` | List Netz-Element-Lokationen (`?nb_mp_id=` filter) |
| `GET` | `/api/v1/nelo/{id}` | Fetch a NeLo by EIC or BDEW Codenummer |
| `PUT` | `/api/v1/nelo/{id}` | Upsert a NeLo (NB role required) |
| `GET` | `/api/v1/subscriptions` | List event subscriptions |
| `POST` | `/api/v1/subscriptions` | Register webhook subscription |
| `DELETE` | `/api/v1/subscriptions/{id}` | Remove subscription |
| `POST` | `/api/v1/subscriptions/{id}/test` | Send test event to one subscription endpoint |
| `POST` | `/api/v1/events` | Ingest inbound CloudEvent (idempotent) |
| `GET` | `/api/v1/correlations/{malo_id}` | Query active process correlations for a MaLo |
| `GET` | `/api/v1/preisblaetter/{nb_mp_id}` | Fetch price sheet valid on query date |
| `PUT` | `/api/v1/preisblaetter/{nb_mp_id}` | Upsert price sheet + store versioned snapshot + emit `de.markt.pricat.published` |
| `GET` | `/api/v1/pricat/{nb_mp_id}/history` | PRICAT version history (newest first) |
| `GET` | `/api/v1/pricat/{nb_mp_id}/dispatch-log/{version_id}` | Dispatch audit log for a PRICAT version |
| `POST` | `/api/v1/pricat/{nb_mp_id}/dispatch` | Enqueue (re-)dispatch to all active LF partners |

---

## Quick start

### With Docker Compose

```yaml
services:
  postgres:
    image: postgres:17
    environment:
      POSTGRES_DB: marktd
      POSTGRES_USER: marktd
      POSTGRES_PASSWORD: secret
    ports: ["5432:5432"]

  marktd:
    image: ghcr.io/hupe1980/marktd:0.7.0
    depends_on: [postgres]
    environment:
      DATABASE_URL: postgres://marktd:secret@postgres/marktd
      MARKTD_TENANT: "9900357000004"
      MARKTD_AUTH_ISSUER: https://auth.example.com
      MARKTD_AUTH_AUDIENCE: marktd
    ports: ["8180:8180"]
```

### Binary

```bash
marktd \
  --database-url postgres://marktd:secret@localhost/marktd \
  --tenant 9900357000004 \
  --auth-issuer https://auth.example.com \
  --auth-audience marktd \
  --addr 0.0.0.0:8180
```

### Environment variables

Every CLI flag has a corresponding environment variable with the `MARKTD_` prefix:

| Variable | CLI flag | Default |
|---|---|---|
| `DATABASE_URL` | `--database-url` | required |
| `MARKTD_ADDR` | `--addr` | `0.0.0.0:8180` |
| `MARKTD_TENANT` | `--tenant` | required |
| `MARKTD_AUTH_ISSUER` | `--auth-issuer` | required |
| `MARKTD_AUTH_AUDIENCE` | `--auth-audience` | required |
| `MARKTD_AUTH_JWKS_REFRESH_SECS` | `--auth-jwks-refresh-secs` | `3600` |

---

## Configuration file (`marktd.toml`)

```toml
addr       = "0.0.0.0:8180"
tenant = "9900357000004"

[database]
url          = "postgres://marktd:secret@localhost/marktd"
max_conn     = 20

[auth]
issuer       = "https://auth.example.com"
audience     = "marktd"
jwks_refresh = 3600   # seconds
```

---

## Database migrations

Migrations run automatically at startup.  The schema is defined across two migration files:

- `migrations/0001_initial_schema.sql` — core tables
- `migrations/0002_phase3_history.sql` — supply-state history + NeLo (Redispatch 2.0)

Tables: `malo`, `melo`, `lokationszuordnung`, `contracts`, `nb_contracts`,
`versorgungsstatus`, `versorgungsstatus_history`, `nelo`, `preisblaetter`,
`pricat_versions`, `pricat_dispatch_log`, `partners`, `subscriptions`,
`process_correlation`, `processed_events`.

### PostgreSQL requirements

- PostgreSQL 15+
- Extensions: `pgcrypto` (UUID generation), `btree_gin` (GIN index support)

```sql
CREATE EXTENSION IF NOT EXISTS pgcrypto;
CREATE EXTENSION IF NOT EXISTS btree_gin;
```

---

## Authentication and authorization

`marktd` uses OIDC/JWT bearer tokens. The JWKS endpoint is discovered from the issuer URL
(`<issuer>/.well-known/jwks.json`) and refreshed in the background every `jwks_refresh` seconds.

Supported signing algorithms: **RS256, ES256, PS256** only.
HS256 and HS512 are rejected — symmetric keys are not acceptable for public OIDC issuers.

All error responses (including `401 Unauthorized` and `403 Forbidden`) return RFC 7807
Problem Details:

```json
{
  "type": "https://docs.rs/mako-markt/latest/mako_markt/error/enum.MdmError.html#variant.Forbidden",
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
    "endpoint_url": "https://erp.example.com/markt/events",
    "secret":       "mysecret",
    "event_types":  ["de.markt.malo.updated", "de.markt.pricat.published"],
    "sparte":       "STROM",
    "role":         "NB"
  }'
```

The webhook is delivered as `application/cloudevents+json`. When `secret` is set, an
`X-Mako-Signature: <hmac-sha256-hex>` header is included for verification:

```python
import hmac, hashlib

def verify(secret: str, body: bytes, signature: str) -> bool:
    expected = hmac.new(secret.encode(), body, hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected, signature)
```

### Inbound events (pull from makod)

`marktd` can receive process lifecycle events from `makod` via `POST /api/v1/events`.
Delivery is idempotent — duplicate CloudEvent IDs are silently ignored.

Configure `makod` to push events to `marktd`:

```toml
# makod.toml
[erp]
webhook_url    = "http://marktd:8180/api/v1/events"
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

`marktd` tracks which MaKo processes are currently running against a MaLo, enabling ERP
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
# Full workspace CI gate (includes marktd):
just ci

# Build only marktd:
cargo build -p marktd --release

# Run tests (requires PostgreSQL on localhost):
cargo test -p marktd --all-features
```

Requires a running PostgreSQL instance for integration tests. Set `DATABASE_URL` before
running tests:

```bash
export DATABASE_URL=postgres://marktd:secret@localhost/marktd
cargo test -p marktd
```
