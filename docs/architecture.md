---
layout: default
title: Architecture
nav_order: 4
has_children: true
description: >-
  mako system architecture: event-sourced process runtime, AS4/REST transport,
  ERP integration via CloudEvents 1.0, API-Webdienste Strom, and the six
  companion daemons (makod, marktd, processd, invoicd, edmd, obsd).
---

# Architecture

This document covers the design of `mako-engine` and the full service mesh:
event-sourced process runtime, inbound/outbound transport channels, ERP
integration via BO4E + CloudEvents 1.0, and the SlateDB persistence layer.
It also describes all six companion daemons and the `mako-service` shared
infrastructure library they build on.

---

## Design principles

| Principle | Consequence |
|---|---|
| **Protocol processor, not a business system** | `makod` handles EDIFACT, BDEW rules, AS4 delivery, and regulatory deadlines. Contract data and billing logic live in your ERP. |
| **`Workflow::handle` and `Workflow::apply` are pure functions** | All I/O, parsing, and clock access happens at the transport boundary before a command is constructed. This makes processes deterministic, replayable, and trivially testable. |
| **Atomic dual-write** | Events and outbox entries are written in a single `WriteBatch` via `AtomicAppend::append_with_outbox`. There is no two-phase commit, no compensation path for a lost APERAK. |
| **Event sourcing** | State is rebuilt by replaying the append-only event log. Audit trails, bug reproductions, and format-version migrations are a consequence of the model, not bolt-ons. |
| **Format-version coexistence** | `FV2025-10-01` and `FV2026-10-01` coexist in the same running instance. A process started under the old format version continues under those rules until it completes. |
| **Persist before dispatch** | `invoicd` writes each INVOIC receipt to PostgreSQL before issuing the settlement command to `makod`. A crash between check and dispatch is recoverable; a crash between persist and dispatch is not a data-loss event. |

---

## Service topology

```mermaid
graph TB
    NB["BDEW counterparty<br/>(NB / MSB / LF)"]
    AS4["AS4/ebMS3<br/>:4080"]
    REST["HTTP REST<br/>:8080"]
    API["API-Webdienste Strom<br/>:8090 (iMS)"]

    subgraph makod ["makod — Protocol daemon"]
        EDI["edi-energy<br/>Parse · Validate"]
        ENG["mako-engine<br/>Process Runtime"]
        SLATE["SlateDB<br/>events / outbox / deadlines"]
        EDI --> ENG --> SLATE
    end

    subgraph marktd ["marktd :8180 — Market Data Hub (pure data hub)"]
        MDM_DB["PostgreSQL\nMaLo · MeLo · contracts\nVersorgungsStatus + history · NeLo\nNbContracts · partners · preisblaetter\nmalo_grid (NB STP)"]
        FANOUT["EventBus fan-out\n→ ERP + processd + invoicd + obsd\n(WebhookBus default · KafkaBus via krafka feature)"]
    end

    subgraph processd ["processd :8580 — Process Decision Engine"]
        NB_MOD["NB module\nnetz-checker (6 checks)\nAnmeldung STP ≥ 95%"]
        LF_MOD["LF module\nE_0624 auto-response\napproval_queue"]
        PROC_DB["PostgreSQL\nanmeldung_decisions\napproval_queue"]
        NB_MOD & LF_MOD --> PROC_DB
    end

    subgraph invoicd ["invoicd :8280 — Billing settlement"]
        CHK["invoic-checker<br/>5 plausibility checks"]
        INV_DB["PostgreSQL<br/>invoic_receipts (§22 MessZV)"]
        CHK --> INV_DB
    end

    subgraph edmd ["edmd :8380 — Energy data"]
        EDM_DB["PostgreSQL<br/>meter_reads · receipts"]
    end

    subgraph obsd ["obsd :8480 — Observability"]
        PROJ["process_projections<br/>KPI · overdue · §20 parity"]
    end

    ERP["ERP system<br/>(SAP · CATENA-X · custom)"]
    OPS["Alertmanager · Grafana<br/>BNetzA KPI reports"]

    NB <-->|AS4/SOAP+MTOM| AS4
    NB <-->|HTTP REST| REST
    NB <-->|iMS REST/WS| API
    AS4 & REST & API --> EDI

    SLATE -->|CloudEvents 1.0<br/>HMAC-signed POST| marktd
    FANOUT -->|de.mako.process.initiated| processd
    FANOUT -->|de.mako.process.initiated| invoicd
    FANOUT -->|de.mako.*| edmd
    FANOUT -->|de.mako.*| obsd
    FANOUT -->|CloudEvents 1.0<br/>HMAC-signed| ERP

    NB_MOD -->|GET /api/v1/versorgung<br/>GET /api/v1/malo/{id}/grid| marktd
    LF_MOD -->|GET /api/v1/versorgung| marktd
    NB_MOD & LF_MOD -->|POST /api/v1/commands| makod

    CHK -->|GET /api/v1/preisblaetter| marktd
    CHK -->|POST /api/v1/commands| makod
    PROJ --> OPS
```

---

```
┌─────────────────────────────────────────────────────────────────────┐
│  Transport                                                           │
│  ┌──────────┐  ┌─────────────┐  ┌──────────────────────────────┐   │
│  │ AS4/SOAP │  │ HTTP REST   │  │ BDEW API-Webdienste Strom     │   │
│  │ :4080    │  │ :8080       │  │ :8090                         │   │
│  └────┬─────┘  └──────┬──────┘  └──────────────┬───────────────┘   │
└───────┼───────────────┼──────────────────────────┼──────────────────┘
        │               │                          │
┌───────▼───────────────▼──────────────────────────▼──────────────────┐
│  edi-energy — Parse · Validate · Build                              │
│  Profile registry (MIG + AHB rules) · 17 message types             │
└───────────────────────────┬─────────────────────────────────────────┘
                            │ typed Command
┌───────────────────────────▼─────────────────────────────────────────┐
│  mako-engine — Process Runtime                                      │
│  PidRouter · EngineContext · Process · Workflow (handle / apply)    │
│  DeadlineStore · OutboxStore · EventStore · SnapshotStore           │
└───────┬──────────────────────────────────────────────────────────┬──┘
        │                                                          │
        ▼  events + outbox (single WriteBatch)                     ▼  HTTP POST (CloudEvents 1.0)
┌───────────────────────────────┐         ┌────────────────────────────────────┐
│  SlateDB (object store)       │         ┌────────────────────────────────────┐
│  e/ events                    │         │  marktd :8180                        │
│  om/ outbox messages          │  POST   │  MaLo / MeLo / contracts           │
│  dl/ deadlines                │ ──────► │  partners / preisblaetter          │
│  pr/ process registry         │CloudEv. │  PostgreSQL · OIDC/JWT             │
│  pt/ partner directory        │         │  Cedar ABAC · fan-out to ERP       │
│  ib/ inbox dedup              │         └────────────┬───────────┬───────────┘
│  sv/ stream versions          │                      │           │
└───────────────────────────────┘           CloudEv.   │           │ CloudEv. 1.0 + HMAC
                                           ┌────────────▼──────┐  ┌▼───────────────────┐
                                           │  invoicd :8280    │  │  ERP system         │
                                           │  invoic-checker   │  │  BO4E JSON          │
                                           │  PostgreSQL audit │  │  HMAC-SHA256 signed │
                                           └────────┬──────────┘  └────────────────────┘
                                                    │ POST /api/v1/commands
                                           ┌────────▼──────────┐
                                           │  makod :8080      │
                                           │  annehmen/ablehnen│
                                           │  → REMADV/COMDIS  │
                                           └───────────────────┘
```

```
BDEW counterparty
    │  AS4/ebMS3 push (SOAP+MTOM over HTTPS)
    ▼
makod/as4_ingest
    │  WSS-verify signature · extract MIME attachment
    ▼
InboxStore::accept     ← 72-hour dedup (prevents double-processing)
    │  raw EDIFACT bytes
    ▼
Platform::parse_interchange (edi-energy)
    │  structured messages, detected PID per message
    ▼
PidRouter::route       ← selects domain module by Prüfidentifikator
    │  workflow_name + PID
    ▼
EdifactIngestDispatcher::dispatch   ← spawns or resumes process by MaLo business key
    │  typed Command (via AdapterRegistry → MessageAdapter)
    ▼
Process::execute_and_enqueue_with_snapshot_and_retry
    ├── replay EventStore → rebuild State   (Workflow::apply — pure)
    ├── Workflow::handle(state, command)     (pure, returns events + outbox)
    └── AtomicAppend::append_with_outbox    (single WriteBatch)
         ├── EventStore  (e/<tenant>/<stream_id>/seq)
         └── OutboxStore (om/<tenant>/<id>)
```

---

## Companion daemons

All six daemons share a common operational model:
- **TOML configuration** — loaded from a file (`makod.toml`, `marktd.toml`, …) with `env:VAR_NAME` secret interpolation
- **Cedar ABAC** — all HTTP endpoints gated by Cedar attribute-based access control
- **OIDC/JWT** — asymmetric algorithm only; JWKS cached with background refresh; omit `[oidc]` for dev mode
- **MCP server** — built-in `POST|GET /mcp` endpoint (MCP Streamable HTTP, 2025-11-25) for LLM tooling
- **OpenTelemetry** — OTLP traces on all workflow commands, event appends, and webhook deliveries

| Daemon | Port | Role | Config file |
|--------|------|------|-------------|
| `makod` | `:8080` / `:4080` / `:8090` | Protocol gateway — EDIFACT ↔ BO4E, 45+ workflows, AS4 ingest, deadlines | `makod.toml` |
| `marktd` | `:8180` | Market Data Hub — MaLo/MeLo/VersorgungsStatus/preisblaetter, EventBus fan-out | `marktd.toml` |
| `processd` | `:8580` | Process decision engine — NB STP (netz-checker) + LF E_0624 auto-response | `processd.toml` |
| `invoicd` | `:8280` | INVOIC plausibility — REMADV, selbstausstellen, overdue-REMADV, §22 MessZV audit | `invoicd.toml` |
| `edmd` | `:8380` | Energy data management — MSCONS meter readings, time-series, `MeterBillingPeriod` | `edmd.toml` |
| `obsd` | `:8480` | Process observability — KPI reports, deadline-risk alerts, §20 EnWG parity | `obsd.toml` |

### `marktd` — Market Data Hub (`:8180`)

`marktd` is the single source of truth for all market entity state.
It stores Marktlokationen (MaLo), Messlokationen (MeLo), contracts, trading
partners, network contracts (NbContractRecord), price sheets,
**VersorgungsStatus per MaLo** (with full history and `?at=YYYY-MM-DD`
point-in-time queries), **MaLo grid topology** (`malo_grid` table, sourced from
the NB’s NIS/GIS system and provisioned via `PUT /api/v1/malo/{id}/grid`; **not**
from MaStR), and **Netz-Element-Lokationen (NeLo)** for Redispatch 2.0.

`makod` pushes `de.mako.process.*` CloudEvents to `marktd`'s ingest endpoint;
`marktd` fans them out to all registered subscribers. The `VersorgungsStatus`
is derived automatically on `de.mako.process.completed` (PIDs 55003/44003 →
Beliefert, 55013/44013 → Unbeliefert). Every supply-state change is written
to `versorgungsstatus_history`, enabling both full audit logs and bitemporal
"as-of" queries by date.

`marktd` is a **pure data hub** — it stores market entity state and fans out
CloudEvents to subscribers but contains no domain policy. Automated Anmeldung
decisions live in `processd`’s NB module.

See [`marktd` Operator Guide](./marktd.md).

### `processd` — Process Decision Engine (`:8580`)

`processd` consumes `de.mako.process.initiated` CloudEvents from `marktd` and
makes automated decisions within regulatory deadlines.

**NB module** (`--features nb-only` or `integrated`):
- Handles GPKE Lieferbeginn (55001/55016) and GeLi Gas Lieferbeginn (44001)
- Fetches `VersorgungsStatus` + `MaloGridRecord` from `marktd`
- Evaluates 6 objective checks via the pure `netz-checker` library
- Dispatches `bestaetigen`/`ablehnen` to `makod` with §20 EnWG parity logging
- STP target ≥ 95 % (requires NIS/GIS grid records via `nis-syncd` or manual provisioning)

**LF module** (`--features lf-only` or `integrated`):
- Handles LFA E_0624 (PID 55008) within the 45-minute LFW24 window
- Auto-consents clean Abmeldungen; auto-rejects Einzug (A32) scenarios
- Queues ambiguous cases in `approval_queue` for ERP operator review

See [`processd` Operator Guide](./processd.md).

### `invoicd` — Automated Billing Settlement (`:8280`)

`invoicd` is the autonomous INVOIC plausibility-check pipeline for the
Lieferant role. It subscribes to `de.mako.process.initiated` events from `marktd`,
runs five checks (period validity, position arithmetic, document total, tariff
match, tariff found), persists the receipt to PostgreSQL, then issues
`gpke.abrechnung.annehmen` or `gpke.abrechnung.ablehnen` back to `makod`.

The PostgreSQL persistence provides a durable audit trail of all received
invoices, plausibility outcomes, and check findings — satisfying the 3-year
retention requirement under §22 MessZV and §41 EnWG.

**Supported PIDs:** 31001, 31002, 31005, 31006 (GPKE MMM-Rechnung); 31009
(WiM MSB-Rechnung — DLQ path pending full N3 integration).

**M16 additions:**
- `POST /api/v1/selbstausstellen/{malo_id}` — outbound INVOIC 31006 (LF selbstausgestellt)
- `GET /api/v1/overdue-remadv` — receipts approaching Zahlungsziel without REMADV
- MCP tool `list_overdue_remadv` — deadline monitoring for LLM tooling

### `edmd` — Energy Data Management (`:8380`)

`edmd` stores MSCONS meter readings received from `marktd` and makes them
queryable via a REST time-series API. It is the authoritative source of
LF-side metered consumption data for Mehr-/Mindermengen (MMM) imbalance
calculations and billing plausibility.

Key facts:
- Subscribes to `de.mako.process.completed` events from `marktd` where `makopid`
  is in the MSCONS PID set (`mako_edm::domain::MSCONS_PIDS`).
- Stores typed kWh interval reads with `(malo_id, dtm_from, dtm_to)` primary key.
- Exposes `GET /api/v1/deliveries/{malo_id}`,
  `GET /api/v1/imbalance/{malo_id}/{year}/{month}`, and (M15)
  `GET /api/v1/billing-period/{malo_id}?from=&to=`.
- `MeterBillingPeriod` provides `arbeitsmenge_kwh`, `spitzenleistung_kw` (RLM Strom),
  `brennwert_kwh_per_m3` + `zustandszahl` (Gas) for billing plausibility (M16)
  and NNE invoice generation (N4).
- Pre-aggregated `meter_billing_periods` table (migration 0002) for fast billing queries.

### `obsd` — Business-Process Observability (`:8480`)

`obsd` projects all `de.mako.*` CloudEvents from `marktd` into a queryable CQRS
read-model of running and completed MaKo processes. It has no authoritative
state — the projection is fully rebuildable by replaying the event stream.

Key facts:
- Wildcard subscription to all `de.mako.*` events from `marktd`.
- One `process_projections` row per MaKo process, with state, deadline, and
  pre-computed `deadline_risk` (`green` / `amber` / `red` / `overdue`).
- `GET /obs/processes`, `GET /obs/kpis`, `GET /obs/overdue` REST endpoints.
- BNetzA KPI report via `GET /obs/kpis?pid=55001&period=2025-10`.
- Integrates with Alertmanager: `GET /obs/overdue` as a Prometheus alert target.

### `mako-service` — Shared service infrastructure (library)

`mako-service` is a library crate that all mako daemons build on. It provides:
- `ServiceBuilder` — composable Axum router builder with health and metrics routes
- `load_config` — type-safe TOML configuration loader with `env:VAR_NAME` interpolation
- `health_routes` — `/health/live` (liveness) and `/health/ready` (readiness) endpoints
- `verify_hmac` / `hmac_hex` — constant-time HMAC-SHA256 webhook signature helpers

---

## Outbound flows

### AS4 EDIFACT delivery

`OutboxWorker` polls `OutboxStore` every 5 seconds. For each pending message:

1. Render EDIFACT interchange via `edi-energy` builders.
2. Look up trading partner AS4 endpoint in `PartnerStore`.
3. Sign with operator PKCS#12 credential.
4. POST via `asx-rs` AS4 sender.
5. On HTTP 200: delete outbox entry. On 4xx/5xx: back-off and retry.

**Self-addressed messages** (`recipient == tenant_party_id`) bypass the AS4
transport entirely.  `BdewAs4Sender` renders the EDIFACT bytes, re-parses
them via `Platform::parse_interchange`, and passes each message to
`EdifactIngestDispatcher::dispatch` for in-process delivery to the correct
workflow.  See [Integrated operators](./makod.md#integrated-operators-nb--msb-same-gln)
for the full dispatch table and configuration notes.

### ERP CloudEvents delivery

`OutboxErpWorker` polls `OutboxStore` every 5 seconds. For each ERP-targeted message:

1. Build a [CloudEvents 1.0](https://cloudevents.io) envelope from the `ErpEvent`.
2. Set `Content-Type: application/cloudevents+json`.
3. Sign with `HMAC-SHA256` over the raw body (when `--erp-webhook-secret` is set).
4. POST to the configured `--erp-webhook-url`.
5. On `2xx`: acknowledged. On `429`/`5xx`: exponential back-off. On `4xx`: dead-letter immediately.

See [ERP Integration](./erp-integration.md) for the full CloudEvents schema and receiver implementation guide.

### Deadline scheduler

`DeadlineScheduler` ticks every **30 seconds** by default (configurable via
`--deadline-poll-interval-secs`; minimum 1 second). For each due entry in `DeadlineStore`:

1. Reconstruct the `ProcessIdentity` from the deadline record.
2. Dispatch a `TimeoutExpired` command to the workflow.
3. The workflow produces a `DeadlineExpired` event and an `AperakTimeout` outbox entry.
4. The outbox entry routes to `OutboxErpWorker`, which delivers the `de.mako.aperak.timeout` CloudEvent to the ERP.

---

## Domain crate layering

Each domain crate is a thin wrapper that:
- Defines `Command`, `Event`, and `State` enums specific to its regulatory process family.
- Implements `Workflow` with pure `handle` and `apply` functions.
- Registers itself in the `PidRouter` via a `register_*` function called from `makod`.

`makod` wires the domain modules, transport adapters, and the ingest dispatcher
at startup:

```
makod (binary)
├── registers mako-gpke    → PIDs 55001–55024, 55555, 55607–55609, 17115–17117 (Strom NB),
│                            17134/17135, 19001/19002, 31001–31002, 31005–31006, 37000–37006
├── registers mako-wim     → PIDs 55039, 55042, 55051, 55168, 31009, 23001/23003/23004/23008,
│                            17001–17011, 19001/19002 (nMSB role), 39000, 35001–35005, 15001–15005
├── registers mako-geli-gas → PIDs 44001–44021, 44022* (Nb role), 44023–44024* (Lf role),
│                             37008–37014, 31011, 17115–17117 (Gas NB)
├── registers mako-mabis   → PID 13003
├── registers mako-wim-gas → PIDs 44022–44024* (Msb/Nmsb role), 44039–44053, 44168–44170,
│                            31003, 31004, 23005, 23009
├── registers mako-gabi-gas → PIDs 31007, 31008, 31010, ORDERS 17110, ORDRSP 19110,
│                             MSCONS 13013, synthetic PIDs 90001–90062 (DVGW gas transport)
├── registers mako-redispatch → Redispatch 2.0 XML workflows
│
└── wires EdifactIngestDispatcher
         ├── called by: AS4 inbound (as4_ingest), REST ingest (edifact_api)
         └── called by: AS4 sender loopback (BdewAs4Sender, recipient == own GLN)
```

`*` PIDs 44022–44024 use role-conditional routing:
- `mako-wim-gas` `wim-gas-stornierung`: Msb/Nmsb/all-role deployments
- `mako-geli-gas` `geli-gas-stornierung`: Nb-only (44022 inbound as GNB)
- `mako-geli-gas` `geli-gas-stornierung-lf`: Lf-only (44023/44024 inbound as LFN/LFA)

See [PID Reference](./pid-reference.md) for the complete table.

---

## SlateDB key schema

All state is stored in a single SlateDB column family. Keys are byte-sortable
to enable efficient range scans per tenant and stream.

| Prefix | Content | Key pattern |
|--------|---------|-------------|
| `e/` | Event log | `e/<tenant_id>/<stream_id>/<seq_u64_big_endian>` |
| `sv/` | Stream version (optimistic lock) | `sv/<tenant_id>/<stream_id>` |
| `om/` | Outbox messages | `om/<tenant_id>/<ulid>` |
| `dl/` | Deadlines | `dl/<tenant_id>/<due_timestamp_secs>/<id>` |
| `pr/` | Process registry | `pr/<tenant_id>/<conversation_id>` |
| `pt/` | Partner directory | `pt/<tenant_id>/<gln>` |
| `ib/` | Inbox dedup | `ib/<tenant_id>/<message_ref>` |
| `sn/` | Snapshots | `sn/<tenant_id>/<stream_id>` |

The `dl/` prefix sorts by due timestamp, so `range_scan(prefix, now_key)` is
the entire scheduler implementation.

---

## Testing strategy

| Layer | Test type | Tooling |
|---|---|---|
| EDIFACT parse/validate | Unit + property | `edi-energy` tests, `cargo-fuzz` (1 100+ corpus entries) |
| Workflow logic | Unit (sync) | `InMemoryEventStore`, `InMemoryOutboxStore`, `NoopErpAdapter` |
| End-to-end process flows | Async integration | `mako-engine` integration tests; `makod` e2e AHB conformance test |
| Deadline arithmetic | Unit | `fristen` crate with Germany public holiday fixtures |
| CloudEvents delivery | Integration | `OutboxErpWorker` test with mock HTTP server |
| AS4 inbound routing | Integration | `e2e_ahb_conformance.rs` — real fixture EDIFACT → full pipeline |

---

## Related documentation

| Topic | File |
|---|---|
| Getting started | [getting-started.md](getting-started.md) |
| Engine internals | [engine.md](engine.md) |
| `makod` operator guide | [makod.md](makod.md) |
| `marktd` operator guide | [marktd.md](marktd.md) |
| `invoicd` operator guide | [services/invoicd/README.md](https://github.com/hupe1980/mako/blob/main/services/invoicd/README.md) |
| `edmd` operator guide | [services/edmd/README.md](https://github.com/hupe1980/mako/blob/main/services/edmd/README.md) |
| `obsd` operator guide | [services/obsd/README.md](https://github.com/hupe1980/mako/blob/main/services/obsd/README.md) |
| ERP integration | [erp-integration.md](erp-integration.md) |
| PID reference | [pid-reference.md](pid-reference.md) |
| Compensation flows | [compensation.md](compensation.md) |
