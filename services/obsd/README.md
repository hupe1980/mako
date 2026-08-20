# obsd — Business-Process Observability daemon

`obsd` projects all `de.mako.*` CloudEvents into a queryable read-model of running and completed MaKo processes. From it: per-PID KPIs, business-Antwortfrist alerts, and the § 7a Abs. 5 EnWG Gleichbehandlung parity evidence.

**Two deadline clocks, never one number.** The *APERAK Frist* is the technical acknowledgement (45 min Strom weekday; Gas next Werktag 12:00 or 3 Werktage) and arrives as `de.mako.aperak.timeout`. The *Antwortfrist* is the business answer (11:00 of the 1. Werktag for a GPKE Anmeldung, 4 Werktage for a Gas Anmeldung, 3/5/7/1 WT for WiM Strom) and is `deadline_at`. They differ by orders of magnitude and fail for different reasons; every report carries them as separate fields.

| Feature | Detail |
|---|---|
| HTTP port | `:8480` |
| Database | PostgreSQL 15+ (single `process_projections` table) |
| Inbound | All `de.mako.*` CloudEvents from `marktd` (wildcard subscriber) |
| REST API | `GET /obs/processes`, `GET /obs/processes/{id}`, `GET /obs/kpis`, `GET /obs/overdue`, `GET /api/v1/audit/gleichbehandlung` |
| MCP | 6 tools + 2 prompts at `/mcp` (see [MCP Tools](#mcp-tools)) |
| Deadlines | `mako-fristen` — obsd computes none of its own; `makod` and `processd` read the same table |
| § 7a Abs. 5 EnWG | `initiator_is_affiliate` on `ProcessProjection` — affiliate vs third-party evidence for the annual Gleichbehandlungsbericht (filed by 31 March) |
| Health | `GET /health/live`, `GET /health/ready` |
| Auth | REST + MCP: OIDC/JWT + Cedar ABAC (dev bypass when no `[oidc]` configured); inbound webhook Standard Webhooks (`webhook-signature`) |

The projection is a CQRS read-model: it holds no authoritative data and is fully rebuildable by replaying the CloudEvent stream from `marktd`.

---

## Quick Start

```bash
OBSD_CONFIG=obsd.toml obsd        # obsd --check is the container HEALTHCHECK probe
```

`obsd` runs on the `mako-service` daemon runner (`mako_service::run::<Obsd>()`), which owns
tracing, the tuned connection pool (`application_name = "obsd"`), migrations, graceful shutdown,
and a real `/health/ready` (bounded `SELECT 1`). Config is TOML with `env:` substitution; the file
path comes from `OBSD_CONFIG`. Log level is `RUST_LOG`; OTLP export is the `[otel]` block.

---

## Configuration

```toml
[http]
addr = "0.0.0.0:8480"

[database]
url       = "env:OBSD_DATABASE_URL"   # postgresql://obsd:secret@db:5432/obsd
pool_size = 10

[identity]
tenant     = "9900357000004"
# All operator MP-IDs for § 7a Abs. 5 EnWG affiliate detection — include both
# Strom (BDEW 99…) and Gas (DVGW 98…) codes for an integrated NB+GNB deployment.
own_mp_ids = ["9900357000004", "9800357000004"]

[marktd]
url     = "http://marktd:8180"
api_key = "env:OBSD_MARKTD_API_KEY"

[webhook]
# Verifies inbound events from marktd (webhook-signature: sha256=…).
inbound_secret = "env:OBSD_INBOUND_SECRET"
# Target + secret for the de.obs.* CloudEvents obsd emits (deadline.approaching,
# stp.parity.alert). When outbound_url is unset the sweep workers do not run.
outbound_url    = "env:OBSD_OUTBOUND_URL"
outbound_secret = "env:OBSD_OUTBOUND_SECRET"

[subscription]
webhook_url   = "http://obsd:8480/webhook"
subscriber_id = "obsd"

# [oidc]                                  # omit for dev mode
# issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
# audience = "api://mako-obsd"
# [otel]
# endpoint = "http://otel-collector:4317"
```

The `[worker]` block tunes the background sweeps (`deadline_sweep_secs` 900, `deadline_warn_hours`
24, `parity_sweep_secs` 86400, `parity_threshold_pp` 5.0, `parity_window_days` 90).

---

## REST API

### `GET /obs/processes`

List process projections with optional filters.

Query parameters:
- `state` — filter by state: `initiated`, `running`, `aperak_timeout`, `completed`, `rejected`, `failed`. An unknown value is a `400` naming the six, not a silently dropped filter
- `family` — filter by process family: `gpke`, `geli-gas`, `wim`, `wim-gas`, `gabi-gas`, `mabis`, `invoic-storno`, `unknown`
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
  "malo_id":       "51238696012",
  "partner_mp_id": "4012345000023",
  "mdm_role":      "LF",
  "deadline_at":   "2025-10-02T08:00:00Z",
  "deadline_risk": "amber",
  "started_at":    "2025-10-01T08:00:00Z",
  "last_event_at": "2025-10-01T08:01:00Z",
  "erc_code":      null
}
```

`deadline_risk` values: `unknown` (no published Antwortfrist for this PID — never `green`, because an unread Festlegung is not headroom), `green` (> 24 h), `amber` (< 24 h), `red` (passed, process still open).

### `GET /obs/kpis`

BNetzA KPI report — response times per PID and period.

Query parameters:
- `pid` — filter to a single PID
- `period` — billing period in `YYYY-MM` format

```bash
curl "http://localhost:8480/obs/kpis?pid=55001&period=2025-10"
```

### `GET /obs/overdue`

All processes where `deadline_at < now()` and the state is still non-terminal (not `completed` / `rejected` / `failed`), ordered by `deadline_at` ascending. `aperak_timeout` rows are included: a counterparty that missed the acknowledgement still owes the business answer. Processes with no published Antwortfrist carry no `deadline_at` and are therefore absent — unknown, never measured against an instant nobody can cite.

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
| `state` | `initiated` / `running` / `aperak_timeout` / `completed` / `rejected` / `failed` |
| `deadline_at` | Business Antwortfrist, from `mako-fristen`. `NULL` = no published window for this PID |
| `deadline_source` | The Festlegung `deadline_at` came from — cite it rather than asserting a number |
| `deadline_risk` | Pre-computed risk level: `unknown` / `green` / `amber` / `red` |
| `erc_code` | ERC error code if process was rejected or disputed |

Indexes cover `(pid, state)`, `(tenant, family, started_at)`, `malo_id`, `partner_mp_id`, `deadline_at`, and `(tenant, started_at DESC)` for efficient KPI aggregation and overdue queries. Both the KPI buckets and the Gleichbehandlungsbericht are keyed on `started_at`: a report grouped by `updated_at` migrates rows between periods as later events touch them, so re-running a closed period yields different numbers.

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
| `de.mako.process.failed` | Set state `failed` |

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
| `list_overdue_processes` | Processes past their business Antwortfrist (most urgent first); each row carries `deadline_source`, and the result reports `saturated` when the cap bit |
| `get_kpi_report` | Per-PID KPIs for a calendar month (`YYYY-MM`). Reports `aperak_timeout` and `frist_breached` as separate clocks; rates are `null` when nothing in the bucket is measurable |
| `get_parity_report` | § 7a Abs. 5 EnWG parity: affiliate vs third-party completion rates over the NB-answered Lieferanten processes. `gap_pp` = affiliate − third_party; positive means the affiliate fared better. `null` below 10 processes per group — unstatable, not zero. No BNetzA threshold exists |
| `get_stp_rate` | Completions over processes that **ended** in the last N days. `aperak_timeout` is not an ending. No regulatory target exists |
| `list_processes_by_family` | List processes by workflow family (`gpke` / `wim` / `geli-gas` / `wim-gas` / `gabi-gas` / `mabis`) |

Two prompts (`process-kpis`, `investigate-overdue-process`) guide agents through a period's KPIs and a missed Antwortfrist. Both keep the two deadline clocks apart, and both say what a `null` rate means: nothing measurable in the bucket, not perfect performance.

## See Also

- [Architecture overview](https://hupe1980.github.io/mako/docs/architecture/)
- [mako-obs library](../../crates/mako-obs/) — `ProcessProjection`, `KpiReport`, `DeadlineRisk`, `ProcessProjectionRepository`
- [marktd](../marktd/README.md) — event source
- [BNetzA regulatory reference](https://hupe1980.github.io/mako/docs/regulatory/bnetza/)
