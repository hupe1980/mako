# mako demo

Quick-start guide for running and testing the full mako stack:

- **`makod`** `:8080` — EDIFACT process engine (GPKE, WiM, GeLi Gas, MABIS, GaBi Gas)
- **`marktd`** `:8180` — Market Data Hub (MaLo/MeLo, contracts, VersorgungsStatus, PRICAT, subscriptions, konfigurationsprodukte typed API, MMMA monthly price import worker, ZeitvariablePreisposition validation)
- **`processd`** `:8580` — NB STP auto-responder (validates Anmeldungen, dispatches bestaetigen/ablehnen) + LF E_0624 auto-response + MSB REQOTE auto-response from PreisblattMessung + §14a Steuerungsauftrag produktcode contract check
- **`invoicd`** `:8280` — INVOIC plausibility-check daemon (LF role; auto-settles/disputes inbound invoices)
- **`edmd`** `:8380` — Energy Data Management (MSCONS meter readings, iMSys direct push for §41a real-time billing, Hampel-filter quality scoring A/B/C/F, Ablesesteuerung reading orders with INSRPT auto-scheduling, Lastgang/Zeitreihe export, billing period)
- **`obsd`** `:8480` — Observability daemon (process projections, BNetzA KPIs)
- **`netzbilanzd`** `:8680` — NNE/KA/MMM/MSB/AWH billing daemon (NB role; generates INVOIC 31001/31002/31005/31009/31011; §14a Modul 2 ToU; §42a GGV; REMADV lifecycle; Redispatch 2.0 Kostenblatt; 13-tool MCP server)
- **`sperrd`** `:8780` — Sperrung execution tracking (NB role; IFTSTA 21039 auto-dispatch)
- **`nis-syncd`** `:9680` — NIS/GIS grid topology import adapter (pushes malo_grid, drift CloudEvents)
- **`einsd`** `:9180` — Einspeiser Registry + EEG/KWKG Settlement (9 settlement models: Vergütung, Mieterstrom §38a, Direktvermarktung Marktprämie, Ausschreibung, Post-EEG Spot, Eigenverbrauch, KWKG-Zuschlag, Flexibilitätsprämie §50b, Flexibilitätszuschlag §50a; Repowering §22 EEG; Zusammenlegung §24; KWKG Förderdauer; CloudEvents `de.eeg.verguetung.berechnet` + `de.eeg.marktpraemie.berechnet`)
- **`tarifbd`** `:9080` — Product & Tariff Catalog (user-defined energy products: STROM/GAS/WAERME/SOLAR/EEG/EINSPEISUNG/WAERMEPUMPE/WALLBOX/HEMS/EMOBILITY/ENERGIEDIENSTLEISTUNG/BUNDLE; all prices in `Tarifpreisblatt` JSONB; EPEX Spot for §41a)
- **`billingd`** `:9280` — Energy Billing Engine (STROM/GAS/WAERME/SOLAR/EEG/EINSPEISUNG/WAERMEPUMPE/WALLBOX/HEMS/EMOBILITY/ENERGIEDIENSTLEISTUNG — all prices user-defined; §41a dynamic 15-min Lastgang × EPEX; `POST /preview` dry-run; Gas Brennwertkorrektur with H2-blend `gasqualitaet` audit annotation; `GET /{id}/xrechnung` XRechnung 3.0)
- **`accountingd`** `:9380` — Customer Account Ledger (running debit/credit ledger; CAMT.054 bank statement import; SEPA pain.008 XML with N−5 scheduler; Vorauszahlung BO4E typed; IBAN mod-97 validation; Mahnwesen Mahnstufe 1–3; Sperrauftrag trigger)
- **`portald`** `:9480` — Customer Portal read-model gateway (aggregates edmd + billingd + accountingd + marktd + einsd; `GET /portal/{malo_id}/dashboard`; SSE `/events` stream)
- **`vertragd`** `:9780` — Contract & Customer Management (B2C + B2B Kunden with `kunden_identitaeten` N-login portal access; Rahmenverträge for B2B portfolio contracts; Versorgungsverträge per site/commodity; Tarifwechsel §41 EnWG with Preisgarantie guard + typed BO4E `Preisgarantie` resource; Person BO4E (GDPR Art. 15); Kündigung with Schlussablesung; OIDC→MaLo auth gateway for portald)
- **`agentd`** `:9580` — Multi-agent LLM orchestration daemon (Orchestrator + Specialist Mesh with 8 specialists: billing anomaly AI, payment reconciliation, MSB device history RAG, grid anomaly detection, EEG lifecycle, compliance; LanceDB vector store; all MCP tools wired; OpenAI/Anthropic/Bedrock providers)
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
docker pull ghcr.io/hupe1980/makod:0.9.0 && docker tag ghcr.io/hupe1980/makod:0.9.0 makod:dev
docker pull ghcr.io/hupe1980/marktd:0.9.0  && docker tag ghcr.io/hupe1980/marktd:0.9.0  marktd:dev
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
| einsd | HTTP port | `:9180` |
| einsd | Auth | disabled (dev mode) |
| tarifbd | HTTP port | `:9080` |
| vertragd | HTTP port | `:9780` |
| agentd | HTTP port | `:9580` |
| tarifbd | Auth | disabled (dev mode) |
| vertragd | Auth | disabled (dev mode) |
| agentd | Auth | disabled (dev mode) |
| billingd | HTTP port | `:9280` |
| billingd | Auth | disabled (dev mode) |
| accountingd | HTTP port | `:9380` |
| accountingd | Auth | disabled (dev mode) |
| portald | HTTP port | `:9480` |
| portald | Auth | disabled (dev mode) |

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
| einsd plants | http://localhost:9180/api/v1/anlagen | EEG/KWKG plant register |
| einsd settle | http://localhost:9180/api/v1/anlagen/{tr_id}/settle/{year}/{month} | Monthly settlement |
| einsd rate lookup | http://localhost:9180/api/v1/verguetungssatz-lookup | EEG/KWKG tariff rate lookup |
| tarifbd products | http://localhost:9080/api/v1/products/9910000000002 | Tariff catalog |
| tarifbd epex | http://localhost:9080/api/v1/epex-prices/2026-07-12/hourly | EPEX day-ahead prices |
| billingd calculate | http://localhost:9280/api/v1/billing/51238696781/calculate | Trigger billing run |
| billingd preview | http://localhost:9280/api/v1/billing/51238696781/preview | Dry-run (no persist) |
| billingd xrechnung | http://localhost:9280/api/v1/billing/{id}/xrechnung | XRechnung 3.0 / ZUGFeRD 2.3 CII XML |
| billingd records | http://localhost:9280/api/v1/billing | Billing record history |
| accountingd balance | http://localhost:9380/api/v1/accounts/51238696781/balance | Customer balance |
| accountingd kontoauszug | http://localhost:9380/api/v1/accounts/51238696781/kontoauszug | Account statement |
| portald dashboard | http://localhost:9480/api/v1/portal/51238696781/dashboard | Aggregated customer snapshot |
| portald events | http://localhost:9480/api/v1/portal/51238696781/events | SSE stream |
| ERP webhook receiver | http://localhost:8000/events | View delivered CloudEvents |

