---
layout: default
title: marktd Operator Guide
nav_order: 25
parent: Architecture
description: >
  marktd operator guide: Market Data Hub for Marktlokation, Messlokation,
  contracts, VersorgungsStatus (with history + point-in-time queries), NeLo
  (Redispatch 2.0), MaLo grid topology (NB STP), trading-partner management,
  price sheets, Cedar ABAC, EventBus fan-out.
  PostgreSQL-backed, OIDC-secured, OpenAPI 3.1, CloudEvents 1.0 outbound webhooks.
---

# `marktd` Operator Guide

`marktd` is the **Market Data Hub** — the single source of truth for all market entity
state in a MaKo deployment. It stores Marktlokationen (MaLo), Messlokationen (MeLo),
energy contracts, trading partners, network contracts (NbContractRecord), price sheets
(PreisblattNetznutzung), **VersorgungsStatus per MaLo** (with full history and
point-in-time queries), **MaLo grid topology** (`malo_grid` — sourced from the NB's
NIS/GIS system and provisioned via `PUT /api/v1/malo/{id}/grid`; read by `processd`
for Anmeldung STP decisions), and
**Netz-Element-Lokationen (NeLo)** for Redispatch 2.0.

Beyond data storage, `marktd` includes:

- **EventBus fan-out** — enriches inbound `de.mako.*` events with `marktrole` and fans out
  to all registered subscribers (ERP, `processd`, `invoicd`, `obsd`) via HMAC-signed webhooks.
- **VersorgungsStatus derivation** — on `de.mako.process.completed` with PIDs 55003/44003/55013/44013, derives and persists supply state per MaLo; maintains a full audit history for `?at=YYYY-MM-DD` point-in-time queries.

`marktd` is a **pure data hub**. Automated Anmeldung STP decisions are the
responsibility of `processd`'s NB module, which subscribes to `marktd`'s EventBus
and uses the pure `netz-checker` library for all decisions.
This separation keeps `marktd` free of domain policy and makes `processd` independently
scalable and testable.

```mermaid
graph TB
    makod["makod :8080\nEDIFACT ↔ BO4E"]
    marktd["marktd :8180\nMarket Data Hub\n(this service)"]
    processd["processd :8580\nProcess decisions\n(NB STP + LF E_0624)"]
    erp["ERP\n(Powercloud / SAP IS-U)"]
    invoicd["invoicd :8280\nBilling"]
    edmd["edmd :8380\nEnergy Data"]
    obsd["obsd :8480\nObservability"]
    pg["PostgreSQL"]

    makod -->|"de.mako.process.*\nHMAC-signed POST"| marktd
    marktd -->|"VersorgungsStatus\nderivation"| pg
    marktd -->|"de.mako.* + de.markt.*\nHMAC webhooks"| erp
    marktd -->|"de.mako.process.initiated"| processd
    marktd -->|"de.mako.process.initiated"| invoicd
    marktd -->|"de.mako.*"| edmd
    marktd -->|"de.mako.*"| obsd
    marktd --- pg
    erp -->|"PUT /api/v1/malo\nPUT /api/v1/contracts"| marktd
    invoicd -->|"GET /api/v1/preisblaetter\nGET /api/v1/nb-contracts"| marktd
    processd -->|"GET /api/v1/versorgung\nGET /api/v1/malo/{id}/grid\nGET /api/v1/partners"| marktd
    processd -->|"POST /api/v1/commands"| makod
```

The clean separation of concerns:

| Service | Responsibility |
|---------|----------------|
| `makod` | EDIFACT parsing, BDEW process rules, AS4 delivery, regulatory deadlines |
| `marktd` | Market data, VersorgungsStatus, ERP subscriptions, EventBus fan-out |
| `processd` | Automated STP decisions (NB: netz-checker; LF: E_0624 auto-response) |

---

## Port Layout

```
┌─────────────────────────────────────────────────────────────────┐
│  marktd  :8180                                                    │
│                                                                 │
│  Axum REST API                                                  │
│   ├─ OIDC/JWT middleware  → JwtClaims { sub, mako_tenant,      │
│   │                                     mako_roles, mako_sparte}│
│   ├─ Cedar ABAC enforcer  → permit / deny                      │
│   └─ Handlers             → PostgreSQL (SQLx)                  │
│                                                                 │
│  POST /api/v1/events  ← makod CloudEvents ingest               │
│   ├─ Verify HMAC signature                                     │
│   ├─ Deduplicate via processed_events table                    │
│   ├─ Fan-out to all EventBus subscribers                       │
│   └─ Derive VersorgungsStatus (PIDs 55003/44003/55013/44013)  │
│                                                                 │
│  GET  /admin/fanout/dlq             ← DLQ inspection           │
│  POST /admin/fanout/dlq/{id}/retry  ← re-deliver entry         │
│  DEL  /admin/fanout/dlq/{id}        ← discard entry            │
│  GET  /metrics                      ← Prometheus metrics       │
│                                                                 │
│  Note: Automated STP decisions live in processd :8580          │
│  marktd is a pure data hub — no domain policy.                  │
│                                                                 │
│  GET /health  — liveness (no DB check)                         │
│  GET /ready   — readiness (PostgreSQL ping)                     │
└─────────────────────────────────────────────────────────────────┘
```

---

## Quick Start

### With Docker Compose (full stack)

See `demo/docker-compose.yml` for the complete 8-service stack (postgres + webhook +
marktd + processd + makod + invoicd + edmd + obsd).

Minimal compose snippet for marktd alone:

```yaml
services:
  postgres:
    image: postgres:17-alpine
    environment:
      POSTGRES_DB:       marktd
      POSTGRES_USER:     marktd
      POSTGRES_PASSWORD: secret
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U marktd -d marktd"]
      interval: 5s
      retries: 10

  marktd:
    image: ghcr.io/hupe1980/marktd:0.7.0
    depends_on:
      postgres:
        condition: service_healthy
    volumes:
      - ./marktd.toml:/etc/marktd/marktd.toml:ro
    environment:
      DATABASE_URL:         postgres://marktd:secret@postgres/marktd
      MAKOD_API_KEY:        my-makod-api-key
      MAKOD_WEBHOOK_SECRET: my-webhook-secret
    command: ["--config=/etc/marktd/marktd.toml"]
    ports: ["8180:8180"]
```

### Binary

```bash
marktd --config /etc/marktd/marktd.toml
# or: MARKTD_CONFIG=/etc/marktd/marktd.toml marktd
```

Migrations run automatically at startup via `sqlx migrate run`.

---

## Configuration

`marktd` reads its configuration from a **TOML file** (default: `marktd.toml`),
with secrets deferred to environment variables via `"env:VAR_NAME"` values.

### Full `marktd.toml` reference

```toml
[http]
addr = "0.0.0.0:8180"     # default

[storage.postgres]
url = "env:DATABASE_URL"  # required; use env: for secrets

[makod]
base_url  = "http://makod:8080"   # required
api_key   = "env:MAKOD_API_KEY"   # required
tenant_id = "9900357000004"        # required — operator primary MP-ID

[webhook]
inbound_path   = "/api/v1/events"             # default
inbound_secret = "env:MAKOD_WEBHOOK_SECRET"   # optional; omit for dev

# [oidc]            # omit to disable auth (dev only — never omit in production)
# issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
# audience = "api://mako-marktd"
# jwks_refresh_secs = 300

# [otel]            # omit to disable tracing
# endpoint = "http://otel-collector:4317"
```

### CLI flags

| Flag | Env var | Default | Description |
|---|---|---|---|
| `--config` / `-c` | `MARKTD_CONFIG` | `marktd.toml` | Path to `marktd.toml` |
| `--log-level` | `RUST_LOG` | `info` | Log level (`info`, `debug`, `marktd=trace`) |
| `--check` | `MARKTD_CHECK` | `false` | Validate config + DB connectivity, then exit 0. Used by Dockerfile HEALTHCHECK. |

---

## Authentication & JWT Claims

`marktd` validates every request using a JWT Bearer token. The JWT must contain these
custom claims (in addition to standard OIDC claims):

| Claim | Type | Required | Description |
|---|---|---|---|
| `sub` | `string` | yes | Principal identifier |
| `mako_tenant` | `string` | yes | GLN of the tenant this principal belongs to |
| `mako_roles` | `string[]` | yes | Roles, e.g. `["NB"]`, `["LF"]`, `["MSB","NB"]` |
| `mako_sparte` | `string[]` | no | Optional commodity scope, e.g. `["Strom","Gas"]` |

Configure your OIDC provider (Keycloak, Zitadel, Auth0, Entra ID) to populate
`mako_tenant` and `mako_roles` from your user store or service account attributes.

**Supported signing algorithms: RS256, ES256, PS256.**
HS256/HS512 are rejected — symmetric algorithms are not acceptable for OIDC.

---

## Authorization: Cedar ABAC

`marktd` uses [Cedar](https://www.cedarpolicy.com/) (AWS PARC model) for fine-grained
Attribute-Based Access Control. The policy file is loaded once at startup.

### Default policy (`policies/marktd.cedar`)

```cedar
// Any authenticated principal of the same tenant can read all resources.
permit(
    principal,
    action in [
        Action::"read-malo", Action::"read-melo", Action::"read-contract",
        Action::"read-partner", Action::"read-preisblatt"
    ],
    resource
)
when {
    principal.tenant == resource.tenant
};

// Any principal of the same tenant can write malo/melo/contracts/partners.
permit(
    principal,
    action in [
        Action::"write-malo", Action::"write-melo", Action::"write-contract",
        Action::"write-partner"
    ],
    resource
)
when {
    principal.tenant == resource.tenant
};

// Only NB-role principals may write price sheets.
permit(
    principal,
    action == Action::"write-preisblatt",
    resource
)
when {
    principal.tenant == resource.tenant &&
    principal.roles.contains("NB")
};

// Only operator-admin principals may manage the fanout dead-letter queue.
// Grant this action to your on-call / operations service account.
permit(
    principal,
    action == Action::"manage-fanout",
    resource
)
when {
    principal.tenant == resource.tenant &&
    principal.roles.contains("ADMIN")
};
```

### Context fields

| Field | Value |
|---|---|
| `principal.tenant` | `mako_tenant` JWT claim |
| `principal.roles` | `mako_roles` JWT claim |
| `resource.tenant` | Tenant GLN of the requested resource |

### Denied response

```json
{
  "error": "Forbidden",
  "detail": "action=write-preisblatt denied for principal=svc@example.com resource_tenant=9900357000004"
}
```

### Custom policies

Replace `policies/marktd.cedar` and restart `marktd`. Hot-reload is not yet implemented.

---

## REST API

Interactive docs: `http://localhost:8180/api/v1/docs/`

OpenAPI spec: `GET /api/v1/openapi.json`

### Endpoints

| Method | Path | Cedar action | Description |
|---|---|---|---|
| `GET` | `/health` | — | Health check (no auth) |
| `GET` | `/ready` | — | Readiness check (DB ping, no auth) |
| `PUT` | `/api/v1/malo/{malo_id}` | `write-malo` | Upsert Marktlokation; pushes to makod MaLo cache |
| `GET` | `/api/v1/malo/{malo_id}` | `read-malo` | Get Marktlokation |
| `GET` | `/api/v1/malo` | `read-malo` | List Marktlokationen |
| `PUT` | `/api/v1/melo/{melo_id}` | `write-melo` | Upsert Messlokation |
| `GET` | `/api/v1/melo/{melo_id}` | `read-melo` | Get Messlokation |
| `PUT` | `/api/v1/contracts/{id}` | `write-contract` | Upsert contract (with `valid_from` / `valid_to`) |
| `GET` | `/api/v1/contracts/{id}` | `read-contract` | Get contract |
| `PUT` | `/api/v1/partners/{mp_id}` | `write-partner` | Upsert trading partner |
| `GET` | `/api/v1/partners/{mp_id}` | `read-partner` | Get trading partner |
| `GET` | `/api/v1/partners` | `read-partner` | List partners |
| `PUT` | `/api/v1/preisblaetter/{nb_mp_id}` | `write-preisblatt` | Upsert price sheet + store versioned snapshot + emit `de.markt.pricat.published` |
| `GET` | `/api/v1/preisblaetter/{nb_mp_id}` | `read-preisblatt` | Get price sheet valid on date |
| `GET` | `/api/v1/pricat/{nb_mp_id}/history` | `read-preisblatt` | List PRICAT version history (newest first) |
| `GET` | `/api/v1/pricat/{nb_mp_id}/dispatch-log/{version_id}` | `read-preisblatt` | PRICAT dispatch audit log for a version |
| `POST` | `/api/v1/pricat/{nb_mp_id}/dispatch` | `write-preisblatt` | Enqueue (re-)dispatch of latest PRICAT to all active LF partners |
| `GET` | `/api/v1/versorgung/{malo_id}` | `read-versorgungsstatus` | Current VersorgungsStatus; add `?at=YYYY-MM-DD` for point-in-time |
| `GET` | `/api/v1/versorgung/{malo_id}/history` | `read-versorgungsstatus` | Full supply-state change history (newest first, paged) |
| `PUT` | `/api/v1/versorgung/{malo_id}` | `write-versorgungsstatus` | Upsert VersorgungsStatus (ERP-driven override) |
| `GET` | `/api/v1/nelo` | `read-nelo` | List NeLos (`?nb_mp_id=` filters by Netzbetreiber) |
| `GET` | `/api/v1/nelo/{id}` | `read-nelo` | Get a NeLo by EIC / BDEW Codenummer |
| `PUT` | `/api/v1/nelo/{id}` | `write-nelo` (NB role) | Insert or update a NeLo |
| `POST` | `/api/v1/events` | — | Inbound CloudEvent from `makod` (HMAC-verified) |
| `GET` | `/admin/fanout/dlq` | `manage-fanout` | List unresolved DLQ entries |
| `POST` | `/admin/fanout/dlq/{id}/retry` | `manage-fanout` | Re-deliver a DLQ entry |
| `DELETE` | `/admin/fanout/dlq/{id}` | `manage-fanout` | Discard a DLQ entry |
| `GET` | `/metrics` | — | Prometheus metrics (no auth, internal only) |

---

## Price Sheets — PreisblattNetznutzung

Price sheets record the Netznutzungspreise for a Netzbetreiber. Validity is
derived from the BO4E `gueltigkeit.startdatum` / `gueltigkeit.enddatum` fields
inside the JSON payload.

### PUT request body

```json
{
  "data": {
    "bo_typ": "PREISBLATT_NETZNUTZUNG",
    "bezeichnung": "Netznutzungspreise 2025 — 9900357000004",
    "gueltigkeit": { "startdatum": "2025-10-01", "enddatum": "2026-09-30" },
    "marktteilnehmer": {
      "bo_typ": "MARKTTEILNEHMER",
      "marktrolle": "NB",
      "rollencodenummer": "9900357000004",
      "rollencodetyp": "BDEW"
    },
    "preispositionen": [ ... ]
  },
  "bo4e_version": "v202501.0.0"
}
```

### GET response

```json
{
  "data":         { "bo_typ": "PREISBLATT_NETZNUTZUNG", ... },
  "source":       "api",
  "bo4e_version": "v202501.0.0",
  "updated_at":   "2025-10-01T08:15:00Z"
}
```

Query parameter: `?date=YYYY-MM-DD` (defaults to today in CET/CEST).

### Source field

Every price sheet row carries a `source` field:

| Source | Set by | Semantics |
|---|---|---|
| `api` | REST `PUT /api/v1/preisblaetter/{nb_mp_id}` | Operator-supplied via REST API or ERP export |
| `mako` | Future: PRICAT 27003 ingest path in invoicd/makod | Received as EDIFACT from the NB |

**Operator-override rule:** an `api` entry always supersedes a `mako` entry for
the same NB GLN and validity period. A price sheet uploaded via the REST API
cannot be silently overwritten by an incoming EDIFACT PRICAT.

Enforced in SQL:

```sql
ON CONFLICT (nb_mp_id, valid_from)
DO UPDATE SET data = EXCLUDED.data, ...
WHERE preisblaetter.source <> 'api' OR EXCLUDED.source = 'api';
```

### PRICAT 27003 dispatch pipeline

Every `PUT /api/v1/preisblaetter/{nb_mp_id}` call:

1. Writes or updates the current price sheet in `preisblaetter` (existing behaviour)
2. Inserts a versioned snapshot in `pricat_versions` keyed on `(nb_mp_id, tenant, valid_from)`
3. Emits `de.markt.pricat.published` → fan-out to ERP webhook subscribers
4. A background task dispatches PRICAT 27003 per active LF partner via `MakodClient`

The dispatch audit log (`pricat_dispatch_log`) records every outbound dispatch attempt
(NB × LF pair) with outcome and `makod` process ID.

```mermaid
sequenceDiagram
    participant ERP
    participant marktd
    participant pricat_versions
    participant MakodClient
    participant AS4

    ERP->>marktd: PUT /api/v1/preisblaetter/{nb_mp_id}
    marktd->>preisblaetter: UPSERT (existing)
    marktd->>pricat_versions: UPSERT versioned snapshot
    marktd-->>ERP: 204 No Content
    marktd-)fanout: de.markt.pricat.published
    Note over marktd: background task
    marktd->>pricat_versions: mark_queued(version_id)
    loop per active LF partner
        marktd->>MakodClient: POST /api/v1/commands dispatch-pricat-27003
        MakodClient->>AS4: PRICAT 27003
        marktd->>pricat_dispatch_log: log(lf_mp_id, outcome)
    end
    marktd->>pricat_versions: mark_done(version_id)
```

**Auto-dispatch on LF partner registration:** when `PUT /api/v1/partners/{mp_id}` registers
a partner with `marktrolle = "LF"`, the latest PRICAT version for the operator's NB GLN is
automatically re-queued for dispatch to the new partner.

**Manual re-dispatch:** `POST /api/v1/pricat/{nb_mp_id}/dispatch` resets dispatch state to
`queued` so the background task picks it up again. Use this after AS4 outages or to
force distribution to newly on-boarded partners.

**Dispatch states:**

| State | Meaning |
|---|---|
| `pending` | Version stored; no dispatch started yet |
| `queued` | Dispatch task picked this version up |
| `done` | All active LF partners successfully reached |
| `error` | Last dispatch attempt failed; will be retried on next poll |

---

## Database Schema

`marktd` uses a single SQL migration file (`migrations/0001_initial_schema.sql`).
Migrations run automatically at startup via `sqlx migrate run`.

### Tables

| Table | Purpose |
|---|---|
| `malo` | Marktlokationen — JSONB payload, `bo4e_version`, GIN index |
| `lokationszuordnung` | Temporal NB/LF/MSB role assignments per MaLo |
| `melo` | Messlokationen — JSONB payload, `bo4e_version` |
| `contracts` | Energy contracts — JSONB payload, `bo4e_version`, **`valid_from DATE`**, **`valid_to DATE`** |
| `partners` | Trading partners (GLN → channels) — JSONB |
| `subscriptions` | ERP webhook registrations |
| `process_correlation` | Running/completed MaKo process tracking per MaLo |
| `processed_events` | Inbound event idempotency log |
| `preisblaetter` | Price sheets — `source CHECK ('api','mako')`, GIN index |
| `versorgungsstatus` | VersorgungsStatus per MaLo — `LieferStatus CHECK`, optimistic concurrency `version BIGINT` |
| `versorgungsstatus_history` | Append-only audit log of every supply-state transition — powers `?at=` and `/history` |
| `nb_contracts` | NB network contracts — billing schedule, netzebene, bilanzierungsmethode |
| `pricat_versions` | Versioned PRICAT snapshots — `(nb_mp_id, tenant, valid_from)` unique, dispatch state |
| `pricat_dispatch_log` | Dispatch audit log — one row per NB × LF dispatch attempt |
| `nelo` | Netz-Element-Lokationen (Redispatch 2.0) — EIC or BDEW Codenummer, owner NB GLN, JSONB data |

### `contracts.valid_from` / `valid_to`

These `DATE` columns define the temporal validity window of each contract and are
used by the Wechselprozess auto-responder's rule 5L to detect conflicting active
supply contracts at `process_date`.

| Column | Type | Semantics |
|---|---|---|
| `valid_from` | `DATE` nullable | Start of the contract validity window. `NULL` for records created before this migration. |
| `valid_to` | `DATE` nullable | End of the validity window (inclusive). `NULL` = open-ended / currently active. |

Query pattern (rule 5L):

```sql
SELECT ... FROM contracts
WHERE malo_id = $1
  AND (valid_from IS NULL OR valid_from <= $2)   -- $2 = process_date
  AND (valid_to   IS NULL OR valid_to   >= $2)
ORDER BY valid_from DESC NULLS LAST
```

Two indexes cover this efficiently:
- `contracts_malo_valid_from (malo_id, valid_from DESC NULLS LAST)` — general range query
- `contracts_malo_open_ended ... WHERE valid_to IS NULL` — partial index for currently active contracts

---

## Contracts — Validity Periods

Contracts support explicit validity dates used by the auto-responder and by any
ERP logic that needs to understand which supply contracts are currently active.

### PUT request body

```json
{
  "malo_id":      "51238696780",
  "sparte":       "STROM",
  "vertragsart":  "Liefervertrag",
  "valid_from":   "2026-10-01",
  "valid_to":     null,
  "data": {
    "bo_typ":          "VERTRAG",
    "vertragsnummer":  "LFV-2026-123",
    "vertragsart":     "NETZNUTZUNGSVERTRAG",
    "vertragsstatus":  "AKTIV",
    "sparte":          "STROM",
    "vertragsbeginn":  "2026-10-01",
    "vertragspartner": "4012345000023"
  },
  "bo4e_version": "v202501.0.0"
}
```

`valid_from` / `valid_to` are ISO 8601 date strings (`YYYY-MM-DD`).
`null` for `valid_to` means the contract is open-ended (currently active, no known end date).
`null` for `valid_from` means the start date is unknown (legacy record).

### GET response

```json
{
  "contract_id": "lv-2026-123",
  "malo_id":     "51238696780",
  "sparte":      "STROM",
  "vertragsart": "Liefervertrag",
  "version":     1,
  "valid_from":  "2026-10-01",
  "valid_to":    null,
  "data":        { ... }
}
```

---

## Inbound Events from `makod`

`marktd` receives process lifecycle events from `makod` via `POST /api/v1/events`.

### Enable push in makod config

```toml
# makod.toml
[erp]
webhook_url    = "http://marktd:8180/api/v1/events"
webhook_secret = "shared-hmac-secret"
```

Inbound delivery is idempotent — duplicates are detected by `event_id` and silently
acknowledged without re-processing.

---

## `PUT /api/v1/malo` — MaLo Cache Push

Every successful `PUT /api/v1/malo/{malo_id}` call automatically pushes the NB and
MSB GLNs to `makod`'s MaLo cache via `PUT /admin/malo/{malo_id}`.  This ensures
`makod` can resolve the grid operator and metering point operator GLNs for outbound
EDIFACT without requiring a separate ERP step.

The push is best-effort — a `makod` connectivity failure is logged but does **not**
fail the `PUT /api/v1/malo` API call.  The MaLo record is durably stored in
PostgreSQL regardless.

Fields forwarded to `makod`:

| Field | Source |
|---|---|
| `nb_mp_id` | `lokationszuordnung[]` entry with `zuordnungstyp == "NB"` or `"GNB"` |
| `msb_gln` | `lokationszuordnung[]` entry with `zuordnungstyp == "MSB"` or `"GMSB"` |
| `bilanzierungsgebiet` | `data.bilanzierungsgebiet` |
| `netzgebiet` | `data.netzgebietsnummer` or `data.netzgebiet` |
| `sparte` | `sparte` field |

---

## ERP Subscriptions & Fan-Out

`marktd` delivers CloudEvents 1.0 to every matching ERP subscriber when master data changes or when `makod` lifecycle events arrive. The fan-out worker runs in a dedicated Tokio task and delivers independently per subscriber — a slow or unavailable ERP does not block other subscribers.

### Event types

| Source | Event type | Trigger |
|---|---|---|
| marktd master data | `de.markt.malo.updated` | `PUT /api/v1/malo/{malo_id}` |
| marktd master data | `de.markt.partner.updated` | `PUT /api/v1/partners/{mp_id}` |
| marktd PRICAT | `de.markt.pricat.published` | `PUT /api/v1/preisblaetter/{nb_mp_id}` |
| makod process relay | `de.mako.process.initiated` | forwarded from `makod` ingest |
| makod process relay | `de.mako.aperak.accepted` | forwarded from `makod` ingest |
| makod process relay | `de.mako.aperak.rejected` | forwarded from `makod` ingest |
| makod process relay | `de.mako.aperak.timeout` | forwarded from `makod` ingest |
| makod process relay | `de.mako.process.completed` | forwarded from `makod` ingest |
| makod process relay | `de.mako.process.failed` | forwarded from `makod` ingest |
| makod process relay | `de.mako.edifact.inbound` | forwarded from `makod` ingest |

> `de.mako.*` events carry the CloudEvents extensions `makoconvid`, `makopid`,
> `makoworkflow`, and `marktrole` (role of the counterparty: `NB`, `LF`, `MSB`,
> `BIKO`). Downstream services (`invoicd`, `edmd`, `obsd`) filter on `makopid`
> to select only the event types they care about.

### Register a subscription

```bash
curl -X POST http://localhost:8180/api/v1/subscriptions \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "endpoint_url": "https://erp.example.com/mdm/events",
    "secret":       "mysecret64hexchars",
    "event_types":  ["de.markt.malo.updated", "de.markt.pricat.published",
                     "de.mako.process.completed"]
  }'
```

### Webhook payload

```
POST https://erp.example.com/mdm/events
Content-Type: application/cloudevents+json
X-Mako-Signature: <hmac-sha256-hex>
```

```json
{
  "specversion":     "1.0",
  "id":              "01932a4f-7b3e-4c5d-8f6a-9e0b1c2d3e4f",
  "source":          "urn:markt:tenant:9900357000004",
  "type":            "de.mako.process.completed",
  "time":            "2025-10-01T08:15:00+02:00",
  "subject":         "018f3a2b-7c4e-7d5f-8a9b-0c1d2e3f4a5b",
  "datacontenttype": "application/json",
  "makoconvid":      "018f3a2b-7c4e-7d5f-8a9b-0c1d2e3f4a5b",
  "makopid":         55001,
  "makoworkflow":    "gpke-lieferbeginn",
  "marktrole":       "LF",
  "data": { "_typ": "MARKTLOKATION", "marktlokationsId": "51238696780", ... }
}
```

### Signature verification

`X-Mako-Signature` is an HMAC-SHA256 hex digest over the raw request body
computed with the `secret` registered in the subscription:

```python
import hmac, hashlib

def verify(body: bytes, secret: str, header: str) -> bool:
    expected = hmac.new(secret.encode(), body, hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected, header)
```

Return `200 OK` for duplicates — fan-out retries on non-2xx.

### Retry behaviour

The fan-out worker retries failed deliveries with exponential back-off
(1 s → 2 s → 4 s → … → 64 s, capped). After exhausting all attempts the event
is written to the `fanout_dlq` table rather than silently dropped.

This durable failure path is required by **§22 MessZV / §41 EnWG**: a silent
drop of a `de.mako.process.initiated` event to `invoicd` would mean the INVOIC
plausibility check never runs, and the 3-year receipt retention obligation
cannot be satisfied.

### Dead-letter queue (DLQ)

Events that exhaust all retry attempts land in `fanout_dlq`. Operators inspect
and remediate via the admin endpoints:

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/admin/fanout/dlq` | List unresolved DLQ entries (newest first, paged; `?include_resolved=true` for history) |
| `POST` | `/admin/fanout/dlq/{id}/retry` | Re-deliver and mark resolved on HTTP 2xx |
| `DELETE` | `/admin/fanout/dlq/{id}` | Discard without retry (marks resolved) |

```bash
# Inspect failures
curl http://localhost:8180/admin/fanout/dlq \
  -H "Authorization: Bearer $TOKEN" | jq '.[] | {id, subscriber_id, event_type, attempts, last_error}'

# Re-deliver a specific entry
curl -X POST http://localhost:8180/admin/fanout/dlq/$ENTRY_ID/retry \
  -H "Authorization: Bearer $TOKEN" | jq .

# Discard after manual ERP re-import
curl -X DELETE http://localhost:8180/admin/fanout/dlq/$ENTRY_ID \
  -H "Authorization: Bearer $TOKEN"
```

The DLQ uses the current webhook secret at retry time — if the subscription secret
was rotated, re-register the subscription before retrying.

### Prometheus metrics (`/metrics`)

`GET /metrics` exposes operational counters in Prometheus text format:

| Metric | Description |
|--------|-------------|
| `marktd_fanout_dlq_depth` | Unresolved entries in `fanout_dlq` |
| `marktd_active_subscriptions` | Registered EventBus subscribers |
| `marktd_processed_events_total` | Events ingested from `makod` (all time) |
| `marktd_db_pool_size` | Current PostgreSQL connection pool size |
| `marktd_db_pool_idle` | Idle connections in the pool |

Scrape via Prometheus `static_configs` or a `ServiceMonitor` in Kubernetes.

---

## Process Correlations

Track which MaKo processes are currently running against a given MaLo:

```bash
curl "http://localhost:8180/api/v1/correlations/51238696780" \
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

## Docker Deployment

```bash
docker pull ghcr.io/hupe1980/marktd:0.7.0

docker run -d \
  --name marktd \
  -p 8180:8180 \
  -v /etc/marktd/marktd.toml:/etc/marktd/marktd.toml:ro \
  -e DATABASE_URL=postgres://marktd:secret@postgres/marktd \
  -e MAKOD_API_KEY=my-api-key \
  -e MAKOD_WEBHOOK_SECRET=my-webhook-secret \
  ghcr.io/hupe1980/marktd:0.7.0 \
  --config=/etc/marktd/marktd.toml
```

---

## Health Checks

| Endpoint | DB check | Use for |
|---|---|---|
| `GET /health` | no | Kubernetes `livenessProbe` |
| `GET /ready` | yes (ping) | Kubernetes `readinessProbe` |

---

## Common Issues

**`401 Unauthorized`**
JWT validation failed. Check: correct `--auth-issuer`, token not expired,
`mako_tenant` claim present.

**`403 Forbidden`**
Cedar denied the request. Check: `mako_tenant` matches tenant GLN in URL,
`mako_roles` contains required role (`NB` for `write-preisblatt`).

**`404 Not Found` on GET preisblatt**
No price sheet valid on the requested date. Upload one first with
`PUT /api/v1/preisblaetter/{nb_mp_id}`.

**Price sheet not updating (`mako` source rejected)**
Intentional. An existing `source=api` row cannot be overwritten by
`source=mako` — operator-override protection is working. Use the REST API
to update operator-controlled price sheets.

**Auto-responder dispatching ablehnen for all requests**
Check rule 3 (NB in grid): your operator GLN (`tenant_id` in `marktd.toml`)
must appear in the MaLo's `lokationszuordnung` as `zuordnungstyp = "NB"`.
Upload the MaLo with `PUT /api/v1/malo/{malo_id}` including the NB entry.

**Auto-responder deferring all requests (no commands dispatched)**
Check rule 1 (MaLo exists): the MaLo referenced in the UTILMD has not been
pre-loaded into marktd.  Use `PUT /api/v1/malo/{malo_id}` to register it.

**Auto-responder rejecting with Z0C (Preisblatt missing)**
Only triggered when `auto_accept = true`.  Upload a price sheet covering the
`process_date` with `PUT /api/v1/preisblaetter/{nb_mp_id}`.  Check that
`gueltigkeit.startdatum` and `gueltigkeit.enddatum` in the BO4E payload
bracket the `process_date` from the UTILMD.

**`relation "malo" does not exist`**
Migrations have not run. Check `DATABASE_URL` and PostgreSQL connectivity.
`marktd` runs `sqlx migrate run` automatically on startup.

---

## MCP Server

`marktd` exposes an MCP (Model Context Protocol) Streamable HTTP server at
`POST /mcp` / `GET /mcp`. The same OIDC + Cedar authorization layer applies.

**Current tools:**

| Tool | Description |
|---|---|
| `get_malo` | Fetch a MaLo by ID — returns MaLoId, Sparte, Netzgebiet, active contract, NB entry |
| `list_partners` | List registered market partners (GLN, name, roles) |
| `get_preisblatt` | Fetch the price sheet valid for a given NB GLN and date |
| `get_versorgungsstatus` | Read supply state for a MaLo (LieferStatus, LF GLN, NB GLN, dates) |

---

## VersorgungsStatus

`marktd` maintains one `VersorgungsStatus` record per MaLo.  Records are derived
automatically from `de.mako.process.completed` events:

| PID | Transition | DB action |
|---|---|---|
| 55003 (GPKE) | `Unbeliefert → Beliefert` | upsert `lieferstatus = Beliefert`, set `lf_mp_id` |
| 44003 (GeLi Gas) | `Unbeliefert → Beliefert` | upsert `lieferstatus = Beliefert`, set `lf_mp_id` |
| 55013 (GPKE) | `Beliefert → Unbeliefert` | upsert `lieferstatus = Unbeliefert`, clear `lf_mp_id` |
| 44013 (GeLi Gas) | `Beliefert → Unbeliefert` | upsert `lieferstatus = Unbeliefert`, clear `lf_mp_id` |

**State machine:**

```mermaid
stateDiagram-v2
    [*] --> Unbeliefert : MaLo registered

    Unbeliefert --> Beliefert : PID 55003 / 44003 completed\n(NB confirms Lieferbeginn)
    Beliefert --> Unbeliefert : PID 55013 / 44013 completed\n(Abmeldung-Bestätigung)

    Unbeliefert --> Grundversorgung : §36 EnWG activation
    Grundversorgung --> Beliefert : GVU converts to standard contract
    Unbeliefert --> Ersatzversorgung : §38 EnWG activation
    Ersatzversorgung --> Unbeliefert : Ersatzversorgung ends (max 3 months)

    Beliefert --> Ruhend : MaKo suspended
    Ruhend --> Beliefert : Reactivated
    Beliefert --> Stillgelegt : Decommissioned
    Stillgelegt --> [*]
```

**Optimistic concurrency.** Every write uses `WHERE malo_id = $1 AND tenant = $2 AND version = $3`.
Conflict → `412 Precondition Failed` → retry after re-read.  At-least-once EventBus delivery
guarantees convergence.

**Every write also appends a row to `versorgungsstatus_history`** in the same transaction,
enabling point-in-time and full audit queries.

### REST API

```http
# Current state
GET  /api/v1/versorgung/{malo_id}

# Point-in-time state (as of end-of-day on that German calendar date, CET/CEST)
GET  /api/v1/versorgung/{malo_id}?at=2025-10-01

# Full state-change history (newest first, paged)
GET  /api/v1/versorgung/{malo_id}/history?page=0&size=50

# Admin override or ERP-driven upsert; supply If-Match: "<version>" for OCC
PUT  /api/v1/versorgung/{malo_id}
```

**Point-in-time query (`?at=YYYY-MM-DD`):**  Returns the supply state as it was at
end-of-day on the given date in German local time (CET/CEST).  Backed by the
`versorgungsstatus_history` table.  Returns `404` when no history exists on or before
that date.

```mermaid
sequenceDiagram
    participant ERP
    participant marktd
    participant PostgreSQL

    ERP->>marktd: GET /api/v1/versorgung/{malo_id}?at=2025-07-01
    marktd->>PostgreSQL: SELECT * FROM versorgungsstatus_history\nWHERE malo_id=$1 AND tenant=$2\nAND (valid_from AT TIME ZONE 'Europe/Berlin')::date <= '2025-07-01'\nORDER BY valid_from DESC LIMIT 1
    PostgreSQL-->>marktd: history row (LieferStatus, LF, NB, …)
    marktd-->>ERP: 200 OK + VersorgungsStatusResponse
```

`processd` (M17) reads `GET /api/v1/versorgung/{malo_id}` to drive automated
LFA E_0624 responses without ERP involvement (GPKE Teil 1 §5).

---

## Netz-Element-Lokationen (NeLo) — Redispatch 2.0

`marktd` maintains a registry of Netz-Element-Lokationen (NeLo) for BDEW Redispatch 2.0.
A NeLo is a network element location identified by a 16-char EIC code (ENTSO-E,
`NAD DE3055 = ZEW`) or a 13-digit BDEW Codenummer.

NeLos are owned by the Netzbetreiber (NB role) responsible for the network element.
They carry structural metadata (Sparte, Netzebene) and an open-ended JSONB `data` payload
for additional Redispatch 2.0 attributes.

**REST API:**

```http
# List all NeLos for this tenant (optionally filter by Netzbetreiber GLN)
GET  /api/v1/nelo
GET  /api/v1/nelo?nb_mp_id=9900357000004&page=0&size=50

# Get a single NeLo by EIC or BDEW Codenummer
GET  /api/v1/nelo/{nelo_id}

# Insert or update a NeLo (NB role required; supply If-Match for OCC)
PUT  /api/v1/nelo/{nelo_id}
```

**Request body for PUT:**

```json
{
  "sparte": "STROM",
  "name": "Umspannwerk Musterstadt 110/20 kV",
  "netzebene": "HS",
  "nb_mp_id": "9900357000004",
  "data": {
    "eic": "10XDE-EON-NETZ--G",
    "regelzone": "10YDE-EON------1"
  }
}
```

**Authorization:** `read-nelo` is open to all authenticated users in the tenant.
`write-nelo` requires the `NB` role (Cedar policy `write-nelo`).

---

## CloudEvents Extensions

All outbound events use the `markt*` extension namespace (CloudEvents §3.3 — lowercase
alphanumeric only):

| Extension | Type | Description |
|---|---|---|
| `marktmaloid` | string | Resolved Marktlokations-ID |
| `marktmeloid` | string | Resolved Messlokations-ID |
| `marktcontractid` | string | Contract UUID |
| `marktrole` | string | Marktrolle: `NB`, `LF`, `MSB`, `BIKO`, `UNB` |
| `markterpref` | string | ERP-supplied idempotency key |
| `makopid` | u32 | Forwarded BDEW Prüfidentifikator |
| `makoworkflow` | string | Workflow family name |
| `makoerc` | string | BDEW ERC error code (on `aperak.rejected`) |

**`marktrole` derivation** from `makoworkflow`:

| Pattern | Role |
|---|---|
| ends with `-lf` or contains `-lf-` | `LF` |
| starts with `wim-` | `MSB` |
| starts with `mabis-` | `BIKO` |
| everything else | `NB` |

**Event source:** `"urn:markt:tenant:{tenant_gln}"`

---

---

## See Also

- [`makod` Operator Guide](./makod.md)
- [`invoicd` README](../services/invoicd/README.md) — INVOIC plausibility, receipt persistence
- [`edmd` README](../services/edmd/README.md) — meter-data store, time-series API
- [`obsd` README](../services/obsd/README.md) — process observability, BNetzA KPI reports
- [ERP Integration](./erp-integration.md)
- [Domain Model](./domain-model.md)
- [Getting Started](./getting-started.md)
