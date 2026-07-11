# edmd — Energy Data Management daemon

`edmd` stores MSCONS meter readings received from `marktd` and serves BO4E typed time-series and imbalance queries — `Energiemenge` deliveries for ERP billing import, `Lastgang`/`Zeitreihe` for API-Webdienste Strom, `MeterBillingPeriod` for `netzbilanzd`, and Mehr-/Mindermengen reconciliation for `invoicd`.

| Feature | Detail |
|---|---|
| HTTP port | `:8380` |
| Database | PostgreSQL 15+ (sqlx 0.8, `meter_reads` + `meter_billing_periods` tables) |
| Inbound | CloudEvents from `marktd` — `de.mako.process.completed` where `makopid` ∈ MSCONS PID set |
| REST API | `GET /api/v1/deliveries/{malo_id}` → `Vec<Energiemenge>` (BO4E typed) · `GET /api/v1/lastgang/{malo_id}` · `GET /api/v1/zeitreihe/{malo_id}` · `GET /api/v1/billing-period/{malo_id}` · `GET /api/v1/imbalance/{malo_id}/{year}/{month}` |
| Auth | OIDC/JWT + Cedar ABAC (`read-timeseries` action); webhook HMAC-SHA256 (`X-Mako-Signature`) |
| Health | `GET /health/live`, `GET /health/ready` (PostgreSQL ping) |
| MCP | `POST|GET /mcp` — MCP Streamable HTTP (LLM tooling) |

---

## Quick Start

```bash
edmd \
  --database-url postgres://edmd:secret@localhost/edmd \
  --webhook-url  http://edmd:8380/webhook \
  --marktd-url     http://marktd:8180
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

---

## REST API

### `GET /api/v1/deliveries/{malo_id}`

Returns all typed meter reads for a Marktlokation within the given time range.

Query parameters:
- `from` — ISO 8601 datetime (inclusive), defaults to Unix epoch
- `to` — ISO 8601 datetime (exclusive), defaults to now

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

### `GET /api/v1/imbalance/{malo_id}/{year}/{month}`

Returns the Mehr-/Mindermengen imbalance report for a single billing month.

```bash
curl "http://localhost:8380/api/v1/imbalance/51238696780/2025/10"
```

Response:

```json
{
  "malo_id":       "51238696780",
  "year":          2025,
  "month":         10,
  "mehr_kwh":      "42.0",
  "minder_kwh":    "0.0",
  "total_reads":   744
}
```

---

## Database Schema

`edmd` uses a single migration file `migrations/0001_initial_schema.sql`.

| Table | Purpose |
|---|---|
| `meter_data_receipts` | One row per received MSCONS process — process-level metadata (idempotency key: `process_id`) |
| `meter_reads` | Typed kWh interval reads; primary key `(malo_id, dtm_from, dtm_to)` |

Both tables carry `tenant_id UUID` for multi-tenant isolation. The `meter_reads` table can optionally be converted to a TimescaleDB hypertable on `dtm_from` for time-series performance at scale.

---

## Event Routing

`edmd` subscribes to `de.mako.process.completed` events from `marktd` where the `makopid` field is in the MSCONS PID set (`mako_edm::domain::MSCONS_PIDS`). All other events return `204 No Content` immediately.

On receipt, `edmd`:
1. Verifies the `X-Mako-Signature` HMAC (if configured)
2. Parses the `data` field into a `MeterDataReceipt`
3. Upserts the receipt row (idempotent on `process_id`)
4. Stores the typed reads from the receipt payload

---

## Relationship to Other Services

```
marktd :8180
  │  POST /webhook  (de.mako.process.completed · MSCONS PIDs)
  ▼
edmd :8380
  │  GET /api/v1/deliveries/{malo_id}
  │  GET /api/v1/imbalance/{malo_id}/{year}/{month}
  ├──► invoicd :8280  — MMM Mehr-/Mindermengen imbalance input for selbstausgestellt INVOIC
  └──► ERP / operator dashboard — historical meter reads and billing data
```

## See Also

- [edmd operator guide](../../docs/architecture.md)
- [mako-edm library](../../crates/mako-edm/) — `MeterDataReceipt`, `TimeSeriesRepository`, MSCONS PID set
- [marktd](../marktd/README.md) — event source
