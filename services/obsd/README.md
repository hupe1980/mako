# obsd — Business-Process Observability daemon

`obsd` projects all `de.mako.*` CloudEvents into a queryable read-model of running and completed MaKo processes. It provides BNetzA KPI reports, deadline-risk alerts, and an overdue-process API.

| Feature | Detail |
|---|---|
| HTTP port | `:8480` |
| Database | PostgreSQL 15+ (single `process_projections` table) |
| Inbound | All `de.mako.*` CloudEvents from `marktd` (wildcard subscriber) |
| REST API | `GET /obs/processes`, `GET /obs/processes/{id}`, `GET /obs/kpis`, `GET /obs/overdue`, `GET /api/v1/audit/bnetza-report` |
| MCP | 6 tools + 2 prompts at `/mcp` (see [MCP Tools](#mcp-tools)) |
| §20 EnWG | `initiator_is_affiliate` flag on `ProcessProjection` — affiliate vs. non-affiliate STP parity for BNetzA audit |
| Health | `GET /health/live`, `GET /health/ready` |
| Auth | REST + MCP: OIDC/JWT + Cedar ABAC (dev bypass when no `[oidc]` configured); inbound webhook HMAC-SHA256 (`X-Mako-Signature`) |

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
- `state` — filter by state: `initiated`, `running`, `completed`, `rejected`, `cancelled`, `aperak_timeout`
- `pid` — filter by BDEW Prüfidentifikator (e.g. `55001`)
- `partner_mp_id` — filter by counterparty MP-ID (BDEW Codenummer / GLN)
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
  "partner_mp_id": "4012345000023",
  "mdm_role":      "LF",
  "deadline_at":   "2025-10-02T08:00:00Z",
  "deadline_risk": "amber",
  "started_at":    "2025-10-01T08:00:00Z",
  "last_event_at": "2025-10-01T08:01:00Z",
  "erc_code":      null
}
```

`deadline_risk` values: `green` (> 24 h to deadline), `amber` (< 24 h to deadline), `red` (deadline passed, process still open).

### `GET /obs/kpis`

BNetzA KPI report — response times per PID and period.

Query parameters:
- `pid` — filter to a single PID
- `period` — billing period in `YYYY-MM` format

```bash
curl "http://localhost:8480/obs/kpis?pid=55001&period=2025-10"
```

### `GET /obs/overdue`

All processes where `deadline_at < now()` and the state is still non-terminal (not `completed` / `rejected` / `cancelled`), ordered by `deadline_at` ascending.

---

## Database Schema

`obsd` uses a single schema file `migrations/0001_schema.sql`.

| Table | Purpose |
|---|---|
| `process_projections` | One mutable row per MaKo process — state machine projection updated on every `de.mako.*` event |

Key columns:

| Column | Description |
|---|---|
| `process_id` | UUID — primary key and `makod` process identity |
| `pid` | BDEW Prüfidentifikator |
| `state` | `initiated` / `running` / `completed` / `rejected` / `cancelled` / `aperak_timeout` |
| `deadline_at` | Regulatory response deadline (CET/CEST-aware) |
| `deadline_risk` | Pre-computed risk level: `green` / `amber` / `red` |
| `erc_code` | ERC error code if process was rejected or disputed |

Indexes cover `(pid, state)`, `malo_id`, `partner_gln`, `deadline_at`, and `started_at DESC` for efficient KPI aggregation and overdue queries.

---

## Event Routing

`obsd` subscribes to **all** `de.mako.*` CloudEvents from `marktd` (wildcard subscription). Each event updates the `process_projections` row for the relevant `process_id`:

| Event type | Action |
|---|---|
| `de.mako.process.initiated` | INSERT projection row with state `initiated` |
| `de.mako.aperak.accepted` | Set state `running` |
| `de.mako.aperak.rejected` | Set state `rejected`, record `erc_code` |
| `de.mako.aperak.timeout` | Set state `aperak_timeout` |
| `de.mako.process.completed` | Set state `completed` |
| `de.mako.process.failed` | Set state `cancelled` |

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

---

## MCP Tools

`obsd` exposes the read-model over MCP at `/mcp` (streamable HTTP) for the `agentd` specialists (`compliance-agent`, `processd-agent`, `deadline-alert-agent`, …). Access is gated by the same OIDC/JWT + Cedar ABAC as the REST API.

| Tool | Description |
|---|---|
| `get_process` | Read a process projection by UUID |
| `list_overdue_processes` | List MaKo processes past their regulatory deadline (most urgent first) |
| `get_kpi_report` | BNetzA KPI report for a PID and billing month (`YYYY-MM`) |
| `get_parity_report` | §20 EnWG parity: affiliate vs. non-affiliate completion rates for Lieferbeginn PIDs |
| `get_stp_rate` | Rolling STP rate across all process families for the last N days |
| `list_processes_by_family` | List processes by workflow family (`gpke` / `wim` / `geli-gas` / `wim-gas` / `gabi-gas` / `mabis`) |

Two prompts (`audit-kpi`, `investigate-aperak-violation`) guide agents through KPI audits and APERAK deadline investigations.

## See Also

- [Architecture overview](../../docs/architecture.md)
- [mako-obs library](../../crates/mako-obs/) — `ProcessProjection`, `KpiReport`, `DeadlineRisk`, `ProcessProjectionRepository`
- [marktd](../marktd/README.md) — event source
- [BNetzA regulatory reference](../../docs/bnetza.md)
