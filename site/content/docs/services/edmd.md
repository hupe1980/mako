+++
title = "edmd Operator Guide"
description = "Operator guide for edmd, the energy-data daemon: MSCONS and iMSys readings, quality scoring, validation, virtual meters and § 60 MsbG Ersatzwerte."
weight = 27
+++
`edmd` is the **Energy Data Management daemon**: it stores meter readings and
computes billing-relevant energy quantities for downstream services. It is the
MSB's data store; LF, NB, BKV and ESA are its callers.

| Term | What it is |
|---|---|
| **Marktlokation** (MaLo) | The point energy is *settled* at — one 11-digit ID. Everything billed is billed per MaLo |
| **Messlokation** (MeLo) | The point energy is *measured* at — one meter; several can feed one MaLo ([MaLo vs MeLo](@/docs/architecture/domain-model.md#malo-vs-melo-the-critical-distinction)) |
| **Sparte** | `STROM`, `GAS`, `WAERME`, `WASSER` |
| **Lastgang** | Energy per interval — quarter-hourly (Strom) or hourly (Gas) |
| **Zählerstandsgang** (ZSG) | The register readings an intelligentes Messsystem produces; the Lastgang is their difference |
| **Ersatzwert** | A substitute value the MSB forms where no usable measurement exists |
| **Prüfidentifikator** (PID) | Five-digit BDEW code naming the business message ([PID reference](@/docs/regulatory/pid-reference.md)) |

Roles and identifiers are in the [domain model](@/docs/architecture/domain-model.md).

## What it does

- Stores MSCONS readings (SLP and RLM) delivered by the `marktd` webhook, and
  readings pushed directly: iMSys/SMGW (§41a EnWG), gas, IoT submetering, bulk
  import, Kafka, and the Zählerstandsgang.
- Validates every batch, one series per register, with `metering`'s
  validation rules and grades it A/B/C/F; raises
  `de.messwert.reading.quality.warning` from every ingest door.
- Fills gaps with § 60 Abs. 1 MsbG Ersatzwerte and tracks their confirmation.
- Serves the projected energy series, BO4E `Energiemenge` / `Lastgang` /
  `Zeitreihe` exports, `MeterBillingPeriod`, the Mehr-/Mindermengensaldo, the
  MaBiS Summenzeitreihe, resampled Lastgänge and § 40a Abs. 2 EnWG annual
  projections.
- Schedules reading orders (Ablesesteuerung) for LF, MSB, NB and ESA, and
  creates them automatically from GPKE and INSRPT events.
- Computes virtual meters (UTILTS Berechnungsformel, §42b EnWG GGV) and §42c
  Energy-Sharing allocation and readiness.
- Watches for measuring points that stop delivering, and runs the SMGW §14a /
  certificate compliance sweep.
- Keeps ESA „Werte nach Typ 2" in a separate store no billing path reaches.
- Executes GDPR Art. 17 erasure as pseudonymisation.

Readings live in [`meterstore`](https://github.com/hupe1980/meterstore): a recent
window in PostgreSQL and the settled history in Apache Iceberg V2 on S3/GCS/Azure,
split by a tiering watermark, version-resolved on read. edmd's own business tables
(receipts, corrections, confirmations, billing-period cache, reading orders,
SMGW) are ordinary tables in its PostgreSQL pool.

The domain arithmetic is the [`metering`](https://github.com/hupe1980/metering)
crate (zero I/O, no async, no clock). Its input is a validated `Series` —
sorted, unique, on one grid, one channel — so edmd splits a measuring point's
readings by register and builds each into a series on its observed grid
(`edmd::domain::vee`). The BSI TR-03109 SMGW device model is not energy
arithmetic and lives in `edmd::smgw_model`. `metering::MaloId` enforces the MaLo
check digit at the store boundary; a malformed ID is `400`.

| `metering` | §-basis | Used in |
|---|---|---|
| `gas::conversion::gas_m3_to_kwh_hs(m3, hs, z)` | §25 Nr. 4 MessEV / DVGW G 685 | Gas ingest |
| `billing::imbalance::ImbalanceSaldo::new(actual, bilanziert)` | GPKE (BK6-24-174) Teil 1 Kap. 8.4 | Mehr-/Mindermengensaldo |
| `vee::validation::validate(series, Rules)` | § 60 Abs. 1 MsbG (Plausibilisierung) | Validation and the A/B/C/F grade |
| `vee::substitute::substitute(series, report, Policy)` | § 60 Abs. 1 MsbG; MeteringCode 2006 A8.2.2 | Ersatzwerte |
| `series::resample::resample(series, resolution)` | GPKE Teil 1 Kap. 8.4, MaBiS | Hourly/daily/monthly buckets |
| `billing::aggregation::aggregate(series, period)` | § 17 Abs. 2 StromNEV | A bucket's peak |
| `allocation::formula::Formula::eval` | UTILTS AHB 1.1 (PID 25001) | Virtual meters |
| `allocation::community::allocate` | §42b Abs. 5 EnWG · §42c EnWG | Community shares |
| `allocation::sharing::assess_delivery` | §42c Abs. 1 EnWG | Sharing readiness |
| `billing::forecast::project_annual_consumption(series, prior)` | § 40a Abs. 2 EnWG · § 13 Abs. 1 StromGVV | Annual projection |
| `billing::losses::NetworkLosses::new(einspeisung, entnahme)` | §22 Abs. 1 EnWG | Netzverlust |

```mermaid
graph TB
    marktd["marktd :8180<br/>fan-out"]
    smgw["SMGW / iMSys<br/>(direct push)"]
    edmd["edmd :8380<br/>(this service)"]

    subgraph store["meterstore — hot/cold tiered store for meter_reads / esa_typ2_reads"]
        hot["Hot tier — PostgreSQL<br/>(recent window)"]
        cold["Cold tier — S3 / GCS / AzureDLS<br/>Iceberg V2 settled history"]
    end

    subgraph edmdpg["edmd PostgreSQL — business tables"]
        biz["meter_billing_periods<br/>ablese_auftraege<br/>direct_push_sessions<br/>gdpr_deletions"]
    end

    erp["ERP / netzbilanzd"]
    duckdb["DuckDB / Spark<br/>Trino / PyIceberg"]
    catalog["/api/v1/iceberg<br/>meterstore CatalogFacade<br/>(read-only · Cedar read-archive-olap)"]
    qa["quality engine<br/>metering vee::validation<br/>per register"]

    marktd -->|"de.mako.process.* <br/>HMAC POST /webhook"| edmd
    smgw -->|"POST /api/v1/meter-reads/rlm/{malo_id}<br/>POST /api/v1/meter-reads/gas/{malo_id}"| edmd
    edmd --> qa
    qa -->|"annotated reads (ValidatedReads)"| store
    qa -->|"grade C/F <b>or</b> any finding<br/>de.messwert.reading.quality.warning"| erp
    edmd --> edmdpg
    edmd -->|"mounts"| catalog
    catalog -.->|"table schemas + locations"| cold
    hot -->|"tiering watermark<br/>(one-week settlement lag)"| cold
    erp -->|"GET /api/v1/lastgang/{malo_id}<br/>Accept: arrow.stream"| edmd
    erp -->|"GET /api/v1/billing-period/{malo_id}"| edmd
    erp -->|"POST /api/v1/query/sql"| edmd
    duckdb -->|"ATTACH — metadata only"| catalog
    duckdb -->|"read Parquet directly<br/>(engine's own credentials)"| cold
```

---

## Port layout

```
┌───────────────────────────────────────────────────────────────────────────────────────────────┐
│  edmd  :8380                                                                                  │
│                                                                                               │
│  POST /webhook                                             ← marktd CloudEvents (HMAC)        │
│                                                                                               │
│  ── Reads ─────────────────────────────────────────────────────────────────────────────────── │
│  GET  /api/v1/deliveries/{malo_id}                         ← BO4E Energiemenge                │
│  GET  /api/v1/lastgang/{malo_id}                           ← BO4E Lastgang (Arrow IPC)        │
│  GET  /api/v1/lastgang/{malo_id}/resampled                 ← hourly/daily/monthly             │
│  GET  /api/v1/zeitreihe/{malo_id}                          ← BO4E Zeitreihe (Arrow IPC)       │
│  GET  /api/v1/energy/{malo_id}?direction=                  ← the projected series             │
│  GET  /api/v1/summenzeitreihe/{malo_id}                    ← MaBiS monthly aggregate          │
│  GET  /api/v1/billing-period/{malo_id}                     ← MeterBillingPeriod               │
│  GET  /api/v1/billing-periods                              ← collection (netzbilanzd)         │
│  GET  /api/v1/imbalance/{malo_id}/{y}/{m}?bilanziert_kwh=  ← MMM saldo                        │
│  GET  /api/v1/forecast/{malo_id}                           ← § 40a Abs. 2 EnWG Schätzung      │
│  GET  /api/v1/gas-quality/{malo_id}                        ← Brennwert + Zustandszahl         │
│  GET  /api/v1/quality-assessments/{malo_id}                ← quality grade history            │
│  GET  /api/v1/netzverlust                                  ← §22 EnWG grid-loss balance       │
│  GET  /api/v1/confirmations                                ← § 60 Abs. 1 Bestätigungen        │
│                                                                                               │
│  ── Ingest ────────────────────────────────────────────────────────────────────────────────── │
│  POST /api/v1/meter-reads/rlm/{malo_id}                    ← Strom 15-min direct push         │
│  POST /api/v1/meter-reads/gas/{malo_id}                    ← Gas push (m³ → kWh_Hs)           │
│  POST /api/v1/meter-reads/iot/{malo_id}                    ← LoRaWAN / M-Bus / heat           │
│  POST /api/v1/meter-reads/{malo_id}/bulk                   ← batched import                   │
│  POST /api/v1/meter-reads/{malo_id}/substitute             ← § 60 Abs. 1 Ersatzwerte          │
│  POST /api/v1/corrections/{malo_id}                        ← file a bitemporal correction     │
│  POST /api/v1/quality-score/{malo_id}                      ← retroactive rescore              │
│  POST|GET /api/v1/zaehlerstandsgang/{malo_id}              ← ZSG in, registers out            │
│                                                                                               │
│  ── ESA "Werte nach Typ 2" — separate store, never billing ────────────────────────────────── │
│  GET  /api/v1/esa/typ2/{malo_id}                           ← read what was delivered          │
│  POST /api/v1/esa/typ2/{malo_id}                           ← Kap. 4.6.2 SM-PKI door           │
│                                                                                               │
│  ── Reading orders (Ablesesteuerung) ──────────────────────────────────────────────────────── │
│  POST|GET /api/v1/reading-orders                           ← schedule / list                  │
│  GET  /api/v1/reading-orders/{id}                          ← order detail                     │
│  PUT  /api/v1/reading-orders/{id}/complete                 ← record the reading               │
│  PUT  /api/v1/reading-orders/{id}/cancel                   ← withdraw the order               │
│  PUT  /api/v1/reading-orders/{id}/fail                     ← Ablesehindernis — still owed     │
│  POST /api/v1/reading-orders/campaign                      ← Jahresablese-Kampagne            │
│  GET  /api/v1/compliance/jahresablesung/{year}             ← § 40b Abs. 1 EnWG report         │
│                                                                                               │
│  ── Virtual meters (UTILTS formula, §42b GGV) and §42c Energy Sharing ─────────────────────── │
│  POST|GET /api/v1/virtual                                  ← create / list configs            │
│  GET|DELETE /api/v1/virtual/{virtual_malo_id}              ← one config                       │
│  GET  /api/v1/virtual/{virtual_malo_id}/lastgang           ← computed series                  │
│  GET  /api/v1/sharing/{community_id}/allocation            ← §42c allocation                  │
│  GET  /api/v1/sharing/readiness                            ← §42c delivery readiness          │
│                                                                                               │
│  ── SMGW session registry (§ 25 MsbG / BSI TR-03109) ──────────────────────────────────────── │
│  PUT  /api/v1/smgw/{malo_id}                               ← upsert SmgwSession               │
│  GET  /api/v1/smgw/{malo_id}                               ← session + recent issues          │
│  GET  /api/v1/smgw                                         ← fleet list with issue counts     │
│  GET  /api/v1/smgw/compliance                              ← read-only compliance scan        │
│  POST /api/v1/smgw/compliance/scan                         ← side-effecting fleet sweep       │
│                                                                                               │
│  ── Delivery surveillance (§ 60 Abs. 1 MsbG) ──────────────────────────────────────────────── │
│  GET  /api/v1/surveillance/delivery                        ← points that stopped              │
│  POST /api/v1/surveillance/delivery/scan                   ← sweep now                        │
│                                                                                               │
│  ── Analytical / OLAP over meterstore's resolved relation ─────────────────────────────────── │
│  GET  /api/v1/archive/status                               ← tiering / store stats            │
│  GET  /api/v1/archive/olap/{malo_id}                       ← MMM aggregation                  │
│  GET  /api/v1/archive/portfolio                            ← portfolio-level OLAP             │
│  GET  /api/v1/archive/timeseries/{malo_id}                 ← historical export                │
│  POST /api/v1/query/sql                                    ← DataFusion SQL                   │
│  GET  /api/v1/iceberg/v1/…                                 ← Iceberg REST catalog, read-only  │
│                                                                                               │
│  ── GDPR ──────────────────────────────────────────────────────────────────────────────────── │
│  DELETE /api/v1/gdpr/erasure/{malo_id}                     ← Art. 17 DSGVO erasure            │
│  GET  /api/v1/gdpr/erasures                                ← the erasure audit trail          │
│                                                                                               │
│  POST|GET /mcp                                             ← MCP Streamable HTTP              │
│  GET  /metrics                                             ← the runner's request metrics     │
│  GET  /edmd/metrics                                        ← edmd's own gauges                │
│  GET  /health/live  /health/ready                          ← liveness + real DB ping          │
└───────────────────────────────────────────────────────────────────────────────────────────────┘
```

---

## Configuration

`edmd` runs on the shared `mako-service` runner:

| Invocation | Effect |
|---|---|
| `edmd` | Start; config from `$EDMD_CONFIG`, else `./edmd.toml` (absent: environment only) |
| `edmd --check` | Probe the running instance's `/health/ready` on loopback, exit 0/1 — the Docker `HEALTHCHECK` |
| `edmd --print-config` | Print the merged file + environment config (secrets redacted) and report every missing, unknown or invalid key |
| `edmd --help` | Usage |

`EDMD_<KEY>` overrides a key, nested keys joined with `__`
(`EDMD_DATABASE__URL`); `EDMD_<KEY>_FILE` reads a value from a file; a value
`"env:VAR"` in the file reads it from `VAR`. Logging: `EDMD_LOG_LEVEL`,
`EDMD_LOG_FORMAT`, `RUST_LOG`. `just dev edmd` runs it against
`dev/config/edmd.toml`.

```toml
[http]
addr = "0.0.0.0:8380"                 # default

[database]
url                  = "env:DATABASE_URL"   # required; name an sslmode (below)
pool_size            = 20             # default 10
min_connections      = 0
acquire_timeout_secs = 30
idle_timeout_secs    = 600
max_lifetime_secs    = 1800

[identity]
tenant = "9900357000004"              # required — data-isolation key
own_mp_id = "9900000000003"           # this deployment's MSB; operator of readings edmd produces
                                      # itself. Unset, such readings are refused (422)

[marktd]
url     = "http://marktd:8180"        # required
api_key = "env:EDMD_MARKTD_API_KEY"   # required

[webhook]
inbound_secret     = "env:EDMD_INBOUND_SECRET"     # verifies marktd's deliveries
erp_webhook_url    = "https://erp.example/events"  # outbound CloudEvent target
erp_webhook_secret = "env:EDMD_ERP_WEBHOOK_SECRET" # signs them

[subscription]
webhook_url   = "http://edmd:8380/webhook"  # set → self-registers with marktd at startup
subscriber_id = "edmd"                       # default

[oidc]
issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
audience = "api://mako-edmd"
# jwks_refresh_secs = 300

[[oidc.service_keys]]                 # machine-to-machine callers
name   = "einsd"
key    = "env:EDMD_EINSD_SERVICE_KEY"
roles  = ["nb"]
# sparte = ["strom"]

# allow_insecure_no_auth = true       # top-level; dev only

[mcp]
# api_key       = "env:EDMD_MCP_API_KEY"
# api_key_roles = ["MSB"]
```

| Section | Keys (defaults) | Notes |
|---|---|---|
| `[archive]` | `enabled` (false), `storage_uri`, `region`, `endpoint_url`, `access_key_id`, `secret_access_key`, `settlement_lag_days` (7), `archival_step_days` (1), `cold_file_target_mib` (512), `maintenance_interval_secs` (3600), `ddl_lock_timeout_secs` (3) | [Cold tier](#cold-tier-and-olap) |
| `[rate_limit]` | `requests_per_second` (500), `burst` (1000), `per_caller_requests_per_second` (100) | [Rate limiting](#rate-limiting) |
| `[kafka_ingest]` | `enabled`, `bootstrap_servers`, `topic` (`edmd.meter-reads`), `group_id` (`edmd-ingest`), `poll_ms` (500), `message_hmac_secret` | [Kafka](#kafka-batch-ingest-head-end-systems) |
| `[confirmation]` | `enabled` (true), `deadline_weeks` (8) | [§ 60 confirmation loop](#confirmation-loop) |
| `[smgw]` | `enabled` (true), `cert_warning_days` (30), `comm_fault_threshold_hours` (2), `sweep_interval_secs` (86400) | [SMGW compliance](#smgw-compliance) |
| `[surveillance]` | `enabled` (true), `silent_after_hours` (36), `min_coverage_pct` (95.0), `coverage_window_days` (7), `sweep_interval_secs` (3600), `max_events_per_sweep` (500), `typ2_enabled` (true), `typ2_silent_after_hours` (36) | [Delivery surveillance](#delivery-surveillance) |
| `[privacy]` | `erasure_secret`, `retired_erasure_secrets` | [Erasure suppression](#erasure-suppression-privacy) |
| `[policy]` | `physische_loeschung` (`false`) | [Physical deletion](#policy-physische-loeschung) |
| `[mcp]` | `api_key`, `named_keys`, `api_key_roles` | MCP key fallback beside OIDC |

**TLS to PostgreSQL.** meterstore reads `[database] url` for its Iceberg
catalog and connects over TLS: a URL that names no `sslmode` (in the URL or in
`PGSSLMODE`) connects with **`verify-full`**, the production default — the
server's certificate must chain to a trusted root and name the host. `require`,
`verify-ca` and `disable` are honoured; `prefer` and `allow` are refused at
startup. Local development and the test containers speak plaintext and say so
(`postgres://dev:dev@localhost:5432/edmd?sslmode=disable`).

**Connection budget.** Size `pool_size` and PostgreSQL's `max_connections` for
the main pool (which also backs meterstore's hot tier) plus one shared Iceberg
catalog metadata pool of up to 4 connections — `meter_reads`, `esa_typ2_reads`
and `meter_readings` are three tables of one `MeterCatalog`. Connections are
tagged `edmd` in `pg_stat_activity`.

**marktd subscription.** With `subscription.webhook_url` set, edmd registers for
`de.mako.process.initiated` and `de.mako.process.completed` at startup. Verify:

```bash
curl -s http://marktd:8180/api/v1/subscriptions/edmd -H "Authorization: Bearer <token>" | jq .
```

---

## Authentication and authorization

**`[oidc]` is required.** Without it every request would be admitted as
`dev-admin` with all roles — GDPR erasure and `POST /api/v1/query/sql`
included — so edmd refuses to start unless `allow_insecure_no_auth = true` is
set, and logs a warning when it is.

**Service keys.** Sibling services (`einsd`, `billingd`, `vertragd`, `portald`)
send a static opaque `Authorization: Bearer <key>`, matched in constant time
against `[[oidc.service_keys]]` and admitted as a service principal of the
deployment tenant with the declared `roles` (default NB/LF/MSB) and `sparte`
(default STROM/GAS). A token that looks like a JWT takes the OIDC path. The
caller stores the same value as its `edmd_api_key`.

### Cedar ABAC

Every action requires the caller's tenant to equal the deployment tenant; writes
additionally require a market role (`mako_roles` claim). The policy is
`policies/edmd.cedar`, pinned by the `cedar_policy` test suite.

| Action group | Actions | Required role |
|---|---|---|
| Reads | `read-timeseries`, `read-imbalance`, `read-billing-period`, `read-archive-olap`, `read-archive-status`, `read-reading-order`, `use-mcp` | any (tenant match) |
| Reading ingest | `write-meter-reads` (direct push, gas, IoT, ZSG, SMGW registry, virtual-meter configs) | `MSB`, `ADMIN` |
| Series mutation | `write-timeseries` (bulk import, corrections, § 60 Abs. 1 Ersatzwerte), `write-quality-rescore` (rescore) | `MSB`, `NB`, `ADMIN` |
| Field dispatch | `write-reading-order` (orders, §40b campaign) | `NB`, `MSB`, `ADMIN` |
| ESA Typ-2 filing | `write-esa-typ2` (`POST /api/v1/esa/typ2/{malo_id}`) | `ESA`, `MSB`, `ADMIN` |
| Erasure | `write-gdpr-erasure`, `read-gdpr-erasures` | `NB`, `MSB`, `ADMIN` |

- `POST /api/v1/query/sql` and the Iceberg catalog under `/api/v1/iceberg` are
  gated by `read-archive-olap`.
- `write-esa-typ2` is separate both ways: the ESA's SM-PKI gateway carries the
  `ESA` role, and it must not reach the authoritative store.
- `read-corrections` and `write-corrections` are declared but no route checks
  them: corrections are filed under `write-timeseries`, and their history is read
  through MCP `get_correction_history`.
- **MCP write tools keep the REST gate.** `/mcp` is admitted by `use-mcp`, but
  `trigger_substitution` and `trigger_jahresablesung` additionally require
  `write-timeseries` / `write-reading-order`, also for a configured `[mcp]` key,
  which authorizes under its declared roles.
- A `?tenant=` that differs from the authorised tenant is `403`.

### Signed CloudEvents

Every CloudEvent edmd emits goes through the transactional outbox
(`event_outbox`) and, with `webhook.erp_webhook_secret` set, carries
[Standard Webhooks](https://www.standardwebhooks.com/) headers (`webhook-id`,
`webhook-timestamp`, `webhook-signature: v1,<base64>`, HMAC-SHA256 over
`{id}.{timestamp}.{body}`). Without the secret the transport is the trust
boundary.

---

## Ingest

### Storage model

| Rule | Detail |
|---|---|
| **Tenant is part of a reading's identity** | `meter_reads` is keyed `(tenant, malo_id, dtm_from, obis_code_norm)`; `tenant` is meterstore's identity column, so two tenants' readings for one MaLo never merge, and every read binds it |
| **One batched, validated write** | Every door goes through `store_reads`, one meterstore `append` that splits the batch across tiers and lands whole or not at all. It accepts only a `ValidatedReads`, whose sole constructor runs the validation — no path reaches the store unvalidated |
| **The operator is a checked Marktpartner-ID** | The version scope is keyed on the reading's `sender_mp_id`, else the tenant, parsed as a `metering::ids::BdewCode`: its thirteenth digit must satisfy the BDEW or the GS1 procedure. Any other is refused (`422`) before it is stored |
| **Provenance rides along** | `source` (`IngestionSource`), `sender_mp_id` (§ 60 Abs. 1 per-interval MSB attribution) and `allocation_version` are meterstore attribute columns folded from the newest delivery; `source` is CHECK-constrained |
| **Billing unit** | Gas is converted with the Brennwert before storage, so the stored `unit` is the Sparte's billing unit (`kWh`); water stays `m³`; heat is `kWh_th`. The unit reaches BO4E `Mengeneinheit` and the Iceberg Parquet |
| **Constraints** | The 8-value `QualityFlag` vocabulary is a `CHECK` on every quality column; every interval/period table enforces `dtm_to > dtm_from`, and a read whose end is not after its start is refused at the store boundary |
| **Version resolution** | A stated MSCONS version (≥ 14 digits) decides which delivery wins; without one, arrival milliseconds (13 digits) — so a stated version always outranks it. Values edmd authors (corrections, Ersatzwerte) are re-appended above the version in force (up to four attempts) |
| **Displacement audit** | An overwritten interval leaves an immutable § 60 Abs. 6 MsbG audit row spanning `[dtm_from, dtm_to)` |
| **Billing cache** | `store_reads` drops any cached `meter_billing_periods` row the new readings fall inside |

Validation **annotates and never rejects**: findings land in `quality_warnings`
on the rows they name, and a batch with any finding is answered `202 Accepted`
with a `validation` block (`201` when clean). Every response carries
`skipped_rules`, so a clean verdict says which rules stood behind it.
`stored_count` reports committed rows. An unrecognised `quality` flag is `422`.

### What a failed ingest answers

`marktd` treats `2xx` as delivered and redelivers on `5xx`, so the status says
whether a retry can work (`EdmError::is_retryable`):

| Condition | Status |
|---|---|
| Transient store failure (lost connection, lock not available) | **500** — redelivery is the point |
| The store refused the write (overlapping spans in one version, a value restated under an existing version, a second network operator on one reading, a non-canonical OBIS code) | **422** — names the constraint; redelivery would loop |
| Nothing stored, nothing wrong | 204 |

An MSCONS interval whose quantity will not parse is dropped and counted, never
defaulted to zero.

### Inbound event routing (`POST /webhook`)

| `ce_type` | `makopid` | Action |
|-----------|-----------|--------|
| `de.mako.process.completed` | MSCONS set | Store readings |
| `de.mako.process.completed` | 55001 | `LIEFERBEGINN` reading order (GPKE Beginn-/Schlussablesung) |
| `de.mako.process.completed` | 55004 / 55007 | `LIEFERENDE` reading order |
| `de.mako.process.initiated` | 23001 (INSRPT Störungsmeldung) | `INSRPT_STOERUNG` reading order |
| `de.mako.process.initiated` | 23003 / 23004 / 23008 | `SONDERABLESUNG` reading order |
| `de.mako.process.initiated` | 23005 / 23009 (WiM Gas INSRPT) | `SONDERABLESUNG` reading order |
| anything else | — | 204 (ignored) |

### MSCONS PIDs handled

`ALL_MSCONS_PIDS` is what the webhook accepts and the only set it branches on;
the marktd subscription has no PID filter. Descriptions are the AHB's column
headings (`domain::mscons_pid_description`).

| PID | Description | Handling |
|-----|-------------|----------|
| 13005 | EEG-Überführungszeitreihe | stored |
| 13006 | Messwert Storno | receipt recorded, payload not stored |
| 13007 | Gasbeschaffenheit — Brennwert + Zustandszahl | `QTY+Z08` / `QTY+Z10` populate `meter_billing_periods` |
| 13013 | Marktlokationsscharfe Allokationsliste Gas (MMMA) | stored |
| 13015 | Arbeit + Leistungsmaximum im Kalenderjahr vor Lieferbeginn | stored |
| 13016 | Energiemenge und Leistungsmaximum | stored |
| 13017 | Zählerstand (Strom) | stored |
| 13018 | Lastgang Messlokation, Netzkoppelpunkt, Netzlokation | stored |
| 13019 | Energiemenge (Strom) | stored |
| 13025 | Lastgang Marktlokation, Tranche | stored |
| 13027 | Werte nach Typ 2 (ESA) | [`esa_typ2_reads`](#esa-typ2), never `meter_reads` |
| 13020 · 13023 | Ausfallarbeitsüberführungszeitreihe · Ausfallarbeitssummenzeitreihe | stored; routed by `mako-mabis` `mabis-billing` |
| 13021 · 13022 | Meteorologische Daten · Einzelzeitreihe Ausfallarbeit | stored; routed by `mako-redispatch` `redispatch-aktivierung` |
| 13026 | EEG-Überführungszeitreihe aufgrund Ausfallarbeit | stored; no routing family |
| 13002, 13003, 13008–13012, 13014, 13028 | — | accepted so a delivery is never discarded |

On-demand gas quality data: makod `geli.datenabruf.anfragen` (ORDERS 17103; the
GNB answers within ten Werktage, not before the 10. Werktag of the month after
the Zeitspanne — GeLi Gas 3.0 Kap. 4.2).

### iMSys direct push (§41a)

```http
POST /api/v1/meter-reads/rlm/{malo_id}
Content-Type: application/json

{
  "session_id": "SMGW-SN-00112233-20260713T0600Z",
  "source": "DIRECT_PUSH",
  "obis_code": "1-0:1.8.0",
  "sender_mp_id": "9900000000003",
  "max_plant_power_kw": "30.0",
  "intervals": [
    { "from": "2026-07-13T00:00:00Z", "to": "2026-07-13T00:15:00Z", "value": "2.345", "unit": "kWh" },
    { "from": "2026-07-13T00:15:00Z", "to": "2026-07-13T00:30:00Z", "value": "2.412", "unit": "kWh" }
  ]
}
```

`/api/v1/meter-reads/gas/{malo_id}` takes m³ plus `brennwert_kwh_per_m3`
(required) and optional `zustandszahl`, and stores kWh_Hs.

| Field | Note |
|---|---|
| `source` | Optional; one of `MSCONS`, `DIRECT_PUSH`, `DIRECT_GAS`, `API_IMPORT`, `AUTO_SUBSTITUTE`, `CORRECTION`, `MANUAL`, `ESTIMATED`, `IOT_PUSH`. Anything else is `422`. Default `DIRECT_PUSH` / `DIRECT_GAS`, `API_IMPORT` on bulk |
| `sender_mp_id` | The reporting MSB/NB's Codenummer; keys the version scope, so omitting it means the push cannot supersede the MSCONS delivery it corrects. Its check digit must hold under the BDEW or the GS1 procedure — the store refuses any other (`422`), and without it the tenant keys the scope and must pass the same check |
| `max_plant_power_kw` | Enables `CAPACITY`; absent, the rule is off |
| `mscons_version` | Correction version (≥ 14 digits); absent, arrival order decides |
| `unit` | Parsed with `metering::Unit::parse_scaled` and checked against the Sparte's measured or billing unit |

`201` clean · `202` stored with warnings · `200` `session_id` already committed.

### IoT meter ingest (LoRaWAN, M-Bus, REST heat meters)

For heat and water submetering without a Smart-Meter-Gateway. HeizkostenV §5
Abs. 3 requires non-remote-readable devices to be retrofitted by
**31 December 2026**, §6a requires monthly consumption information, and §12
Abs. 1 attaches a **3 % Kürzungsrecht** to both.

```http
POST /api/v1/meter-reads/iot/62345678906
Content-Type: application/json

{
  "sparte": "WAERME",
  "unit": "KWH",
  "session_id": "70B3D57ED0012345:4711",
  "transport": "LORAWAN",
  "device_id": "70B3D57ED0012345",
  "obis_code": "6-0:1.0.0",
  "eichung_bis": "2027-12-31",
  "raw_payload": "AwAAECcAAA==",
  "intervals": [
    { "from": "2026-07-13T00:00:00Z", "to": "2026-07-13T01:00:00Z", "value": "4.120" }
  ]
}
```

| Rule | Detail |
|---|---|
| `session_id` | **Required** — `devEUI:fCnt` (LoRaWAN) or the telegram access number (OMS/M-Bus) |
| `device_id`, `transport` | Recorded on the session row; `device_id` is **not** the `sender_mp_id` (that is a BDEW Codenummer keying the version scope) |
| Payload | Must be decoded upstream — edmd does not decode wM-Bus/OMS frames (vendor payloads and keys are proprietary). `raw_payload` is kept verbatim for re-derivation |
| Units | `STROM` kWh · `GAS` m³ or kWh · `WAERME` kWh · `WASSER` m³. Gas in m³ requires `brennwert_kwh_per_m3`; `zustandszahl` defaults to 1.0. `kWh`/`Wh`/`MWh`/`GWh`/`GJ`/`MJ` and `m³`/`m3`/`cbm`/`l` are accepted and rescaled exactly (3.6 GJ = 1000 kWh). The response reports `unit_submitted`, `unit_stored`, `converted` |
| Negative values | Rejected — direction belongs in the OBIS code |
| Outliers (`SPIKE`, `CAPACITY`) | Stored as `PRELIMINARY` (MSCONS Z84), not discarded |
| Eichfrist | An expired `eichung_bis` is a **warning**, never a rejection — see below |

| Code | Meaning |
|---|---|
| 201 | All intervals stored, no warnings |
| 202 | Stored, with calibration warnings and/or per-interval rejections |
| 200 | `session_id` already committed — `already_committed` |
| 422 | Unknown `sparte`/`unit`, unit/Sparte mismatch, or nothing storable |

**Eichfrist.** §37 Abs. 1 Satz 1 Nr. 1 MessEG and §33 Abs. 1 MessEG bar a value
from a device past its Eichfrist, but BGH VIII ZR 112/10 holds that in civil
billing it only loses its Vermutung der Richtigkeit, so edmd warns. §34 Abs. 2
MessEV ends an Eichfrist at year end — send `YYYY-12-31`. Leave `eichung_bis`
unset for Heizkostenverteiler, which are not Messgeräte under MessEG (HeizkostenV
§5 Abs. 1).

### Push-session idempotency

RLM, Gas, IoT and Zählerstandsgang share `direct_push_sessions`, keyed
`(tenant, session_id)`. The key is claimed before the work
(`INSERT … ON CONFLICT DO NOTHING`, `status = 'partial'`) and flipped to
`committed` after it.

| Outcome | Response |
|---|---|
| Claim taken | ingest, then commit |
| Row `committed` | `200`, recorded result replayed |
| Row `partial` | this request takes it over — an uncommitted batch stays retryable |
| Row for a **different** `malo_id` | `409`, naming both |
| Claim unreadable/unwritable | `500` with an `error_id` — the emitter retries it like any 5xx |
| Commit not written | `500` with an `error_id` — retry under the same key |

### Kafka batch ingest (head-end systems)

```toml
[kafka_ingest]
enabled           = true
bootstrap_servers = "kafka-1:9092,kafka-2:9092"
topic             = "edmd.meter-reads"
group_id          = "edmd-ingest"
# message_hmac_secret = "env:EDMD_KAFKA_HMAC"
```

One JSON document per record, the bulk-endpoint shape:

```json
{
  "malo_id": "51238696012",
  "sparte": "STROM",
  "source": "IOT_PUSH",
  "intervals": [
    {"from": "2026-07-01T00:00:00Z", "to": "2026-07-01T00:15:00Z",
     "value_kwh": "1.25", "quality": "MEASURED", "obis_code": "1-0:1.8.0"}
  ]
}
```

- Same pipeline as every door: validation, quality grade, audited upsert.
- An unrecognised `sparte` or `source` is refused, and the record skipped and
  logged like any poison pill.
- With `message_hmac_secret`, every record must carry the Standard Webhooks
  headers (`webhook-id`, `webhook-timestamp`, `webhook-signature`) over
  `{id}.{timestamp}.{record value}`; failures are skipped. No freshness check —
  the offset is the replay boundary. Without it, the topic ACL is the trust
  boundary.
- At-least-once: offsets commit after the store; a storage failure redelivers
  the batch. A new consumer group starts at the **earliest** offset.

### Zählerstandsgang

§ 2 Satz 1 Nr. 27 MsbG defines the Zählerstandsgang as „die Messung einer Reihe
viertelstündig ermittelter Zählerstände von elektrischer Arbeit und stündlich
ermittelter Zählerstände von Gasmengen", and BK6-24-174 (Datenübermittlung ZSG,
wirksam 06.06.2025) places the differencing at the MSB — which is edmd.

```http
POST /api/v1/zaehlerstandsgang/{malo_id}
Content-Type: application/json

{
  "session_id": "SMGW-SN-00112233-20260713T0600Z",
  "sparte": "STROM",
  "obis_code": "1-0:1.8.0",
  "sender_mp_id": "9900000000003",
  "register_digits": 6,
  "max_plant_power_kw": "30.0",
  "readings": [
    { "at": "2026-07-13T00:00:00Z", "value": "14230.500" },
    { "at": "2026-07-13T00:15:00Z", "value": "14231.250" },
    { "at": "2026-07-13T00:30:00Z", "value": "14232.100" }
  ]
}
```

| Rule | Detail |
|---|---|
| Both halves stored and tiered | Readings in the `meter_readings` point table, derived intervals in the interval table (§ 146 Abs. 4 AO: the original stays recoverable). `GET` on the same path reads the registers back |
| Keyed by Messlokation | Two meters under one MaLo report the same OBIS code; a delivery naming no Messlokation is refused |
| Readings unconverted | A gas register stays m³; the difference becomes kWh_Hs (§ 25 Nr. 4 MessEV) |
| Intervals relabelled | `1-0:1.8.0` (Zählerstand, `D = 8`) becomes `1-0:1.29.0` (Lastgang, `D = 29`) |
| Nothing invented | Where no honest difference exists, no interval is emitted; the reason goes to `zsg_conversion_log`, and the hole surfaces as a `GAP` |
| `register_digits` | Enables wrap reconstruction (`(10^digits − previous) + current`); omitted, a backwards step is refused |
| `max_plant_power_kw` | Ceiling per observed reading interval, in the register's unit, that tells a wrap from a meter exchange |
| § 40 Abs. 2 Nr. 6 EnWG | `meter_billing_periods.zaehlerstand_anfang` / `_ende` are the last reading at or before each bound |

`PUT /api/v1/reading-orders/{id}/complete` files its Zählerstand into the same
store — for an SLP point that is the whole billing path. An order names its
`sparte` (a value in the wrong dimension is refused), optionally its
`obis_code`, and the completion carries `abgelesen_am`.

### Substitute values (§ 60 Abs. 1 MsbG)

`POST /api/v1/meter-reads/{malo_id}/substitute` validates the register over the
gap window and fills every slot an **error** finding covers — a `GAP`, and a
value rejected as `NON_BILLABLE` (whose original is kept in the audit row) —
with `metering::vee::substitute`. A slot that already holds a **billable** value
(every quality except `FAULTY`/`UNKNOWN`) is never overwritten and comes back in
`skipped_measured`.

The method follows the gap, not the request (MeteringCode 2006 A8.2.2):

| Gap | Method | `STS+Z32` |
|---|---|---|
| up to two hours between validated measured values | `INTERPOLATION` | `Z92` |
| longer, or at the edge of the data | `VERGLEICHSWERT` — the mean of the same slot on the previous three days of the same day class (Werktag, Samstag, Sonn-/Feiertag; the Bundesland's holidays count as Sundays) | `ZJ2` Strom · `Z95` Gas |
| gas, with `hold` | `HOLD` — the last measured value | `Z93` |
| `confirmed_outage` | `ZERO` | — |

A slot no rule can fill is **refused**, never invented, and listed in
`refused`; a window where nothing could be formed is `422`.

| Parameter | Rule |
|---|---|
| `gap_from`, `gap_to` | RFC 3339; `gap_to` not in the future — an Ersatzwert stands in for a value that should already have arrived |
| `obis_code` | The register filled; default the point's dominant energy register. Name it on a multi-register point — an unlabelled reading is the total register |
| `reason` | **Required**: one of the 28 `STS+Z40` Gründe der Ersatzwertbildung of MSCONS MIG 2.5, by name (`NO_ACCESS`, `METERING_EQUIPMENT_FAULT`, `COMMUNICATION_FAILURE`, …) or code (`Z74`, `Z81`, `Z75`, …); otherwise `422` listing them |
| `bundesland` | **Required** — `BE`, `NW`, `DE-BY`, …; its statutory holidays decide the day class. edmd holds no address for a Marktlokation |
| `sparte` | Taken from the register's stored reads; the request's `STROM` · `GAS` · `WAERME` · `WASSER` is read only when there are none, and with neither the request is `422` |
| `prior_days` | The reference window, default 7 — it has to hold three days of the gap's class |
| `hold` | Gas only (`Z93` is coded for gas alone); refused for any other Sparte |
| `confirmed_outage` | The whole window is an established supply interruption |

The response reports per interval the `method`, its `code`, the `reason`, the
`reference_count` and any `original`, plus `methods_applied`, the `resolution`
and the `obis_code` used. Generated values run the same validation pass with
the batch's Sparte preset, and the substitute and its § 147 Abs. 1 AO / § 146
Abs. 4 AO audit row commit in one transaction; `substitute_value_log.method`
holds the method that ran and `substitute_value_log.bundesland` the Land whose
holidays decided the day class.

The authority is **§ 60 Abs. 1 MsbG** (Aufbereitung includes Plausibilisierung
and Ersatzwertbildung). § 60 Abs. 2 is a *Soll*-rule about *where* that runs —
im Smart-Meter-Gateway once BSI and a § 75 Satz 1 Nr. 4 Festlegung allow it — and
its Satz 2 lets the MSB do it outside the gateway meanwhile, for gas permanently.

### Corrections

`POST /api/v1/corrections/{malo_id}` appends the corrected value at a higher
version under the original reading's `sender_mp_id`, keyed on
`(tenant, malo_id, dtm_from, obis_code_norm)` — so only the named register
changes — and advances `allocation_version` to `CORRECTION`. The audit row and
the value commit together. History: MCP `get_correction_history` or
`meter_read_corrections`.

### § 60 Abs. 1 MsbG confirmation loop {#confirmation-loop}

Every stored `ESTIMATED`/`SUBSTITUTED` interval opens an entry in
`estimated_read_confirmations`; a `MEASURED` or `CORRECTED` value for the same
slot (MaLo, `dtm_from`, register) resolves it. The daily `[confirmation]` worker
moves entries open longer than `deadline_weeks` (8, modelled on the MaBiS
BKA-Korrekturfenster; no statutory Frist exists) to `UEBERFAELLIG` and emits
`de.messwert.reading.confirmation.overdue`.

`GET /api/v1/confirmations?status=UEBERFAELLIG` lists them.

### Rate limiting

| Limiter | Key | Bounds |
|---|---|---|
| per caller | peer address (runs before auth) | `per_caller_requests_per_second` (100) |
| global | — | `requests_per_second` (500), `burst` (1000) |

Rejections are `429` with `Retry-After` rounded up to whole seconds.

---

## Validation and quality

### Validation rules

`domain::validation::findings` is the only validation pass — every ingest door,
the substitute path and MCP `validate_timeseries` use it. It splits the batch by
(Sparte, OBIS register), builds each register into a `metering::Series` on its
observed grid and day boundary (the 06:00 Gastag for gas), and runs
`metering::vee::validation` over the register's own span with the preset for its
Sparte.

**Structural findings** come first: a register that is not one series cannot be
judged by any content rule, and every content rule is reported as skipped for it.

| Code | Finding | Severity |
|---|---|---|
| `DUPLICATE` | Two values for one slot of one register | Blocking |
| `OVERLAP` | A span overlapping the one before it | Blocking |
| `MISALIGNED` | A span that is not a slot of the observed grid on the day boundary | Blocking |
| `EMPTY_SPAN` | A read whose end is not after its start | Blocking |
| `NO_GRID` | No observable grid (a median length that is no resolution) | Blocking |
| `OUT_OF_RANGE` | Outside the calendar's years (1900–9998) | Blocking |

**Content rules** are `metering`'s:

| Code | Finding | Severity |
|---|---|---|
| `GAP` | Slots of the register's span with no value | Error |
| `NON_BILLABLE` | A value flagged `FAULTY` or `UNKNOWN` | Error |
| `NEGATIVE` | A negative quantity | Error |
| `CAPACITY` | Average power above the plant's capacity — only with a caller-supplied `max_plant_power_kw` | Error |
| `FUTURE` | A slot that has not ended yet | Blocking |
| `ZERO_RUN` | A run of zeros at least as long as the preset | Warning |
| `STALE` | A non-zero value frozen for six slots (Strom only) | Warning |
| `SPIKE` | A value far from its neighbours (Hampel, twelve either side, 6 robust σ) | Warning |

| Sparte | `ZERO_RUN` from | `STALE` |
|---|---|---|
| Strom | 1 h | 6 values, exact |
| Gas | 48 h | off |
| Wärme, Wasser | 720 h | off |

- **One series per register**, so import and export on one MeLo are two series
  and not one slot delivered twice.
- **A pushed batch declares its own span**: its leading and trailing slots are
  not gaps.
- **Findings name the rows they cover**, tagged with the register; a `GAP` names
  no row. Batch counts ride under a `batch` key. Delivery order is no finding.
- **`CAPACITY`** needs the plant's capacity, which edmd does not hold. Every ingest
  body (RLM, gas, IoT, bulk, ZSG, Kafka, MSCONS `process.completed`, MCP
  `validate_timeseries`) takes an optional `max_plant_power_kw`; absent, the rule
  is off and reported in `skipped_rules`.

**DST.** A quarter-hour day has 92 slots on the spring and 100 on the autumn
transition. A fall-back day that lost its repeated hour is a four-slot `GAP`
whose message names the repeated hour 02:00–03:00.

| Local day (Europe/Berlin) | Hours | ¼-h intervals | UTC span |
|---|---:|---:|---|
| 2026-03-29 (CET→CEST) | 23 | **92** | `2026-03-28T23:00Z` → `2026-03-29T22:00Z` |
| ordinary day | 24 | 96 | — |
| 2026-10-25 (CEST→CET) | 25 | **100** | `2026-10-24T22:00Z` → `2026-10-25T23:00Z` |

### Quality grade

Every door grades its batch over the batch's own span, and a rescore over the
requested window; `BatchQuality::record` writes the verdict to
`quality_assessments` after the store returns. The window need not be a calendar
period: it is validated over the smallest day, month or year that covers it (one
per year across New Year), and the findings are cut back to the window.

| Field | Source |
|---|---|
| `grade` | `A` no finding · `B` warnings only · `C` errors a substitute repairs · `F` blocking |
| `gaps_detected` | Missing slots inside the window |
| `outlier_intervals` | `SPIKE` and `SEASONAL` findings |
| `spike_intervals` | `CAPACITY` findings |
| `zero_run_length` | The longest `ZERO_RUN`, in slots |
| `coverage_pct` | Delivered time inside the window ÷ the window's length, per register, the worst register reported |
| `expected_intervals` | Slots of the observed grid inside the window |
| `intervals_consistent` | `false` for a structural finding — and the grade is then `F` |

Only energy registers are graded; a measuring point is no better than its worst
energy register.

| Grade | Billing |
|-------|---------|
| **A** | Normal |
| **B** | Bill and note it |
| **C** | Substitute the affected slots first |
| **F** | `billing_blocked` — the window needs clarifying |

`de.messwert.reading.quality.warning` fires on a grade C/F **or** any finding,
from every door; `ingest_door` names it (`mscons`, `rlm-direct-push`,
`iot-push`, `bulk-import`, `kafka-ingest`). The same predicate decides `202` vs
`201`. In `agentd` the event starts `msb-history-agent`, `meter-data-agent` and
`replacement-value-agent`.

Re-scoring supersedes the previous verdict for the same source:

```http
POST /api/v1/quality-score/{malo_id}?from=2026-01-01T00:00:00Z&to=2026-07-01T00:00:00Z
```

Returns `{ malo_id, rows_rescored, warnings_found, grade }`.

---

## Reads

### The register projection

A MaLo delivers a set of OBIS registers, and a meterstore read spans them.
Summing them mixes incompatible quantities:

| Mixed | What the sum becomes |
|---|---|
| `1-0:1.8.0` + `1-0:1.8.1` + `1-0:1.8.2` | Consumption counted twice — the total already is HT + NT |
| `1-0:1.8.0` + `1-0:2.8.0` | Grid draw plus feed-in |
| `1-0:1.8.0` + `1-0:3.8.0` | kWh plus kvarh |
| any + `1-0:1.6.0` | kWh plus a kW peak register |

`domain::register` decides once, for every figure (billing period, saldo,
Summenzeitreihe, resampling, forecast, Netzverlust, OLAP, energy sharing,
virtual meters, and their MCP twins):

- **`energy_intervals(reads, direction)`** drops non-billable qualities
  (§ 40a Abs. 2 EnWG), non-kWh registers and the other direction; where a total
  register overlaps tariff registers it wins, otherwise the tariff registers are
  summed. Decided per interval, by overlap.
- **`register_groups(reads)`** splits by canonical OBIS for anything judging
  shape (validation, grading, surveillance, §42c readiness, BO4E exports).
- Value group C is a direction for electricity only; gas, water and heat are
  Bezug. Einspeisung requires an explicit `C = 2` electricity code. Group D is not
  filtered; the `D = 6` maximum register is excluded by its kW unit.

| OBIS | Meaning |
|------|---------|
| `1-0:1.8.0` | Active import — strictly a Zählerstand, and the common label for per-interval energy |
| `1-0:1.29.0` | Active import, Lastgang |
| `1-0:2.8.0` | Active export (Einspeisung) |
| `1-0:1.6.0` | Maximum demand (kW) — excluded from energy |
| `7-1:99.33.17` | Gas energy after Brennwert conversion |

### `GET /api/v1/energy/{malo_id}` — the projected series

`?direction=BEZUG|EINSPEISUNG&from=&to=` returns one direction through the
register projection: per interval `start`, `end`, `kwh`, `quality`, plus
`resolution_min`, `coverage_pct`, `billable_pct` (share of the series by
duration, before filtering, that is billable; absent when the point reports no
register in that direction) and `interval_count`. `?as_of=` reads it at a past
transaction time.

Consumers: `netzbilanzd` (Summenzeitreihe), `billingd` (§41a),
`einsd` (§51 EEG, Einspeisemenge). The BO4E exports are not inputs to a figure.

### Read windows

Every materialising read defaults to the **last 31 days** and refuses a window
wider than **732 days**. A malformed `from`/`to`/`as_of` is `400`. Bulk history
streams instead: Arrow IPC on `/lastgang` and `/zeitreihe`,
`POST /api/v1/query/sql`, or the Iceberg catalog.

### The balancing day

Electricity balances on the calendar day (00:00–00:00 Europe/Berlin); **gas on
the Gastag, 06:00–06:00** (GaBi Gas, Art. 3 Nr. 6 VO (EU) 312/2014). Aggregating
endpoints take the commodity:

| Endpoint | Parameter |
|---|---|
| `GET /api/v1/billing-period/{malo_id}` | `?sparte=strom` (default) · `gas` · `wasser` · `waerme` |
| `GET /api/v1/imbalance/{malo_id}/{year}/{month}` | `?sparte=strom` (default) · `gas` |

Bounds resolve through `metering::DayBoundary` (`Strom`, `Gas`), so DST days are
23 or 25 hours; the long or short Gastag is the one named after the Saturday. A
date outside the calendar's years (1900–9998) is refused.

### `MeterBillingPeriod`

`GET /api/v1/billing-period/{malo_id}?from=&to=` (read-through cache
`meter_billing_periods`); `GET /api/v1/billing-periods` for the collection.

| Field | Type | Content |
|-------|------|--------|
| `arbeitsmenge_kwh` | `Decimal` | Bezug over the period (HT + NT) |
| `arbeitsmenge_ht_kwh` / `_nt_kwh` | `Option<Decimal>` | Dual-tariff split |
| `spitzenleistung_kw` | `Option<Decimal>` | RLM Strom: highest 15-min average demand |
| `brennwert_kwh_per_m3` · `zustandszahl` | `Option<Decimal>` | Gas (from PID 13007) |
| `zaehlerstand_anfang` / `_ende` | `Option<Decimal>` | § 40 Abs. 2 Nr. 6 EnWG register readings |
| `coverage_pct` | `Decimal` | Billable coverage, by duration |
| `quality` | `QualityFlag` | Worst contributing flag |

`netzbilanzd` uses it for the Leistungspreisanteil and gas conversion; `billingd`
for the Zählerstände.

### Mehr-/Mindermengensaldo

`GET /api/v1/imbalance/{malo_id}/{year}/{month}` **requires**
`?bilanziert_kwh=` — the bilanzierte Menge is the supplier's figure, not a
measurement; without it the answer is `422`. `gemessen_kwh` is the projected
Bezug.

```bash
curl "http://edmd:8380/api/v1/imbalance/51238696012/2026/07?bilanziert_kwh=1000" \
  -H "Authorization: Bearer $TOKEN"
```

```json
{
  "gemessen_kwh":    "962.5",
  "bilanziert_kwh":  "1000",
  "mehrmenge_kwh":   "37.5",
  "mindermenge_kwh": "0",
  "delta_kwh":       "-37.5",
  "delta_pct":       "-3.75",
  "quality":         "MEASURED",
  "interval_count":  2976,
  "richtung":        "MEHRMENGE — Netzbetreiber vergütet dem Lieferanten"
}
```

Naming is from the NB's side (GPKE Teil 1 Kap. 8.4 Nr. 3): consuming *less* than
the profile is the **Mehr**menge, which the NB credits; consuming more is the
**Minder**menge, which it invoices. `quality` is the worst contributing flag.
REST and MCP `get_imbalance` share `metering::billing::imbalance::ImbalanceSaldo`.

### Buckets and the annual projection

`GET /api/v1/lastgang/{malo_id}/resampled?resolution=HOUR|DAY|MONTH|YEAR` and
`GET /api/v1/summenzeitreihe/{malo_id}` (monthly, also MCP
`get_summenzeitreihe`) sum the projected Bezug with
`metering::series::resample::resample` on the Berlin calendar: **a bucket missing
any slot is left out**, so a partial month is absent rather than a short total —
substitute first. Each bucket carries `total_kwh`, its worst `quality` and
`peak_kw`, the highest average demand of one source interval in it. A target
that does not nest the series' grid (a daily series asked for hours) is `422`.

`GET /api/v1/forecast/{malo_id}` (and MCP `get_annual_forecast`, the same
function) is `metering::billing::forecast::project_annual_consumption` over the
projected Bezug: at least seven days and 90 % billable coverage, the
coverage-corrected daily average scaled to the target year's length. A seasonal
factor — the prior year's rate over the shifted window divided by its rate over
its whole span, read from the year ending one year before `to` — is divided
out when the prior year supports one; `seasonal_correction_skipped` says why it
did not.

### BO4E exports

| Endpoint | Shape |
|---|---|
| `GET /api/v1/deliveries/{malo_id}` | One `Energiemenge` per stored interval |
| `GET /api/v1/lastgang/{malo_id}` | One `Lastgang` per canonical OBIS register; `zeitIntervallLaenge` is the register's observed cadence (one `VIERTELSTUNDE`, `STUNDE`, `TAG`, else N `MINUTE`) |
| `GET /api/v1/zeitreihe/{malo_id}` | One `Zeitreihe` per register, with `medium`, `messart`, `einheit` — the API-Webdienste shape |

All take `from`/`to` (RFC 3339; bounded as above) and need `read-timeseries`.
`/lastgang` and `/zeitreihe` also take `?as_of=` — a bitemporal read through
meterstore's `recorded_at` axis (`store.as_known_at`), so corrections **and
intervals** recorded later are invisible (§ 147 Abs. 1 AO / § 146 Abs. 4 AO) —
and answer with an `x-mako-coverage-pct` header (worst register). Overlapping or
unplaceable entries are logged at `error`.

```json
[
  {
    "sparte": "STROM",
    "obis_kennzahl": "1-0:1.29.0",
    "zeitIntervallLaenge": { "wert": "1", "einheit": "VIERTELSTUNDE" },
    "werte": [
      {
        "zeitraum": {
          "startdatum": "2026-01-01", "startuhrzeit": "00:00:00+00:00",
          "enddatum":   "2026-01-01", "enduhrzeit":   "00:15:00+00:00"
        },
        "wert": "1.234",
        "status": "ABGELESEN"
      }
    ]
  }
]
```

### Arrow IPC bulk export

`Accept: application/vnd.apache.arrow.stream` on `/lastgang` or `/zeitreihe`
returns a columnar stream with the same data (`esa/typ2` is JSON only).

```bash
curl -s "http://edmd:8380/api/v1/lastgang/51238696012?from=2026-01-01T00:00:00Z&to=2026-02-01T00:00:00Z" \
  -H "Authorization: Bearer <token>" \
  -H "Accept: application/vnd.apache.arrow.stream" > reads.arrows
duckdb -c "SELECT SUM(quantity_kwh), quality FROM read_ipc_stream('reads.arrows') GROUP BY quality"
```

| Column | Type |
|---|---|
| `malo_id` | `Utf8` |
| `dtm_from` / `dtm_to` | `Timestamp(µs, UTC)` |
| `quantity_kwh` | `Decimal128(18,5)` |
| `quality` | `Utf8` |
| `sparte` | `Utf8` |
| `obis_code` | `Utf8?` |
| `pid` | `Int32` |

---

## Cold tier and OLAP

`meter_reads` is a meterstore table; meterstore owns the Iceberg format,
partitioning, layout, overlap exclusion and version resolution. edmd's
`TableConfig` sets the daily partition and archival steps, the settlement lag and
the `tenant` identity column, and edmd starts meterstore's maintenance loop,
which advances the watermark. The hot tier is reclaimed by tiering, not
`DELETE`. Startup runs meterstore's `create_tables` over the catalog: it
provisions both tiers and adds every declared column a table lacks, and until it
has run a store refuses to build, archive or write.

Data written by an earlier meterstore is not always readable: substitute
payloads, resolutions spelled in seconds and Marktpartner-IDs that fail both
check-digit procedures do not decode, and an erasure registry without
`hmac_key_id` is refused by `create_tables`. A deployment holding such data is
rebuilt — in development `just infra-reset`.

```toml
[archive]
enabled              = true
storage_uri          = "s3://my-bucket/edmd/warehouse"   # file:// memory:// s3:// minio:// r2:// gs:// abfss://
region               = "eu-central-1"
# access_key_id / secret_access_key — omit to use the instance-role chain
# endpoint_url       = "http://minio:9000"
settlement_lag_days  = 7      # how long a reading stays correctable in the hot tier
archival_step_days   = 1      # watermark step and partition granularity
cold_file_target_mib = 512
maintenance_interval_secs = 3600   # without cycles the cold tier never fills
ddl_lock_timeout_secs = 3     # 0 = wait indefinitely
```

`ddl_lock_timeout_secs` bounds how long meterstore's DDL (partition detach on
archival, partition creation on append) waits for its lock; a timed-out cycle
reports `deferred` and the append gets a retryable error. GCS and Azure use their
platform credential chains. `store.as_of(snapshot)` and `store.as_known_at(t)`
reproduce a past settlement.

| Endpoint | Description |
|---|---|
| `GET /api/v1/archive/status` | Rows per tier, watermark position |
| `GET /api/v1/archive/olap/{malo_id}?from=&to=` | MMM aggregation over the resolved relation: `total_kwh`, `read_count`, bounds |
| `GET /api/v1/archive/portfolio?from=&to=&limit=N` | Top-N measuring points by **projected** Bezug; `truncated` when the scan cap was hit |
| `GET /api/v1/archive/timeseries/{malo_id}?from=&to=` | One row per interval; bounded by the read window |

### DataFusion SQL — `POST /api/v1/query/sql`

```bash
curl -s -X POST http://edmd:8380/api/v1/query/sql \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" -d '{
    "sql": "SELECT malo_id, CAST(SUM(\"value\") AS DOUBLE) AS kwh_total FROM meter_reads WHERE \"from\" >= TIMESTAMP '\'2025-01-01T00:00:00Z\'' GROUP BY malo_id ORDER BY kwh_total DESC LIMIT 20",
    "limit": 20
  }' | jq .
```

- Runs over meterstore's resolved relation `meter_reads`: `malo_id`, `"from"`
  (quote it), `to`, `value` (kWh), `sparte`, `obis_code`, `quality`, `unit`,
  `tenant`, `subject_ref`, `source`, `sender_mp_id`, `allocation_version`.
- Only `SELECT`, `WITH`, `SHOW`, `DESCRIBE`. The tenant predicate is injected
  below the projection.
- `403` for the raw every-version table and both ESA Typ-2 relations.
- JSON rows, or Arrow IPC with `"format": "arrow_ipc"`; every result carries
  `spans_tiers` and `touched_hot_tier`.

### Iceberg REST catalog

`/api/v1/iceberg` mounts meterstore's read-only `CatalogFacade`
(`…/v1/config`, `…/v1/namespaces`, `…/v1/namespaces/{ns}/tables/{table}`).
Mutating routes are `405`; the catalog returns locations, no credentials — each
engine reads Parquet with its own. Gated by `read-archive-olap`;
unauthenticated requests get `403`.

```sql
ATTACH 'rest+http://edmd:8380/api/v1/iceberg' AS mako (TYPE ICEBERG);

SELECT malo_id, DATE_TRUNC('month', "from") AS month, SUM("value") AS arbeitsmenge_kwh
FROM mako.meter_reads
WHERE "from" BETWEEN TIMESTAMP '2025-01-01' AND TIMESTAMP '2025-12-31'
  AND quality NOT IN ('FAULTY', 'UNKNOWN')
GROUP BY 1, 2 ORDER BY 1, 2;
```

---

## ESA „Werte nach Typ 2" {#esa-typ2}

MSCONS **13027** values are non-authoritative: *Codeliste der Konfigurationen*
1.4 Kap. 4.6 and WiM Strom Teil 2 §4 give them no bearing on Netznutzungs-,
Bilanzkreis- or Mehr-/Mindermengenabrechnung.

- The webhook forks 13027 (`ESA_TYP2_PIDS`) to `esa_typ2_reads` before any
  `meter_reads` write, validation or substitution.
- Every billing read aggregates `meter_reads` only; the Typ-2 store has no
  billing periods, correction audit, substitutes or `allocation_version`.
- `GET /api/v1/esa/typ2/{malo_id}` reads values back verbatim, multi-register
  (a QUOTES 15003 names 1–23 OBIS codes, QUOTES AHB 1.1a §4.3 `[2073]`).
- Kap. 4.6.2 deliveries arrive straight from the iMS over SM-PKI:
  `POST /api/v1/esa/typ2/{malo_id}` (Cedar `write-esa-typ2`). edmd does not
  terminate SM-PKI. The body is read leniently: a gateway on a newer BSI
  TR-03109 profile may carry keys edmd does not model, and they are ignored
  rather than refused; every field edmd reads stays required.
- The SQL endpoint refuses both Typ-2 relations.
- Both stores enrol the same `(tenant, MaLo)` erasure subject, so one Art. 17
  erasure unlinks both (§ 60 Abs. 6 MsbG makes no exception for Typ-2).

---

## Reading orders (Ablesesteuerung)

Orders live in `ablese_auftraege`, linked to their origin by
`auftrag_position_id` (O2C) or `insrpt_process_id` (INSRPT).

| Method | Path | Description |
|---|---|---|
| `POST` | `/api/v1/reading-orders` | Create |
| `GET` | `/api/v1/reading-orders` | List (`?malo_id=&status=&anlass=&limit=`) |
| `GET` | `/api/v1/reading-orders/{id}` | Status and result |
| `PUT` | `/api/v1/reading-orders/{id}/complete` | Record the reading (with `mscons_ref` when it came by MSCONS) |
| `PUT` | `/api/v1/reading-orders/{id}/cancel` | Withdraw — the obligation is discharged |
| `PUT` | `/api/v1/reading-orders/{id}/fail` | Ablesehindernis — still owed |
| `POST` | `/api/v1/reading-orders/campaign` | Jahresablese-Kampagne |
| `GET` | `/api/v1/compliance/jahresablesung/{year}` | § 40b Abs. 1 EnWG report |

| Anlass | Auftraggeber | Triggered by |
|---|---|---|
| `LIEFERBEGINN` | LF | GPKE 55001 completion, or `vertragd` |
| `LIEFERENDE` | LF | GPKE 55004 / 55007 completion, or `vertragd` |
| `JAHRESABLESUNG` | LF, NB | Campaign or ERP (§40 EnWG) |
| `ZWISCHENABLESUNG` | LF, ESA | On demand; an ESA may order value delivery (WiM Strom Teil 2 Kap. 4 / § 60 Abs. 1 MsbG) |
| `SPERRUNG` · `ENTSPERRUNG` | NB | `processd` Sperrauftrag queue (§41f EnWG; Wiederherstellung §41f Abs. 7) |
| `SONDERABLESUNG` | MSB | INSRPT 23003/23004/23008, Gas 23005/23009 |
| `INSRPT_STOERUNG` | MSB | INSRPT 23001 — `geplant_am` tomorrow, `ausfuehrt_bis` +7 days, idempotent on `insrpt_process_id` |
| `ISMS_AUSLESUNG` | MSB | iMSys auto-readout |

`anlass` and `auftraggeber_rolle` (`LF`, `MSB`, `NB`, `ESA`) are `CHECK`
constraints. `ablese_insrpt_unique (tenant, insrpt_process_id)` and
`ablese_scheduled_unique (tenant, malo_id, anlass, geplant_am)` make redelivery
and campaign reruns idempotent.

```
OFFEN → BEAUFTRAGT → AUSGEFUEHRT   reading taken, obligation met
   └──────────────→ STORNIERT      withdrawn
   └──────────────→ FEHLGESCHLAGEN Ablesehindernis — still owed
```

```http
PUT /api/v1/reading-orders/{id}/fail
{ "grund": "KEIN_ZUTRITT", "notiz": "3x angetroffen, niemand vor Ort" }
```

`grund`: `KEIN_ZUTRITT`, `ZAEHLER_UNZUGAENGLICH`, `ZAEHLER_DEFEKT`,
`ZAEHLER_NICHT_AUFFINDBAR`, `KUNDE_VERWEIGERT`, `ABLESUNG_UNPLAUSIBEL`,
`SONSTIGES` (use `notiz`). `FEHLGESCHLAGEN` without a grund is refused by a
`CHECK`. Failing emits `de.messwert.reading.order.failed`; completing emits
nothing — `billingd` reads the Zählerstand from the billing period.

- **No order closes itself**, an iMSys's included: an arriving MSCONS reading is
  stored but does not touch the order. Until `complete`/`cancel`/`fail`, an order
  past `ausfuehrt_bis` stays in MCP `list_overdue_reading_orders`.
- **The campaign** enumerates the NB's SLP MaLos from `marktd` and raises one
  `JAHRESABLESUNG` per MaLo with its `sparte`, in one
  `INSERT … SELECT unnest(…) ON CONFLICT DO NOTHING`. A MaLo whose `sparte`
  marktd leaves absent or names outside edmd's set is ordered nothing and listed
  in `refused_unknown_sparte` — the Sparte decides whether the reading is kWh or
  m³, so it is never assumed to be Strom.
- **The § 40b report** breaks orders into `AUSGEFUEHRT` (discharged),
  `STORNIERT`, `FEHLGESCHLAGEN` (with `fehlschlag_gruende`) and late.
  `ablesequote` is over orders raised, not the SLP population — cross-check the
  population against `marktd`.

```mermaid
sequenceDiagram
    autonumber
    participant LF as vertragd (LF)
    participant edmd
    participant MSB as MSB / iMSys
    participant billingd

    LF->>edmd: POST /api/v1/reading-orders<br/>{ malo_id, anlass: "LIEFERBEGINN",<br/>  auftraggeber_rolle: "LF", geplant_am }
    edmd-->>LF: 201 { id, status: "OFFEN" }
    MSB->>edmd: PUT /api/v1/reading-orders/{id}/complete<br/>{ zaehlerstand_kwh: 12345.678 }
    edmd-->>MSB: 204 No Content
    Note over edmd: AUSGEFUEHRT — Zählerstand filed<br/>into the register store. No event.
    billingd->>edmd: GET /api/v1/billing-period/{malo_id}
    edmd-->>billingd: zaehlerstand_anfang / _ende
```

---

## Virtual meters {#virtual-meters}

A virtual meter is a `virtual_meter_configs` row keyed by its own
`virtual_malo_id`, computed on demand and never stored in `meter_reads`. Its
`rule_json` is one of two kinds, and `rule_type` is the kind the document is
tagged with:

| `rule_type` | Arithmetic | Basis |
|---|---|---|
| `FORMULA` | `metering::allocation::formula::Formula::eval` over Messlokation registers | UTILTS AHB 1.1 Berechnungsformel (PID 25001) |
| `COMMUNITY_SHARE` | one participant's `net_grid_draw` from `metering::allocation::community::allocate` over every rule naming the same plant | §42b Abs. 5 EnWG in a building; §42c EnWG via the public grid, told apart by `legal_basis` (BNetzA Mitteilung Nr. 73, BK6-06-009) |

Both read **Messlokationen**, and edmd files readings under the Marktlokation
they were delivered for, so every rule carries `sources` — for each Messlokation
it reads, the MaLo its readings are filed under. A rule naming a Messlokation it
has no source for is refused at the door. A source series is the MaLo's
projected register in the operand's direction (Z71 Verbrauch → Bezug, Z72
Erzeugung → Einspeisung) — readings naming another Messlokation are left out —
built into one series on its observed grid. Every input must share that grid; a
source with no series in the window is `422`, a failed read `500` with an
`error_id`.

**`FORMULA`** is the UTILTS step graph: Rechenschritte keyed by number, each a
`SUM` of Z69/Z70 terms, a `QUOTIENT` (Z81 ÷ Z80, cut to six places toward zero;
a zero divisor gives 0, BDEW Anwendungshilfe *Beispiele von Berechnungsformeln
für das Solarpaket 1*, Beispiel 3), a `PRODUCT` of Z82 factors, or `POSITIVE`
(Z83, `max(0, x)`); a Messlokation operand carries Z16/ZB2 Verlustfaktoren and
the ZG6 Aufteilungsfaktor. The § 42b constant share of AWH Beispiel 1 —
`Pos(tenant Verbrauch − 0,5 × plant Erzeugung)`:

```bash
curl -X POST http://edmd:8380/api/v1/virtual \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" -d '{
    "virtual_malo_id": "10001234095",
    "display_name":    "GGV MaLo2 — 50 %",
    "sparte":          "STROM",
    "legal_basis":     "§42b EnWG Solarpaket I",
    "rule_json": { "FORMULA": {
      "formula": { "result": 3, "steps": {
        "1": { "PRODUCT": [ { "MELO": { "melo": "DE0001234567890000000000000000001",
                                        "direction": "Z72", "loss_trafo": null,
                                        "loss_line": null, "split": "0.5" } } ] },
        "2": { "SUM": [ [ "Z70", { "STEP": 1 } ],
                        [ "Z69", { "MELO": { "melo": "DE0001234567890000000000000000002",
                                             "direction": "Z71", "loss_trafo": null,
                                             "loss_line": null, "split": null } } ] ] },
        "3": { "POSITIVE": 2 } } },
      "sources": { "DE0001234567890000000000000000001": "51238696012",
                   "DE0001234567890000000000000000002": "51238696129" } } }
  }'
```

**`COMMUNITY_SHARE`** names the `plant`, this `tenant` and the `key`
(`{"kind": "PROPORTIONAL"}`, `{"kind": "CONSTANT", "fractions": {melo: "0.5", …}}`,
`{"kind": "EQUAL_SHARES"}` — § 42b Abs. 5 Satz 3 — or
`{"kind": "CASCADING", "weights": {…}}`). One row per participant; every
participant of one plant must carry the same key and file the plant under the
same MaLo, or the community is `422`. § 42b Abs. 5 caps each share at the
tenant's own consumption: „Die einem einzelnen teilnehmenden Letztverbraucher im
Wege der rechnerischen Aufteilung innerhalb eines 15-Minuten-Zeitintervalls
zuteilbare Strommenge ist begrenzt auf die durch ihn in diesem Zeitintervall
verbrauchte Strommenge." The proportional ratio is cut to six places toward
zero, so the shares never exceed the generation.

Routes: `POST|GET /api/v1/virtual`, `GET|DELETE /api/v1/virtual/{virtual_malo_id}`,
`GET /api/v1/virtual/{virtual_malo_id}/lastgang`.

### §42c allocation and readiness

`GET /api/v1/sharing/{community_id}/allocation` — `community_id` is the plant's
MeLo; participants are the `COMMUNITY_SHARE` rules naming it. Per interval it
reports the generation, each participant's consumption, nominal share,
allocation and net grid draw (`max(0, consumption) == allocated + net_grid_draw`),
`capped`, and the residual that fed the grid
(`Σ allocated + residual == generation`). A failed source read is `500` with an
`error_id`.

`GET /api/v1/sharing/readiness` answers whether the quarter-hour series §42c
Abs. 1 EnWG requires is **arriving** (marktd
`GET /api/v1/melos/{id}/sharing-eligibility` answers whether the metering is
**capable**). The verdict is `metering::allocation::sharing::assess_delivery`
over one Berlin calendar month: the dominant energy register as a series, its
billable quarter-hours against those the month holds.

| Parameter | Default |
|---|---|
| `month` | the last complete month (`YYYY-MM`) |
| `malo_ids` | every MaLo with readings in the month |
| `min_coverage_pct` | 95 (integer percent) |

Per point: `DELIVERING` · `INSUFFICIENT` · `ABSENT`, the observed `resolution`,
billable coverage and a `required_action`. A register that is not one series is
`INSUFFICIENT`, its structural code as the reason.

---

## Delivery surveillance

An hourly sweep looks for measuring points that **stopped** delivering — the
failure no ingest-triggered check can see.

| State | Meaning |
|---|---|
| `SILENT` | Newest interval ended more than `silent_after_hours` (36) ago |
| `UNDER_COVERED` | Billable coverage below `min_coverage_pct` over `coverage_window_days` |

Coverage is a duration ratio over billable qualities. A point that has never
delivered is not reported (that is `marktd`'s question).
`max_events_per_sweep` (500) caps events; the register still records everything
and the response carries `suppressed`.

```bash
curl -s "http://edmd:8380/api/v1/surveillance/delivery?state=SILENT" -H "Authorization: Bearer $TOKEN"
```

```json
{
  "count": 1,
  "points": [{
    "malo_id":           "51238696012",
    "state":             "SILENT",
    "last_interval_end": "2026-08-15 06:00:00 +00:00:00",
    "hours_silent":      74,
    "coverage_pct":      "56.55",
    "first_detected_at": "2026-08-16 19:00:00 +00:00:00"
  }],
  "legal_basis": "§ 60 Abs. 1 MsbG — a measuring point that stops delivering leaves the Übermittlung an die berechtigten Stellen owing"
}
```

**ESA Typ-2 stream** (`stream = 'TYP2'`, `typ2_enabled`): keyed on
(Meldepunkt, subscription, register) from `SG9 PIA+5 … :SRW` and `SG1 RFF+AGI`
(MSCONS AHB 3.2 §11.2 `[574]`). The threshold comes from the ordered Messprodukt,
resolved at marktd (`GET /api/v1/esa/subscriptions/{bestellung_ref}`): Rohdaten
products („unverzüglich, jedoch spätestens bis 9:30 Uhr") go late after **33 h**;
otherwise `typ2_silent_after_hours` (36). Only silence is scored.

**Standing faults are announced on transition.** `delivery_surveillance`,
`cls_compliance_issues` and `smgw_cert_expiry_alerts` are registers keyed on the
problem; each emits on entering and leaving the state, using the database clock
for the "not re-sighted" watermark.

---

## SMGW session registry and §14a compliance {#smgw-compliance}

The duty is **§ 25 MsbG** (the Smart-Meter-Gateway-Administrator configures,
administers, monitors and maintains the iMSys and reports security deficiencies
to the BSI) and **BSI TR-03109**. A silent gateway breaches the **§ 60 Abs. 1
MsbG** delivery duty. The §14a Konfigurationsprodukt is **BK6-22-300**. The
90/30/7-day certificate ladder is an operational choice.

`check_session_compliance()` is pure; the daily worker
(`[smgw] sweep_interval_secs`) runs it over every `smgw_sessions` row except
`REPLACED` gateways, records issues and emits CloudEvents.

| Stage | `issue_type` | Severity | Basis / meaning |
|---|---|---|---|
| 1 | `GATEWAY_REVOKED` | **CRITICAL** | § 25 MsbG security incident — replace the gateway |
| 2 | `COMMUNICATION_FAULT` | **CRITICAL** | § 60 Abs. 1 MsbG — silence beyond `comm_fault_threshold_hours` |
| 3 | `TLS_CERT_MISSING` | **CRITICAL** | BSI TR-03109-4 — admin protocol unreachable |
| 3 | `CERT_EXPIRED` | **CRITICAL** | Past `valid_to` — §14a eligibility lost |
| 3 | `CERT_REVOKED` | **CRITICAL** | Withdrawn by the SM-PKI — report to the BSI |
| 3 | `CERT_NOT_YET_VALID` | **CRITICAL** | `valid_from` in the future — provisioning fault or wrong gateway clock |
| 3 / 4 | `CERT_EXPIRING` | WARNING | Inside `cert_warning_days` (30) |
| 5 | `CLS_NOT_COMPLIANT` | WARNING | No §14a Konfigurationsprodukt (BK6-22-300) |

Stage 3 covers TLS certificates, stage 4 the others.

```bash
# Register or update a session (after a TR-03109-4 admin session or GWA sync)
curl -s -X PUT "http://edmd:8380/api/v1/smgw/51238696012" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "device_id": "SMGW-2026-001", "firmware_version": "3.1.2",
    "msb_mp_id": "9900000000003", "malo_id": "51238696012", "status": "OPERATIONAL",
    "certificates": [{ "serial_number": "AA:BB:CC:DD", "cert_type": "TLS",
      "subject_cn": "SMGW-2026-001", "issuer_cn": "BSI-Smart-Meter-CA",
      "valid_from": "2025-01-01", "valid_to": "2027-06-30", "is_revoked": false }],
    "cls_channels": [{ "channel_id": "CLS-00042", "malo_id": "51238696012",
      "device_type": "HEAT_PUMP", "max_power_kw": "8.50", "channel_status": "ACTIVE",
      "produktcode": "FLEX-001", "valid_from": "2026-01-01" }],
    "last_contact_at": "2026-07-18T07:55:00Z", "installed_at": "2025-06-01"
  }'
# → 204 when compliant; 200 { "status": "accepted_with_compliance_issues", "issues": [...] }

curl -s "http://edmd:8380/api/v1/smgw/51238696012" -H "Authorization: Bearer <token>"   # session + 10 recent issues
curl -s "http://edmd:8380/api/v1/smgw?status=OPERATIONAL" -H "Authorization: Bearer <token>"
curl -s "http://edmd:8380/api/v1/smgw/compliance" -H "Authorization: Bearer <token>"     # read-only scan
curl -s -X POST "http://edmd:8380/api/v1/smgw/compliance/scan" -H "Authorization: Bearer <token>"  # sweep + events
```

`de.messwert.cls.compliance-issue` payload:

```json
{
  "malo_id": "51238696012", "device_id": "SMGW-2026-001",
  "issue_type": "CERT_EXPIRING", "severity": "WARNING",
  "cert_serial": "AA:BB:CC:DD", "cert_type": "TLS", "days_to_expiry": 12,
  "channel_id": null,
  "description": "SMGW SMGW-2026-001 TLS cert AA:BB:CC:DD expires in 12 days — renew now"
}
```

`agentd`'s `smgw-diagnostics-agent` subscribes to it.

---

## CloudEvents emitted

Eleven types from `mako_events::messwert`, all through the transactional outbox
(retried with backoff, dead-lettered on exhaustion). Emission does not depend on
`erp_webhook_url`: without one, events accumulate in `event_outbox`.

| Type | Raised by | When |
|---|---|---|
| `de.messwert.reading.direct.stored` | every direct-push ingest | A batch committed — `billingd` recomputes |
| `de.messwert.reading.quality.warning` | every ingest door + rescore | grade C/F or any validation finding |
| `de.messwert.reading.order.failed` | `PUT /reading-orders/{id}/fail` | An Ablesehindernis; the reading is still owed |
| `de.messwert.reading.confirmation.overdue` | confirmation worker | One aggregate per sweep of newly overdue § 60 Abs. 1 estimates |
| `de.messwert.reading.delivery.overdue` / `.resumed` | delivery sweep | A point went `SILENT`/`UNDER_COVERED` / recovered |
| `de.messwert.esa.typ2.delivery.overdue` / `.resumed` | Typ-2 sweep | An ESA subscription register went silent / resumed |
| `de.messwert.cls.compliance-issue` / `-resolved` | SMGW sweep | A §14a / certificate fault entered / left the register |
| `de.messwert.smgw.cert.expiry-warning` | cert-expiry worker | 90 / 30 / 7 days before `valid_to`, once per tier |

Subscribe to both halves of each pair; the registers announce only transitions.

---

## GDPR Art. 17 erasure

The cold tier is append-only, so erasure is **pseudonymisation**: every MaLo is
enrolled as an erasure subject at ingest (a `subject_ref` on each row, one per
collection year), and erasure destroys the mapping in meterstore's registry
(`meterstore_subject_map` / `meterstore_erasures`). Readings survive in both
tiers, unattributable — § 147 Abs. 1 AO retention is preserved.

§ 60 Abs. 6 MsbG
(personal Messwerte deleted or anonymized at the latest three years after the
collection year) is discharged by the same mechanism.

`DELETE /api/v1/gdpr/erasure/{malo_id}` runs in **one transaction** on edmd's
pool (the registry shares the database):

1. Records the request in `gdpr_deletions` (idempotent on `malo_id + tenant`).
2. Destroys every subject mapping of the tenant-qualified MaLo
   (`SubjectRegistry::erase_all_in`) — intervals, ZSG readings and ESA Typ-2
   values at once.
3. Rewrites or deletes edmd's own MaLo-keyed rows:

| Treatment | Tables |
|---|---|
| `malo_id` → subject reference (Buchungsbelege: § 147 Abs. 1 Nr. 4, Abs. 3 AO, eight years; Art. 17 Abs. 3 lit. b DSGVO). A MaLo with no mapping gets a freshly minted reference recorded nowhere else, so these rows are never deleted | `zsg_conversion_log`, `meter_read_corrections`, `substitute_value_log`, `meter_data_receipts`, `ablese_auftraege`, `gas_quality_data` |
| deleted (derived, operational, device) | `meter_billing_periods`, `quality_assessments`, `estimated_read_confirmations`, `direct_push_sessions`, `smgw_sessions`, `cls_compliance_issues`, `delivery_surveillance`, `smgw_cert_expiry_alerts` |
| deleted where it names the MaLo as `virtual_malo_id` or as a source in `rule_json` (bound JSON path variable) | `virtual_meter_configs` |

```bash
curl -X DELETE "http://edmd:8380/api/v1/gdpr/erasure/51238696012" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" -d '{
    "reason":        "Customer right-to-erasure request #2026-42",
    "authorized_by": "gdpr-officer@example.com"
  }'
```

```json
{
  "malo_id":          "51238696012",
  "status":           "erased",
  "subject_unlinked": true,
  "mechanism":        "meterstore subject-registry pseudonymisation (append-only tiers)",
  "legal_basis":      "DSGVO Art. 17 right to erasure"
}
```

`subject_unlinked` is `false` when the MaLo had no mapping (never stored or
already erased) — recorded, not an error. Its Buchungsbelege are pseudonymised
all the same.

### Erasure suppression (`[privacy]`) {#erasure-suppression-privacy}

```toml
[privacy]
erasure_secret          = "env:EDMD_ERASURE_SECRET"    # ≥ 32 bytes
retired_erasure_secrets = ["env:EDMD_ERASURE_SECRET_2025"]
```

With a key, an erasure records `HMAC(key, identifier)` and later registrations of
that identifier are refused — so a redelivery cannot re-link the subject, and a
request arriving before the data is honoured. Rotation is additive: move the old
key to `retired_erasure_secrets`, which stay as long as their erasures must stay
suppressed. Retired keys without a current one are refused at startup; omitting
the section disables suppression (logged at startup).

### `[policy] physische_loeschung` {#policy-physische-loeschung}

Whether an Art. 17 erasure also removes the erased rows physically is the data
protection officer's reading. Physical removal serves Art. 17 Abs. 1 DSGVO most
literally and can collide with the § 147 Abs. 1 AO duty to keep the same values
as Buchungsbelege, which Art. 17 Abs. 3 lit. b DSGVO exempts.

| `physische_loeschung` | Effect |
|---|---|
| `false` (default) | The erasure above: the subject mapping is destroyed in both tiers and a tombstone refuses re-linking; the readings survive, attributable to no one |
| `true` | Physical removal from the cold tier as well. This build performs none, so the value is refused at startup rather than reporting a deletion that did not happen |

Every erasure response names the reading in `policy` (`{key, value, quelle}`).
The startup report also states whether `[privacy] erasure_secret` is present —
never its value — and logs its absence at `warn`. The report is served at
`GET /api/v1/policy` (Cedar `read-policy`) and by the MCP tool `get_policy`;
every operator setting is listed in
[Operator policy](@/docs/reference/operator-policy.md).

### The trail

`GET /api/v1/gdpr/erasures?since=&until=&trigger=&limit=` (`read-gdpr-erasures`)
lists when, why, by whom, which duty (`request` = Art. 17, `retention` = § 60
Abs. 6 sweep) and any suppression lift — no natural identifier. A quarter with no
`retention` rows is a sweep that stopped. A backwards period or non-positive
limit is refused.

---

## MCP server tools

`/mcp` (Streamable HTTP), Cedar `use-mcp`; the two write tools additionally need
the REST write action.

| Tool | Description |
|------|-------------|
| `get_policy` | The operator policy and posture the deployment runs under |
| `get_timeseries` | Meter data for a MaLo in a date range |
| `get_imbalance` | Mehr-/Mindermengen report (`metering::billing::imbalance::ImbalanceSaldo`; an unknown Sparte is refused) |
| `get_billing_period` | `MeterBillingPeriod` |
| `get_device_history` | MSB device history as narrative text |
| `get_quality_warnings` | Quality grade and findings over a window, with the Sparte's preset |
| `list_reading_orders` | Reading orders for a MaLo |
| `list_overdue_reading_orders` | § 40b Abs. 1 EnWG gaps |
| `trigger_jahresablesung` | Launch or preview the annual campaign (`write-reading-order`) |
| `trigger_substitution` | Generate and store § 60 Abs. 1 MsbG Ersatzwerte (`write-timeseries`) |
| `get_correction_history` | Correction audit trail (§ 147 Abs. 1 AO / § 146 Abs. 4 AO) |
| `validate_timeseries` | Validation over stored reads — the ingest pass; reports `rules_evaluated` / `rules_skipped` |
| `get_quality_assessments` | Per-batch quality history |
| `get_summenzeitreihe` | Monthly aggregated kWh for MaBiS |
| `get_annual_forecast` | § 40a Abs. 2 EnWG Verbrauchsschätzung |
| `get_gas_quality` | PID 13007 Brennwert + Zustandszahl |

Prompts: `analyze-consumption`, `submit-mscons`, `quality-assessment`,
`jahresablesung-workflow`, `reading-order-lifecycle`.

Each tool shares its REST twin's implementation (the register projection, the
validation pass, the forecast, the repository methods), so both surfaces answer
alike.

---

## Monitoring

`GET /edmd/metrics` serves `edmd_billing_periods_total`, `edmd_db_pool_size` and
`edmd_db_pool_idle`; the runner serves request metrics on `/metrics`.
meterstore's own instruments — `meterstore.archival.windows`,
`meterstore.partitions.reclaimed`, `meterstore.partitions.reclaim_held`,
`meterstore.tiering.boundary_moved`, `meterstore.tiering.watermark_lag` and the
rest — record through the OpenTelemetry global meter provider, which edmd does
not install; `GET /api/v1/archive/status` is the tiering signal it serves.

| Watch | Target |
|--------|--------|
| Webhook `de.mako.process.*` success rate | > 99 % |
| DB pool utilisation | < 80 % |
| meterstore watermark lag (oldest hot-tier interval) | Within the settlement lag |
| `GET /api/v1/surveillance/delivery?state=SILENT` | Empty, or explained |
| `event_outbox` backlog | Draining |

| Symptom | Look at |
|---|---|
| Startup refused: no `[oidc]` | Configure OIDC, or `allow_insecure_no_auth = true` in dev |
| MSCONS deliveries redelivered in a loop | Should not happen: a refused write is `422`. A `500` means a transient store failure |
| `422` on gas push | `brennwert_kwh_per_m3` missing, or a unit outside the Sparte |
| Ersatzwert written but aggregate unchanged | Check `methods_applied`, `refused`, and whether the slots were already billable (`skipped_measured`) |
| Startup refused: `sslmode` | `prefer`/`allow` are refused; name `disable`, `require`, `verify-ca` or `verify-full` in `[database] url` |
| `422` naming a check digit | `sender_mp_id` (or the tenant standing in for it) is not an issued Marktpartner-ID |
| Cold tier not filling | `[archive] enabled`, `maintenance_interval_secs`, `deferred` cycles (`ddl_lock_timeout_secs`) |
| SQL query `403` | Querying the raw versioned table or an ESA Typ-2 relation |

---

## See also

- [Domain model](@/docs/architecture/domain-model.md) — Marktrollen, MaLo/MeLo,
  identifiers, the BO4E gate.
- [PID reference](@/docs/regulatory/pid-reference.md) — every Prüfidentifikator
  and its routing workflow.
- [makod](@/docs/services/makod.md) — produces the `de.mako.process.*` events edmd
  consumes.
- [netzbilanzd](@/docs/services/netzbilanzd.md) — reads `MeterBillingPeriod`, and the
  Summenzeitreihe it files with the BIKO.
- [billingd](@/docs/services/billingd.md) — reads the projected series for §41a
  dynamic billing.
- [einsd](@/docs/services/einsd.md) — reads the Einspeisung for the §51 EEG
  reduction.
- [agentd](@/docs/services/agentd.md) — the specialists consuming
  `de.messwert.*`.
