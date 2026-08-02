+++
title = "obsd Operator Guide"
description = "obsd operator guide: Business-process observability daemon. Process projections, BNetzA KPI reports (§20 EnWG parity, deadline monitoring), Alertmanager bridge, overdue-process detection. PostgreSQL-backed, OIDC-secured."
weight = 29
[extra]
mermaid = true
+++
# `obsd` Operator Guide

`obsd` is the **business-process observability daemon** — the service that tracks
every active MaKo process, monitors regulatory deadlines, and produces the
BNetzA-mandated §20 EnWG parity reports.

Key responsibilities:
- Build and maintain **`ProcessProjection`** records from `de.mako.process.*` events.
- Detect and report **overdue processes** (approaching or past regulatory deadline).
- Produce **KPI reports** for BNetzA audit — decision times, affiliate/non-affiliate
  parity (`initiator_is_affiliate`), STP rates.
- Bridge to **Alertmanager** for operational alerting on deadline violations.

```mermaid
graph TB
    marktd["marktd :8180\nEventBus"]
    obsd["obsd :8480\n(this service)"]
    pg["PostgreSQL\nprocess_projections"]
    alert["Alertmanager /\nGrafana"]
    erp["ERP / BNetzA report"]

    marktd -->|"de.mako.process.*\nde.mako.aperak.*\nHMAC POST /webhook"| obsd
    obsd --> pg
    obsd -->|"de.obs.deadline.approaching\nde.obs.stp.parity.alert\n(HMAC POST → marktd fan-out → agentd)"| marktd
    obsd -->|"deadline_risk\nalerts"| alert
    erp -->|"GET /obs/processes\nGET /obs/kpis\nGET /obs/overdue"| obsd
```

---

## Port layout

```
┌─────────────────────────────────────────────────────────────────┐
│  obsd  :8480                                                     │
│                                                                 │
│  POST /webhook                  ← marktd CloudEvents (HMAC)    │
│  GET  /obs/processes            ← list / filter projections     │
│  GET  /obs/processes/{id}       ← single process by UUID        │
│  GET  /obs/kpis                 ← BNetzA KPI report (per PID)   │
│  GET  /obs/overdue              ← processes past deadline        │
│  GET  /api/v1/audit/bnetza-report ← §20 Abs.1 EnWG annual audit │
│  GET  /obs/metrics              ← obsd business gauges (Prom)   │
│  GET  /metrics                  ← request metrics (SDK)         │
│  GET  /health/live  /health/ready ← liveness + real DB-ping    │
│  POST|GET /mcp                  ← MCP Streamable HTTP           │
└─────────────────────────────────────────────────────────────────┘
```

---

## ProcessProjection

Each `ProcessProjection` record is a read-model built from the event stream:

| Field | Description |
|-------|-------------|
| `process_id` | UUID from `de.mako.process.initiated` |
| `pid` | BDEW Prüfidentifikator (e.g. 55001) |
| `family` | Process family: `gpke`, `geli-gas`, `wim`, `wim-gas`, `gabi-gas`, `mabis`, `unknown` |
| `workflow_name` | Workflow name from `makoworkflow` CE extension |
| `state` | `initiated` \| `running` \| `completed` \| `rejected` \| `cancelled` \| `aperak_timeout` |
| `malo_id` | 11-digit Marktlokations-ID |
| `partner_mp_id` | GLN of the counterparty (NB/GNB/MSB) |
| `mdm_role` | Canonical Marktrolle (`LF`, `NB`, …) |
| `started_at` | UTC timestamp of the first `process.initiated` event |
| `last_event_at` | UTC timestamp of the most recently received event |
| `completed_at` | Set when state transitions to `completed`, `rejected`, or `cancelled`; used for cycle-time KPIs |
| `deadline_at` | Regulatory deadline, computed from PID on `Initiated` event (see below) |
| `deadline_risk` | `green` \| `amber` (< 24 h) \| `red` (past deadline) |
| `erc_code` | BDEW ERC error code when `state == rejected` |
| `initiator_is_affiliate` | `true` if initiating LF MP-ID ∈ operator's `own_mp_ids` (§20 parity flag) |

---

## Deadline computation

`obsd` computes `deadline_at` **automatically** when processing `de.mako.process.initiated` events.
Deadlines are derived from the PID using conservative calendar-day approximations:

| Process family | Deadline | Regulatory source |
|---|---|---|
| GPKE (PIDs 55001–55609) | **24 wall-clock hours** | BK6-22-024 §5 |
| WiM Strom (PIDs 55039, 55042, 55051, 55168) | **7 calendar days** (≥ 5 Werktage) | BK6-24-174 |
| GeLi Gas (PIDs 44001–44024) | **14 calendar days** (≥ 10 Werktage) | BK7-24-01-009 §5 |
| WiM Gas (PIDs 44039–44053, 44168–44170) | **14 calendar days** (≥ 10 Werktage) | BK7-24-01-009 §5 |
| MABIS (PID 13003) | **2 calendar days** (≥ 1 Werktag) | BK6-24-174 §13.8 |
| Billing / PARTIN / INSRPT PIDs | `null` (no per-process deadline) | — |

> **Conservative approximations:** 7 calendar days ≥ 5 Werktage in all cases
> (Saturdays, Sundays and public holidays are not Werktage).
> `obsd` therefore never marks a process as overdue before its true BNetzA deadline.
> Exact Werktage arithmetic lives in `processd`/`mako-engine`; `obsd` uses the coarser
> approximation for alerting.

`deadline_risk` is re-classified on every event:
- `green` — more than 24 h before deadline
- `amber` — less than 24 h before deadline
- `red` — deadline has passed and process is still open

---

## §20 EnWG parity

`processd` and `obsd` together implement the **§20 EnWG Diskriminierungsfreiheitspflicht**
(non-discrimination obligation) for vertically integrated utilities operating both NB
and LF roles (§6b EnWG deployment).

### How it works

When a Lieferbeginn Anmeldung (PID 55001, 55016, or 44001) arrives, `processd` computes:

```rust
initiator_is_affiliate = new_supplier_mp_id ∈ own_mp_ids
```

- `own_mp_ids` is a `Vec<String>` configured per service instance — covering
  **all** operator MP-IDs (Strom NB `99…` and Gas GNB `98…` for integrated Stadtwerk deployments).
- Falls back to `[tenant]` when `own_mp_ids` is not explicitly configured.

`processd` **blocks automatic acceptance** (`auto_accept = false` is enforced) when
`initiator_is_affiliate = true`, forcing operator review for all affiliate Anmeldungen.
This ensures the NB cannot give its subsidiary LF an automatic processing advantage.

`obsd` records `initiator_is_affiliate` on every `ProcessProjection`, enabling
BNetzA audit evidence as a structured query:

```bash
# §20 EnWG parity audit: affiliate vs. non-affiliate STP rates
curl -s "http://obsd:8480/obs/kpis?days=90" \
  -H "Authorization: Bearer <token>" | jq '{
    affiliate_stp_rate:     .affiliate.stp_rate,
    non_affiliate_stp_rate: .non_affiliate.stp_rate,
    parity_delta:           (.affiliate.stp_rate - .non_affiliate.stp_rate | fabs),
    bnetza_limit_pp:        2.0
  }'
```

BNetzA expects the parity delta to be **< 2 percentage points**.

### Multi-MP-ID configuration

An integrated NB+GNB instance operates under multiple MP-IDs. Configure all of them:

```toml
[identity]
tenant     = "9900357000004"   # primary (for Cedar resource checks)
# §20 EnWG: list all operator MP-IDs — Strom NB (BDEW 99…) + Gas GNB (DVGW 98…)
own_mp_ids = ["9900357000004", "9800357000004"]
```

When `own_mp_ids` is omitted, it defaults to `[tenant]` (pure single-role deployments).

---

## Deadline monitoring

`obsd` monitors regulatory deadlines:

| PID family | Deadline |
|------------|---------|
| GPKE (55001–55018) | 24 wall-clock hours |
| WiM Strom (55039…) | 5 Werktage |
| GeLi Gas (44001…) | 10 Werktage |
| MABIS (13003) | 1 Werktag |

Processes approaching the deadline within a configurable window (`WARNING`) or
past it (`BREACH`) appear in `GET /obs/overdue`:

```bash
curl -s "http://obsd:8480/obs/overdue" \
  -H "Authorization: Bearer <token>" | jq '.[] | {
    process_id, pid, malo_id, deadline_at, deadline_risk
  }'
```

---

## Events produced

`obsd` is a read-model, but two background **sweep workers** produce `de.obs.*`
CloudEvents. They run only when `webhook.outbound_url` is configured (in
production, the `marktd` event-ingest endpoint, whose fan-out delivers to the
`agentd` subscribers). Events are HMAC-signed when `webhook.outbound_secret` is
set.

| Event | Producer | When | Consumed by |
|-------|----------|------|-------------|
| `de.obs.deadline.approaching` | deadline sweep (`deadline_sweep_secs`) | a tracked, still-open process has `deadline_at` within `deadline_warn_hours` and has not been alerted yet (idempotent per process via `deadline_alerted_at`) | agentd `deadline-alert-agent` |
| `de.obs.stp.parity.alert` | parity sweep (`parity_sweep_secs`) | the §20 EnWG completion-rate gap between affiliate- and non-affiliate-initiated Anmeldungen (55001/55016/44001) exceeds `parity_threshold_pp` (both groups ≥ 10 samples) | agentd `compliance-agent` |

**`de.obs.deadline.approaching` payload:** `process_id`, `pid`, `family`,
`workflow_name`, `malo_id`, `partner_mp_id`, `due_at` (RFC 3339),
`hours_remaining`, `deadline_risk`, `tenant`.

**`de.obs.stp.parity.alert` payload:** `tenant`, `window_days`, `threshold_pp`,
`affiliate` + `non_affiliate` `{total, completed, completion_rate}`,
`parity_gap_pp` (signed — positive = affiliate favoured), `favored`, `note`.

---

## Configuration reference

`obsd` reads its configuration from a **TOML file** (default: `obsd.toml`),
with secrets deferred to environment variables via `"env:VAR_NAME"` values.

The config file is located via `OBSD_CONFIG` (an absolute or relative path), or
`./obsd.toml` in the working directory when that variable is unset. Individual
keys may be overridden by `OBSD_`-prefixed environment variables (`__` separates
nested sections, e.g. `OBSD_DATABASE__URL`); `RUST_LOG` sets the log level.

```bash
OBSD_CONFIG=/etc/obsd/obsd.toml obsd
```

### Full `obsd.toml` reference

```toml
[http]
addr = "0.0.0.0:8480"          # default

[database]
url       = "env:DATABASE_URL"  # required; use env: for secrets
pool_size = 10                  # default

[identity]
tenant = "9900357000004"        # required — MP-ID of the operator

[marktd]
url     = "http://marktd:8180"      # required
api_key = "env:OBSD_MARKTD_API_KEY" # required

[webhook]
inbound_secret  = "env:OBSD_INBOUND_SECRET"   # optional; verifies inbound POST /webhook
# Outbound target for the de.obs.* events obsd produces. In production this is
# the marktd event-ingest endpoint, whose fan-out delivers to agentd. Omit to
# disable the sweep producers.
outbound_url    = "env:OBSD_OUTBOUND_URL"      # e.g. http://marktd:8180/api/v1/mako/events
outbound_secret = "env:OBSD_OUTBOUND_SECRET"   # HMAC; must match the target's inbound secret

[worker]
deadline_sweep_secs = 900     # deadline sweep interval (default 15 min)
deadline_warn_hours = 24      # alert when a deadline is within this many hours
parity_sweep_secs   = 86400   # §20 parity sweep interval (default daily)
parity_threshold_pp = 5.0     # parity-gap threshold in pp (BNetzA scrutiny)
parity_window_days  = 90      # parity look-back window

[subscription]
# Self-registers with marktd on startup — no manual curl required.
webhook_url   = "http://obsd:8480/webhook"  # public URL marktd POSTs to
subscriber_id = "obsd"                       # default
event_types   = [
  "de.mako.process.initiated",
  "de.mako.process.completed",
  "de.mako.aperak.timeout",
  "de.mako.process.failed",
  "de.mako.aperak.rejected",
]

# [oidc]          # omit to disable auth (dev only — never omit in production)
# issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
# audience = "api://mako-obsd"
# jwks_refresh_secs = 300

# [otel]          # omit to disable tracing
# endpoint = "http://otel-collector:4317"
```

---

## marktd subscription

`obsd` **auto-registers** its EventBus subscription with `marktd` on startup
when `subscription.webhook_url` is set in the config — no manual `curl` required.

To force re-registration or verify the subscription:

```bash
curl -s http://marktd:8180/api/v1/subscriptions/obsd \
  -H "Authorization: Bearer <token>" | jq .
```

---

## Query examples

```bash
# Open processes for a MaLo
curl -s "http://obsd:8480/obs/processes?state=Open&pid=55001" \
  -H "Authorization: Bearer <token>" | jq '.[] | {process_id, initiated_at, deadline_at}'

# 90-day KPI report
curl -s "http://obsd:8480/obs/kpis?days=90" \
  -H "Authorization: Bearer <token>" | jq .

# Overdue processes (deadline breached or within 2 hours)
curl -s "http://obsd:8480/obs/overdue" \
  -H "Authorization: Bearer <token>" | jq '.[] | select(.deadline_risk == "Breach")'
```

---

## Alertmanager integration

`obsd` can fire Alertmanager webhook alerts when processes breach their deadline.
Configure the Alertmanager webhook receiver URL via environment:

```bash
OBSD_ALERTMANAGER_URL=http://alertmanager:9093/api/v2/alerts
```

Alert labels include `pid`, `workflow`, `malo_id`, and `deadline_risk`.

---

## Monitoring (self-monitoring)

| Metric | Target |
|--------|--------|
| Projection build lag | < 5 s from `ProcessInitiated` |
| `deadline_risk = 'Breach'` count | 0 |
| `initiator_is_affiliate` parity delta | < 2 pp |
| DB pool utilisation | < 80 % |

The `obsd` `GET /obs/kpis` endpoint is also the input for BNetzA audit submissions
under §20 EnWG — export as JSON or CSV before each annual report.

---

## MCP server

`obsd` exposes an MCP server at `/mcp` for LLM-based compliance automation.

### Tools (6)

| Tool | Description |
|---|---|
| `get_process(process_id)` | Full process projection by UUID — state, PIDs, deadlines, ERC code |
| `list_overdue_processes` | All MaKo processes past their regulatory deadline, ordered by urgency |
| `get_kpi_report(pid, period)` | BNetzA KPI for a single PID and billing month (`YYYY-MM`) |
| `get_parity_report(days)` | §20 EnWG compliance: affiliate vs. non-affiliate completion rates; target gap < 2 pp |
| `get_stp_rate(days)` | Rolling STP rate across all process families; target ≥ 95% |
| `list_processes_by_family(family, state, limit)` | Drill into a process family (`gpke`, `wim`, `geli-gas`, …) |

### Prompts (2)

| Prompt | Description |
|---|---|
| `audit-kpi` | Generate a BNetzA KPI report for a reporting period |
| `investigate-aperak-violation` | Root-cause an APERAK deadline violation |

### Example: rolling STP rate check

```bash
curl -X POST http://obsd:8480/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_stp_rate","arguments":{"days":30}}}'
```

Returns `{ "stp_rate": 0.9720, "stp_pct": 97.2, "target_stp": 0.95, "compliant": true, ... }`.
