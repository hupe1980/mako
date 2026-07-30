# edmd — Energy Data Management daemon

`edmd` stores MSCONS meter readings received from `marktd`, accepts direct iMSys/SMGW interval push, scores data quality with a Hampel filter, schedules reading orders (Ablesesteuerung), and serves BO4E typed time-series and imbalance queries — `Energiemenge` deliveries for ERP billing import, `Lastgang`/`Zeitreihe` for API-Webdienste Strom, `MeterBillingPeriod` for `netzbilanzd`, and Mehr-/Mindermengen reconciliation for `invoicd`. `meter_reads` (and the non-authoritative `esa_typ2_reads` stream) are stored through the [`meterstore`](https://github.com/hupe1980/meterstore) crate — a recent window in PostgreSQL and the settled history in **Apache Iceberg V2** on S3/GCS/Azure, split by a tiering watermark that meterstore owns.

| Feature | Detail |
|---|---|
| HTTP port | `:8380` |
| Database | PostgreSQL 17+ (sqlx 0.8, schema from `migrations/0001_schema.sql`) |
| Tiering | `meter_reads` / `esa_typ2_reads` are meterstore tables — a hot PostgreSQL window plus settled Iceberg V2 history, split by a tiering watermark (one-week settlement lag). edmd supplies a `TableConfig` (daily partition/archival steps, `tenant` identity column); meterstore owns partitioning, retention, version resolution and cold-tier archival |
| Schema | edmd business tables in its own PgPool: `meter_billing_periods` — NUMERIC aggregates, `tenant TEXT NOT NULL`; `gdpr_deletions`, `ablese_auftraege`, `direct_push_sessions`, `meter_read_corrections`, `substitute_value_log`, `quality_assessments`. `meter_reads` / **`esa_typ2_reads`** are meterstore-owned, not declared in edmd's DDL |
| ESA Typ-2 store | ESA-delivered "Werte nach Typ 2" (MSCONS **13027**) are **non-authoritative** (Codeliste 1.4 Kap. 4.6 · WiM Strom Teil 2 §4) — no bearing on Netznutzungs-/Bilanzkreis-/Mehr-/Mindermengenabrechnung. They land in a **separate `esa_typ2_reads` table**, never `meter_reads`, so no billing query can reach them by omission. No correction, substitution, reconciliation, or billing-period participation. Read via `GET /api/v1/esa/typ2/{malo_id}` |
| Inbound | CloudEvents from `marktd` — `de.mako.process.completed` (MSCONS billing PIDs 13005–13025 → `meter_reads`; **13027 → `esa_typ2_reads`**; GPKE 55001 → LIEFERBEGINN and 55004/55007 → LIEFERENDE reading orders), `de.mako.process.initiated` (INSRPT 23001/23003/23004/23005/23008/23009 → auto reading orders) |
| Kafka ingest | Optional `[kafka_ingest]` consumer (krafka) for head-end systems — at-least-once, earliest offset reset, same V01–V10 + audit path as REST; e2e-tested against an in-process `FakeBroker`**Trust boundary: topic-level** — no per-message auth; restrict topic ACLs to the head-end system |
| Direct push | `POST /api/v1/meter-reads/rlm/{malo_id}` (Strom), `POST /api/v1/meter-reads/gas/{malo_id}` (Gas m³→kWh_Hs) — idempotent on `session_id` |
| Quality scoring | `metering::score_intervals_f64` — Hampel filter (k=3, t=3.0, MAD×1.4826σ), auto-vectorises to AVX2/NEON; grades A/B/C/F; retroactive: `POST /api/v1/quality-score/{malo_id}` |
| Reading orders | `POST/GET /api/v1/reading-orders` — Ablesesteuerung for LF/MSB/NB/ESA (an ESA may order value delivery, §60 Abs. 1 MsbG); `/complete`, `/cancel`, `/fail` (Ablesehindernis); auto-creates `INSRPT_STOERUNG` on INSRPT PID 23001 (WiM Störungsmeldung) |
| § 60 MsbG confirmations | Every stored ESTIMATED/SUBSTITUTED interval opens an obligation in `estimated_read_confirmations`; auto-discharged when a MEASURED/CORRECTED value for the slot arrives (ingest or correction path). Daily worker (`[confirmation]`, default on, `deadline_weeks = 8` — aligned with the MaBiS BKA correction window, no statute fixes a number) escalates stale ones to UEBERFAELLIG and emits `de.messwert.reading.confirmation.overdue`; `GET /api/v1/confirmations?status=` lists them |
| §40 compliance | `GET /api/v1/compliance/jahresablesung/{year}` — only `AUSGEFUEHRT` discharges the annual-reading obligation |
| REST API | `GET /api/v1/deliveries/{malo_id}` → `Vec<Energiemenge>` · `GET /api/v1/lastgang/{malo_id}` · `GET /api/v1/zeitreihe/{malo_id}` · `GET /api/v1/billing-period/{malo_id}` · `GET /api/v1/imbalance/{malo_id}/{year}/{month}` · `GET /api/v1/netzverlust?from=&to=` (§22 EnWG indicative grid-loss balance) · `GET /api/v1/esa/typ2/{malo_id}` (ESA Typ-2 store — never billing) |
| Arrow IPC | `Accept: application/vnd.apache.arrow.stream` on `GET /api/v1/lastgang` + `GET /api/v1/zeitreihe` — 10–50× throughput vs JSON for bulk reads |
| Archive OLAP | `GET /api/v1/archive/status` · `GET /api/v1/archive/olap/{malo_id}` · `GET /api/v1/archive/portfolio` · `GET /api/v1/archive/timeseries/{malo_id}` · `POST /api/v1/query/sql` (DataFusion, JSON or Arrow IPC, over meterstore's resolved relation) |
| Iceberg REST | `GET /api/v1/iceberg/v1/...` — read-only Iceberg REST catalog (meterstore's `CatalogFacade`, mounted by edmd, Cedar-gated by `read-archive-olap`; mutating routes → 405). DuckDB / Spark / Trino / PyIceberg attach for schema + table locations, then read Parquet from object storage with their own credentials |
| GDPR | `DELETE /api/v1/gdpr/erasure/{malo_id}` — Art. 17 pseudonymisation: destroys the MaLo's subject mapping in meterstore's registry and deletes the derived edmd tables, in one transaction. Readings survive in both tiers but become unattributable |
| Auth | OIDC/JWT + Cedar ABAC — reads tenant-scoped, **writes role-gated** (`write-meter-reads` → MSB/admin; series mutation, reading orders, GDPR erasure → MSB/NB/admin; LF-role tokens are read-only; gates pinned by the `cedar_policy` test suite); webhook HMAC-SHA256 (`X-Mako-Signature`). Refuses to start without `[oidc]` unless `allow_insecure_no_auth = true` |
| Rate limiting | Per-tenant and global GCRA buckets; `429` carries `Retry-After` |
| Health | `GET /health/live`, `GET /health/ready` (PostgreSQL ping) |
| MCP | `POST\|GET /mcp` — 15 tools + 5 prompts, including `get_timeseries`, `validate_timeseries`, `trigger_substitution` (§ 60 Abs. 2 MsbG Ersatzwerte), `trigger_jahresablesung`, `get_correction_history` |
| CloudEvents emitted | `de.messwert.reading.direct.stored`, `de.messwert.reading.quality.warning` (grade C/F), `de.messwert.reading.order.failed`, `de.messwert.cls.compliance_issue`, `de.messwert.smgw.cert.expiry_warning` |
| SMGW cert expiry (BSI TR-03109-4 §6.3) | Daily worker sweeps every certificate in `smgw_sessions` and emits `de.messwert.smgw.cert.expiry_warning` at **90 / 30 / 7 days** before `valid_to` (`SMGW_CERT_ABLAUFDATUM`), once per tier per certificate (dedup in `smgw_cert_expiry_alerts`); severity INFO → WARNING → CRITICAL. An expired cert silently ends §14a Fernsteuerbarkeit; `agentd` `smgw-diagnostics-agent` consumes the warning and escalates renewal to the MSB |
| Quality history | Every scoring path records a verdict in `quality_assessments`; re-scoring supersedes rather than appends |
| § 60 Abs. 6 MsbG audit trail | Every value-changing overwrite — corrections **and** redeliveries, on every transport — leaves an immutable `meter_read_corrections` row; `?as_of=` reconstructs prior knowledge states |
| Overlap exclusion | Per-partition `EXCLUDE USING gist` (`btree_gist`): a delivery whose range overlaps a stored reading is refused rather than double-counted |

---

## Quick Start

```bash
edmd --config edmd.toml
```

Migrations run automatically at startup from `migrations/0001_schema.sql`.
The schema is designed for a fresh install — no incremental migration state is maintained.

---

## Configuration

All settings live in `edmd.toml`. The binary takes three arguments:

| Flag | Env var | Default | Description |
|---|---|---|---|
| `-c`, `--config` | `EDMD_CONFIG` | `edmd.toml` | Path to the configuration file |
| `--log-level` | `RUST_LOG` | `info` | Log level |
| `--check` | `EDMD_CHECK` | `false` | Validate configuration and database connectivity, then exit 0 |

`--check` is the container health gate: it resolves every `env:` reference, opens
the database, and exits without binding a port.

### Sections

```toml
# Required unless an [oidc] section is present. Without token verification every
# request is admitted as `dev-admin` with all market roles.
allow_insecure_no_auth = false

[http]
addr = "0.0.0.0:8380"

[database]
url       = "env:EDMD_DATABASE_URL"
pool_size = 10

[identity]
tenant = "9900357000004"          # BDEW Codenummer

[marktd]
url     = "http://marktd:8180"
api_key = "env:EDMD_MARKTD_API_KEY"

[subscription]
subscriber_id = "edmd"
webhook_url   = "http://edmd:8380/webhook"

[webhook]
inbound_secret   = "env:EDMD_INBOUND_SECRET"   # verifies X-Mako-Signature
erp_webhook_url  = "http://erp:9000/events"    # outbound CloudEvents

[oidc]
issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
audience = "api://mako-edmd"

[rate_limit]
requests_per_second            = 500    # global sustained
burst                          = 1000   # ingest is bursty by nature
per_tenant_requests_per_second = 100

[otel]
endpoint = "http://otel-collector:4317"

[mcp]
api_key = "env:EDMD_MCP_API_KEY"

# Optional: high-throughput reading intake from a Kafka topic.
[kafka_ingest]
enabled           = true
bootstrap_servers = "kafka-1:9092,kafka-2:9092"
topic             = "edmd.meter-reads"   # default
group_id          = "edmd-ingest"        # default
```

### Archive configuration (`[archive]` in `edmd.toml`)

meterstore is a library edmd links in-process (the published `meterstore` crate)
and reads no config of its own, so its tiering knobs are supplied here:

```toml
[archive]
enabled             = true
storage_uri         = "s3://my-bucket/edmd/warehouse"   # scheme picks the backend
region              = "eu-central-1"
# access_key_id / secret_access_key optional — omit to use the instance-role chain.
# endpoint_url      = "http://minio:9000"   # S3-compatible (MinIO/Ceph/R2 → path-style)
settlement_lag_days = 7    # age at which an interval settles hot → cold (mutable below this)
partition_step_days = 1    # cold-tier partition granularity
archival_step_days  = 1    # watermark advance per archival sweep
cold_file_target_mib = 512 # target Parquet file size
maintenance_interval_secs = 3600  # tiering loop cycle period (archives due windows)
```

`enabled` turns the cold tier on; the `storage_uri` **scheme** selects the
warehouse backend — `file://`, `memory://` (dev), `s3://` (and S3-compatible
`minio://` / `r2://`), `gs://`, `abfss://`. For S3, credentials come from `region`
+ the optional `access_key_id`/`secret_access_key` (prefer `"env:…"` refs), or —
recommended in production — are **omitted** so the EC2/IRSA instance-role chain
supplies them. GCS and Azure authenticate through their platform chains (ADC /
managed identity). There is no retention window or archival interval to tune —
meterstore reclaims the hot tier through its tiering watermark and owns the Iceberg
format, partitioning and layout; `settlement_lag_days` is simply how long a reading
stays correctable in the hot tier before it settles.

---

## REST API

### `GET /api/v1/deliveries/{malo_id}`

Returns all typed meter reads for a Marktlokation within the given time range.

Query parameters: `from`, `to` (ISO 8601, defaults to epoch / now).

```bash
curl "http://localhost:8380/api/v1/deliveries/51238696780?from=2025-10-01T00:00:00Z&to=2026-10-01T00:00:00Z"
```

Response:

```json
[
  {
    "malo_id":      "51238696780",
    "melo_id":      "DE0001234567890123456789012345678",
    "dtm_from":     "2025-10-01T00:00:00Z",
    "dtm_to":       "2025-10-01T01:00:00Z",
    "quantity_kwh": "123.456",
    "quality":      "ABLESEWERT",
    "pid":          13002
  }
]
```

### `GET /api/v1/archive/olap/{malo_id}`

MMM aggregation over meterstore's version-resolved, tier-split series (hot window
+ settled Iceberg history). Typical use case: MMM Jahresabrechnung spanning 3+
billing years.

Query parameters: `from`, `to` (ISO 8601).

```bash
curl "http://localhost:8380/api/v1/archive/olap/51238696780?from=2022-01-01T00:00:00Z&to=2024-12-31T23:59:59Z" \
  -H "Authorization: Bearer <token>"
```

Response:

```json
{
  "malo_id":    "51238696780",
  "total_kwh":  "98765.43200",
  "read_count": 105120,
  "from":       "2022-01-01 00:00:00 +00:00:00",
  "to":         "2024-12-31 23:59:59 +00:00:00"
}
```

### `GET /api/v1/imbalance/{malo_id}/{year}/{month}`

Returns the Mehr-/Mindermengen imbalance report for a single billing month.

```bash
curl "http://localhost:8380/api/v1/imbalance/51238696780/2025/10"
```

Response:

```json
{
  "malo_id":     "51238696780",
  "year":        2025,
  "month":       10,
  "mehr_kwh":    "42.0",
  "minder_kwh":  "0.0",
  "total_reads": 744
}
```

---

## Database Schema

`migrations/0001_schema.sql` is the single authoritative DDL — the schema is
designed for a fresh install, so no incremental migration state is maintained.

| Area | Tables |
|---|---|
| Metered data receipts | `meter_data_receipts` · `meter_billing_periods` (billing-period cache) |
| Corrections & substitution | `meter_read_corrections` · `substitute_value_log` |
| Quality | `quality_assessments` |
| Reading orders | `ablese_auftraege` |
| Ingest sessions | `direct_push_sessions` |
| Gas | `gas_quality_data` |
| Virtual meters (§42b/§42c EnWG) | `virtual_meter_configs` |
| Devices | `meter_exchange_events` · `smgw_sessions` · `cls_compliance_log` · `smgw_cert_expiry_alerts` |
| GDPR | `gdpr_deletions` (erasure requests) — the subject-mapping tables (`meterstore_subject_map` / `meterstore_erasures`) live in the same database but are meterstore's |

The authoritative `meter_reads` store and the non-authoritative `esa_typ2_reads`
store are **meterstore** tables (hot PostgreSQL + cold Iceberg), created and
owned by `store.create_tables()` — not declared in edmd's DDL. meterstore owns
their partitioning, version resolution and the hot-tier overlap exclusion.

All edmd tables carry `tenant TEXT NOT NULL` for multi-tenant isolation — the
operator's MP-ID (BDEW/DVGW Codenummer, not a UUID) — and every query filters
on it. edmd also declares `tenant` as meterstore's non-nullable identity column,
so two tenants' readings for one measuring point never merge.

---

## Event Routing

`edmd` subscribes to `de.mako.process.completed` events from `marktd` where `makopid`
is in the MSCONS PID set (`edmd::domain::MSCONS_PIDS`). On receipt:

1. Verifies the `X-Mako-Signature` HMAC (if configured)
2. Parses `data` into a `MeterDataReceipt`
3. Upserts the receipt row (idempotent on `process_id`)
4. Stores typed interval reads

---

## Relationship to Other Services

```
marktd :8180
  │  POST /webhook  (de.mako.process.completed · MSCONS PIDs)
  ▼
edmd :8380
  │  GET /api/v1/deliveries/{malo_id}           (meterstore hot PostgreSQL window)
  │  GET /api/v1/archive/olap/{malo_id}          (meterstore cold tier — Iceberg V2 on S3)
  │  GET /api/v1/imbalance/{malo_id}/{year}/{month}
  ├──► invoicd :8280       — MMM imbalance input for selbstausgestellt INVOIC
  ├──► netzbilanzd :8680   — MeterBillingPeriod (HT/NT kwh) for NNE / §14a ToU billing
  └──► ERP / operator dashboard — historical reads and billing data
```

## Testing

Two suites run on every plain `cargo test`, no external services:

- `cedar_policy` — pins the authorization gates: who may read, write meter reads,
  mutate series, run reading orders, and request GDPR erasure.
- `schema_code_guard` — pins the schema↔enum contracts so a column and its Rust
  enum cannot silently drift apart.

Storage integration runs against a real PostgreSQL + a filesystem Iceberg
warehouse via testcontainers — `#[ignore]`d by default (Docker required), run with
`just test-edmd-db`:

- `meterstore_integration` — ingest → version-resolved read-back (Sparte survives,
  not defaulted to Strom), latest-read, correction supersession with the § 60
  Abs. 6 audit row, GDPR Art. 17 subject-mapping erasure, and tenant isolation
  (the same MaLo under two tenants never merges on read).

The hot/cold tiering, watermark and cold-tier archival internals are meterstore's
own real-PostgreSQL tests; edmd exercises them through the `store` handle it
configures.

## See Also

- [edmd operator guide](../../docs/edmd.md)
- `edmd::domain` (in-crate) — `MeterDataReceipt`, `TimeSeriesRepository`/`Typ2Repository`, MSCONS PID set
- [meterstore](https://crates.io/crates/meterstore) — hot/cold tiered store (PostgreSQL + Apache Iceberg) backing `meter_reads` / `esa_typ2_reads`
- [marktd](../marktd/README.md) — event source
