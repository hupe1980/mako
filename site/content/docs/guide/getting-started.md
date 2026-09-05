+++
title = "Getting Started"
description = "Run the mako NB straight-through-processing demo in about five minutes: submit a UTILMD 55001 Anmeldung and receive the 55002 confirmation back."
weight = 2
+++
This guide runs the full NB STP demo stack locally and walks through the
complete end-to-end flow: UTILMD 55001 → automatic NB decision → UTILMD 55002.

## What you're running

| Service | Port | Role |
|---|---|---|
| `postgres` | `5432` | PostgreSQL — one database per service |
| `webhook` | `8000` | Demo ERP event receiver (Python, in-memory) |
| `marktd` | `8180` | Market Data Hub — MaLo/MeLo/NeLo/TR, VersorgungsStatus, durable fan-out, `event_log` replay |
| `processd` | `8580` | NB STP auto-responder — `mako-pruefung` (`E_0622`/`G_0011`), LF answers 55007/55010 inside their per-PID Frist |
| `makod` | `8080` | EDIFACT process engine — GPKE/WiM/GeLi Gas, in-memory |

```mermaid
sequenceDiagram
    participant LF as LF 4012345000023
    participant makod as makod :8080
    participant marktd as marktd :8180
    participant processd as processd :8580
    participant webhook as webhook :8000

    LF->>makod: POST /edifact<br/>UTILMD 55001
    makod-->>webhook: APERAK BGM+312<br/>(45-min deadline, auto)
    makod->>marktd: de.mako.process.initiated<br/>HMAC POST /api/v1/events (demo override)
    marktd->>processd: de.mako.process.initiated<br/>HMAC POST /webhook
    marktd-->>webhook: de.mako.process.initiated<br/>(ERP subscription)
    processd->>marktd: GET /api/v1/versorgung/{malo_id}
    processd->>marktd: GET /api/v1/malos/{malo_id}/grid
    processd->>marktd: GET /api/v1/partners/{lf_mp_id}
    Note over processd: `mako-pruefung`: E_0622 → Accept (A51)
    processd->>makod: gpke.lieferbeginn.bestaetigen
    makod-->>webhook: UTILMD 55002 Bestätigung
```

Total time once the images are built: **~5 minutes**. The first build takes
longer — see [Step 1](#step-1-clone-and-build), and rather longer again for the
[second demo](#the-second-demo-eeg-feed-in-settlement), whose two services come
out of the full builder stage.

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

just build-demo
```

`just build-demo` is the three builds the compose file needs, tagged the way it
expects them:

```bash
docker build --target runtime          -t makod:dev     .
docker build --target marktd-runtime   -t marktd:dev    .
docker build --target processd-runtime -t processd:dev  .
```

> Not `docker buildx bake` — `docker-bake.hcl` is the CI push path
> (`push-by-digest`), so it fails on the default docker driver and tags
> `ghcr.io/hupe1980/mako-*` rather than the `:dev` images the compose file starts.

> The `processd-runtime` stage builds with `--features integrated` (includes
> both the NB `mako-pruefung` and the LF answer modules).

These three come from a builder stage carrying only their dependencies — no
Iceberg/DataFusion, no LanceDB — so a cold build is a few minutes and a warm one
under a minute.

---

## Step 2 — Start the demo stack

```bash
cd demos/nb-stp
docker compose up -d
docker compose ps   # wait until all containers are running
```

Expected:

```
NAME                 IMAGE              STATUS         PORTS
nb-stp-postgres-1    postgres:17-alpine Up (healthy)   5432/tcp
nb-stp-webhook-1     python:3.12-alpine Up             0.0.0.0:8000->8000/tcp
nb-stp-marktd-1      marktd:dev         Up             0.0.0.0:8180->8180/tcp
nb-stp-processd-1    processd:dev       Up             0.0.0.0:8580->8580/tcp
nb-stp-makod-1       makod:dev          Up             0.0.0.0:8080->8080/tcp
```

**What happens at startup:**  
`processd` self-registers its fan-out subscription with `marktd` on startup —
no manual subscription curl required. Both `marktd` and `processd` run SQLx
migrations automatically on first boot (databases are created by `init-db.sh`).

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

`processd`'s `mako-pruefung` needs three items in `marktd` to reach an `Accept`
decision. Every path below is relative to `demos/nb-stp`, the directory
[Step 2](#step-2-start-the-demo-stack) changed into.

### 4a — Price sheet

```bash
curl -s -X PUT http://localhost:8180/api/v1/preisblaetter/9900357000004 \
  -H "Content-Type: application/json" \
  --data-binary @fixtures/preisblatt-nb.json \
  -w "\nHTTP %{http_code}\n"
# → HTTP 204
```

### 4b — MaLo + MaLo grid record

```bash
MALO_ID=51238696012

# MaLo (NB=9900357000004, no active LF)
curl -s -X PUT "http://localhost:8180/api/v1/malos/$MALO_ID" \
  -H "Content-Type: application/json" \
  --data-binary "$(jq --arg m "$MALO_ID" '.data.marktlokationsId=$m' fixtures/malo-nb.json)" \
  -w "\nHTTP %{http_code}\n"
# → HTTP 201

# MaLo grid record (`mako-pruefung` check 1)
curl -s -X PUT "http://localhost:8180/api/v1/malos/$MALO_ID/grid" \
  -H "Content-Type: application/json" \
  -d '{"nb_mp_id":"9900357000004","bilanzierungsgebiet":"11YN0------0STXG","netzgebiet":"DEMO-NZ-001","sparte":"STROM","source":"manual"}' \
  -w "\nHTTP %{http_code}\n"
# → HTTP 204
```

### 4c — LF trading partner (`mako-pruefung` check 5)

```bash
# Register in marktd partner directory
curl -s -X PUT http://localhost:8180/api/v1/partners/4012345000023 \
  -H "Content-Type: application/json" \
  -d '{"mp_id":"4012345000023","display_name":"Demo LF","marktrolle":"LF","sparte":"STROM","makoadresse":["https://as4.example.com/as4/receive"],"channels":{}}' \
  -w "\nHTTP %{http_code}\n"
# → HTTP 200

# Register in makod for EDIFACT routing
curl -s -X PUT http://localhost:8080/admin/partners/4012345000023 \
  -H "Authorization: Bearer demo-secret-change-me" \
  -H "Content-Type: application/json" \
  --data-binary @fixtures/partner-lf.json | jq '.'
```

---

## Step 5 — Submit a UTILMD 55001

```bash
curl -s -X POST http://localhost:8080/edifact \
  -H "Authorization: Bearer demo-secret-change-me" \
  -H "Content-Type: text/plain; charset=utf-8" \
  --data-binary @fixtures/utilmd-55001.edi | jq .
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
    "malo_id": "51238696012"
  }]
}
```

---

## Step 6 — Automatic NB decision

Within ~200 ms, `processd` receives the `de.mako.process.initiated` event from
`marktd`'s fan-out and runs all 6 `mako-pruefung` validation checks synchronously.

```bash
# Check the decision log
curl -s http://localhost:8580/api/v1/decisions | jq '.[] | {
  malo_id, decision, antwortcode, decided_at
}'
# → {"malo_id":"51238696012","decision":"Accept","antwortcode":"A51","decided_at":"..."}
```

With `NB_AUTO_ACCEPT=true` (set in the demo compose file), `Accept` automatically
dispatches `gpke.lieferbeginn.bestaetigen` to `makod`, which enqueues the outbound
**UTILMD 55002** (Bestätigung Anmeldung verb. MaLo):

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
MARKTD_URL=http://localhost:8180 WEBHOOK_URL=http://localhost:8000 bash smoke.sh
```

Output ends with:

```
✓ processd NB auto-responder dispatched bestaetigen → UTILMD 55002 already arrived
✓ POST /api/v1/commands → HTTP 409 (duplicate bestaetigen correctly rejected — AntwortGesendet guard confirmed)
✓ UTILMD 55002 was already verified in step 6c (auto-responder path)
All smoke tests passed.
  Wechselprozess auto-responder: ENABLED
  Flow: UTILMD 55001 → makod → marktd ingest → validate → bestaetigen → UTILMD 55002
```

The `HTTP 409` confirms the **workflow state guard** — `processd` already advanced
the workflow to `AntwortGesendet`, so the later manual command is correctly rejected
with `invalid_state: expected ValidationPassed, found AntwortGesendet`. This is not
idempotency: it is the state machine rejecting an out-of-order command.

---

## Step 8 — Explore the APIs

| Interface | URL |
|---|---|
| makod Swagger UI | http://localhost:8080/api/v1/docs/ |
| makod MCP server | http://localhost:8080/mcp |
| marktd Swagger UI | http://localhost:8180/api/v1/docs/ |
| marktd DLQ admin | http://localhost:8180/admin/fanout/dlq |
| marktd metrics | http://localhost:8180/metrics |
| processd decisions | http://localhost:8580/api/v1/decisions |
| processd approval queue | http://localhost:8580/api/v1/queue |
| ERP webhook event log | http://localhost:8000/events |

---

## Stop and clean up

```bash
docker compose down      # keep database volumes
docker compose down -v   # destroy all volumes (full reset)
```

---

## The second demo — EEG feed-in settlement

`demos/eeg-billing` runs the other half of the platform: no EDIFACT at all, but
the settlement path an NB owes an Anlagenbetreiber. `einsd` holds the plant
register and computes the § 21 EEG 2023 Einspeisevergütung; `edmd` holds the
quarter-hour Einspeisemenge it settles against.

| Service | Port | Role |
|---|---|---|
| `marktd` | `8180` | Market Data Hub — the MaLo the plant feeds into |
| `edmd` | `8380` | Energy Data Management — quarter-hour readings, billing periods |
| `einsd` | `9180` | EEG/KWKG settlement — plant register, monthly Vergütung |

```bash
# from the repo root
docker build --target edmd-runtime  -t edmd:dev  .
docker build --target einsd-runtime -t einsd:dev .

cd demos/eeg-billing
docker compose up -d
bash smoke.sh
```

`marktd` comes from the image `just build-demo` already produced. `edmd` and
`einsd` are built on their own, and they are the expensive half of this page:
both copy from the full `builder` stage, which compiles every service in the
workspace in one `cargo build` — Iceberg/DataFusion and LanceDB included. Budget
**20–45 minutes** cold, a few minutes on a warm BuildKit cache.

It registers the Anlagenbetreiber and a 9.8 kWp rooftop plant behind it, pushes a
month of quarter-hour readings under OBIS `1-0:2.8.0`, settles the month at EUR
233.57, and asserts the `de.eeg.verguetung.berechnet` CloudEvent the ERP
receives. The amount alone is not a legal document: under the Gutschriftverfahren
(§ 14 Abs. 2 Satz 2 UStG) the Netzbetreiber issues the Gutschrift, so `einsd`
renders it as a BO4E `Rechnung` whose VAT follows the operator's declared
`ust_status` — the fixture is a Kleinunternehmer (§ 19 UStG), so it carries 0 %
USt.

---

## Next steps

| Topic | Guide |
|---|---|
| EDIFACT parsing and validation | [Parsing guide](@/docs/reference/parsing.md) |
| ERP integration — CloudEvents, HMAC | [ERP integration](@/docs/architecture/erp-integration.md) |
| makod operator reference | [makod guide](@/docs/services/makod.md) |
| marktd operator reference | [marktd guide](@/docs/services/marktd.md) |
| processd — NB STP + LF answer automation | [processd guide](@/docs/services/processd.md) |
| INVOIC plausibility, § 147 AO / GoBD | [invoicd guide](@/docs/services/invoicd.md) |
| Energy data, imbalance, billing periods | [edmd guide](@/docs/services/edmd.md) |
| Process observability, §20 parity | [obsd guide](@/docs/services/obsd.md) |
| Full system architecture | [Architecture](@/docs/architecture/_index.md) |
| Process catalogue (GPKE, WiM, …) | [Processes](@/docs/reference/processes.md) |
| mako-service SDK | [mako-service README](https://github.com/hupe1980/mako/tree/main/crates/mako-service) |
