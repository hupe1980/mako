# mako demo

End-to-end smoke test for the **NB STP auto-responder** — the core flow of German
energy market communication: a UTILMD 55001 Anmeldung arrives at `makod`,
`processd` evaluates it automatically via netz-checker, and a UTILMD 55003
Bestätigung is delivered to the ERP webhook within seconds.

## What runs in this demo

| Service | Port | Purpose |
|---|---|---|
| `postgres` | `5432` | PostgreSQL — one database per service |
| `webhook` | `:8000` | In-memory ERP event receiver (Python) |
| `marktd` | `:8180` | Market Data Hub — MaLo/MeLo/NeLo/TR, VersorgungsStatus, EventBus fan-out |
| `processd` | `:8580` | NB STP auto-responder — netz-checker (6 checks), LF E_0624 (45 min) |
| `makod` | `:8080` | EDIFACT process engine — GPKE/WiM/GeLi Gas, in-memory |

The full platform has [16 production services](https://hupe1980.github.io/mako/docs/services/) — `invoicd`,
`netzbilanzd`, `edmd`, `einsd`, `billingd`, `accountingd`, `vertragd`, `portald`,
`agentd`, and more. Run them individually as needed following their operator guides.

## End-to-end flow

```
ERP        → PUT MaLo + preisblatt into marktd        (master data pre-load)
ERP        → POST UTILMD 55001 to makod                (Anmeldung Lieferbeginn)
makod      → de.mako.process.initiated to marktd        (ERP webhook, HMAC-signed)
marktd     → fans out to processd via subscription
processd   → GET /versorgung, GET /malo/grid, GET /partners  (netz-checker data fetch)
processd   → 6 netz-checker checks → Accept
processd   → gpke.lieferbeginn.bestaetigen to makod
makod      → UTILMD 55003 Bestätigung to webhook        (ERP confirmation)
```

```mermaid
sequenceDiagram
    participant LF as LFN 4012345000023
    participant makod as makod :8080
    participant marktd as marktd :8180
    participant processd as processd :8580
    participant webhook as webhook :8000

    LF->>makod: POST /edifact  UTILMD 55001
    makod-->>webhook: APERAK BGM+312 (auto, 45-min deadline)
    makod->>marktd: de.mako.process.initiated
    marktd->>processd: de.mako.process.initiated (fan-out)
    marktd-->>webhook: de.mako.process.initiated (ERP subscription)
    processd->>marktd: GET /versorgung/{malo_id}
    processd->>marktd: GET /malo/{malo_id}/grid
    processd->>marktd: GET /partners/{lf_mp_id}
    Note over processd: netz-checker: 6 checks → Accept
    processd->>makod: gpke.lieferbeginn.bestaetigen
    makod-->>webhook: UTILMD 55003 Bestätigung Lieferbeginn
```

## Prerequisites

| Tool | Purpose |
|---|---|
| Docker 24+ (with Compose v2) | Run the stack |
| `curl` | HTTP smoke tests |
| `jq` | Parse JSON responses |

## Build images

Build the three demo images from the repo root:

```bash
docker build --target runtime          -t makod:dev     .
docker build --target marktd-runtime   -t marktd:dev    .
docker build --target processd-runtime -t processd:dev  .
```

Or in parallel with `docker buildx bake`:

```bash
docker buildx bake makod marktd processd
```

> The `processd-runtime` target compiles with `--features integrated`
> (NB netz-checker + LF E_0624 auto-response in one binary).

## Quick start

```bash
cd demos/nb-stp
docker compose up -d
docker compose ps   # wait until all containers are Up
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

`processd` self-registers its EventBus subscription with `marktd` on startup —
no manual subscription step required. Both `marktd` and `processd` run SQLx
migrations automatically on first boot.

Watch events arrive in real time:

```bash
docker compose logs webhook -f
```

## Automated smoke test

The smoke test seeds all required master data, submits a UTILMD 55001, and
confirms that `processd` dispatches bestaetigen automatically:

```bash
cd demos/nb-stp
MARKTD_URL=http://localhost:8180 WEBHOOK_URL=http://localhost:8000 bash smoke.sh
```

Expected output:

```
✓ makod is ready
✓ marktd is ready
✓ PUT /api/v1/preisblaetter/9900357000004 → 204 (FV2026 preisblatt stored)
✓ PUT /api/v1/partners/4012345000023 → 200 (partner ready for netz-checker)
✓ PUT /api/v1/malo/17841584182 → 201  (version=1, makod cache push triggered)
✓ PUT /api/v1/malo/17841584182/grid → 204  (grid record ready for netz-checker)
✓ PUT /api/v1/subscriptions/smoke-test-sub → 200
✓ GET /health → ok  (instance: ...)
✓ PUT /admin/partners/4012345000023 → 200
✓ POST /edifact → HTTP 200  accepted=1  rejected=0  status=routed  pid=55001
✓ APERAK BGM+312 (Anerkennungsmeldung) delivered to LFN — automatic (no ERP action)
✓ ProcessInitiated delivered via marktd fan-out (source: urn:markt:tenant:9900357000004)
✓ processd NB auto-responder dispatched bestaetigen → UTILMD 55003 already arrived
✓ POST /api/v1/commands → HTTP 409 (auto-responder already accepted — idempotency confirmed)
✓ UTILMD 55003 Bestätigung Lieferbeginn delivered to LFN
✓ Operator-override protection confirmed (source=api; api > mako enforced by SQL)
All smoke tests passed.
  Wechselprozess auto-responder: ENABLED
  Flow: UTILMD 55001 → makod → marktd ingest → validate → bestaetigen → UTILMD 55003
```

The `HTTP 409` at the manual dispatch step is the **idempotency proof**: `processd`
already dispatched `bestaetigen` automatically — the manual ERP call arrives too late.

## Service URLs

| Service | URL | Purpose |
|---|---|---|
| makod REST API | http://localhost:8080 | EDIFACT ingest, process commands |
| makod Swagger UI | http://localhost:8080/api/v1/docs/ | Interactive API docs |
| makod MCP server | http://localhost:8080/mcp | LLM tooling (Claude Desktop, VS Code) |
| marktd REST API | http://localhost:8180 | Master data (MaLo/MeLo, typed BO4E, VersorgungsStatus) |
| marktd Swagger UI | http://localhost:8180/api/v1/docs/ | Interactive API docs |
| marktd DLQ admin | http://localhost:8180/admin/fanout/dlq | Inspect failed CloudEvent deliveries |
| marktd metrics | http://localhost:8180/metrics | Prometheus metrics |
| processd decisions | http://localhost:8580/api/v1/decisions | NB STP audit log |
| processd queue | http://localhost:8580/api/v1/queue | LF approval queue |
| ERP webhook receiver | http://localhost:8000/events | View delivered CloudEvents |

## Fixtures

| File | Description |
|---|---|
| `fixtures/utilmd-55001.edi` | UTILMD PID 55001 — Anmeldung Lieferbeginn Strom (LFN→NB) |
| `fixtures/partner-lf.json` | Trading partner record for LFN GLN `4012345000023` |
| `fixtures/preisblatt-nb.json` | `PreisblattNetznutzung` for NB `9900357000004` (FV2026-10-01) |
| `fixtures/malo-nb.json` | `MARKTLOKATION` for NB `9900357000004` (demo MaLo) |
| `fixtures/contract-lf.json` | NB network contract (Netznutzungsvertrag) |

## Demo configuration

The demo runs as **Netzbetreiber (NB)** with Marktpartner-ID `9900357000004`.
All services run with authentication **disabled** — do not deploy this configuration
in production.

| Service | Parameter | Value |
|---|---|---|
| makod | Bearer token | `demo-secret-change-me` |
| makod | Tenant / Marktrolle | `9900357000004` / NB Strom |
| marktd | Tenant | `9900357000004` |
| processd | makod API key | `demo-secret-change-me` |
| processd | marktd API key | `demo-processd-key` |
| All services | Authentication | OIDC disabled (dev mode only) |

## Manual curl examples

### Health checks

```bash
curl http://localhost:8080/health | jq .
# → {"status":"ok","instance_id":"..."}

curl http://localhost:8180/health | jq .
# → {"status":"ok"}
```

### Submit EDIFACT

```bash
curl -X POST http://localhost:8080/edifact \
  -H "Authorization: Bearer demo-secret-change-me" \
  -H "Content-Type: text/plain; charset=utf-8" \
  --data-binary "@fixtures/utilmd-55001.edi" | jq .
```

### Trigger NB bestaetigen manually

```bash
curl -X POST http://localhost:8080/api/v1/commands \
  -H "Authorization: Bearer demo-secret-change-me" \
  -H "Content-Type: application/json" \
  -d '{"command":"gpke.lieferbeginn.bestaetigen","payload":{"malo_id":"<malo_id>"}}' | jq .
```

### Inspect marktd master data

```bash
# View VersorgungsStatus for a MaLo
curl http://localhost:8180/api/v1/versorgung/<malo_id> | jq .

# View incoming CloudEvents from makod
curl http://localhost:8000/events | jq '.[].body | {type, subject}'
```

### Upload a NB price sheet

```bash
curl -X PUT http://localhost:8180/api/v1/preisblaetter/9900357000004 \
  -H "Content-Type: application/json" \
  --data-binary "@fixtures/preisblatt-nb.json" \
  -w "\nHTTP %{http_code}\n"
# → HTTP 204
```

## Stop and clean up

```bash
docker compose down       # keep PostgreSQL volume
docker compose down -v    # wipe all data (full reset)
```

Authentication is disabled in this demo — suitable for local development only.
See the [production guide](https://hupe1980.github.io/mako/docs/guide/getting-started/) for OIDC setup, and
[the services guide](https://hupe1980.github.io/mako/docs/services/) for the full 16-service platform.
