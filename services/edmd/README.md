# edmd — Energy Data Management daemon

`edmd` stores MSCONS meter readings received from `marktd`, accepts direct iMSys/SMGW interval push, scores data quality with a Hampel filter, schedules reading orders (Ablesesteuerung), and serves BO4E typed time-series and imbalance queries — `Energiemenge` deliveries for ERP billing import, `Lastgang`/`Zeitreihe` for API-Webdienste Strom, `MeterBillingPeriod` for `netzbilanzd`, and Mehr-/Mindermengen reconciliation for `invoicd`. Meter reads older than the configured retention window are automatically offloaded to **Apache Iceberg V2** tables on S3/GCS/Azure for OLAP MMM aggregation.

| Feature | Detail |
|---|---|
| HTTP port | `:8380` |
| Database | PostgreSQL 15+ (sqlx 0.8, schema from `migrations/0001_schema.sql`) |
| Partitioning | `meter_reads` is range-partitioned monthly on `dtm_from`; retention drops whole partitions once every row in them is durable in the cold tier |
| Schema | `meter_reads` — `quantity_kwh NUMERIC(18,5)`, `tenant TEXT NOT NULL`; `meter_billing_periods` — NUMERIC aggregates, `tenant TEXT NOT NULL`; `gdpr_deletions`, `ablese_auftraege`, `direct_push_sessions`, `archive_batches`, `iceberg_catalog_entries` |
| Inbound | CloudEvents from `marktd` — `de.mako.process.completed` (MSCONS PIDs 13005–13027), `de.mako.process.initiated` (PID 23001 INSRPT → auto reading order) |
| Direct push | `POST /api/v1/meter-reads/rlm/{malo_id}` (Strom), `POST /api/v1/meter-reads/gas/{malo_id}` (Gas m³→kWh_Hs) — idempotent on `session_id` |
| Quality scoring | `metering::score_intervals_f64` — Hampel filter (k=3, t=3.0, MAD×1.4826σ), auto-vectorises to AVX2/NEON; grades A/B/C/F; retroactive: `POST /api/v1/quality-score/{malo_id}` |
| Reading orders | `POST/GET /api/v1/reading-orders` — Ablesesteuerung for LF/MSB/NB; `/complete`, `/cancel`, `/fail` (Ablesehindernis); auto-creates `INSRPT_STOERUNG` on INSRPT PID 23001 (§18 MessZV) |
| §40 compliance | `GET /api/v1/compliance/jahresablesung/{year}` — only `AUSGEFUEHRT` discharges the annual-reading obligation |
| REST API | `GET /api/v1/deliveries/{malo_id}` → `Vec<Energiemenge>` · `GET /api/v1/lastgang/{malo_id}` · `GET /api/v1/zeitreihe/{malo_id}` · `GET /api/v1/billing-period/{malo_id}` · `GET /api/v1/imbalance/{malo_id}/{year}/{month}` |
| Arrow IPC | `Accept: application/vnd.apache.arrow.stream` on `GET /api/v1/lastgang` + `GET /api/v1/zeitreihe` — 10–50× throughput vs JSON for bulk reads |
| Archive OLAP | `GET /api/v1/archive/status` · `GET /api/v1/archive/olap/{malo_id}` · `GET /api/v1/archive/portfolio` · `GET /api/v1/archive/timeseries/{malo_id}` · `POST /api/v1/query/sql` (DataFusion) |
| Iceberg REST | `GET /api/v1/iceberg/v1/...` — Iceberg REST catalog for DuckDB/Snowflake/Databricks direct attach |
| GDPR | `DELETE /api/v1/gdpr/erasure/{malo_id}` — Art. 17 hot-tier erasure (one transaction) + read-time cold-tier exclusion; `POST .../archive-plan` and `.../archive-complete` make the cold-tier rewrite trackable |
| Auth | OIDC/JWT + Cedar ABAC (`read-timeseries`, `write-quality-rescore`, `read-archive-olap`, `read-reading-order`, `write-gdpr-erasure`); webhook HMAC-SHA256 (`X-Mako-Signature`). Refuses to start without `[oidc]` unless `allow_insecure_no_auth = true` |
| Rate limiting | Per-tenant and global GCRA buckets; `429` carries `Retry-After` |
| Health | `GET /health/live`, `GET /health/ready` (PostgreSQL ping) |
| MCP | `POST\|GET /mcp` — 14 tools including `get_timeseries`, `get_imbalance`, `get_billing_period`, `validate_timeseries`, `list_overdue_reading_orders`, `trigger_jahresablesung` |
| CloudEvents emitted | `de.edmd.reading.direct.stored`, `de.edmd.reading.quality.warning` (grade C/F), `de.edmd.reading.order.failed` |
| Quality history | Every scoring path records a verdict in `quality_assessments`; re-scoring supersedes rather than appends |

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
```

### Archive configuration (`[archive]` in `edmd.toml`)

```toml
[archive]
enabled                = true
storage_uri            = "s3://my-bucket/edmd/meter_reads"
access_key_id          = "env:AWS_ACCESS_KEY_ID"
secret_access_key      = "env:AWS_SECRET_ACCESS_KEY"
region                 = "eu-central-1"
# endpoint_url         = "http://minio:9000"   # MinIO / LocalStack / Ceph RGW
retention_months       = 12      # keep in PostgreSQL for this many months
batch_size             = 100000  # rows per archival run
interval_secs          = 3600    # run every hour
iceberg_catalog_schema = "iceberg_catalog"
iceberg_catalog_name   = "edmd"
```

**Storage layout.** Parquet files are written with ZSTD level 3, `DELTA_BINARY_PACKED`
encoding on timestamps (exploits delta-of-delta = 0 for 15-min intervals),
`RLE_DICTIONARY` on `malo_id`/`quality`/`sparte`/`obis_code`, and Bloom filters
on `malo_id` (1 % FPR) for fast single-MaLo lookup from cold tier.

The PostgreSQL database stores the Iceberg SQL catalog — no external catalog
service (Nessie, Apache Polaris, AWS Glue) required.

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

DataFusion OLAP aggregation over Iceberg/S3 Parquet files (cold tier).
Typical use case: MMM Jahresabrechnung spanning 3+ billing years.

Query parameters: `from`, `to` (ISO 8601).

```bash
curl "http://localhost:8380/api/v1/archive/olap/51238696780?from=2022-01-01T00:00:00Z&to=2024-12-31T23:59:59Z" \
  -H "Authorization: Bearer <token>"
```

Response:

```json
{
  "malo_id":     "51238696780",
  "total_kwh":   98765.432,
  "read_count":  105120,
  "period_from": "2022-01-01T00:00:00Z",
  "period_to":   "2024-12-31T23:45:00Z",
  "source":      "iceberg-archive"
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
| Metered data | `meter_reads` (range-partitioned monthly on `dtm_from`) · `meter_data_receipts` · `meter_billing_periods` |
| Corrections & substitution | `meter_read_corrections` · `substitute_value_log` |
| Quality | `quality_assessments` |
| Reading orders | `ablese_auftraege` |
| Ingest sessions | `direct_push_sessions` |
| Gas | `gas_quality_data` |
| Virtual meters (§42b/§42c EnWG) | `virtual_meter_configs` |
| Devices | `meter_exchange_events` · `smgw_sessions` · `cls_compliance_log` |
| Cold tier | `archive_batches` · `iceberg_catalog_entries` |
| GDPR | `gdpr_deletions` · `gdpr_archive_files` |

All tables carry `tenant TEXT NOT NULL` for multi-tenant isolation — the BDEW/DVGW
Codenummer or GLN of the operating entity (not a UUID). `meter_reads.tenant` is the
authoritative column; `meter_data_receipts.tenant` uses the same type for consistency.

---

## Event Routing

`edmd` subscribes to `de.mako.process.completed` events from `marktd` where `makopid`
is in the MSCONS PID set (`mako_edm::domain::MSCONS_PIDS`). On receipt:

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
  │  GET /api/v1/deliveries/{malo_id}           (hot tier — PostgreSQL, ≤ 12 months)
  │  GET /api/v1/archive/olap/{malo_id}          (cold tier — Iceberg V2 Parquet on S3)
  │  GET /api/v1/imbalance/{malo_id}/{year}/{month}
  ├──► invoicd :8280       — MMM imbalance input for selbstausgestellt INVOIC
  ├──► netzbilanzd :8680   — MeterBillingPeriod (HT/NT kwh) for NNE / §14a ToU billing
  └──► ERP / operator dashboard — historical reads and billing data
```

## See Also

- [edmd operator guide](../../docs/edmd.md)
- [mako-edm library](../../crates/mako-edm/) — `MeterDataReceipt`, `TimeSeriesRepository`, MSCONS PID set
- [marktd](../marktd/README.md) — event source
