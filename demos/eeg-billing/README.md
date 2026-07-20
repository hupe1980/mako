# EEG Billing Demo

End-to-end demonstration of **EEG feed-in settlement** using `einsd` (plant registry + §21 EEG 2023 calculation) and `edmd` (15-min meter data storage).

## What you're running

| Service | Port | Role |
|---|---|---|
| `postgres` | `5432` | PostgreSQL — one database per service |
| `webhook` | `8000` | Demo ERP event receiver (Python, in-memory) |
| `marktd` | `8180` | Market Data Hub — MaLo master data |
| `edmd` | `8380` | Energy Data Management — meter readings + billing periods |
| `einsd` | `9180` | EEG/KWKG settlement — plant register + monthly Vergütung |

## End-to-end flow

```
ERP → PUT /api/v1/anlagen/TR0000000001          Register 9.8 kWp solar plant
ERP → POST /api/v1/meter-reads/rlm/17835382035   Push June 2026 Einspeisemenge to edmd
ERP → POST /api/v1/anlagen/TR0000000001/settle/2026/6
                                                  Trigger EEG settlement
einsd → GET {edmd}/api/v1/billing-period/17835382035   Auto-fetch Einspeisemenge
einsd → calculates Vergütung (8.11 ct/kWh × ~2880 kWh ≈ EUR 233.57)
ERP ← de.eeg.verguetung.berechnet CloudEvent     Settlement result
```

## Settlement logic

The demo plant:

| Field | Value |
|---|---|
| ErzeugungsArt | `SOLAR_AUFDACH` (roof-mounted PV) |
| EEG law | EEG 2023 |
| Settlement model | `FEED_IN_TARIFF` (§21 Einspeisevergütung) |
| Installed capacity | 9.8 kWp |
| Feed-in tariff | **8.11 ct/kWh** (Solarpaket I, ≤10 kWp Überschusseinspeisung) |
| Commissioning | 2024-03-15 |
| Förderendedatum | 2044-03-31 (20 years) |

June 2026 result: ~2880 kWh × 8.11 ct = **~EUR 233.57**.

## Build images

```bash
cd ../..  # workspace root

docker build --target marktd-runtime  -t marktd:dev  .
docker build --target edmd-runtime    -t edmd:dev    .
docker build --target einsd-runtime   -t einsd:dev   .
```

Or build all demo images at once:

```bash
docker build --target runtime             -t makod:dev    .
docker build --target marktd-runtime      -t marktd:dev   .
docker build --target processd-runtime    -t processd:dev .
docker build --target edmd-runtime        -t edmd:dev     .
docker build --target einsd-runtime       -t einsd:dev    .
```

## Run the demo

```bash
cd demos/eeg-billing
docker compose up -d
docker compose ps   # wait until all containers are running
```

Then run the smoke test:

```bash
bash smoke.sh
```

Expected output:

```
✓ einsd is ready
✓ edmd is ready
✓ PUT /api/v1/malo/17835382035 → 201
✓ PUT /api/v1/anlagen/TR0000000001 → 201  (plant registered)
✓ GET /api/v1/anlagen/TR0000000001 → status=aktiv  verguetungssatz_ct=8.11 ct/kWh
✓ POST /api/v1/meter-reads/rlm/17835382035 → 200  stored=96 intervals
✓ POST /api/v1/meter-reads/rlm/17835382035 → 200  (29 daily buckets, 2784 kWh)
✓ GET /api/v1/billing-period/17835382035 → arbeitsmenge_kwh=2880.0
✓ POST /settle/2026/6 → 200
      settlement_eur=233.57  einspeisemenge_kwh=2880.0  status=calculated
✓ CloudEvent received: type=de.eeg.verguetung.berechnet
✓ GET /settlements?year=2026&month=6 → status=calculated  einspeisemenge_kwh=2880.0  settlement_eur=233.57
All EEG billing smoke tests passed.
```

## Explore the APIs

| Endpoint | Description |
|---|---|
| `http://localhost:9180/api/v1/anlagen/TR0000000001` | Plant registration details |
| `http://localhost:9180/api/v1/anlagen/TR0000000001/settlements?year=2026&month=6` | Settlement receipt |
| `http://localhost:8380/api/v1/billing-period/17835382035?from=2026-06-01&to=2026-07-01` | edmd billing period aggregate |
| `http://localhost:9180/mcp` | einsd MCP server (18 tools) |
| `http://localhost:8380/mcp` | edmd MCP server (14 tools) |
| `http://localhost:8000/events` | ERP webhook event log |

## Other settlement models

The `einsd` service supports 9 EEG/KWKG settlement schemes. To test other models, modify `fixtures/anlage.json`:

| Model | `settlement_model` | Use case |
|---|---|---|
| §21 fixed tariff | `FEED_IN_TARIFF` | Small solar, wind ≤750 kW |
| §20 Direktvermarktung | `MARKET_PREMIUM` | Plants > threshold MW |
| §38a Mieterstrom | `TENANT_ELECTRICITY` | Building community solar |
| Post-EEG Spot | `POST_EEG` | Plants after 20-year Förderung |
| KWK-Zuschlag | `KWK_SURCHARGE` | Combined heat & power (KWKG) |
| §50 Flexibilitätsprämie | `FLEXIBILITY_PREMIUM` | Biomass demand response |

## Supported EEG laws

`eeg_gesetz` in `fixtures/anlage.json` selects the regulatory version:
`2000 | 2004 | 2009 | 2012 | 2017 | 2021 | 2023 | 0` (0 = KWKG)

## Clean up

```bash
docker compose down      # keep database volumes
docker compose down -v   # destroy all volumes (full reset)
```
