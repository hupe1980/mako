# edmd — Energy Data Management daemon

`edmd` stores MSCONS meter readings received from `marktd`, accepts direct iMSys/SMGW interval push, scores data quality with a Hampel filter, schedules reading orders (Ablesesteuerung), and serves BO4E typed time-series and imbalance queries — `Energiemenge` deliveries for ERP billing import, `Lastgang`/`Zeitreihe` for API-Webdienste Strom, `MeterBillingPeriod` for `netzbilanzd`, and Mehr-/Mindermengen reconciliation for `invoicd`. Meter reads older than the configured retention window are automatically offloaded to **Apache Iceberg V2** tables on S3/GCS/Azure for OLAP MMM aggregation.

| Feature | Detail |
|---|---|
| HTTP port | `:8380` |
| Database | PostgreSQL 15+ (sqlx 0.8, `meter_reads` + `meter_billing_periods` + `ablese_auftraege` + `direct_push_sessions` + `archive_batches` tables) |
| Inbound | CloudEvents from `marktd` — `de.mako.process.completed` (MSCONS PIDs), `de.mako.process.initiated` (PID 23001 INSRPT → auto reading order) |
| Direct push | `POST /api/v1/meter-reads/rlm/{malo_id}` (Strom), `POST /api/v1/meter-reads/gas/{malo_id}` (Gas m³→kWh_Hs) — idempotent on `session_id` |
| Quality scoring | Hampel filter (k=3, t=3.0, MAD×1.4826 σ); grades A/B/C/F; retroactive: `POST /api/v1/quality-score/{malo_id}` |
| Reading orders | `POST/GET /api/v1/reading-orders` — Ablesesteuerung for LF/MSB/NB; auto-creates `INSRPT_STOERUNG` on INSRPT PID 23001 (§18 MessZV) |
| REST API | `GET /api/v1/deliveries/{malo_id}` → `Vec<Energiemenge>` · `GET /api/v1/lastgang/{malo_id}` · `GET /api/v1/zeitreihe/{malo_id}` · `GET /api/v1/billing-period/{malo_id}` · `GET /api/v1/imbalance/{malo_id}/{year}/{month}` |
| Archive OLAP | `GET /api/v1/archive/status` · `GET /api/v1/archive/olap/{malo_id}` · `GET /api/v1/archive/portfolio` · `GET /api/v1/archive/timeseries/{malo_id}` |
| Auth | OIDC/JWT + Cedar ABAC (`read-timeseries`, `write-quality-rescore`, `read-archive-olap`); webhook HMAC-SHA256 (`X-Mako-Signature`) |
| Health | `GET /health/live`, `GET /health/ready` (PostgreSQL ping) |
| MCP | `POST\|GET /mcp` — tools: `get_timeseries`, `get_imbalance`, `get_billing_period`, `get_device_history`, `get_quality_warnings` |
| CloudEvents emitted | `de.edmd.reading.direct.stored`, `de.edmd.reading.quality.warning` (grade C/F) |

---

## Quick Start

```bash
edmd \
  --database-url postgres://edmd:secret@localhost/edmd \
  --webhook-url  http://edmd:8380/webhook \
  --marktd-url   http://marktd:8180
```

Migrations run automatically at startup.

---

## Configuration

| Flag | Env var | Default | Description |
|---|---|---|---|
| `--listen` | `EDMD_LISTEN` | `0.0.0.0:8380` | Bind address |
| `--database-url` | `EDMD_DATABASE_URL` | required | PostgreSQL connection string |
| `--marktd-url` | `EDMD_MARKTD_URL` | `http://localhost:8180` | marktd base URL for subscription registration |
| `--subscriber-id` | `EDMD_SUBSCRIBER_ID` | `edmd` | CloudEvents subscriber ID registered with marktd |
| `--webhook-url` | `EDMD_WEBHOOK_URL` | required | Public URL that marktd will POST events to |
| `--webhook-secret` | `EDMD_WEBHOOK_SECRET` | optional | HMAC-SHA256 secret for outbound webhook signatures |
| `--inbound-secret` | `EDMD_INBOUND_SECRET` | optional | HMAC secret for verifying inbound `X-Mako-Signature` headers (falls back to `--webhook-secret`) |
| `--db-pool-size` | `EDMD_DB_POOL_SIZE` | `10` | Max PostgreSQL connections |
| `--log-level` | `EDMD_LOG_LEVEL` | `info` | Log level — overridden by `RUST_LOG` |
| `--otel-endpoint` | `EDMD_OTEL_ENDPOINT` | optional | OTLP gRPC endpoint (e.g. `http://otel-collector:4317`) |

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
iceberg_catalog_schema = "iceberg_catalog"   # PostgreSQL schema — created automatically
iceberg_catalog_name   = "edmd"
```

Table metadata is stored in the same PostgreSQL database (in the `iceberg_catalog` schema) —
no external catalog service (Nessie, Apache Polaris, AWS Glue) required.

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

| Migration | Tables |
|---|---|
| `0001_initial_schema.sql` | `meter_data_receipts` · `meter_reads` · `meter_billing_periods` |
| `0002_archive_tracking.sql` | `archive_batches` · `iceberg_snapshots` · `archived` column on `meter_reads` |

All tables carry `tenant_id UUID` for multi-tenant isolation.

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
