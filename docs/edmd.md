---
layout: default
title: edmd Operator Guide
nav_order: 27
parent: Services
mermaid: true
description: >
  edmd operator guide: Energy Data Management daemon. Stores MSCONS meter
  readings, iMSys direct push for §41a real-time billing, Hampel-filter quality scoring
  (V01–V10 validation), virtual meters (§42b GGV), §17 MessZV substitution + forecasting,
  reading-order scheduling (Ablesesteuerung), MeterBillingPeriod (RLM
  Spitzenleistung + Gas Brennwert/Zustandszahl), Mehr-/Mindermengensaldo
  imbalance, BSI TR-03109 SMGW lifecycle, Iceberg/S3 OLAP archive, MCP server.
  PostgreSQL-backed, OIDC-secured, CloudEvents webhook.
---

# `edmd` Operator Guide

`edmd` is the **Energy Data Management daemon** — the service that stores meter
readings and computes billing-relevant energy quantities for downstream services.

Key responsibilities:

- Store MSCONS meter readings (SLP and RLM) via the webhook from `marktd`.
- Accept **iMSys / SMGW direct push** (15-min intervals in JSON, bypassing EDIFACT) for §41a real-time billing.
- Run the **Hampel-filter quality scorer** and **V01–V10 validation engine** on all inbound interval data. Emit `de.edmd.reading.quality.warning` CloudEvents for grade C/F data.
- Schedule and track **reading orders** (Ablesesteuerung) for all three market roles (LF, MSB, NB). Auto-creates `INSRPT_STOERUNG` orders when a WiM INSRPT PID 23001 Störungsmeldung arrives.
- Compute and serve **virtual meter time series** (Sum, Residual, PvSelfConsumption, GgvAllocation per §42b EEG GGV community solar) on demand.
- Generate **§17 MessZV annual forecasts** (Jahresprognose) and **prior-period substitute values** for gap intervals.
- Provide resampled Lastgang (hourly / daily / monthly / yearly buckets) and monthly Summenzeitreihe for MaBiS.
- Provide a time-series query API for ERP and `netzbilanzd`.
- Export BO4E `Lastgang` objects and `Zeitreihe` objects for ERP and API-Webdienste Strom consumers.
- Compute `MeterBillingPeriod` — RLM Spitzenleistung (kW) and Gas Brennwert / Zustandszahl — required by `netzbilanzd` for Leistungspreis billing.
- Accumulate **Mehr-/Mindermengensaldo** imbalance records per MaLo.
- **Apache Iceberg V2 OLAP archival**: automatically export `meter_reads` older than the configured retention window (default 12 months) to Parquet files on S3/GCS/Azure in Iceberg V2 table format.

The **domain calculation logic** is provided by the [`metering`](https://github.com/hupe1980/mako/tree/main/crates/metering) library crate (zero I/O, no async, 177 tests):

| Function / Type | §-basis | Used in |
|---|---|---|
| `gas_m3_to_kwh_hs(m3, hs, z)` | §24 GasGVV / DVGW G 685 | Gas direct push |
| `aggregate(intervals, AggregationConfig)` | §2 Nr. 17 MessZV | `MeterBillingPeriod` |
| `classify_messtyp(intervals, source)` | §3/§4 MessZV, §41a EnWG | iMSys classification |
| `compute_imbalance(actual, contracted)` | §27 MessZV | Mehr-/Mindermengensaldo |
| `score_intervals(intervals, config)` | — | Hampel quality scoring (A/B/C/F) |
| `validate_intervals(intervals, config)` | §17–22 MessZV | V01–V10 validation engine |
| `resample(intervals, config)` | §27 MessZV, MaBiS | Hourly/daily/monthly resampling |
| `compute_virtual_meter(rule, sources)` | §42b EEG, §42a EEG | GGV community solar, Residuallast |
| `project_annual_consumption(intervals, _)` | §17 MessZV Jahresprognose | Annual consumption forecast |
| `prior_period_substitutes(gap, _, _, prior, _)` | §17 Abs. 2 MessZV | Prior-period gap filling |
| `SmgwSession`, `ClsChannel` | BSI TR-03109, §14a EnWG | SMGW lifecycle + CLS management |

```mermaid
graph TB
    marktd["marktd :8180\nEventBus"]
    smgw["SMGW / iMSys\n(direct push)"]
    edmd["edmd :8380\n(this service)"]

    subgraph hot["Hot tier — PostgreSQL"]
        pg["meter_reads\nmeter_billing_periods\nablese_auftraege\ndirect_push_sessions\narchive_batches"]
    end

    subgraph cold["Cold tier — S3 / GCS / AzureDLS"]
        iceberg["Iceberg V2 table\nParquet data files\nPostgreSQL SQL catalog"]
    end

    erp["ERP / netzbilanzd"]
    worker["Archive worker\n(hourly)"]
    qa["Hampel quality scorer\n(k=3, t=3.0, MAD σ)"]

    marktd -->|"de.mako.process.initiated (23001 INSRPT)\nde.mako.edifact.inbound (MSCONS)\nHMAC POST /webhook"| edmd
    smgw -->|"POST /api/v1/meter-reads/rlm/{malo_id}\nPOST /api/v1/meter-reads/gas/{malo_id}"| edmd
    edmd --> qa
    qa -->|"grade A/B/C/F\nde.edmd.reading.quality.warning"| hot
    edmd --> hot
    hot -->|"rows > 12 months"| worker
    worker -->|"write Parquet\ncommit snapshot"| cold
    erp -->|"GET /api/v1/deliveries/{malo_id}\n→ Vec<Energiemenge>"| edmd
    erp -->|"GET /api/v1/billing-period/{malo_id}"| edmd
    erp -->|"GET /api/v1/imbalance/{malo_id}/{year}/{month}"| edmd
    erp -->|"GET /api/v1/lastgang/{malo_id}"| edmd
    erp -->|"GET /api/v1/archive/olap/{malo_id}\n→ MMM aggregation"| edmd
```

---

## Port layout

```
┌────────────────────────────────────────────────────────────────────────────┐
│  edmd  :8380                                                                │
│                                                                            │
│  POST /webhook                              ← marktd CloudEvents          │
│  GET  /api/v1/deliveries/{malo_id}          ← BO4E Energiemenge           │
│  GET  /api/v1/billing-period/{malo_id}      ← MeterBillingPeriod          │
│  GET  /api/v1/imbalance/{malo_id}/{y}/{m}   ← Mehr-/Mindermengen          │
│  GET  /api/v1/lastgang/{malo_id}            ← BO4E Lastgang               │
│  GET  /api/v1/zeitreihe/{malo_id}           ← BO4E Zeitreihe              │
│                                                                            │
│  ── iMSys direct push ────────────────────────────────────────────────── │
│  POST /api/v1/meter-reads/rlm/{malo_id}     ← Strom 15-min direct push   │
│  POST /api/v1/meter-reads/gas/{malo_id}     ← Gas direct push (m³→kWh_Hs)│
│                                                                            │
│  ── Reading order scheduling (Ablesesteuerung) ──────────────────────── │
│  POST|GET /api/v1/reading-orders            ← schedule / list orders     │
│  GET  /api/v1/reading-orders/{id}           ← order detail               │
│  PUT  /api/v1/reading-orders/{id}/complete  ← record reading result       │
│  PUT  /api/v1/reading-orders/{id}/cancel    ← cancel                     │
│  POST /api/v1/reading-orders/campaign       ← bulk Jahresablese-Kampagne  │
│                                                                            │
│  ── Quality scoring ──────────────────────────────────────────────────── │
│  POST /api/v1/quality-score/{malo_id}       ← retroactive Hampel rescore  │
│                                                                            │
│  ── Iceberg / S3 OLAP archival ─────────────────────────────────────────  │
│  GET  /api/v1/archive/status                ← archival stats + batches   │
│  GET  /api/v1/archive/olap/{malo_id}        ← MMM aggregation (OLAP)     │
│  GET  /api/v1/archive/portfolio             ← portfolio-level OLAP        │
│  GET  /api/v1/archive/timeseries/{malo_id}  ← historical time-series      │
│                                                                            │
│  GET  /metrics                              ← Prometheus metrics          │
│  GET  /health/live  /health/ready                                         │
│  POST|GET /mcp      ← MCP Streamable HTTP (LLM tooling)                   │
└────────────────────────────────────────────────────────────────────────────┘
```

---

## Inbound event routing

| `ce_type` | `makopid` | Action |
|-----------|-----------|--------|
| `de.mako.process.completed` | MSCONS set | Store meter readings |
| `de.mako.process.initiated` | 23001 (INSRPT Störungsmeldung) | Auto-create `INSRPT_STOERUNG` reading order (§18 MessZV) |
| anything else | — | 204 No Content (ignored) |

### MSCONS PIDs handled

| PID | Description | Direction |
|-----|-------------|-----------|
| 13005, 13006 | Strom Messwerte / Lastgang | NB → LF |
| 13007 | **Gas Datenabruf: Abrechnungsbrennwert + Zustandszahl** | NB → LF |
| 13008, 13009 | Gas Lastgang / Energiemenge | NB → LF |
| 13015–13027 | Strom / Gas various delivery confirmations | NB → LF |

**PID 13007 (Gasbeschaffenheitsdaten):** When a `de.mako.process.completed` event
arrives for PID 13007, `edmd` automatically extracts `brennwert_kwh_per_m3` (from
`QTY+Z08`) and `zustandszahl` (from `QTY+Z10`) and populates `meter_billing_periods`.
This makes Gas NNE billing possible without manual data entry.

To request Gas quality data on-demand, use `makod` command `geli.gas.datenabruf.anfragen`
(dispatches ORDERS 17103 to the GNB, 10-Werktage response deadline).

---

## iMSys direct push (§41a)

For **iMSys / SMGW** customers with 15-min interval meters, `edmd` accepts direct JSON
push bypassing the EDIFACT/MSCONS pipeline entirely. This is required for §41a EnWG
dynamic tariffs where the MSCONS round-trip adds 15–60 min latency.

```http
POST /api/v1/meter-reads/rlm/{malo_id}
Content-Type: application/json

{
  "session_id": "SMGW-SN-00112233-20260713T0600Z",
  "source": "SMGW",
  "obis_code": "1-0:1.8.0",
  "intervals": [
    { "from": "2026-07-13T00:00:00Z", "to": "2026-07-13T00:15:00Z", "value": "2.345", "unit": "kWh" },
    { "from": "2026-07-13T00:15:00Z", "to": "2026-07-13T00:30:00Z", "value": "2.412", "unit": "kWh" }
  ]
}
```

**Gas variant** (`/api/v1/meter-reads/gas/{malo_id}`): supply `unit = "m3"` plus `brennwert_kwh_per_m3` and optionally `zustandszahl`; `edmd` converts m³ × Hs × Z to kWh_Hs before storing.

The response includes a **quality report** (see below). HTTP 201 = clean data; 202 = stored with quality warnings.

Idempotent on `session_id` — re-submitting the same key returns 200 with the original result.

---

## Hampel-filter quality scoring

`edmd` runs a **Hampel filter** (window k=3, threshold t=3.0, MAD × 1.4826 robust σ) on every inbound interval batch. This is state-of-the-art for time-series meter data quality assessment (IEEE 1459-2010, IEC 61968-9).

### Quality checks

| Check | Detection | Grade impact |
|-------|-----------|--------------|
| Gap detection | Adjacent intervals where `to[i] ≠ from[i+1]` | Warnings |
| Consecutive zero-run | Max run of zero-value intervals | Warnings if > 4 |
| **Hampel outliers** | `\|x[i] − window_median\| > 3.0 × 1.4826 × MAD` | Warnings |
| Spike detection | `value > 10 × window_median` of neighbours | Warnings |
| Interval consistency | Mixed SLP/RLM interval durations | Warnings |
| Coverage | `accepted / expected × 100 %` | Grade degrades if < 95 % |

### Quality grades

| Grade | Meaning | Billing action |
|-------|---------|----------------|
| **A** | No anomalies | Normal billing run |
| **B** | Minor issues | Proceed with note |
| **C** | Significant issues | Manual review recommended |
| **F** | Unusable | Block billing run |

Grade F triggers `de.edmd.reading.quality.warning` CloudEvent to the ERP webhook, and also triggers the `msb-history-agent` in `agentd` (LanceDB RAG indexing).

### Retroactive rescoring

To re-score existing historical data (e.g. after a MSCONS delivery of old data, or after a firmware fix):

```http
POST /api/v1/quality-score/{malo_id}?from=2026-01-01T00:00:00Z&to=2026-07-01T00:00:00Z
```

Returns `{ malo_id, rows_rescored, warnings_found, grade }`.

---

## Reading order scheduling (Ablesesteuerung)

`edmd` is the scheduling authority for **all three market roles**:

| Role | Typical `anlass` values |
|------|------------------------|
| LF | `LIEFERBEGINN`, `LIEFERENDE`, `ZWISCHENABLESUNG`, `JAHRESABLESUNG` |
| NB | `JAHRESABLESUNG`, `SPERRUNG`, `ENTSPERRUNG` |
| MSB | `SONDERABLESUNG`, `INSRPT_STOERUNG`, `ISMS_AUSLESUNG` |

### INSRPT → reading order automation (§18 MessZV)

When `edmd` receives `de.mako.process.initiated` for PID 23001 (INSRPT Störungsmeldung), it **automatically** creates an `INSRPT_STOERUNG` reading order:

- `geplant_am` = tomorrow
- `ausfuehrt_bis` = + 7 calendar days (covers 5 Werktage WiM Strom window)
- `auftraggeber_rolle` = `MSB`
- Idempotent on `insrpt_process_id`

This eliminates the risk of billing a zero-reading period after a device swap — the field-service scheduler is unblocked immediately on INSRPT arrival, without any ERP action required.

---

## MCP server tools

`edmd` exposes an MCP server at `/mcp` with the following tools:

| Tool | Description |
|------|-------------|
| `get_timeseries` | Meter data time-series for a MaLo in a date range |
| `get_imbalance` | Mehr-/Mindermengen imbalance report |
| `get_billing_period` | MeterBillingPeriod (arbeitsmenge, spitzenleistung, brennwert) |
| `get_device_history` | MSB device history text for LanceDB RAG indexing |
| `get_quality_warnings` | Hampel-filter quality warnings (grade A/B/C/F) |
| `list_reading_orders` | Ablesesteuerung orders for a MaLo |
| `list_overdue_reading_orders` | §40 EnWG compliance gaps |
| `trigger_jahresablesung` | Launch or preview annual reading campaign |
| `get_correction_history` | Bitemporal correction audit trail (§22 MessZV) |
| `validate_timeseries` | Run V01–V10 validation on stored meter reads |

Prompts: `analyze-consumption`, `submit-mscons`, `quality-assessment`, `jahresablesung-workflow`, `reading-order-lifecycle`.

---
│                                                                       │
---

## Inbound event routing

| `ce_type` | `makopid` | Action |
|-----------|-----------|--------|
| `de.mako.process.completed` | MSCONS set | Store meter readings |
| `de.mako.process.initiated` | 23001 (INSRPT Störungsmeldung) | Auto-create `INSRPT_STOERUNG` reading order (§18 MessZV) |
| anything else | — | 204 No Content (ignored) |

## BO4E `Energiemenge` deliveries export

`GET /api/v1/deliveries/{malo_id}?from=RFC3339&to=RFC3339`

Returns all stored meter readings for a MaLo as a **BO4E `Energiemenge` array** —
the canonical business object for metered energy quantities, identical in
structure to what MSCONS messages carry per OBIS register per interval.

This endpoint is the primary data feed for ERP billing-import pipelines and
Mehr-/Mindermengen reconciliation tools. The response is a hard-typed BO4E
contract — not a raw database dump — so ERP systems can consume it without
parsing EDIFACT format-version details.

```bash
curl -s "http://edmd:8380/api/v1/deliveries/10001234567?from=2026-01-01T00:00:00Z&to=2026-04-01T00:00:00Z" \
  -H "Authorization: Bearer <token>" | jq '.[0] | {
    obisKennzahl,
    menge_wert: .menge.wert,
    menge_einheit: .menge.einheit,
    zeitraum_start: .zeitraum.startdatum,
    zeitraum_ende:  .zeitraum.enddatum
  }'
```

Response shape (one `Energiemenge` per stored interval read):

```json
[
  {
    "_typ": "ENERGIEMENGE",
    "obisKennzahl": "1-0:1.29.0",
    "menge": {
      "wert": 42.375,
      "einheit": "KWH"
    },
    "zeitraum": {
      "startdatum": "2026-01-01",
      "startuhrzeit": "00:00:00+00:00",
      "enddatum":    "2026-01-01",
      "enduhrzeit":  "00:15:00+00:00"
    }
  }
]
```

**Filtering.** Both `from` and `to` are optional; omitting them returns all
stored readings. Times are RFC 3339 UTC; use `?from=2026-01-01T00:00:00Z`
for calendar-day boundaries.

**Grouping.** One `Energiemenge` object per stored interval row. For grouped
aggregate views (one object per register with all intervals nested), use
`GET /api/v1/lastgang/{malo_id}` instead.

**Cedar action:** `read-timeseries`

---

## `MeterBillingPeriod`

The `MeterBillingPeriod` struct contains the billing-relevant quantities for
a MaLo over a calendar billing period:

| Field | Type | Source |
|-------|------|--------|
| `spitzenleistung_kw` | `Option<f64>` | RLM: highest 15-min demand in kW |
| `brennwert_kwh_per_m3` | `Option<f64>` | Gas: calorific value (Brennwert H) |
| `zustandszahl` | `Option<f64>` | Gas: state conversion factor |
| `total_kwh` | `f64` | Consumption sum over billing period |

Used by `netzbilanzd` to compute the Leistungspreisanteil (kW × kW-price)
and Gas quantity conversion (m³ × Brennwert × Zustandszahl = kWh).

---

## BO4E `Zeitreihe` export

`GET /api/v1/zeitreihe/{malo_id}?from=RFC3339&to=RFC3339`

Returns the meter time series as a **BO4E `Zeitreihe`** object array — the
generic time-series format used by API-Webdienste Strom consumers. Unlike
`Lastgang`, `Zeitreihe` carries commodity metadata (`medium`, `messart`,
`einheit`) without interval-specific fields (`zeit_intervall_laenge`, OBIS
structure). One `Zeitreihe` is returned per distinct OBIS register.

```bash
curl -s "http://edmd:8380/api/v1/zeitreihe/10001234567?from=2026-01-01T00:00:00Z&to=2026-02-01T00:00:00Z" \
  -H "Authorization: Bearer <token>" | jq '.[0] | {
    bezeichnung,
    medium,
    messart,
    einheit,
    werte_count: (.werte | length)
  }'
```

Response shape:

```json
[
  {
    "bezeichnung": "Zeitreihe MaLo 10001234567 OBIS 1-0:1.29.0",
    "medium":      "STROM",
    "messart":     "MITTELWERT",
    "einheit":     "KWH",
    "werte": [
      {
        "zeitraum": {
          "startdatum": "2026-01-01", "startuhrzeit": "00:00:00+00:00",
          "enddatum":   "2026-01-01", "enduhrzeit":   "00:15:00+00:00"
        },
        "wert": 1.234,
        "status": "ABGELESEN"
      }
    ]
  }
]
```

**When to use `Zeitreihe` vs. `Lastgang`.** Use `Lastgang` when the consumer
needs interval metadata (register, sparte, interval length) for structured
RLM/SLP processing. Use `Zeitreihe` when the consumer is an API-Webdienste
Strom client that expects the generic time-series contract, or when the
commodity context (`medium`, `messart`) is more relevant than the EDIFACT
structure.

---

## BO4E `Lastgang` export

`GET /api/v1/lastgang/{malo_id}?from=RFC3339&to=RFC3339`

Returns the meter time series as a **BO4E `Lastgang`** object array, suitable
for direct import into ERP systems and for the API-Webdienste Strom interface.
Readings are grouped by OBIS-Kennzahl — one `Lastgang` per distinct measurement
register.

```bash
curl -s "http://edmd:8380/api/v1/lastgang/10001234567?from=2026-01-01T00:00:00Z&to=2026-02-01T00:00:00Z" \
  -H "Authorization: Bearer <token>" | jq '.[0] | {
    sparte,
    obis_kennzahl,
    zeit_intervall_laenge,
    werte_count: (.werte | length)
  }'
```

Response shape (one element per OBIS register):

```json
[
  {
    "sparte": "STROM",
    "obis_kennzahl": "1-0:1.29.0",
    "zeitIntervallLaenge": { "wert": 15, "einheit": "VIERTELSTUNDE" },
    "werte": [
      {
        "zeitraum": {
          "startdatum": "2026-01-01", "startuhrzeit": "00:00:00+00:00",
          "enddatum":   "2026-01-01", "enduhrzeit":   "00:15:00+00:00"
        },
        "wert": 1.234,
        "status": "ABGELESEN"
      }
    ]
  }
]
```

**Interval detection.** The `zeitIntervallLaenge` is inferred from the first
consecutive read pair (15 min → `VIERTELSTUNDE`, 60 min → `MINUTE(60)`, 1440
min → `TAG`). RLM reads are typically 15-minute intervals.

**OBIS codes.** Each `MeterRead` carries an optional `obis_code` field
populated from the MSCONS PIA segment. Common values:

| OBIS | Meaning | Sparte |
|------|---------|--------|
| `1-0:1.8.0` | Active energy import, cumulative | Strom |
| `1-0:1.29.0` | Active energy max demand (Spitzenleistung) | Strom RLM |
| `7-20:3.0.0` | Gas volume unconverted (m³) | Gas |
| `7-20:15.0.0` | Gas energy (kWh, after Brennwert conversion) | Gas |

---

## Ablesesteuerung — Reading Order API

All three market roles schedule meter readings through the same `edmd` API.
Reading orders are stored in `ablese_auftraege` and linked to `auftrag_positionen`
(O2C) or MaKo process IDs (makod-triggered).

```mermaid
sequenceDiagram
    autonumber
    participant LF as vertragd (LF)
    participant edmd
    participant MSB as MSB / iMSys
    participant billingd

    LF->>edmd: POST /api/v1/reading-orders<br/>{ malo_id, anlass: "LIEFERBEGINN",<br/>  auftraggeber_rolle: "LF",<br/>  geplant_am: lieferbeginn_date }
    edmd-->>LF: 201 { id, status: "OFFEN" }

    Note over MSB: Field technician or iMSys<br/>auto-reads on geplant_am

    MSB->>edmd: PUT /api/v1/reading-orders/{id}/complete<br/>{ zaehlerstand_kwh: 12345.678 }
    edmd-->>MSB: 204 No Content

    Note over edmd: status = AUSGEFUEHRT<br/>emits de.edmd.ablesung.ausgefuehrt

    edmd->>billingd: de.edmd.ablesung.ausgefuehrt CloudEvent
    Note over billingd: Schlussrechnung can now<br/>use actual reading value
```

### Anlass types

| Anlass | Triggered by | Purpose |
|---|---|---|
| `LIEFERBEGINN` | `vertragd` after NB confirms Lieferbeginn | Billing cutoff for outgoing supplier |
| `LIEFERENDE` | `vertragd` on Kündigung | Billing cutoff for final invoice |
| `JAHRESABLESUNG` | NB background job or ERP | §40 EnWG annual billing accuracy |
| `ZWISCHENABLESUNG` | LF or ERP | On-demand (tariff change, billing dispute) |
| `EINZUG` | NB on customer move-in | |
| `AUSZUG` | NB on customer move-out | |
| `SPERRUNG` | `sperrd` before disconnection | §19 StromGVV / §33 GasGVV |
| `ENTSPERRUNG` | `sperrd` after reconnection | |
| `SONDERABLESUNG` | MSB on `INSRPT` fault | Billing restart after meter replacement |
| `ISMS_AUSLESUNG` | iMSys automatic | Smart meter daily/15-min auto-readout |

### Endpoints

| Method | Path | Description |
|---|---|---|
| `POST` | `/api/v1/reading-orders` | Create reading order |
| `GET` | `/api/v1/reading-orders` | List (`?malo_id=&status=&anlass=&limit=`) |
| `GET` | `/api/v1/reading-orders/{id}` | Get status and result |
| `PUT` | `/api/v1/reading-orders/{id}/complete` | Record meter reading result |
| `PUT` | `/api/v1/reading-orders/{id}/cancel` | Cancel pending order |

### iMSys auto-close

For smart meters (iMSys), MSCONS data arrives automatically via `makod` → `edmd` webhook.
`edmd` auto-closes open reading orders for the same `malo_id` when the MSCONS timestamp
matches `geplant_am` within ±1 day.

---

## Configuration reference

`edmd` reads its configuration from a **TOML file** (default: `edmd.toml`),
with secrets deferred to environment variables via `"env:VAR_NAME"` values.

### CLI flags

| Flag | Env var | Default | Description |
|------|---------|---------|-------------|
| `--config` / `-c` | `EDMD_CONFIG` | `edmd.toml` | Path to `edmd.toml` |
| `--log-level` | `RUST_LOG` | `info` | Log level |
| `--check` | `EDMD_CHECK` | `false` | Validate config + DB connectivity, then exit 0. Used by Dockerfile HEALTHCHECK. |

```bash
edmd --config /etc/edmd/edmd.toml
# or: EDMD_CONFIG=/etc/edmd/edmd.toml edmd
```

### Full `edmd.toml` reference

```toml
[http]
addr = "0.0.0.0:8380"          # default

[database]
url       = "env:DATABASE_URL"  # required; use env: for secrets
pool_size = 10                  # default

[identity]
tenant = "9900357000004"        # required — MP-ID of the operator

[marktd]
url     = "http://marktd:8180"       # required
api_key = "env:EDMD_MARKTD_API_KEY" # required

[webhook]
inbound_secret = "env:EDMD_INBOUND_SECRET"  # optional; omit for dev

[subscription]
# Self-registers with marktd on startup — no manual curl required.
webhook_url   = "http://edmd:8380/webhook"  # public URL marktd POSTs to
subscriber_id = "edmd"                       # default
event_types   = [
  "de.mako.process.initiated",
  "de.mako.process.completed",
  "de.mako.edifact.inbound",
]

# [oidc]          # omit to disable auth (dev only — never omit in production)
# issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
# audience = "api://mako-edmd"
# jwks_refresh_secs = 300

# [otel]          # omit to disable tracing
# endpoint = "http://otel-collector:4317"
```

---

## marktd subscription

`edmd` **auto-registers** its EventBus subscription with `marktd` on startup
when `subscription.webhook_url` is set in the config — no manual `curl` required.

To force re-registration or verify the subscription:

```bash
curl -s http://marktd:8180/api/v1/subscriptions/edmd \
  -H "Authorization: Bearer <token>" | jq .
```

---

## Query examples

```bash
# BO4E Energiemenge — all meter readings for a MaLo (typed, ERP-consumable)
curl -s "http://edmd:8380/api/v1/deliveries/10001234567?from=2026-01-01T00:00:00Z&to=2026-04-01T00:00:00Z" \
  -H "Authorization: Bearer <token>" | jq '.[0] | {obisKennzahl, menge_kwh: .menge.wert}'

# Billing period for a MaLo (used by netzbilanzd)
curl -s "http://edmd:8380/api/v1/billing-period/10001234567?from=2026-01-01&to=2026-03-31" \
  -H "Authorization: Bearer <token>" | jq '{
    spitzenleistung_kw,
    arbeitsmenge_kwh,
    period_from,
    period_to
  }'

# Mehr-/Mindermengensaldo for January 2026
curl -s "http://edmd:8380/api/v1/imbalance/10001234567/2026/1" \
  -H "Authorization: Bearer <token>" | jq .

# BO4E Lastgang export — one object per OBIS register
curl -s "http://edmd:8380/api/v1/lastgang/10001234567?from=2026-01-01T00:00:00Z&to=2026-02-01T00:00:00Z" \
  -H "Authorization: Bearer <token>" | jq '.[0] | {sparte, obis_kennzahl, zeit_intervall_laenge}'

# BO4E Zeitreihe export — one object per OBIS register (medium/messart metadata)
curl -s "http://edmd:8380/api/v1/zeitreihe/10001234567?from=2026-01-01T00:00:00Z&to=2026-02-01T00:00:00Z" \
  -H "Authorization: Bearer <token>" | jq '.[0] | {bezeichnung, medium, messart, einheit}'
```

---

## Apache Iceberg / S3 OLAP archival

`edmd` can automatically offload `meter_reads` older than the configured
retention window to **Apache Iceberg V2 tables** on S3, GCS, or Azure Data Lake.
A **PostgreSQL-backed SQL catalog** (`iceberg-catalog-sql`) stores all table
metadata (schema, partition spec, snapshots, manifests) in the same database that
`edmd` already manages — no Nessie, Apache Polaris, or AWS Glue required.
[Apache DataFusion](https://arrow.apache.org/datafusion/) executes SQL queries
over the Parquet files with Iceberg partition pruning for ≥ 10× faster MMM
aggregation versus full PostgreSQL scans.

### Why Iceberg?

| Challenge | Solution |
|---|---|
| 35 000 rows/RLM MaLo/year — PG scan degrades after year 2 | Parquet columnar format on object storage |
| MMM aggregation spans 3+ years | DataFusion pushes predicates to Iceberg partitions + Parquet row-group statistics |
| Multi-engine access (Spark, Trino, DuckDB) | Iceberg V2 table format via `iceberg = "0.9.1"` |
| No external catalog service | `iceberg-catalog-sql` stores metadata in existing PostgreSQL |

### File layout

```
{storage_uri}/
  data/
    sparte=STROM/                    ← identity(sparte) partition
      dtm_from_year=2024/            ← year(dtm_from)
        dtm_from_month=1/            ← month(dtm_from)
          edmd-archive-{uuid}.parquet
    sparte=GAS/
      dtm_from_year=2024/
        ...
  metadata/
    v1.metadata.json                 ← Iceberg V2 table metadata
```

### Configuration

```toml
[archive]
enabled                = true
storage_uri            = "s3://my-bucket/edmd/meter_reads"
access_key_id          = "env:AWS_ACCESS_KEY_ID"
secret_access_key      = "env:AWS_SECRET_ACCESS_KEY"
region                 = "eu-central-1"
# Optional — for MinIO, Ceph RGW, LocalStack:
# endpoint_url         = "http://minio:9000"
retention_months       = 12      # keep in PostgreSQL for this many months
batch_size             = 100000  # rows per archive run
interval_secs          = 3600    # run every hour
# Iceberg catalog in the same PostgreSQL — no extra service:
iceberg_catalog_schema = "iceberg_catalog"   # schema created automatically
iceberg_catalog_name   = "edmd"
```

### Archive OLAP endpoints

| Endpoint | Description |
|---|---|
| `GET /api/v1/archive/status` | Archival statistics (total batches, rows archived, bytes written) + 20 most recent batches |
| `GET /api/v1/archive/olap/{malo_id}?from=&to=` | **MMM aggregation**: total kWh, read count, period bounds for one MaLo from the cold tier |
| `GET /api/v1/archive/portfolio?from=&to=&limit=N` | Portfolio-level aggregation: top-N MaLo by consumption across the full archive |
| `GET /api/v1/archive/timeseries/{malo_id}?from=&to=` | Historical time-series export from Parquet (up to 50 000 rows) |

**Typical `mmm_aggregate` query** (executes via DataFusion over S3 Parquet):

```bash
curl "http://edmd:8380/api/v1/archive/olap/10001234567?from=2023-01-01T00:00:00Z&to=2025-12-31T23:59:59Z" \
  -H "Authorization: Bearer <token>" | jq '{total_kwh, read_count, period_from, period_to}'
```

Response:

```json
{
  "malo_id":     "10001234567",
  "total_kwh":   123456.789,
  "read_count":  105120,
  "period_from": "2023-01-01T00:00:00Z",
  "period_to":   "2025-12-31T23:45:00Z",
  "source":      "iceberg-archive"
}
```

### Dependencies

| Crate | Version | Purpose |
|---|---|---|
| `iceberg` | 0.9.1 | Apache Iceberg core — FileIO, table spec, writer |
| `iceberg-storage-opendal` | 0.9.1 | S3/GCS/AzureDLS storage via opendal 0.55 |
| `iceberg-datafusion` | 0.9.1 | `IcebergTableProvider` for DataFusion SQL |
| `iceberg-catalog-sql` | 0.9.1 | PostgreSQL-backed Iceberg catalog |
| `datafusion` | 52 | SQL query engine + partition pruning |
| MSRV | **1.94** | Required by iceberg 0.9.1 |

---

## Cedar ABAC

`edmd` uses Cedar for access control. Grant the `read-timeseries` action to
principals that need meter data access:

```cedar
permit(
  principal,
  action == Action::"read-timeseries",
  resource
) when {
  context.principal_tenant == context.resource_tenant
};
```

Add `read-archive-olap` for access to Iceberg OLAP endpoints:

```cedar
permit(
  principal,
  action == Action::"read-archive-olap",
  resource
) when {
  context.principal_tenant == context.resource_tenant
};
```

---

## Monitoring

| Metric | Target |
|--------|--------|
| Webhook `de.mako.edifact.inbound` success rate | > 99 % |
| DB pool utilisation | < 80 % |
| `meter_reads` rows with `archived = false` and `dtm_from < now() - retention_months` | Should decrease each hour when archival is enabled |

