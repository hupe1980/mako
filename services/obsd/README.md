# obsd — Business-Process Observability daemon

`obsd` projects all `de.mako.*` CloudEvents into a queryable read-model of running and completed MaKo processes. It provides BNetzA KPI reports, deadline-risk alerts, and an overdue-process API.

| Feature | Detail |
|---|---|
| HTTP port | `:8480` |
| Database | PostgreSQL 15+ (`process_projections` table) |
| Inbound | All `de.mako.*` CloudEvents from `marktd` (wildcard subscriber) |
| REST API | `GET /obs/processes`, `GET /obs/processes/{id}`, `GET /obs/kpis`, `GET /obs/overdue` |
| Health | `GET /health/live`, `GET /health/ready` |
| Auth | Webhook HMAC-SHA256 (`X-Mako-Signature`); HTTP endpoints currently unauthenticated |

The projection is a CQRS read-model: it holds no authoritative data and is fully rebuildable by replaying the CloudEvent stream from `marktd`.

---

## Quick Start

```bash
obsd \
  --database-url postgres://obsd:secret@localhost/obsd \
  --webhook-url  http://obsd:8480/webhook \
  --marktd-url     http://marktd:8180
```

Migrations run automatically at startup.

---

## Configuration

| Flag | Env var | Default | Description |
|---|---|---|---|
| `--listen` | `OBSD_LISTEN` | `0.0.0.0:8480` | Bind address |
| `--database-url` | `OBSD_DATABASE_URL` | required | PostgreSQL connection string |
| `--marktd-url` | `OBSD_MARKTD_URL` | `http://localhost:8180` | marktd base URL for subscription registration |
| `--subscriber-id` | `OBSD_SUBSCRIBER_ID` | `obsd` | CloudEvents subscriber ID registered with marktd |
| `--webhook-url` | `OBSD_WEBHOOK_URL` | required | Public URL that marktd will POST events to |
| `--webhook-secret` | `OBSD_WEBHOOK_SECRET` | optional | HMAC-SHA256 secret for outbound webhook signatures |
| `--inbound-secret` | `OBSD_INBOUND_SECRET` | optional | HMAC secret for verifying inbound `X-Mako-Signature` headers (falls back to `--webhook-secret`) |
| `--db-pool-size` | `OBSD_DB_POOL_SIZE` | `10` | Max PostgreSQL connections |
| `--log-level` | `OBSD_LOG_LEVEL` | `info` | Log level — overridden by `RUST_LOG` |
| `--otel-endpoint` | `OBSD_OTEL_ENDPOINT` | optional | OTLP gRPC endpoint (e.g. `http://otel-collector:4317`) |

---

## REST API

### `GET /obs/processes`

List process projections with optional filters.

Query parameters:
- `state` — filter by state: `initiated`, `completed`, `timed_out`, `dead_lettered`
- `pid` — filter by BDEW Prüfidentifikator (e.g. `55001`)
- `partner_gln` — filter by counterparty GLN
- `mdm_role` — filter by Marktrollen role of the counterparty
- `since` — ISO 8601 datetime lower bound on `started_at`
- `limit` — max results (default: 100)

```bash
# All running GPKE Lieferbeginn processes
curl "http://localhost:8480/obs/processes?state=initiated&pid=55001"

# Overdue processes (past deadline)
curl "http://localhost:8480/obs/overdue"
```

### `GET /obs/processes/{process_id}`

Get a single process projection.

```bash
curl "http://localhost:8480/obs/processes/018f3a2b-7c4e-7d5f-8a9b-0c1d2e3f4a5b"
```

Response:

```json
{
  "process_id":    "018f3a2b-7c4e-7d5f-8a9b-0c1d2e3f4a5b",
  "pid":           55001,
  "family":        "gpke",
  "workflow_name": "GpkeLfAnmeldungWorkflow",
  "state":         "initiated",
  "malo_id":       "51238696780",
  "partner_gln":   "4012345000023",
  "mdm_role":      "LF",
  "deadline_at":   "2025-10-02T08:00:00Z",
  "deadline_risk": "amber",
  "started_at":    "2025-10-01T08:00:00Z",
  "last_event_at": "2025-10-01T08:01:00Z",
  "erc_code":      null
}
```

`deadline_risk` values: `green` (> 4 h to deadline), `amber` (1–4 h), `red` (< 1 h), `overdue` (past deadline).

### `GET /obs/kpis`

BNetzA KPI report — response times per PID and period.

Query parameters:
- `pid` — filter to a single PID
- `period` — billing period in `YYYY-MM` format

```bash
curl "http://localhost:8480/obs/kpis?pid=55001&period=2025-10"
```

### `GET /obs/overdue`

All processes where `deadline_at < now()` and `state = 'initiated'`.

---

## Database Schema

`obsd` uses a single migration file `migrations/0001_initial_schema.sql`.

| Table | Purpose |
|---|---|
| `process_projections` | One mutable row per MaKo process — state machine projection updated on every `de.mako.*` event |

Key columns:

| Column | Description |
|---|---|
| `process_id` | UUID — primary key and `makod` process identity |
| `pid` | BDEW Prüfidentifikator |
| `state` | `initiated` / `completed` / `timed_out` / `dead_lettered` |
| `deadline_at` | Regulatory response deadline (CET/CEST-aware) |
| `deadline_risk` | Pre-computed risk level: `green` / `amber` / `red` / `overdue` |
| `erc_code` | ERC error code if process was rejected or disputed |

Indexes cover `(pid, state)`, `malo_id`, `partner_gln`, `deadline_at`, and `started_at DESC` for efficient KPI aggregation and overdue queries.

---

## Event Routing

`obsd` subscribes to **all** `de.mako.*` CloudEvents from `marktd` (wildcard subscription). Each event updates the `process_projections` row for the relevant `process_id`:

| Event type | Action |
|---|---|
| `de.mako.process.initiated` | INSERT projection row with state `initiated` |
| `de.mako.process.aperak_sent` | Update `last_event_at` |
| `de.mako.process.completed` | Set state `completed` |
| `de.mako.process.timed_out` | Set state `timed_out`, record `erc_code` |
| `de.mako.process.dead_lettered` | Set state `dead_lettered` |

Projection rows are never deleted — they provide the historical view used by BNetzA KPI reports.

---

## Relationship to Other Services

```
marktd :8180
  │  POST /webhook  (all de.mako.* CloudEvents)
  ▼
obsd :8480
  │  GET /obs/processes      — ERP / operator dashboard
  │  GET /obs/kpis           — BNetzA KPI report
  │  GET /obs/overdue        — deadline alert feed
  ▼
Alertmanager / Grafana / ERP system
```

The projection is fully rebuildable by replaying the CloudEvent history from `marktd`.

## See Also

- [Architecture overview](../../docs/architecture.md)
- [mako-obs library](../../crates/mako-obs/) — `ProcessProjection`, `KpiReport`, `DeadlineRisk`, `ProcessProjectionRepository`
- [marktd](../marktd/README.md) — event source
- [BNetzA regulatory reference](../../docs/bnetza.md)
