---
layout: default
title: invoicd Operator Guide
nav_order: 27
parent: Architecture
mermaid: true
description: >
  invoicd operator guide: INVOIC plausibility-check daemon (LF role). Subscribes
  to marktd CloudEvents, runs invoic-checker against marktd price sheets, auto-settles
  or disputes, persists receipts to PostgreSQL for §22 MessZV retention.
---

# `invoicd` Operator Guide

`invoicd` is the **INVOIC plausibility-check daemon** for the LF (Lieferant) role.
It subscribes to `marktd`'s EventBus, receives inbound INVOIC events, and:

1. Fetches the `PreisblattNetznutzung` from `marktd`.
2. Runs **5 deterministic checks** via `invoic-checker`.
3. Auto-settles (REMADV 33001) or disputes (COMDIS 29001 / REMADV 33002).
4. Persists every receipt to PostgreSQL for the **3-year §22 MessZV** audit trail.

```mermaid
graph TB
    marktd["marktd :8180\nEventBus"]
    invoicd["invoicd :8280\n(this service)"]
    makod["makod :8080"]
    pg["PostgreSQL\ninvoic_receipts\n(§22 MessZV, 3y)"]

    marktd -->|"de.mako.process.initiated\n(PID 31001/31002/31005/31006)\nHMAC POST /webhook"| invoicd
    invoicd -->|"GET /api/v1/preisblaetter/{nb_mp_id}"| marktd
    invoicd -->|"Persist receipt\n(before dispatch)"| pg
    invoicd -->|"REMADV 33001 annehmen\nCOMDIS 29001 ablehnen\nREMADV 33002 ablehnen"| makod
```

---

## Port layout

```
┌─────────────────────────────────────────────────────────────────┐
│  invoicd  :8280                                                  │
│                                                                 │
│  POST /webhook                      ← marktd CloudEvents        │
│  GET  /api/v1/receipts              ← INVOIC receipt ledger     │
│  GET  /api/v1/receipts/{id}         ← single receipt by UUID    │
│  GET  /api/v1/disputes              ← open disputes             │
│  GET  /api/v1/overdue-remadv        ← receipts near pay_by      │
│  POST /api/v1/selbstausstellen/{malo_id} ← LF selbstausgestellt │
│  GET  /health/live  /health/ready                               │
│  POST|GET /mcp      ← MCP Streamable HTTP (LLM tooling)         │
└─────────────────────────────────────────────────────────────────┘
```

---

## Handled PIDs

| PID | Description | Direction |
|-----|-------------|-----------|
| 31001 | MMM-Rechnung Strom (NB → LF) | Inbound |
| 31002 | MMM-selbst ausgest. Rechnung (LF → LF) | Inbound |
| 31005 | NNE-Rechnung Strom (NB → LF) | Inbound |
| 31006 | NNE-selbst ausgest. Rechnung (LF) | Inbound + outbound |

PIDs 31003, 31004, 31009, 31011 belong to WiM Gas / GeLi Gas billing workflows
whose `ProcessInitiated` payload does not embed a `Rechnung` BO4E object — they
are routed to specialist handlers and are **not** processed by `invoicd`.

---

## invoic-checker — 5 plausibility checks

| # | Check | Outcome on failure |
|---|-------|--------------------|
| 1 | Billing period validity (DTM+163/DTM+164 in scope) | `Dispute` |
| 2 | Position arithmetic (unit price × quantity = line net) | `Dispute` |
| 3 | Document total (sum of positions = INVOIC total) | `Dispute` |
| 4 | Tariff unit price within tolerance of `PreisblattNetznutzung` | `Warn` or `Dispute` |
| 5 | Tariff entry found in `PreisblattNetznutzung` | `Warn` or `Dispute` |

`Warn` outcomes auto-approve when the total net invoice is below
`INVOICD_AUTO_DISPUTE_THRESHOLD_EUR_CENTS`. Set this to `0` to always approve
warnings (default: approve all warnings).

---

## Idempotency and §22 MessZV

`invoicd` writes each receipt to PostgreSQL **before** dispatching any command
to `makod`. The `invoic_receipts` table has a `UNIQUE (process_id)` constraint,
so re-delivery of the same `de.mako.process.initiated` event is a no-op.

Receipts must be retained for **3 years** (§22 MessZV / §41 EnWG).
The `received_at` column drives the retention query:

```sql
-- Receipts eligible for deletion (> 3 years old):
SELECT * FROM invoic_receipts
WHERE received_at < now() - INTERVAL '3 years';
```

---

## Configuration reference

`invoicd` reads its configuration from a **TOML file** (default: `invoicd.toml`),
with secrets deferred to environment variables via `"env:VAR_NAME"` values.

```bash
invoicd --config /etc/invoicd/invoicd.toml
# or: INVOICD_CONFIG=/etc/invoicd/invoicd.toml invoicd
```

### Full `invoicd.toml` reference

```toml
[http]
addr = "0.0.0.0:8280"          # default

[database]
# Required for §22 MessZV 3-year receipt retention.
url             = "env:DATABASE_URL"   # required; use env: for secrets
max_connections = 5                    # default

[identity]
tenant = "9900357000004"               # required — MP-ID of the operator

[makod]
url     = "http://makod:8080"          # required
api_key = "env:INVOICD_MAKOD_API_KEY" # required

[marktd]
url     = "http://marktd:8180"            # required
api_key = "env:INVOICD_MARKTD_API_KEY"   # required

[webhook]
inbound_secret = "env:INVOICD_INBOUND_SECRET"  # optional; omit for dev

[subscription]
# Self-registers with marktd on startup — no manual curl required.
webhook_url   = "http://invoicd:8280/webhook"  # public URL marktd POSTs to
subscriber_id = "invoicd"                        # default
event_types   = [
  "de.mako.process.initiated",
  "de.mako.process.completed",
]

[check]
# Relative tolerances for invoic-checker plausibility pipeline.
arithmetic_tolerance       = 0.01   # 1 % — qty × price = line net
total_tolerance            = 0.01   # 1 % — Σ line nets = Gesamtnetto
tariff_tolerance           = 0.03   # 3 % — PRICAT unit price vs INVOIC
require_tariff             = false  # true → missing tariff escalates to Dispute
auto_dispute_threshold_eur = 0.0    # 0.0 → Warn always auto-approved

# [oidc]          # omit to disable auth (dev only — never omit in production)
# issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
# audience = "api://mako-invoicd"
# jwks_refresh_secs = 300

# [otel]          # omit to disable tracing
# endpoint = "http://otel-collector:4317"
```

---

## marktd subscription

`invoicd` **auto-registers** its EventBus subscription with `marktd` on startup
when `subscription.webhook_url` is set in the config — no manual `curl` required.

To force re-registration or verify the subscription:

```bash
curl -s http://marktd:8180/api/v1/subscriptions/invoicd \
  -H "Authorization: Bearer <token>" | jq .
```

---

## LF selbstausgestellt INVOIC (PID 31006)

When the LF issues the invoice itself (§20 MessZV selbstausgestellt), trigger via:

```bash
curl -X POST http://invoicd:8280/api/v1/selbstausstellen/10001234567 \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "nb_mp_id": "9900000000002",
    "period_from": "2026-01-01",
    "period_to":   "2026-03-31"
  }'
```

`invoicd` dispatches `gpke.abrechnung.selbstausstellen` to `makod`, which generates
and enqueues the outbound INVOIC 31006 for AS4 delivery to the NB.

---

## Monitoring

| Query / metric | Target |
|----------------|--------|
| `outcome IN ('Ok','AcceptedPartial','Warn')` rate | > 95 % |
| `outcome = 'Dispute'` count | < 1 % of volume |
| `pay_by < now() + INTERVAL '3 days' AND dispatched_at IS NULL` | 0 |

Alert when receipts approach `pay_by` without a `dispatched_at` — the NB may
not have received the REMADV and will begin a dispute window.

---

## Schema

```sql
-- invoic_receipts (§22 MessZV, 3-year retention)
SELECT
  process_id,    -- UUID, unique business key
  pid,           -- 31001 | 31002 | 31005 | 31006
  direction,     -- 'Inbound' | 'Outbound'
  sender_mp_id,  -- NB/MSB MP-ID
  outcome,       -- 'Ok' | 'AcceptedPartial' | 'Warn' | 'Dispute' | 'Dispatched' | 'Paid'
  pay_by,        -- Zahlungsziel from INVOIC DTM+92
  received_at,   -- first ingest timestamp
  dispatched_at  -- when REMADV/COMDIS was sent
FROM invoic_receipts
WHERE tenant = 'your-tenant-gln';
```
