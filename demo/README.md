# mako demo

Quick-start guide for running and testing the full mako stack:

- **`makod`** `:8080` — EDIFACT process engine (GPKE, WiM, GeLi Gas, MABIS, GaBi Gas)
- **`marktd`** `:8180` — Market Data Hub (MaLo/MeLo, contracts, VersorgungsStatus, PRICAT, subscriptions)
- **`processd`** `:8580` — NB STP auto-responder (validates Anmeldungen, dispatches bestaetigen/ablehnen)
- **`invoicd`** `:8280` — INVOIC plausibility-check daemon (LF role; auto-settles/disputes inbound invoices)
- **`edmd`** `:8380` — Energy Data Management (MSCONS meter readings, Lastgang/Zeitreihe export, billing period)
- **`obsd`** `:8480` — Observability daemon (process projections, BNetzA KPIs)
- **`netzbilanzd`** `:8680` — NNE/KA/MMM/MSB billing daemon (NB role; generates INVOIC 31001/31002/31005/31009)
- **`sperrd`** `:8780` — Sperrung execution tracking (NB role; IFTSTA 21039 auto-dispatch)
- **`nis-syncd`** `:9680` — NIS/GIS grid topology import adapter (pushes malo_grid, drift CloudEvents)
- **`webhook`** `:8000` — In-memory ERP event receiver

Both daemons run with authentication **disabled** in the demo (`--auth-disabled` / `--auth-key`) — suitable for local development only. See the [production guide](../docs/getting-started.md) for OIDC setup.

---

## Prerequisites

| Tool | Purpose |
|---|---|
| Docker (with Compose v2) | Run the stack |
| `curl` | HTTP smoke tests |
| `jq` | Parse JSON responses |

Build images from the repo root:

```bash
docker build --target runtime          -t makod:dev      .
docker build --target marktd-runtime   -t marktd:dev     .
docker build --target processd-runtime -t processd:dev   .
docker build --target invoicd-runtime  -t invoicd:dev    .
docker build --target edmd-runtime     -t edmd:dev       .
docker build --target obsd-runtime     -t obsd:dev       .
docker build --target netzbilanzd-runtime -t netzbilanzd:dev .
docker build --target sperrd-runtime      -t sperrd:dev      .
docker build --target nis-syncd-runtime   -t nis-syncd:dev   .
```

Or pull published images:

```bash
docker pull ghcr.io/hupe1980/makod:0.8.0 && docker tag ghcr.io/hupe1980/makod:0.8.0 makod:dev
docker pull ghcr.io/hupe1980/marktd:0.8.0  && docker tag ghcr.io/hupe1980/marktd:0.8.0  marktd:dev
```

---

## Demo configuration

The demo runs the stack as **Netzbetreiber Strom (NB)** with GLN `9900357000004`. This matches the `NAD+MR` (receiver) in the bundled EDIFACT fixture, so all routing steps succeed without extra setup.

| Service | Parameter | Value |
|---|---|---|
| makod | Tenant ID / Marktrolle | `9900357000004` / `NB` |
| makod | HTTP port | `:8080` |
| makod | Bearer token | `demo-secret-change-me` |
| marktd | Tenant GLN | `9900357000004` |
| marktd | HTTP port | `:8180` |
| marktd | Auth | disabled (dev mode) |
| processd | HTTP port | `:8580` |
| processd | Auth | disabled (dev mode) |
| invoicd | HTTP port | `:8280` |
| invoicd | Auth | disabled (dev mode) |
| edmd | HTTP port | `:8380` |
| edmd | Auth | disabled (dev mode) |
| obsd | HTTP port | `:8480` |
| obsd | Auth | disabled (dev mode) |
| netzbilanzd | HTTP port | `:8680` |
| netzbilanzd | Auth | disabled (dev mode) |
| sperrd | HTTP port | `:8780` |
| sperrd | Auth | disabled (dev mode) |
| nis-syncd | HTTP port | `:9680` |
| nis-syncd | Auth | disabled (dev mode) |

---

## Quick start — docker compose

```bash
cd demo
docker compose up -d
docker compose ps          # wait for all services (healthy)
docker compose logs -f
```

Services are healthy when `docker compose ps` shows `(healthy)` for both `makod` and `marktd`.

Watch events arrive:
```bash
docker compose logs webhook -f   # ERP CloudEvents from makod/marktd
```

Stop:
```bash
docker compose down       # keep PostgreSQL volume
docker compose down -v    # wipe all data
```

---

## Smoke test — automated

```bash
cd demo

# Test makod only:
./smoke.sh

# Test full stack (makod + marktd):
MARKTD_URL=http://localhost:8180 WEBHOOK_URL=http://localhost:8000 ./smoke.sh
```

Expected output (full stack):
```
▶ Waiting for makod at http://localhost:8080 ...
✓ makod is ready
=================================================
  mako smoke test  →  http://localhost:8080
  marktd           →  http://localhost:8180
=================================================

✓ GET /health → ok  (instance: ...)
✓ GET /api/v1/openapi.json → makod REST API
✓ PUT /admin/partners/4012345000023 → 200
✓ GET /admin/partners → 1 partner(s) registered
✓ POST /edifact → HTTP 200  accepted=1  rejected=0  status=routed  pid=55001
✓ Automatic outbox: APERAK BGM+312 + ProcessInitiated CloudEvent
✓ POST /api/v1/commands bestaetigen → HTTP 202
✓ Outbound EDIFACT: UTILMD 55003 Bestätigung delivered
✓ DELETE /admin/partners/4012345000023 → 200

─────────────────────────────────────────────────
  marktd smoke tests  →  http://localhost:8180
─────────────────────────────────────────────────

✓ GET /health → ok
✓ PUT /api/v1/preisblaetter/9900357000004 → 204 (price sheet stored)
✓ GET /api/v1/preisblaetter/9900357000004 → source=api  bezeichnung=Demo Netznutzungspreise 2025 ...
✓ Operator-override protection verified via source=api field

=================================================
All smoke tests passed.

  makod Swagger UI : http://localhost:8080/api/v1/docs/
  makod MCP server : http://localhost:8080/mcp
  marktd  REST API   : http://localhost:8180/api/v1/docs/
=================================================
```

---

## Manual curl examples

### makod

```bash
# Health check (no auth)
curl http://localhost:8080/health | jq .

# Submit EDIFACT interchange
curl -X POST http://localhost:8080/edifact \
  -H "Authorization: Bearer demo-secret-change-me" \
  -H "Content-Type: text/plain; charset=utf-8" \
  --data-binary "@fixtures/utilmd-55001.edi" | jq .

# Accept a Lieferbeginn Anmeldung (replace <process_id>)
curl -X POST http://localhost:8080/api/v1/commands \
  -H "Authorization: Bearer demo-secret-change-me" \
  -H "Content-Type: application/json" \
  -d '{"command":"bestaetigen","process_id":"<process_id>"}' | jq .
```

### marktd (auth disabled in demo)

```bash
# Health check
curl http://localhost:8180/health | jq .

# Upload a NB price sheet (source=api — operator override)
curl -X PUT http://localhost:8180/api/v1/preisblaetter/9900357000004 \
  -H "Content-Type: application/json" \
  --data-binary "@fixtures/preisblatt-nb.json" \
  -w "\nHTTP %{http_code}\n"

# Read back the price sheet valid on a billing date
curl "http://localhost:8180/api/v1/preisblaetter/9900357000004?date=2025-10-15" | jq '{
  source: .source,
  bo4e_version: .bo4e_version,
  bezeichnung: .data.bezeichnung,
  updated_at: .updated_at
}'

# View incoming CloudEvents from makod
curl http://localhost:8000/events | jq '.[].body | {type,subject}'
```

---

## Fixtures

| File | Description |
|---|---|
| `fixtures/utilmd-55001.edi` | UTILMD PID 55001 — Anmeldung Lieferbeginn Strom (LFN→NB) |
| `fixtures/partner-lf.json` | Trading partner record for LFN GLN `4012345000023` |
| `fixtures/preisblatt-nb.json` | `PreisblattNetznutzung` for NB `9900357000004` (2025-10-01..2026-09-30) |

---

## Service URLs

| Service | URL | Purpose |
|---|---|---|
| makod REST API | http://localhost:8080 | EDIFACT ingest, process commands |
| makod Swagger UI | http://localhost:8080/api/v1/docs/ | Interactive API docs |
| makod MCP server | http://localhost:8080/mcp | LLM tooling (Claude Desktop, VS Code) |
| marktd REST API | http://localhost:8180 | Master data (MaLo/MeLo, typed BO4E responses, NB contracts, price sheets) |
| marktd Swagger UI | http://localhost:8180/api/v1/docs/ | Interactive API docs |
| marktd DLQ admin | http://localhost:8180/admin/fanout/dlq | Inspect failed CloudEvent deliveries |
| marktd metrics | http://localhost:8180/metrics | Prometheus metrics |
| processd decisions | http://localhost:8580/api/v1/decisions | NB STP audit log |
| processd queue | http://localhost:8580/api/v1/queue | LF approval queue |
| invoicd receipts | http://localhost:8280/api/v1/receipts | INVOIC receipt ledger |
| invoicd overdue | http://localhost:8280/api/v1/overdue-remadv | Approaching Zahlungsziel |
| edmd deliveries | http://localhost:8380/api/v1/deliveries/{malo_id} | BO4E `Energiemenge` array (typed ERP feed) |
| edmd Lastgang | http://localhost:8380/api/v1/lastgang/{malo_id} | BO4E interval time-series (grouped by OBIS) |
| edmd Zeitreihe | http://localhost:8380/api/v1/zeitreihe/{malo_id} | BO4E generic time-series (commodity metadata) |
| obsd projections | http://localhost:8480/obs/processes | Live process projections |
| obsd KPIs | http://localhost:8480/obs/kpis | BNetzA KPI report |
| netzbilanzd billing | http://localhost:8680/api/v1/billing/run | NNE/MMM/MSB invoice generation |
| netzbilanzd drafts | http://localhost:8680/api/v1/billing/drafts | Invoice draft review queue |
| sperrd orders | http://localhost:8780/api/v1/sperr-orders | Sperrung execution tracker |
| nis-syncd sync | http://localhost:9680/api/v1/grid/sync | NIS grid topology import |
| ERP webhook receiver | http://localhost:8000/events | View delivered CloudEvents |

