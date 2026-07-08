---
layout: default
title: Getting Started
nav_order: 2
mermaid: true
description: >
  Run the full mako NB STP demo stack — makod, marktd, processd, and a webhook
  receiver — in under 5 minutes. Submit a UTILMD 55001, watch processd
  auto-accept via netz-checker, and receive the UTILMD 55003 confirmation.
---

# Getting Started

This guide runs the full NB STP demo stack locally and walks through the
complete end-to-end flow: UTILMD 55001 → automatic NB decision → UTILMD 55003.

## What you're running

| Service | Port | Role |
|---|---|---|
| `postgres` | `5432` | Shared state store (marktd + processd schemas) |
| `webhook` | `8000` | Demo ERP event receiver (Python, in-memory) |
| `marktd` | `8180` | Market Data Hub — MaLo/MeLo/contracts/VersorgungsStatus/preisblaetter |
| `processd` | `8580` | NB STP auto-responder — netz-checker decisions, §20 EnWG parity |
| `makod` | `8080` | EDIFACT process engine — GPKE/WiM/GeLi Gas, AS4, SlateDB |

```mermaid
sequenceDiagram
    participant LF as LF 4012345000023
    participant makod as makod :8080
    participant marktd as marktd :8180
    participant processd as processd :8580
    participant webhook as webhook :8000

    LF->>makod: POST /edifact<br/>UTILMD 55001
    makod-->>webhook: APERAK BGM+312<br/>(45-min deadline, auto)
    makod->>marktd: de.mako.process.initiated<br/>HMAC POST /api/v1/mako/events
    marktd->>processd: de.mako.process.initiated<br/>HMAC POST /webhook
    marktd-->>webhook: de.mako.process.initiated<br/>(ERP subscription)
    processd->>marktd: GET /api/v1/versorgung/{malo_id}
    processd->>marktd: GET /api/v1/malo/{malo_id}/grid
    processd->>marktd: GET /api/v1/partners/{lf_mp_id}
    Note over processd: netz-checker: 6 checks → Accept
    processd->>makod: gpke.lieferbeginn.bestaetigen
    makod-->>webhook: UTILMD 55003 Bestätigung
```

Total time: **~5 minutes**.

---

## Prerequisites

| Tool | Version | Install |
|---|---|---|
| Docker | 24+ with Compose v2 | https://docs.docker.com/get-docker/ |
| `curl` | any | OS package manager |
| `jq` | any | OS package manager |

---

## Step 1 — Clone and build

```bash
git clone https://github.com/hupe1980/mako.git
cd mako

# Build all three daemon images (~3–5 min first time, cached on rebuild)
docker build --target runtime          -t makod:dev     .
docker build --target marktd-runtime   -t marktd:dev    .
docker build --target processd-runtime -t processd:dev  .
```

> The `processd-runtime` stage builds with `--features integrated` (includes
> both NB netz-checker and LF E_0624 auto-response modules).

---

## Step 2 — Start the demo stack

```bash
cd demo
docker compose up -d
docker compose ps   # wait until all containers are running
```

Expected:

```
NAME              IMAGE              STATUS         PORTS
demo-postgres-1   postgres:17-alpine Up (healthy)   5432/tcp
demo-webhook-1    python:3.12-alpine Up             0.0.0.0:8000->8000/tcp
demo-marktd-1     marktd:dev         Up             0.0.0.0:8180->8180/tcp
demo-processd-1   processd:dev       Up (healthy)   0.0.0.0:8580->8580/tcp
demo-makod-1      makod:dev          Up             0.0.0.0:8080->8080/tcp
```

**What happens at startup:**  
`processd` self-registers its EventBus subscription with `marktd` on startup via
`PROCESSD_SELF_REGISTER_WEBHOOK_URL` — no manual subscription curl required.
`marktd` and `processd` each run SQLx migrations on their own PostgreSQL database
(`marktd` and `processd` databases in the same instance).

---

## Step 3 — Verify health

```bash
curl -s http://localhost:8080/health | jq .
# → {"status":"ok","instance_id":"..."}

curl -s http://localhost:8180/health | jq .
# → {"status":"ok"}

curl -s http://localhost:8580/health/ready
# → 200 OK
```

---

## Step 4 — Seed master data

`processd`'s netz-checker needs three items in `marktd` to reach an `Accept`
decision.

### 4a — Price sheet

```bash
curl -s -X PUT http://localhost:8180/api/v1/preisblaetter/9900357000004 \
  -H "Content-Type: application/json" \
  --data-binary @demo/fixtures/preisblatt-nb.json \
  -w "\nHTTP %{http_code}\n"
# → HTTP 204
```

### 4b — MaLo + NIS grid record

```bash
MALO_ID=17835382035

# MaLo (NB=9900357000004, no active LF)
curl -s -X PUT "http://localhost:8180/api/v1/malo/$MALO_ID" \
  -H "Content-Type: application/json" \
  --data-binary "$(jq --arg m "$MALO_ID" '.data.marktlokations_id=$m' demo/fixtures/malo-nb.json)" \
  -w "\nHTTP %{http_code}\n"
# → HTTP 201

# NIS grid record (netz-checker check 1)
curl -s -X PUT "http://localhost:8180/api/v1/malo/$MALO_ID/grid" \
  -H "Content-Type: application/json" \
  -d '{"nb_mp_id":"9900357000004","bilanzierungsgebiet":"11YN0------0STXC","netzgebiet":"DEMO-NZ-001","sparte":"STROM","source":"manual"}' \
  -w "\nHTTP %{http_code}\n"
# → HTTP 204
```

### 4c — LF trading partner (netz-checker check 5)

```bash
# Register in marktd partner directory
curl -s -X PUT http://localhost:8180/api/v1/partners/4012345000023 \
  -H "Content-Type: application/json" \
  -d '{"mp_id":"4012345000023","display_name":"Demo LF","marktrolle":"LF","sparte":"STROM","channels":{}}' \
  -w "\nHTTP %{http_code}\n"
# → HTTP 200

# Register in makod for EDIFACT routing
curl -s -X PUT http://localhost:8080/admin/partners/4012345000023 \
  -H "Authorization: Bearer demo-secret-change-me" \
  -H "Content-Type: application/json" \
  --data-binary @demo/fixtures/partner-lf.json | jq '.version'
```

---

## Step 5 — Submit a UTILMD 55001

```bash
curl -s -X POST http://localhost:8080/edifact \
  -H "Authorization: Bearer demo-secret-change-me" \
  -H "Content-Type: text/plain; charset=utf-8" \
  --data-binary @demo/fixtures/utilmd-55001.edi | jq .
```

Expected response:

```json
{
  "accepted": 1,
  "rejected": 0,
  "messages": [{
    "message_type": "UTILMD",
    "pid": 55001,
    "workflow": "gpke-supplier-change",
    "status": "routed",
    "process_id": "...",
    "malo_id": "17835382035"
  }]
}
```

---

## Step 6 — Automatic NB decision

Within ~200 ms, `processd` receives the `de.mako.process.initiated` event from
`marktd`'s EventBus and runs all 6 netz-checker validation checks synchronously.

```bash
# Check the decision log
curl -s http://localhost:8580/api/v1/decisions | jq '.[] | {
  malo_id, decision, erc_code, decided_at
}'
# → {"malo_id":"17835382035","decision":"Accept","erc_code":null,"decided_at":"..."}
```

With `NB_AUTO_ACCEPT=true` (set in the demo compose file), `Accept` automatically
dispatches `gpke.lieferbeginn.bestaetigen` to `makod`, which enqueues the outbound
**UTILMD 55003** (Bestätigung Lieferbeginn):

```bash
curl -s http://localhost:8000/events | jq '[.[] |
  select(.body.makomessagetype=="UTILMD")
  | {edifact: .body.data.edifact}
]'
```

---

## Step 7 — Run the automated smoke test

The demo ships `smoke.sh` which runs all of the above automatically and asserts
every step passes, including the auto-accept timing:

```bash
cd demo
MARKTD_URL=http://localhost:8180 WEBHOOK_URL=http://localhost:8000 bash smoke.sh
```

Output ends with:

```
✓ processd NB auto-responder dispatched bestaetigen → UTILMD 55003 already arrived
✓ POST /api/v1/commands → HTTP 409 (auto-responder already accepted — idempotency confirmed)
All smoke tests passed.
```

---

## Step 8 — Explore the APIs

| Interface | URL |
|---|---|
| makod Swagger UI | http://localhost:8080/api/v1/docs/ |
| makod MCP server | http://localhost:8080/mcp |
| marktd Swagger UI | http://localhost:8180/api/v1/docs/ |
| processd decisions | http://localhost:8580/api/v1/decisions |
| processd approval queue | http://localhost:8580/api/v1/queue |
| Webhook event log | http://localhost:8000/events |

---

## Stop and clean up

```bash
docker compose down      # keep database volumes
docker compose down -v   # destroy all volumes (full reset)
```

---

## Next steps

| Topic | Guide |
|---|---|
| EDIFACT parsing and validation | [Parsing guide](./parsing.md) |
| ERP integration — CloudEvents, HMAC | [ERP integration](./erp-integration.md) |
| makod operator reference | [makod guide](./makod.md) |
| marktd operator reference | [marktd guide](./marktd.md) |
| processd — NB STP + LF E_0624 | [processd guide](./processd.md) |
| INVOIC plausibility, §22 MessZV | [invoicd guide](./invoicd.md) |
| Energy data, imbalance, billing periods | [edmd guide](./edmd.md) |
| Process observability, §20 parity | [obsd guide](./obsd.md) |
| Full system architecture | [Architecture](./architecture.md) |
| Process catalogue (GPKE, WiM, …) | [Processes](./processes.md) |
