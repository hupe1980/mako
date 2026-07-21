---
layout: default
title: marktd Operator Guide
nav_order: 22
parent: Services
description: >
  marktd operator guide: Market Data Hub for Marktlokation, Messlokation,
  contracts, VersorgungsStatus (with history + point-in-time queries), NeLo
  (Redispatch 2.0), MaLo grid topology (NB STP), trading-partner management,
  price sheets, Cedar ABAC, EventBus fan-out.
  PostgreSQL-backed, OIDC-secured, OpenAPI 3.1, CloudEvents 1.0 outbound webhooks.
---

# `marktd` Operator Guide

`marktd` is the **Market Data Hub** — the single source of truth for all market entity
state in a MaKo deployment. It stores Marktlokationen (MaLo) with typed `rubo4e::current::Marktlokation`
API responses (schema validated on PUT), Messlokationen (MeLo) with typed `rubo4e::current::Messlokation`
responses, Zaehler + Geraete (device registry) with typed `rubo4e::current::Zaehler`/`Geraet` responses,
energy contracts, trading partners, network contracts (`nb_contracts`) with full BO4E **`Vertrag`** payload
(`vertragsart`, `vertragsstatus` as indexed columns for ERP digital LRV exchange), price sheets
(PreisblattNetznutzung), **VersorgungsStatus per MaLo** (with full history and
point-in-time queries), **MaLo grid topology** (`malo_grid` — sourced from the NB's
NIS/GIS system and provisioned via `PUT /api/v1/malo/{id}/grid`; read by `processd`
for Anmeldung STP decisions), and
**Netz-Element-Lokationen (NeLo)** for Redispatch 2.0.

Beyond data storage, `marktd` includes:

- **EventBus fan-out** — enriches inbound `de.mako.*` events with `marktrole` and fans out
  to all registered subscribers (ERP, `processd`, `invoicd`, `obsd`) via HMAC-signed webhooks.
- **VersorgungsStatus derivation** — three-phase lifecycle: `announce_lf_next` (55001/44001 `process.initiated`), `confirm_supply` (55003/44003 `process.completed`), `end_supply` (55013/44013 `process.completed`). Tracks `lf_mp_id_next` + `lf_next_lieferbeginn` (pending transition), appends every change to `versorgungsstatus_history`, and supports `?at=YYYY-MM-DD` point-in-time queries.

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

See `demos/nb-stp/docker-compose.yml` for the complete 8-service stack (postgres + webhook +
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
    image: ghcr.io/hupe1980/marktd:0.12.0
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
| `PUT` | `/api/v1/malo/{malo_id}` | `write-malo` | Upsert Marktlokation; validates `_typ = MARKTLOKATION` and enum fields (422 on violation); pushes to makod MaLo cache |
| `GET` | `/api/v1/malo/{malo_id}` | `read-malo` | Get Marktlokation as typed `rubo4e::current::Marktlokation` (canonical BO4E camelCase) |
| `GET` | `/api/v1/malo` | `read-malo` | List Marktlokationen (schema-drift records silently filtered) |
| `PUT` | `/api/v1/melo/{melo_id}` | `write-melo` | Upsert Messlokation; validates `_typ = MESSLOKATION` and enum fields (422 on violation) |
| `GET` | `/api/v1/melo/{melo_id}` | `read-melo` | Get Messlokation as typed `rubo4e::current::Messlokation` |
| `PUT` | `/api/v1/contracts/{id}` | `write-contract` | Upsert contract (with `valid_from` / `valid_to`) |
| `GET` | `/api/v1/contracts/{id}` | `read-contract` | Get contract |
| `PUT` | `/api/v1/partners/{mp_id}` | `write-partner` | Upsert trading partner — validates payload as `rubo4e::current::Geschaeftspartner` (auto-injects `_typ`; validates `marktrolle`, `rollencodetyp`, `marktteilnehmerstatus`, `adresse`; canonicalises camelCase) |
| `GET` | `/api/v1/partners/{mp_id}` | `read-partner` | Get trading partner — returns a `geschaeftspartner` field with the typed `rubo4e::current::Geschaeftspartner` payload (graceful fallback for legacy records) |
| `GET` | `/api/v1/partners` | `read-partner` | List partners |
| `GET/PUT` | `/api/v1/mmma-preise/gas/{year}/{month}` | `read/write-preisblatt` | Gas MMM Abrechnungspreise (Trading Hub Europe / MGV, monthly) — `{mehr_ct_kwh, minder_ct_kwh}`; queried by `netzbilanzd` for INVOIC 31007/31008 billing and `invoicd` check 6 validation |
| `GET` | `/api/v1/mmma-preise/gas` | `read-preisblatt` | List all Gas MMM price records (newest first; `?limit=`) |
| `GET/PUT` | `/api/v1/mmm-preise/strom/{year}/{month}` | `read/write-preisblatt` | Strom MMM prices (VNB per GPKE BK6-24-174 Teil 1 Kap. 8.4) — `{vnb_mp_id, mehr_ct_kwh, minder_ct_kwh}`; queried by `netzbilanzd` for INVOIC 31002/31005 and `invoicd` check 6 |
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
| `GET` | `/api/v1/malo/{malo_id}/grid` | `read-malo` | MaLo grid topology (Netzgebiet, Bilanzierungsgebiet) |
| `PUT` | `/api/v1/malo/{malo_id}/grid` | `write-malo` (NB role) | Upsert grid record from NIS/GIS |
| `GET` | `/api/v1/preisblaetter-messung/{msb_mp_id}` | `read-preisblatt` | `PreisblattMessung` valid on date (MSB metering tariffs); includes `auf_abschlaege` |
| `PUT` | `/api/v1/preisblaetter-messung/{msb_mp_id}` | `write-preisblatt` | Upsert MSB metering price sheet |
| `GET` | `/api/v1/steuerbare-ressourcen/{sr_id}` | `read-sr` | Get a `SteuerbareRessource` by SR-ID |
| `PUT` | `/api/v1/steuerbare-ressourcen/{sr_id}` | `write-sr` | Upsert a `SteuerbareRessource` |
| `GET` | `/api/v1/steuerbare-ressourcen/{sr_id}/konfigurationsprodukte` | `read-sr` | List `Konfigurationsprodukte` (§14a steuerbare Verbrauchseinrichtungen) |
| `GET` | `/api/v1/technische-ressourcen/{tr_id}` | `read-device` | Get a `TechnischeRessource` by `TrId` |
| `PUT` | `/api/v1/technische-ressourcen/{tr_id}` | `write-device` | Upsert a `TechnischeRessource` (E-mobility, generation, storage) |
| `GET` | `/api/v1/malos/{malo_id}/technische-ressourcen` | `read-device` | List `TechnischeRessource` for a `MaLo` |
| `GET` | `/api/v1/malo/{id}/lokationen` | `read-malo` | Recursive `Lokationszuordnung` graph from a MaLo (`?at=YYYY-MM-DD`) |
| `GET` | `/api/v1/melos/{id}/lokationen` | `read-melo` | Recursive `Lokationszuordnung` graph from a MeLo |
| `PUT` | `/api/v1/lokationszuordnungen` | `write-malo` | Upsert a directed location graph edge |
| `DELETE` | `/api/v1/lokationszuordnungen/{von_id}/{nach_id}` | `write-malo` | Hard-delete an edge pair (all temporal variants) |
| `GET` | `/api/v1/melos/{melo_id}/zaehler` | `read-device` | List `Zaehler` for a MeLo (typed `Vec<ZaehlerResponse>` with `data: rubo4e::current::Zaehler`) |
| `GET` | `/api/v1/melos/{melo_id}/sharing-eligibility` | `read-sharing-eligibility` | §42c EnWG metering **capability** — qualifies via Zählerstandsgangmessung (§2 Satz 1 Nr. 27 MsbG) **or** viertelstündliche RLM. Returns `capability`, `basis`, `required_action`, `reasons`, `bilanzierungsgebiet`, and the master-data `evidence` it decided from. |
| `GET` | `/api/v1/zaehler/{zaehler_id}/zaehlwerke` | `read-device` | List `Zaehlwerk` registers for a Zaehler (typed `Vec<Zaehlwerk>` from JSONB) |
| `PUT` | `/api/v1/zaehler/{zaehler_id}` | `write-device` | Upsert a `Zaehler`; validates `_typ = ZAEHLER` and schema (422 on violation) |
| `GET` | `/api/v1/zaehler/{zaehler_id}/geraete` | `read-device` | List `Geraete` for a `Zaehler` (typed `Vec<GeraetResponse>` with `data: rubo4e::current::Geraet` + `konfigurationen: Vec<GeraetKonfiguration>`) |
| `GET` | `/api/v1/zaehler/{zaehler_id}/geraete/{geraet_id}` | `read-device` | Get a single `Geraet` — full BO4E payload + `konfigurationen`; 404 when not found |
| `GET/PUT` | `/api/v1/zaehler/{zaehler_id}/geraete/{geraet_id}/konfigurationen` | `read-device` / `write-device` | Get or atomically replace typed `GeraetKonfiguration` entries (MsbG §23); PUT emits `de.markt.geraet.konfiguration.updated` |
| `GET/PUT` | `/api/v1/zaehler/{zaehler_id}/register` | `read-device` / `write-device` | List/upsert iMSys TOU registers (`ZaehlzeitRegister`) |
| `GET/PUT` | `/api/v1/zaehler-register/{register_id}/saisons` | `read-device` / `write-device` | List/upsert seasonal TOU windows (`ZaehlzeitSaison`) |
| `GET` | `/api/v1/zaehler/{zaehler_id}/tariff-zone` | `read-device` | Resolve HT/NT/EINZEL tariff zone for a given local datetime |
| `GET` | `/api/v1/zaehler/{zaehler_id}/zaehlzeitdefinitionen` | `read-device` | Return typed `rubo4e::current::Zaehlzeitdefinition` assembled from `zaehler_register` + `zaehler_saisons`; `?valid_only=true` filters to current registers |
| `PUT` | `/api/v1/geraete/{geraet_id}` | `write-device` | Upsert a `Geraet`; validates `_typ = GERAET` and schema (422 on violation) |
| `GET` | `/api/v1/nb-contracts/{id}` | `read-nb-contract` | Get NB network contract with typed BO4E `Vertrag` payload |
| `PUT` | `/api/v1/nb-contracts/{id}` | `write-nb-contract` | Upsert NB network contract; validates `Vertrag` `_typ` and enums (422 on violation); emits `de.markt.nb-contract.updated` |
| `GET` | `/api/v1/nb-contracts` | `read-nb-contract` | List NB contracts (`?nb_mp_id=...` required) |
| `POST` | `/api/v1/events` | — | Inbound CloudEvent from `makod` (HMAC-verified); appended to `event_log` before fan-out |
| `GET` | `/admin/fanout/dlq` | `manage-fanout` | List unresolved DLQ entries |
| `POST` | `/admin/fanout/dlq/{id}/retry` | `manage-fanout` | Re-deliver a DLQ entry |
| `DELETE` | `/admin/fanout/dlq/{id}` | `manage-fanout` | Discard a DLQ entry |
| `GET` | `/admin/events` | — | CloudEvent replay log — `?from=RFC3339&to=RFC3339&type=&limit=` |
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
  "bo4e_version": "v202607.0.0"
}
```

### GET response

```json
{
  "data":         { "bo_typ": "PREISBLATT_NETZNUTZUNG", ... },
  "source":       "api",
  "bo4e_version": "v202607.0.0",
  "updated_at":   "2025-10-01T08:15:00Z",
  "zeitvariable_preispositionen": [
    {
      "bezeichnung": "HT-Arbeitspreis",
      "einheit": "CT_PRO_KWH",
      "bezugsgroesse": "STUNDE",
      "zeitfenster": { "startzeit": "07:00", "endzeit": "21:00" },
      "preis": "8.35"
    }
  ]
}
```

`zeitvariable_preispositionen` contains the `ZeitvariablePreisposition` array
from the BO4E payload. If the price sheet has no ToU tariffs the field is omitted
(serialized with `#[serde(skip_serializing_if = "Vec::is_empty")]`). This array
is consumed by `netzbilanzd` for §14a Modul 2 ToU billing (BNetzA BK6-22-300).

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

## Trading Partners — `Geschaeftspartner` BO4E

`marktd` stores trading partners in the `partners` table, keyed by `mp_id`
(BDEW-Codenummer or DVGW-Codenummer). Every `PUT` validates and normalises the
partner payload as `rubo4e::current::Geschaeftspartner`.

### Schema validation on PUT

The `PUT /api/v1/partners/{mp_id}` handler:
1. Auto-injects `"_typ": "GESCHAEFTSPARTNER"` when absent.
2. Rejects 422 when `_typ` is wrong.
3. Validates all enum fields (`marktrolle`, `rollencodetyp`, `marktteilnehmerstatus`, `adresse`) via `rubo4e::current::Geschaeftspartner`.
4. Re-serialises to canonical BO4E camelCase before storage.

```bash
# Register a trading partner (LF, validated as Geschaeftspartner)
curl -s -X PUT "http://marktd:8180/api/v1/partners/9904234560001" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "channels": {
      "marktrolle": "LF",
      "rollencodetyp": "BDEW",
      "marktteilnehmerstatus": "AKTIV",
      "adresse": {
        "strasse": "Musterstraße",
        "hausnummer": "1",
        "postleitzahl": "10115",
        "ort": "Berlin",
        "landescode": "DE"
      }
    }
  }'
```

### Typed GET response

`GET /api/v1/partners/{mp_id}` returns a structured response with a `geschaeftspartner`
field containing the typed `rubo4e::current::Geschaeftspartner` payload:

```json
{
  "mp_id":   "9904234560001",
  "display_name": "Muster Energieversorgung GmbH",
  "marktrolle": "LF",
  "rollencodetyp": "BDEW",
  "makoadresse": ["https://as4.muster-ev.de/as4/in"],
  "geschaeftspartner": {
    "_typ": "GESCHAEFTSPARTNER",
    "marktrolle": "LF",
    "rollencodetyp": "BDEW",
    "marktteilnehmerstatus": "AKTIV",
    "adresse": { "strasse": "Musterstraße", "hausnummer": "1", ... }
  },
  "version": 3,
  "updated_at": "2026-07-11T09:15:00Z"
}
```

Legacy partner records written before schema validation was introduced are returned
with the raw `channels` JSONB in the `geschaeftspartner` field (graceful fallback).

---

## Database Schema

`marktd` uses a single SQL schema file (`migrations/0001_initial.sql`).
Migrations run automatically at startup via `sqlx migrate run`.

### Tables

| Table | Purpose |
|---|---|
| `malo` | Marktlokationen — JSONB payload, `bo4e_version`, GIN index |
| `lokationszuordnung` | Temporal NB/LF/MSB role assignments per MaLo (legacy embedded JSON) |
| `lokationszuordnungen` | Location graph edges — `(tenant, von_id, von_typ, nach_id, nach_typ, valid_from, valid_to)`; recursive-CTE BFS traversal |
| `melo` | Messlokationen — JSONB payload, `bo4e_version` |
| `contracts` | Energy contracts — JSONB payload, `bo4e_version`, **`valid_from DATE`**, **`valid_to DATE`** |
| `partners` | Trading partners (GLN → channels) — JSONB |
| `subscriptions` | ERP webhook registrations |
| `process_correlation` | Running/completed MaKo process tracking per MaLo |
| `processed_events` | Inbound event idempotency log |
| `preisblaetter` | NB price sheets — `source CHECK ('api','mako')`, GIN index |
| `preisblaetter_messung` | MSB metering price sheets — same source-override protection |
| `versorgungsstatus` | VersorgungsStatus per MaLo — `LieferStatus CHECK`, optimistic concurrency `version BIGINT` |
| `versorgungsstatus_history` | Append-only audit log of every supply-state transition — powers `?at=` and `/history` |
| `nb_contracts` | NB network contracts — typed SQL columns (`netzebene`, `bilanzierungsmethode`, `billing_schedule`) + full BO4E `Vertrag` JSONB (`data`) for ERP digital LRV exchange |
| `pricat_versions` | Versioned PRICAT snapshots — `(nb_mp_id, tenant, valid_from)` unique, dispatch state |
| `pricat_dispatch_log` | Dispatch audit log — one row per NB × LF dispatch attempt |
| `nelo` | Netz-Element-Lokationen (Redispatch 2.0) — EIC or BDEW Codenummer, owner NB GLN, JSONB data |
| `malo_grid` | MaLo grid topology — Netzgebiet, Bilanzierungsgebiet, sourced from NIS/GIS |
| `steuerbare_ressourcen` | WiM iMS controllable resources — keyed by SR-ID (`C[A-Z0-9]{9}[0-9]`), linked to MaLo; `konfigurationsprodukte JSONB` for contracted iMS control products  |
| `technische_ressourcen` | E-mobility, generation, storage resources — keyed by TrId; `tr_typ`, `ist_fernschaltbar` typed columns; linked to MaLo/MeLo |
| `zaehler` | Meter registry — linked to MeLo; `zaehler_typ` (CHECK-constrained to BO4E `Zaehlertyp`), `eichung_bis` typed columns; BO4E payload with `zaehlwerke` array |
| `geraete` | Device registry — linked to Zaehler, stores `geraet_typ`, BO4E payload, and `geraet_konfigurationen JSONB` (typed `GeraetKonfiguration[]` per MsbG §23; GIN-indexed for cert-expiry queries) |
| `event_log` | Durable CloudEvent replay log — keyed by `event_id` (unique); indexed by `ce_type` + `received_at` |

All columns listed below are part of the single **`migrations/0001_initial.sql`** file.
There are no incremental migration files — the initial schema is the authoritative source.

### Key typed columns on `malo`

| Column | Type | Description |
|---|---|---|
| `netzebene` | `TEXT` | Netzebene (voltage/pressure level) — drives NNE tariff selection |
| `bilanzierungsgebiet` | `TEXT` | Bilanzierungsgebiet code for EEG/KWK allocation |
| `energierichtung` | `TEXT` | `AUSSP` (consumption) / `EINSP` (generation) |
| `gasqualitaet` | `TEXT` | Gas quality (H/L) |
| `bilanzierungsmethode` | `TEXT` | `RLM` \| `SLP` \| `IMS` \| `TLP_*`; drives `netzbilanzd` Leistungspreis routing |
| `regelzone` | `TEXT` | Regelzone EIC code — maps to ÜNB for Redispatch 2.0 + MABIS |
| `fallgruppe` | `TEXT` | GaBi Gas RLM category (e.g. `LNF`, `LF`, `TK`) |

### Key typed columns on `melo`

| Column | Type | Description |
|---|---|---|
| `netzebene_messung` | `TEXT` | Netzebene where the meter is installed |
| `regelzone` | `TEXT` | Regelzone EIC code — extracted from `standorteigenschaften.eigenschaftenStrom[0].regelzone` |
| `standorteigenschaften` | `JSONB` | Full `StandortEigenschaften` object (GIN indexed) for WiM Stammdaten enrichment |

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
  "bo4e_version": "v202607.0.0"
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

## NB Network Contracts — `Vertrag` BO4E

NB network contracts are stored in `nb_contracts` as **both** fast-query typed SQL
columns (`netzebene`, `bilanzierungsmethode`, `billing_schedule`, `valid_from`, `valid_to`)
**and** a full BO4E `Vertrag` JSON payload for ERP digital LRV exchange.

`vertragsart` and `vertragsstatus` are extracted from the payload as indexed columns,
enabling SQL-level filtering. A `de.markt.nb-contract.updated` CloudEvent is emitted on
every successful upsert so ERP subscribers can rebuild `Vertrag` caches without polling.

### `PUT /api/v1/nb-contracts/{contract_id}`

```json
{
  "malo_id":             "51238696780",
  "nb_mp_id":            "9900357000004",
  "sparte":              "STROM",
  "netzebene":           "NS",
  "bilanzierungsmethode": "SLP",
  "billing_schedule":    "MONTHLY",
  "valid_from":          "2026-10-01",
  "valid_to":            null,
  "data": {
    "_typ":            "VERTRAG",
    "vertragsart":     "NETZNUTZUNGSVERTRAG",
    "vertragsstatus":  "AKTIV",
    "sparte":          "STROM",
    "vertragsbeginn":  "2026-10-01T00:00:00+00:00"
  }
}
```

`data` is optional — if omitted, a minimal `Vertrag` is auto-constructed from the other
fields (`vertragsart = NETZNUTZUNGSVERTRAG`, `vertragsstatus = AKTIV`).

**Validation:** `_typ` must be `"VERTRAG"` (422 if wrong). All enum fields
(`vertragsart`, `vertragsstatus`) are validated against `rubo4e::current::Vertrag`.

### `GET /api/v1/nb-contracts/{contract_id}` response

```json
{
  "contract_id":         "nv-9900357000004-51238696780",
  "malo_id":             "51238696780",
  "nb_mp_id":            "9900357000004",
  "sparte":              "STROM",
  "netzebene":           "NS",
  "bilanzierungsmethode": "SLP",
  "billing_schedule":    "MONTHLY",
  "valid_from":          "2026-10-01",
  "valid_to":            null,
  "data": {
    "_typ":           "VERTRAG",
    "vertragsart":    "NETZNUTZUNGSVERTRAG",
    "vertragsstatus": "AKTIV",
    "sparte":         "STROM",
    "vertragsbeginn": "2026-10-01T00:00:00+00:00"
  },
  "vertragsart":    "NETZNUTZUNGSVERTRAG",
  "vertragsstatus": "AKTIV",
  "version":        1,
  "tenant":         "9900357000004"
}
```

`netzebene` accepts all Strom voltage levels (`NS`/`MS`/`MSP`/`HSP`/`HS`/`HöS`/`HöS/HS`)
and Gas pressure levels (`GND`/`GMT`/`GHD`). `bilanzierungsmethode` accepts `RLM`, `SLP`,
`IMS`, and TLP variants.

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

### Automatic `malo.bilanzierungsmethode` + `malo.fallgruppe` update

When `marktd` receives `de.mako.process.initiated` for PIDs 55001 (GPKE) or 44001 (GeLi
Gas), it calls `MaloRepository::patch_typenmerkmal()` to update the `malo` table:

| Payload field | Column | Source |
|---|---|---|
| `bilanzierungsmethode` | `malo.bilanzierungsmethode` | UTILMD `TM+EM` segment (Z01→SLP, Z02→RLM, Z04→IMS) extracted by `makod` adapter |
| `fallgruppe` | `malo.fallgruppe` | UTILMD `TM+Z10` segment (Gas GaBi RLM category) extracted by `makod` adapter |

This keeps the MaLo's billing mode and GaBi Gas Fallgruppe in sync with the UTILMD
without requiring a separate ERP `PUT /api/v1/malo` call. The update is best-effort:
if the MaLo row does not yet exist, the patch silently no-ops (the values will be set
on the first `PUT /api/v1/malo`).

```bash
# After a 55001 Anmeldung, verify the update:
curl -s "http://marktd:8180/api/v1/malo/10001234567" \
  -H "Authorization: Bearer <token>" | jq '.bilanzierungsmethode, .fallgruppe'
# → "SLP", null     (for a Strom SLP point)
# → "RLM", "Z01"   (for a Gas RLM point with GaBi category Z01)
```

---

## `PUT /api/v1/malo` — MaLo Typed Columns & Schema Validation

Every `PUT /api/v1/malo/{malo_id}` call:
1. **Validates** the incoming `data` payload as `rubo4e::current::Marktlokation`:
   - Auto-injects `_typ: "MARKTLOKATION"` if absent
   - Returns **422** if `_typ` is present but not `MARKTLOKATION`
   - Returns **422** if any typed field contains an unknown enum value
     (e.g. `"bilanzierungsmethode": "UNKNOWN"`)
2. **Normalises** to canonical camelCase BO4E form before storage
   (non-standard keys like `fallgruppenzuordnung` are preserved via the
    `_additional` extension map)
3. **Extracts typed columns** for efficient SQL queries:

| Column | Source field | Purpose |
|---|---|---|
| `netzebene` | `data.netzebene` | Voltage/pressure level for NNE billing tier |
| `bilanzierungsgebiet` | `data.bilanzierungsgebiet` | EIC code; drives `processd` NB check 4 |
| `gasqualitaet` | `data.gasqualitaet` | `HGas` \| `LGas`; Gas tariff routing |
| `energierichtung` | `data.energierichtung` | `Aussp` = generation, `Einsp` = consumption |
| `bilanzierungsmethode` | `data.bilanzierungsmethode` | `RLM` \| `SLP` \| `IMS` \| `TLP_*`; drives `netzbilanzd` Leistungspreis routing — RLM requires `spitzenleistung_kw` |
| `regelzone` | `data.regelzone` | Regelzone EIC code → maps MaLo to ÜNB for MABIS IFTSTA 21000 routing and Redispatch 2.0 Stammdaten forwarding |

All columns are `NULL` when the BO4E payload does not carry the field.

The call also automatically pushes the NB and MSB GLNs to `makod`'s MaLo cache
via `PUT /admin/malo/{malo_id}` — fire-and-forget; `makod` failure does not fail
the API call.

Fields forwarded to `makod`:

| Field | Source |
|---|---|
| `nb_mp_id` | `lokationszuordnung[]` entry with `zuordnungstyp == "NB"` or `"GNB"` |
| `msb_mp_id` | `lokationszuordnung[]` entry with `zuordnungstyp == "MSB"` or `"GMSB"` |
| `bilanzierungsgebiet` | `data.bilanzierungsgebiet` |
| `netzgebiet` | `data.netzgebietsnummer` or `data.netzgebiet` |
| `sparte` | `sparte` field |

**`MaloResponse`** (GET) exposes all typed columns as top-level fields alongside
the raw `data` JSONB for backward compatibility:

```json
{
  "malo_id": "10001234567",
  "sparte": "STROM",
  "version": 3,
  "netzebene": "NS",
  "bilanzierungsgebiet": "11YDE-RWE-NETZ-Y",
  "gasqualitaet": null,
  "energierichtung": "Einsp",
  "bilanzierungsmethode": "SLP",
  "regelzone": "10YDE-EON------1",
  "lokationszuordnung": [...],
  "data": { "_typ": "MARKTLOKATION", ... }
}
```

---

## ERP Subscriptions & Fan-Out

`marktd` delivers CloudEvents 1.0 to every matching ERP subscriber when master data changes or when `makod` lifecycle events arrive. The fan-out worker runs in a dedicated Tokio task and delivers independently per subscriber — a slow or unavailable ERP does not block other subscribers.

### Event types

| Source | Event type | Trigger |
|---|---|---|
| marktd master data | `de.markt.malo.updated` | `PUT /api/v1/malo/{malo_id}` |
| marktd master data | `de.markt.partner.updated` | `PUT /api/v1/partners/{mp_id}` |
| marktd NB contract | `de.markt.nb-contract.updated` | `PUT /api/v1/nb-contracts/{id}` — carries `vertragsart`, `version`, `tenant` in `data` |
| marktd PRICAT | `de.markt.pricat.published` | `PUT /api/v1/preisblaetter/{nb_mp_id}` |
| marktd supply | `de.markt.versorgung.changed` | any VersorgungsStatus transition (announce/confirm/end/clear), incl. PIDs 55003/44003 |
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

This durable failure path is required by **§ 147 AO / GoBD / §41 EnWG**: a silent
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
docker pull ghcr.io/hupe1980/marktd:0.12.0

docker run -d \
  --name marktd \
  -p 8180:8180 \
  -v /etc/marktd/marktd.toml:/etc/marktd/marktd.toml:ro \
  -e DATABASE_URL=postgres://marktd:secret@postgres/marktd \
  -e MAKOD_API_KEY=my-api-key \
  -e MAKOD_WEBHOOK_SECRET=my-webhook-secret \
  ghcr.io/hupe1980/marktd:0.12.0 \
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

`marktd` maintains one `VersorgungsStatus` record per MaLo, derived automatically from
inbound CloudEvents.  Every write appends a row to `versorgungsstatus_history` in the same
transaction, enabling full audit trails and `?at=YYYY-MM-DD` point-in-time queries.

### Supplier-transition lifecycle

A Lieferantenwechsel spans three distinct phases, each triggering a targeted partial update:

| Phase | CloudEvent | PID | Operation | Effect |
|---|---|---|---|---|
| **Announce** | `process.initiated` | 55001 / 44001 | `announce_lf_next` | Sets `lf_mp_id_next` (WHO) + `lf_next_lieferbeginn` (WHEN). Does **not** change `lieferstatus`. |
| **Confirm** | `process.completed` | 55003 / 44003 | `confirm_supply` | Atomic SQL: `lf_mp_id ← lf_mp_id_next`, `lieferbeginn ← lf_next_lieferbeginn`, `lieferstatus = Beliefert`, clears `lf_mp_id_next`. |
| **End** | `process.completed` | 55013 / 44013 | `end_supply` | `lieferstatus = Unbeliefert`, clears `lf_mp_id`/`lieferbeginn` — preserves `lf_mp_id_next` if another transition is already announced. |

All three operations are idempotent under at-least-once EventBus delivery.

### Schema

```
VersorgungsStatusRecord
├── malo_id              — 11-digit Marktlokations-ID
├── lieferstatus         — Beliefert | Unbeliefert | Grundversorgung | Ersatzversorgung | Ruhend | Stillgelegt
├── lf_mp_id             — active Lieferant MP-ID (set when Beliefert)
├── lf_mp_id_next        — announced future Lieferant MP-ID (WHO; set on 55001/44001)
├── lf_next_lieferbeginn — announced Lieferbeginn date (WHEN; paired with lf_mp_id_next)
├── lieferbeginn         — current supply start date
├── lieferende           — announced supply end date
├── msb_mp_id            — active Messstellenbetreiber MP-ID
├── nb_mp_id             — Netzbetreiber MP-ID (partition key)
├── last_process_id      — UUID of the last process that triggered a state change
├── updated_at           — UTC timestamp of last write
└── version              — optimistic-concurrency counter (OCC)
```

### State machine

```mermaid
stateDiagram-v2
    [*] --> Unbeliefert : MaLo registered

    Unbeliefert --> Unbeliefert : 55001/44001 process.initiated\n→ lf_mp_id_next + lf_next_lieferbeginn set
    Unbeliefert --> Beliefert   : 55003/44003 process.completed\n→ confirm_supply (lf_mp_id_next → lf_mp_id)

    Beliefert --> Beliefert     : 55001/44001 process.initiated\n→ next LF announced; current LF still active
    Beliefert --> Unbeliefert   : 55013/44013 process.completed\n→ end_supply (lf_mp_id_next preserved if set)

    Unbeliefert --> Grundversorgung  : §36 EnWG activation (GVU)
    Grundversorgung --> Beliefert    : GVU converts to standard contract
    Unbeliefert --> Ersatzversorgung : §38 EnWG activation (NB obligation)
    Ersatzversorgung --> Unbeliefert : Ersatzversorgung ends (max 3 months)

    Beliefert --> Ruhend    : MaKo suspended
    Ruhend --> Beliefert    : Reactivated
    Beliefert --> Stillgelegt : Decommissioned
    Stillgelegt --> [*]
```

**NB gap-closure (§38 EnWG).** When `end_supply` results in `lieferstatus = Unbeliefert`
and `lf_mp_id_next IS NULL`, the NB must activate Ersatzversorgung immediately.  When
`lf_mp_id_next IS NOT NULL`, the NB can schedule activation for `lf_next_lieferbeginn`.

**GPKE rule A06.** `processd` reads `lf_mp_id_next` before accepting a new 55001.  If
`lf_mp_id_next IS NOT NULL`, a second Anmeldung is already pending → `Reject A06`.

**Optimistic concurrency.** Every write uses `WHERE malo_id = $1 AND tenant = $2 AND version = $3`.
Conflict → `412 Precondition Failed` → retry after re-read.

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

**Response shape** (`GET /api/v1/versorgung/{malo_id}`):

```json
{
  "malo_id": "51238696780",
  "lieferstatus": "Beliefert",
  "lf_mp_id": "4012345000023",
  "lf_mp_id_next": null,
  "lf_next_lieferbeginn": null,
  "lieferbeginn": "2026-10-01",
  "lieferende": null,
  "msb_mp_id": "9900000000002",
  "nb_mp_id": "9900357000004",
  "last_process_id": "...",
  "updated_at": "2026-07-10T08:23:41Z",
  "version": 5
}
```

**Point-in-time query (`?at=YYYY-MM-DD`):** Returns the supply state as it was at
end-of-day on the given date in German local time (CET/CEST).  Returns `404` when no
history exists on or before that date.

```mermaid
sequenceDiagram
    participant ERP
    participant marktd
    participant PostgreSQL

    ERP->>marktd: GET /api/v1/versorgung/51238696780?at=2025-07-01
    marktd->>PostgreSQL: SELECT * FROM versorgungsstatus_history\nWHERE malo_id=$1 AND tenant=$2\nAND (valid_from AT TIME ZONE 'Europe/Berlin')::date <= '2025-07-01'\nORDER BY valid_from DESC LIMIT 1
    PostgreSQL-->>marktd: snapshot (LieferStatus, LF, NB, …)
    marktd-->>ERP: 200 OK + VersorgungsStatusResponse
```

`processd` reads `GET /api/v1/versorgung/{malo_id}` to drive automated LFA E_0624
responses without ERP involvement (GPKE Teil 1 §5).

---

## MSB Price Sheets — PreisblattMessung

`marktd` stores **MSB metering price sheets** (`PreisblattMessung`) in the
`preisblaetter_messung` table. These cover Messentgelte per Messpreistyp and
form the tariff basis for REQOTE/QUOTES (PIDs 35001–35005) and for
`invoicd` plausibility checks on INVOIC 31009 (MSB-Rechnung).

The API mirrors `PreisblattNetznutzung` exactly but is keyed by `msb_mp_id`
(the MSB's BDEW-Codenummer) instead of `nb_mp_id`.

```bash
# Upload an MSB price sheet (operator or ERP)
curl -s -X PUT "http://marktd:8180/api/v1/preisblaetter-messung/9900012345678" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "data": {
      "bo_typ": "PREISBLATT_MESSUNG",
      "bezeichnung": "Messentgelte 2026",
      "gueltigkeit": { "startdatum": "2026-10-01", "enddatum": "2027-09-30" },
      "preispositionen": [],
      "zeitvariablePreispositionen": [
        { "zaehlzeitregister": "HT", "preis": { "wert": "12.50", "einheit": "EUR_PRO_KWH" } },
        { "zaehlzeitregister": "NT", "preis": { "wert": "8.75",  "einheit": "EUR_PRO_KWH" } }
      ]
    },
    "bo4e_version": "v202607.0.0"
  }'

# Retrieve for a billing date — response includes typed zeitvariable_preispositionen
curl -s "http://marktd:8180/api/v1/preisblaetter-messung/9900012345678?date=2026-01-15" \
  -H "Authorization: Bearer <token>"
# → {
#     "data": { ... },
#     "zeitvariable_preispositionen": [
#       { "zaehlzeitregister": "HT", "preis": { "wert": "12.50", ... } },
#       { "zaehlzeitregister": "NT", "preis": { "wert": "8.75",  ... } }
#     ],
#     "auf_abschlaege": [],
#     "schema_drift_count": 0
#   }
```

### §14a Modul 2 — `zeitvariablePreispositionen`

For MSBs that operate under §14a Modul 2 (time-of-use pricing for controllable loads), each `ZeitvariablePreisposition` element in the price sheet **must** carry a non-empty `zaehlzeitregister` band code (e.g. `"HT"`, `"NT"`, `"ST"`). The PUT endpoint validates this:

| Validation | Error |
|-----------|-------|
| Missing `zaehlzeitregister` | 422 — mandatory per §14a Modul 2 (BK6-22-300) |
| `bandNummer` field present | 422 — does not exist in BO4E v202607 |
| Invalid BO4E schema | 422 — `serde_json` schema error |

`invoic-checker` check 4 uses the `zaehlzeitregister` codes to route INVOIC 31009 positions against the correct ToU band price, rather than guessing from `positionstext` keywords.

**Source-override protection.** Same as `preisblaetter`: an operator REST
upload (`source = 'api'`) is never silently overwritten by an engine ingest
(`source = 'mako'`).

---

## MMM Settlement Prices — Gas MMMA + Strom Ausgleichsenergie

`marktd` stores monthly settlement reference prices for Mehr-/Mindermengenabrechnungen
(MMM). These are **B2B settlement prices** — not B2C retail tariffs — and must therefore
live in `marktd`, not in a retail tariff service. Both `netzbilanzd` (NB, generates MMM
invoices) and `invoicd` (LF, validates inbound MMM invoices) need them, and they cannot
share a database directly.

### Gas MMM Abrechnungspreise — Trading Hub Europe (THE)

Published monthly by Trading Hub Europe (THE, the German gas market area operator
since 2021). `netzbilanzd` auto-fetches these when `mehr_preis_ct_per_kwh` /
`minder_preis_ct_per_kwh` are not supplied in the `POST /api/v1/billing/run` request.
`invoicd` uses them for **check 6** on inbound INVOIC 31007/31008.

```bash
# Import THE Gas MMMA prices for a billing month (operator monthly task)
curl -s -X PUT "http://marktd:8180/api/v1/mmma-preise/gas/2026/7" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "marktgebiet": "THE",
    "mehr_ct_kwh": "1.25",
    "minder_ct_kwh": "0.87",
    "source": "manual"
  }'

# Query (used by netzbilanzd and invoicd)
curl -s "http://marktd:8180/api/v1/mmma-preise/gas/2026/7" \
  -H "Authorization: Bearer <token>"
# → { "price_month": "2026-07-01", "marktgebiet": "THE", "mehr_ct_kwh": "1.25", ... }

# List all imported months
curl -s "http://marktd:8180/api/v1/mmma-preise/gas?limit=12" \
  -H "Authorization: Bearer <token>"
```

### Strom MMM Ausgleichsenergie — ÜNB (GPKE (BK6-24-174) Teil 1 Kap. 8.4)

Published monthly per ÜNB (50Hertz, TenneT, Amprion, TransnetBW). Used by
`netzbilanzd` for INVOIC 31002/31005 and `invoicd` check 6 on inbound Strom MMM invoices.

```bash
# Import Strom MMM prices for TenneT (example)
curl -s -X PUT "http://marktd:8180/api/v1/mmm-preise/strom/2026/7" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "vnb_mp_id": "9900823780008",
    "mehr_ct_kwh": "2.10",
    "minder_ct_kwh": "1.45",
    "source": "manual"
  }'

# Query
curl -s "http://marktd:8180/api/v1/mmm-preise/strom/2026/7?vnb_mp_id=9900823780008" \
  -H "Authorization: Bearer <token>"
```

**Operator task:** import these prices monthly before running MMM billing.
A missed monthly import causes `netzbilanzd` to require manual ERP input and
`invoicd` to skip check 6 (logged at debug level, not a hard error).

### Automated monthly import

`marktd` includes a **background worker** that automatically fetches and imports
Gas MMMA and Strom MMM prices on the 1st of each month. Configure it in `marktd.toml`:

```toml
[mmma_import]
enabled       = true
gas_url       = "https://www.the-group.de/gas/mmma/export.csv"  # THE CSV endpoint
strom_url     = "https://www.netztransparenz.de/mmm/strom.json" # ÜNB JSON endpoint
check_hour_utc = 6   # import at 06:00 UTC (after THE typically publishes ~05:00 UTC)
```

The worker wakes every hour; on the 1st of the month at or after `check_hour_utc`
it fetches, parses (CSV or JSON), and upserts — idempotent if prices already exist
for the current month.

**Supported formats:**

```csv
# CSV (5-column)
year,month,marktgebiet,mehr_ct_kwh,minder_ct_kwh
2026,7,THE,1.25,0.87
```

```json
// JSON (single object or array)
{ "mehr_ct_kwh": "1.25", "minder_ct_kwh": "0.87", "marktgebiet": "THE" }
```

**Manual trigger** (catch-up after downtime or testing):

```bash
curl -s -X POST "http://marktd:8180/api/v1/mmma-preise/import-trigger?year=2026&month=7" \
  -H "Authorization: Bearer <token>"
# → { "year": 2026, "month": 7, "import_enabled": true, "results": [...] }
```

**CloudEvents emitted:**

| Event type | Trigger |
|------------|---------|
| `de.markt.mmma.import.success` | Successful monthly import |
| `de.markt.mmma.import.failed` | Fetch or parse failure (requires operator action) |

---

## Device Registry — Zaehler + Geraete

`marktd` maintains a **device registry** for meters (Zähler) and their
associated devices (Geräte). This is populated by WiM MSB/NB device-handover
processes (ORDERS PIDs 17001–17011) and by operator REST uploads.

### Hierarchy

```
MeLo ──► Zaehler ──► Geraete
         (1..n)       (0..n)
              │
              └──► Zaehlwerk (0..n)
                   (OBIS registers)
```

A `Zaehler` carries:
- `zaehler_typ` — BO4E `Zaehlertyp`, **CHECK-constrained** to the 14 v202607 wire
  values (`DREHSTROMZAEHLER`, `INTELLIGENTES_MESSSYSTEM`, `MODERNE_MESSEINRICHTUNG`, …).
  `GASZAEHLER` is *not* one of them. §42c Energy-Sharing eligibility reads this
  column, so an unrecognised value would silently degrade a delivery point to
  `UNKNOWN`; the `schema_enum_guard` test pins the list to `rubo4e`.
  Watch the spelling: `Zaehlertyp` uses `INTELLIGENTES_MESSSYSTEM` (three `s`),
  while `Geraetetyp` uses `INTELLIGENTES_MESSYSTEM` (two). That is a BO4E quirk.
- `eichung_bis` — calibration valid-until date (Eichgültigkeitsdatum)
- `data` — full BO4E `Zaehler` payload (the `_typ` discriminator is **auto-injected**
  to `"Zaehler"` if absent, ensuring every stored object is self-describing)
- `data.zaehlwerke` — list of `Zaehlwerk` OBIS registers; exposed via
  `GET /api/v1/zaehler/{id}/zaehlwerke` as typed `Vec<Zaehlwerk>`

A `Geraet` carries:
- `geraet_typ` — e.g. `SMARTMETER_GATEWAY`, `WANDLER`, `MULTIPLEXANLAGE`
- `data` — full BO4E `Geraet` payload (`_typ` auto-injected to `"Geraet"` if absent)
- `konfigurationen` — typed `Vec<GeraetKonfiguration>` for MSB device management (see below)

> **BO4E `_typ` discriminator.** All four PUT device endpoints
> (`zaehler`, `geraete`, `steuerbare-ressourcen`, `technische-ressourcen`) automatically
> inject the correct `_typ` discriminator into the `data` JSONB blob if the caller omits it.
> Callers that include `_typ` in the request body have their value preserved.

### `Zaehlwerk` registers

Each `Zaehler` stores 0..n `Zaehlwerk` objects in `data["zaehlwerke"]` (BO4E `v202607`).
A `Zaehlwerk` is an individual measurement register on the meter, identified by its OBIS
code. iMSyS (intelligent metering systems) expose multiple registers simultaneously —
demand, reactive energy, export, time-of-use tariff splits.

`GET /api/v1/zaehler/{zaehler_id}/zaehlwerke` extracts the `zaehlwerke` array from
`data` and returns it as typed `Vec<Zaehlwerk>`:

```bash
curl -s "http://marktd:8180/api/v1/zaehler/Z001234567/zaehlwerke" \
  -H "Authorization: Bearer <token>" | jq '.[] | {
    obisKennzahl,
    richtung,
    verbrauchsart,
    anzahlAblesungen
  }'
```

Response shape:

```json
[
  {
    "_typ": "ZAEHLWERK",
    "obisKennzahl": "1-0:1.8.0",
    "richtung":     "EINSP",
    "verbrauchsart": "WIRKARBEIT",
    "anzahlAblesungen": 1
  },
  {
    "_typ": "ZAEHLWERK",
    "obisKennzahl": "1-0:2.8.0",
    "richtung":     "AUSSP",
    "verbrauchsart": "WIRKARBEIT",
    "anzahlAblesungen": 1
  }
]
```

Returns `[]` (not 404) when no registers are stored. Cedar action: `read-device`.

**Use cases:**
- TOU (time-of-use) billing: identify HT/NT registers before computing Arbeitspreis split
- iMSyS demand management: enumerate active demand registers for `wim.steuerungsauftrag.bestaetigen`
- MSB tariff selection: `PreisblattMessung` Preisstaffel matching uses `richtung` + OBIS

### Endpoints

```bash
# List meters for a MeLo
curl -s "http://marktd:8180/api/v1/melos/DE00056789000000000000000012345678/zaehler" \
  -H "Authorization: Bearer <token>" | jq '.[] | {zaehler_id, zaehler_typ, eichung_bis}'

# Register or update a meter (include zaehlwerke in data for structured register access)
curl -s -X PUT "http://marktd:8180/api/v1/zaehler/Z001234567" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "melo_id": "DE00056789000000000000000012345678",
    "zaehler_typ": "DREHSTROMZAEHLER",
    "eichung_bis": "2030-12-31",
    "data": {
      "zaehlwerke": [
        { "_typ": "ZAEHLWERK", "obisKennzahl": "1-0:1.8.0", "richtung": "EINSP" }
      ]
    },
    "bo4e_version": "v202607.0.0"
  }'

# List Zaehlwerk registers for a meter (typed Vec<Zaehlwerk>)
curl -s "http://marktd:8180/api/v1/zaehler/Z001234567/zaehlwerke" \
  -H "Authorization: Bearer <token>" | jq .

# List devices for a meter
curl -s "http://marktd:8180/api/v1/zaehler/Z001234567/geraete" \
  -H "Authorization: Bearer <token>" | jq '.[] | {geraet_id, geraet_typ}'

# Get a single Geraet — full BO4E payload + typed konfigurationen
curl -s "http://marktd:8180/api/v1/zaehler/Z001234567/geraete/SMGW-2026-001" \
  -H "Authorization: Bearer <token>" | jq '{geraet_id, geraet_typ, konfigurationen}'
```

---

## Geraet Konfigurationen — device configuration records (MsbG §23)

`marktd` maintains typed **device-configuration entries** for each `Geraet`, stored in the
`geraet_konfigurationen` JSONB column (separate from the BO4E `data` payload so they can
be updated atomically without rewriting the full `Geraet`).

The `geraet_konfigurationen` column has a GIN index, enabling fast SQL queries such as
"find all devices with SMGW cert expiring within 30 days":

```sql
SELECT malo_id FROM geraete
WHERE geraet_konfigurationen @> '[{"parameter":"SMGW_CERT_ABLAUFDATUM"}]'
  AND tenant = '...'
```

### `Konfigurationsparameter` enum

| Value | Type | Legal basis | Purpose |
|---|---|---|---|
| `FIRMWARE_VERSION` | string | BSI TR-03109-1 §4.3 | Current firmware version (e.g. `"3.1.2"`) |
| `HARDWARE_REVISION` | string | MsbG §23 | Hardware revision string |
| `KOMMUNIKATION` | enum string | — | Communication technology: `"GPRS"` / `"PLC"` / `"ETHERNET"` / `"FUNK"` / `"FESTNETZ"` / `"GSM"` |
| `FERN_UPDATE_FAEHIG` | bool string | BSI TR-03109-4 | Supports OTA firmware update (`"true"` / `"false"`) |
| `CLS_FAEHIG` | bool string | §14a EnWG BK6-22-300 | CLS channel capable (`"true"` / `"false"`) — checked by `processd` §14a auto-acknowledge |
| `SMGW_TLS_CERT_FINGERPRINT` | hex string | BSI TR-03109-3 | SHA-256 fingerprint (64 hex chars) of the SMGW TLS cert |
| `SMGW_CERT_ABLAUFDATUM` | ISO date | BSI TR-03109-4 §6.3 | TLS cert expiry date — monitored by `edmd` cert-expiry worker |
| `CLS_KANAL_ID` | string | BK6-24-174 §4.3 | CLS channel ID for §14a Steuerungsauftrag routing |
| `GWA_CODENUMMER` | 13-digit | BDEW | GWA (Gateway-Administrator) BDEW-Codenummer |
| `HERSTELLER` | string | MsbG §23 | Manufacturer name |
| `INBETRIEBNAHMEDATUM` | ISO date | § 13 StromNZV | Commissioning date |
| `LETZTE_WARTUNG` | ISO date | § 13 StromNZV | Last maintenance date |
| `NAECHSTE_WARTUNG` | ISO date | § 13 StromNZV | Next scheduled maintenance date |
| `AUSLESEE_PROTOKOLL` | enum string | — | Readout protocol: `"SML"` / `"DLMS"` / `"IEC62056"` |
| `MSB_VERTRAGSNUMMER` | string | MsbG §23 | MSB contract number for this device |
| `SONSTIGES` | string | — | Custom parameter — use `notiz` for the actual key name |

### `GeraetKonfiguration` entry shape

```json
{
  "parameter":  "SMGW_CERT_ABLAUFDATUM",
  "wert":       "2027-06-30",
  "updated_at": "2026-07-18T08:00:00Z",
  "notiz":      null
}
```

`updated_at` is **always set server-side** on write — callers must not include it in PUT requests.
Duplicate `parameter` values in a single PUT body are deduplicated (last entry wins) before storage.

### Endpoints

```bash
# Get current configuration entries for a device
curl -s "http://marktd:8180/api/v1/zaehler/Z001234567/geraete/SMGW-2026-001/konfigurationen" \
  -H "Authorization: Bearer <token>" | jq '.[] | {parameter, wert, updated_at}'

# Set SMGW configuration after BSI TR-03109-4 Admin session (§14a fleet rollout)
curl -s -X PUT "http://marktd:8180/api/v1/zaehler/Z001234567/geraete/SMGW-2026-001/konfigurationen" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "konfigurationen": [
      { "parameter": "FIRMWARE_VERSION",       "wert": "3.1.2"            },
      { "parameter": "HARDWARE_REVISION",      "wert": "Rev. C"           },
      { "parameter": "KOMMUNIKATION",          "wert": "GPRS"             },
      { "parameter": "CLS_FAEHIG",             "wert": "true"             },
      { "parameter": "CLS_KANAL_ID",           "wert": "CLS-00042"        },
      { "parameter": "SMGW_TLS_CERT_FINGERPRINT", "wert": "a1b2c3d4..."   },
      { "parameter": "SMGW_CERT_ABLAUFDATUM",  "wert": "2027-06-30"       },
      { "parameter": "GWA_CODENUMMER",         "wert": "9900000000099"    },
      { "parameter": "HERSTELLER",             "wert": "Sagemcom"         },
      { "parameter": "INBETRIEBNAHMEDATUM",    "wert": "2024-03-15"       }
    ]
  }'
# → 204 No Content + emits de.markt.geraet.konfiguration.updated CloudEvent
# If CLS_FAEHIG is set, processd auto-acknowledges §14a Steuerungsauftrag for this device.
# If SMGW_CERT_ABLAUFDATUM is set, edmd cert-expiry worker starts monitoring.
```

### Integration with `processd` (§14a Steuerungsauftrag)

When `CLS_FAEHIG = "true"` is stored, `processd` **auto-acknowledges** §14a
`WimSteuerungsauftrag` requests for this device (BK6-24-174 §4.3 rules).
When `CLS_FAEHIG = "false"` or absent, `processd` rejects with ERC `A97`.

### Integration with `edmd` (SMGW cert-expiry monitoring)

Setting `SMGW_CERT_ABLAUFDATUM` triggers the `edmd` **daily compliance worker** to monitor
the device. When the expiry date is ≤ 30 days away, `edmd` emits
`de.edmd.cls.compliance_issue` (severity `WARNING`); after expiry, severity `CRITICAL`.

---

## ZaehlzeitDefinition — typed TOU definition for ERP and portals

`GET /api/v1/zaehler/{zaehler_id}/zaehlzeitdefinitionen` assembles a complete
`rubo4e::current::Zaehlzeitdefinition` BO4E object from `zaehler_register` + `zaehler_saisons`
rows and returns it in canonical JSON. This is the endpoint ERP systems and customer portals
use to **display ToU register schedules** to end customers without custom ETL.

```bash
curl -s "http://marktd:8180/api/v1/zaehler/Z001234567/zaehlzeitdefinitionen" \
  -H "Authorization: Bearer <token>" | jq .
```

Response shape (BO4E `Zaehlzeitdefinition`):

```json
{
  "_typ": "ZAEHLZEITDEFINITION",
  "_id": "Z001234567",
  "saisons": [
    {
      "_typ": "ZAEHLZEITSAISON",
      "bezeichnung": "WINTER",
      "tagtypen": [
        {
          "_typ": "ZAEHLZEITTAGTYP",
          "tagtyp": "WERKTAGS",
          "umschaltzeiten": [
            { "_typ": "UMSCHALTZEIT", "registercode": "HT", "umschaltzeit": "07:00" },
            { "_typ": "UMSCHALTZEIT", "registercode": "NT", "umschaltzeit": "22:00" }
          ]
        },
        {
          "_typ": "ZAEHLZEITTAGTYP",
          "tagtyp": "WOCHENENDE",
          "umschaltzeiten": [
            { "_typ": "UMSCHALTZEIT", "registercode": "NT", "umschaltzeit": "00:00" }
          ]
        }
      ]
    }
  ]
}
```

The `?valid_only=true` query parameter restricts the response to currently valid registers
(`valid_to IS NULL OR valid_to >= today`).

**Why this endpoint?** ERP systems (Schleupen, SAP IS-U, powercloud) need the nested
`Zaehlzeitdefinition` shape for customer portal display. Without it, clients must query
two endpoints and assemble the hierarchy themselves. The endpoint returns canonical BO4E
that can be schema-validated client-side.

**§14a Modul 2 context.** Under BK6-22-300, the NB assigns HT/NT registers to controllable
loads at specific switching times communicated via WiM Stammdaten (ORDERS 17102–17133 ZAK+ZE segments).
`marktd` auto-populates the underlying data from those events; this endpoint exposes it in
BO4E form. See also [`billingd`](billingd.md) §14a Modul 2 billing.

---

## ZaehlzeitRegister — iMSys TOU register definitions

`ZaehlzeitRegister` and `ZaehlzeitSaison` provide structured Time-of-Use (TOU) register
definitions for iMSys (intelligent metering systems). They underpin §14a Modul 2 billing
by enabling automatic classification of 15-min Lastgang intervals into HT/NT tariff bands
without per-meter manual configuration.

### Data model

```
Zaehler (1) ──► ZaehlzeitRegister (N)
                   zaehlerauspraegung: HT | NT | EINZEL
                   obis_kennzahl: "1-1:1.29.0" (HT), "1-1:1.49.0" (NT)
                   valid_from / valid_to  (seasonal changeover supported)

ZaehlzeitRegister (1) ──► ZaehlzeitSaison (N)
                               saison: SOMMER | WINTER | GESAMT
                               wochentage: [1,2,3,4,5] (Mon–Fri)
                               zeit_von: "07:00"   (inclusive, local German time CET/CEST)
                               zeit_bis: "22:00"   (exclusive)
```

A typical residential iMSys meter has two registers (HT + NT) each with two seasons
(SOMMER, WINTER). Weekdays differ from weekends. `marktd` stores all combinations and
resolves them efficiently via a single PostgreSQL JOIN with JSONB `@>` containment.

### Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/v1/zaehler/{id}/register` | List all TOU registers for a Zaehler |
| `PUT` | `/api/v1/zaehler/{id}/register` | Upsert a ZaehlzeitRegister |
| `GET` | `/api/v1/zaehler-register/{id}/saisons` | List seasonal windows for a register |
| `PUT` | `/api/v1/zaehler-register/{id}/saisons` | Upsert a ZaehlzeitSaison |
| `GET` | `/api/v1/zaehler/{id}/tariff-zone` | Resolve HT/NT/EINZEL at a given datetime |

### Setting up TOU registers

```bash
# 1. Create an HT register
REGISTER_ID=$(uuidgen)
curl -s -X PUT "http://marktd:8180/api/v1/zaehler/Z001234567/register" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d "{
    \"id\": \"${REGISTER_ID}\",
    \"bezeichnung\": \"HT\",
    \"zaehlerauspraegung\": \"HT\",
    \"obis_kennzahl\": \"1-1:1.29.0\",
    \"einheit\": \"KWH\",
    \"valid_from\": \"2025-01-01\"
  }"

# 2. Add winter season: Mon–Fri 07:00–22:00
curl -s -X PUT "http://marktd:8180/api/v1/zaehler-register/${REGISTER_ID}/saisons" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d "{
    \"id\": \"$(uuidgen)\",
    \"saison\": \"WINTER\",
    \"wochentage\": [1,2,3,4,5],
    \"zeit_von\": \"07:00\",
    \"zeit_bis\": \"22:00\"
  }"

# 3. Resolve tariff zone at a specific time
curl -s "http://marktd:8180/api/v1/zaehler/Z001234567/tariff-zone?datetime=2026-01-15T14:30:00" \
  -H "Authorization: Bearer <token>" | jq .
# → { "zaehler_id": "Z001234567", "local_datetime": "2026-01-15T14:30:00", "tariff_zone": "HT" }
```

The `tariff-zone` endpoint performs a single JOIN between `zaehler_register` and
`zaehler_saisons`, filtering by JSONB array containment (`wochentage @> $3::jsonb`)
and time range. No application-level iteration is needed on the consumer side.

**Integration with `billingd` (§14a Modul 2):**
`billingd` calls `GET /api/v1/zaehler/{id}/tariff-zone?datetime=<slot-start>` for each
15-min slot in the billing month and aggregates kWh by zone. This eliminates the need for
operators to maintain manual HT/NT time-window configuration per meter in the billing engine.

### Automatic population from WiM Stammdaten (ORDERS 17102–17133)

`marktd` populates `ZaehlzeitRegister` and `ZaehlzeitSaison` **automatically** when `makod`
receives an inbound WiM Stammdaten response (ORDERS PIDs 17102–17133) from the MSB.

The adapter `extract_zak_ze_zaehlwerke()` in `makod` parses the EDIFACT **ZAK+ZE+ZD** segments:

| Segment | Field | Codes |
|---------|-------|-------|
| `ZAK` element 0 | `obis_kennzahl` | OBIS code (e.g. `"1-1:1.8.0"`) |
| `ZAK` element 1 | `zaehlerauspraegung` | `Z01`→`HT`, `Z02`→`NT`, `Z03`→`EINZEL` |
| `ZAK` element 2 | `bezeichnung` | Human-readable register label |
| `ZE` element 0 | `saison` | `Z01`→`SOMMER`, `Z02`→`WINTER`, `Z03`→`GESAMT` |
| `ZD` element 0 | `tagtyp` | `Z01`→`WERKTAG`, `Z02`→`SAMSTAG`, `Z03`→`SONNTAG_FEIERTAG` |
| `ZD` elements 1..N | time windows | `"HHMM:RegisterCode"` switch-point pairs |

After parsing, `makod` emits a `ProcessCompleted` outbox entry (CloudEvent type
`de.mako.process.completed`, PID 17102–17133) carrying `melo_id` and `zaehlwerke`.
When `marktd`'s `event_ingest` handler receives this event, it:

1. Looks up the Zaehler associated with the MeLo via `list_zaehler_by_melo()`
2. For each `zaehlwerk`, calls `upsert_register()` (idempotent on `zaehler_id + bezeichnung + valid_from`)
3. For each season window (ZE→ZD), calls `upsert_saison()` with a deterministic UUID
   derived from `(register_id, saison, tagtyp, zeit_von)` — safe for at-least-once delivery

This means operators **do not need to manually provision** ZaehlzeitRegister entries for
meters where the MSB sends WiM ORDERS Stammdaten responses — `marktd` and `makod` handle
it automatically.

**Data flow:**

```mermaid
sequenceDiagram
    participant MSB
    participant makod
    participant marktd
    participant billingd

    MSB->>makod: ORDERS PID 17102–17133<br/>ZAK+ZE+ZD segments
    makod->>makod: extract_zak_ze_zaehlwerke()<br/>parse OBIS / saisons / fenster
    makod--)marktd: de.mako.process.completed<br/>{melo_id, zaehlwerke:[...]}
    marktd->>marktd: list_zaehler_by_melo(melo_id)
    marktd->>marktd: upsert_register() + upsert_saison()<br/>(idempotent, deterministic UUID)
    billingd->>marktd: GET /zaehler/{id}/tariff-zone?datetime=...
    marktd-->>billingd: { "tariff_zone": "HT" }
```

---

## SteuerbareRessource Registry

`marktd` stores **steuerbare Ressourcen** (SR) — iMS controllable resources per
BK6-24-174 §6. An SR-ID has the format `C[A-Z0-9]{9}[0-9]` (Codetyp `C` +
9 alphanumeric chars + ASCII-Verfahren check digit).

Populated by WiM iMS Steuerungsauftrag processes (PID 55168) and by operator
uploads. Linked optionally to a MaLo and/or MeLo.

The `konfigurationsprodukte` field stores the contracted iMS control products
— used for pre-dispatch eligibility checks in `wim.steuerungsauftrag.bestaetigen`.
The value is preserved across PUT calls unless explicitly replaced via the sub-resource endpoint.

### Konfigurationsprodukte — typed API

The `konfigurationsprodukte` sub-resource has its own endpoints with **full BO4E validation** per BK6-24-174 §4.3:

```bash
# Retrieve typed Konfigurationsprodukte (returns Vec<ZeitvariablePreisposition> deserialized)
curl -s "http://marktd:8180/api/v1/steuerbare-ressourcen/C0001234567890/konfigurationsprodukte" \
  -H "Authorization: Bearer <token>"
# → {
#     "sr_id": "C0001234567890",
#     "konfigurationsprodukte": [{ "produktcode": "FLEX-001", ... }],
#     "count": 1,
#     "schema_drift": 0
#   }

# Replace all contracted products (validates each element + enforces non-empty produktcode)
curl -s -X PUT "http://marktd:8180/api/v1/steuerbare-ressourcen/C0001234567890/konfigurationsprodukte" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '[
    { "produktcode": "FLEX-PRODUCT-001", "zaehlzeitregister": "HT" }
  ]'
# → 204 No Content + emits de.markt.sr.konfigurationsprodukt.updated CloudEvent

# Remove a single product by produktcode
curl -s -X DELETE "http://marktd:8180/api/v1/steuerbare-ressourcen/C0001234567890/konfigurationsprodukte/FLEX-PRODUCT-001" \
  -H "Authorization: Bearer <token>"
# → 204 No Content
```

**Validation rules (BK6-24-174 §4.3):**

- Each element must deserialize as `rubo4e::current::Konfigurationsprodukt`
- `produktcode` **must not be empty** — every contracted product requires a unique code
- `bandNummer` is **rejected with 422** — it does not exist in BO4E v202607; use `zaehlzeitregister`

`processd` checks this list before auto-confirming a Steuerungsauftrag. An uncontracted `produktcode` triggers `wim.steuerungsauftrag.ablehnen` automatically.

---

## `PUT /api/v1/melo` — MeLo Typed Columns & Schema Validation

Every `PUT /api/v1/melo/{melo_id}` call:
1. **Validates** the incoming `data` as `rubo4e::current::Messlokation`: auto-injects `_typ: "MESSLOKATION"`, rejects wrong `_typ` or invalid enum values with 422.
2. **Normalises** to canonical camelCase BO4E form before storage.
3. **Extracts typed columns** for efficient SQL queries:

| Column | Source field | Purpose |
|---|---|---|
| `netzebene_messung` | `data.netzebeneMessung` | Voltage/pressure level at the metering point (e.g. `"NS"`, `"MS"`) |
| `regelzone` | `data.standorteigenschaften.eigenschaftenStrom[0].regelzone` | Regelzone EIC code → maps MeLo to the ÜNB for Redispatch 2.0 Stammdaten forwarding and MABIS IFTSTA 21000 routing |

**`MeloResponse`** (GET) returns `data: rubo4e::current::Messlokation` (typed):

```json
{
  "melo_id": "DE00056789000000000000000012345678",
  "malo_id": "10001234567",
  "version": 2,
  "netzebene_messung": "NS",
  "regelzone": "10YDE-EON------1",
  "data": { "_typ": "MESSLOKATION", "netzebeneMessung": "NS", ... }
}
```

To populate `regelzone` from a NIS export, include the BO4E path in the PUT body:

```json
{
  "malo_id": "10001234567",
  "data": {
    "_typ": "MESSLOKATION",
    "standorteigenschaften": {
      "eigenschaftenStrom": [
        { "regelzone": "10YDE-EON------1" }
      ]
    }
  }
}
```

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

**Request body for PUT** (includes typed NeLo columns):

```json
{
  "sparte": "STROM",
  "name": "Umspannwerk Musterstadt 110/20 kV",
  "netzebene": "HS",
  "nb_mp_id": "9900357000004",
  "steuerkanal": true,
  "eigenschaft_msb_lokation": "NB",
  "grundzustaendiger_msb_codenr": "9900357000004",
  "data": {
    "eic": "10XDE-EON-NETZ--G",
    "regelzone": "10YDE-EON------1"
  }
}
```

The `steuerkanal`, `eigenschaft_msb_lokation`, and `grundzustaendiger_msb_codenr` fields
are extracted into typed SQL columns at write time for efficient Redispatch 2.0 queries
(e.g. "find all controllable NeLos for a given NB").

**Authorization:** `read-nelo` is open to all authenticated users in the tenant.
`write-nelo` requires the `NB` role (Cedar policy `write-nelo`).

---

## Location Graph — `Lokationszuordnung`

The `lokationszuordnungen` table stores the full MaKo location graph as directed edges with
temporal validity (`valid_from`, `valid_to`). Each edge connects two location nodes by type:
`malo`, `melo`, `nelo`, `sr` (SteuerbareRessource), or `tr` (TechnischeRessource).

```mermaid
graph LR
    MaLo["MaLo (51238696780)"] --> MeLo["MeLo (DE-MEL-001)"]
    MeLo --> NeLo["NeLo (EIC 10XDE-…)"]
    MeLo --> SR["SR (C0001234567)"]
    MeLo --> TR["TR (T0001234567)"]
```

### Graph traversal

`GET /api/v1/malo/{id}/lokationen` runs a recursive-CTE BFS query (max depth 8) and returns
all reachable edges from the given MaLo, ordered by depth.

```http
# Full graph from a MaLo (all edges regardless of validity)
GET /api/v1/malo/51238696780/lokationen

# Graph valid on a specific date (temporal filter)
GET /api/v1/malo/51238696780/lokationen?at=2025-01-15

# Graph from a MeLo
GET /api/v1/melos/DE-MEL-001/lokationen?at=2025-01-15
```

Response: `Vec<LokationszuordnungEdge>` ordered by `depth` (0 = direct edges from root).

```json
[
  {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "tenant": "9900357000004",
    "von_id": "51238696780",
    "von_typ": "malo",
    "nach_id": "DE-MEL-001",
    "nach_typ": "melo",
    "valid_from": null,
    "valid_to": null,
    "data": {},
    "depth": 0
  }
]
```

### Upsert and delete

```http
# Upsert an edge (idempotent)
PUT /api/v1/lokationszuordnungen
Content-Type: application/json

{
  "von_id":    "51238696780",
  "von_typ":   "malo",
  "nach_id":   "DE-MEL-001",
  "nach_typ":  "melo",
  "valid_from": null,
  "valid_to":   null,
  "data":       {}
}

# Hard-delete an edge pair (all temporal variants)
DELETE /api/v1/lokationszuordnungen/51238696780/DE-MEL-001
```

**Temporal succession:** Multiple edges between the same `(von_id, nach_id)` pair are
allowed when `valid_from` differs. One open-ended edge (`valid_from IS NULL`) is permitted
per pair. Dated edges allow modelling supplier-switch driven reassignments.

---

## TechnischeRessource (E-mobility, Generation, Storage)

`TechnischeRessource` records link E-mobility charging points, generation units, and storage
to MaLos/MeLos. Required for WiM iMS Steuerungsauftrag routing and Redispatch 2.0 flexibility
registration.

```http
# Get a TechnischeRessource by TrId
GET  /api/v1/technische-ressourcen/{tr_id}

# Upsert
PUT  /api/v1/technische-ressourcen/{tr_id}

# List all TechnischeRessourcen linked to a MaLo
GET  /api/v1/malos/{malo_id}/technische-ressourcen
```

**Request body for PUT:**

```json
{
  "data":              { "_typ": "TechnischeRessource", ... },
  "malo_id":           "51238696780",
  "melo_id":           "DE-MEL-001",
  "tr_typ":            "EMobilitaet",
  "ist_fernschaltbar": true,
  "bo4e_version":      "v202607.0.0"
}
```

`tr_typ` values: `"EMobilitaet"` | `"Erzeugung"` | `"Speicher"` (or omit for unknown).
Invalid values are **rejected with `400 Bad Request`**.
`ist_fernschaltbar: true` marks the resource as remotely controllable for Redispatch 2.0.

---

## CloudEvent Replay Log

Every inbound CloudEvent is appended to the durable `event_log` table **before** fan-out,
enabling full replay without data loss.

```http
# Query the event log (all parameters optional)
GET /admin/events?from=2025-01-01T00:00:00Z&to=2025-02-01T00:00:00Z&type=de.mako.process.initiated&limit=500
```

Response: `Vec<EventLogRow>` ordered by `received_at ASC` (oldest first = deterministic replay).

**Use cases:**
- New subscriber onboarding: replay all `de.mako.process.initiated` events since go-live
- Bug fix replay: re-deliver specific event types after a `invoicd` fix
- Post-incident forensics: trace which events were delivered to which subscriber

---

## CloudEvents — outbound event catalog

`marktd` emits CloudEvents to all registered ERP webhook subscribers for the following
domain events. Each event carries the `markt*` extension attributes listed below the table.

| `type` | `subject` | Trigger | Consumers |
|---|---|---|---|
| `de.markt.malo.updated` | `malo_id` | MaLo PUT | `edmd`, `processd`, ERP |
| `de.markt.melo.updated` | `melo_id` | MeLo PUT | `edmd`, `processd`, ERP |
| `de.markt.pricat.published` | `nb_mp_id` | PRICAT 27003 dispatch | `netzbilanzd`, `invoicd`, ERP |
| `de.markt.nb-contract.updated` | `contract_id` | NB contract PUT | ERP |
| `de.markt.sr.konfigurationsprodukt.updated` | `sr_id` | SR Konfigurationsprodukt replace | `processd` (§14a eligibility check), ERP |
| `de.markt.geraet.konfiguration.updated` | `geraet_id` | Geraet konfigurationen PUT | `edmd` cert-expiry worker, `processd` §14a auto-ack check, ERP |
| `de.markt.mmma.import.success` | `year-month`, `commodity` | Monthly MMMA/MMM price import (Gas or Strom) | `netzbilanzd`, `invoicd` |
| `de.markt.mmma.import.failed` | `year-month`, `commodity` | Monthly import fetch/parse/store failure | operator |

### `de.markt.geraet.konfiguration.updated` data payload

```json
{
  "specversion": "1.0",
  "type":        "de.markt.geraet.konfiguration.updated",
  "source":      "urn:markt:tenant:9900000000003",
  "subject":     "SMGW-2026-001",
  "id":          "a1b2c3d4-...",
  "time":        "2026-07-18T08:01:00Z",
  "data": {
    "geraet_id":  "SMGW-2026-001",
    "zaehler_id": "Z001234567",
    "count":      10
  }
}
```

`edmd` subscribes to this event type and, upon receipt, updates the `smgw_sessions` table
for the corresponding MaLo if an existing session exists — ensuring the cert-expiry
compliance worker always has fresh `SMGW_CERT_ABLAUFDATUM` data without polling `marktd`.

## CloudEvents Extensions

All outbound events carry the `markt*` extension namespace (CloudEvents §3.3 — lowercase
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
| `traceparent` | string | W3C Trace Context — `00-<trace-id>-<parent-id>-<flags>` |
| `tracestate` | string | W3C Trace Context vendor bag — forwarded unchanged |

**`marktrole` derivation** from `makoworkflow`:

| Pattern | Role |
|---|---|
| ends with `-lf` or contains `-lf-` | `LF` |
| starts with `wim-` | `MSB` |
| starts with `mabis-` | `BIKO` |
| everything else | `NB` |

**Event source:** `"urn:markt:tenant:{tenant_gln}"`

---

## See Also

- [`makod` Operator Guide](./makod.md)
- [`netzbilanzd` Operator Guide](./netzbilanzd.md) — NNE/KA/MMM billing (NB role)
- [`sperrd` Operator Guide](./sperrd.md) — Sperrung execution tracking (NB role)
- [`invoicd` Operator Guide](./invoicd.md) — INVOIC plausibility, receipt persistence
- [`edmd` Operator Guide](./edmd.md) — meter-data store, time-series API
- [`obsd` Operator Guide](./obsd.md) — process observability, BNetzA KPI reports
- [ERP Integration](./erp-integration.md)
- [Domain Model](./domain-model.md)
- [Getting Started](./getting-started.md)
