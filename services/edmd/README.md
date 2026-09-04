# edmd — Energy Data Management daemon

`edmd` stores MSCONS meter readings received from `marktd`, accepts direct iMSys/SMGW interval push, scores data quality with a Hampel filter, schedules reading orders (Ablesesteuerung), and serves BO4E typed time-series and imbalance queries — `Energiemenge` deliveries for ERP billing import, `Lastgang`/`Zeitreihe` for API-Webdienste Strom, `MeterBillingPeriod` for `netzbilanzd`, and Mehr-/Mindermengen reconciliation for `invoicd`. `meter_reads` (and the non-authoritative `esa_typ2_reads` stream) are stored through the [`meterstore`](https://github.com/hupe1980/meterstore) crate — a recent window in PostgreSQL and the settled history in **Apache Iceberg V2** on S3/GCS/Azure, split by a tiering watermark that meterstore owns.

| | |
|---|---|
| HTTP port | `:8380` · MCP `POST\|GET /mcp` (15 tools, 5 prompts) |
| Database | PostgreSQL 17+ (sqlx 0.8, `migrations/0001_schema.sql`) |
| Readings | `meter_reads`, `esa_typ2_reads`, `meter_readings` — meterstore-owned, hot PostgreSQL window + settled Iceberg V2 history |
| Own tables | `meter_billing_periods`, `zsg_conversion_log`, `ablese_auftraege`, `meter_read_corrections`, `substitute_value_log`, `quality_assessments`, `direct_push_sessions`, `gdpr_deletions` |
| Auth | OIDC/JWT + Cedar ABAC — reads tenant-scoped, writes role-gated; service-to-service keys via `[[oidc.service_keys]]`. Refuses to start without `[oidc]` unless `allow_insecure_no_auth` |
| Lifecycle | `mako_service::run` — tracing, tuned pool, migrations, DB-ping readiness, `/metrics`, graceful shutdown |
| Rate limiting | Per-tenant and global GCRA; `429` carries `Retry-After` |

---

## Ingest

| Door | Path |
|---|---|
| MSCONS | CloudEvents from `marktd` — billing PIDs 13005–13025 → `meter_reads`, **13027** → `esa_typ2_reads` |
| Direct push | `POST /api/v1/meter-reads/rlm/{malo_id}` (Strom), `.../gas/{malo_id}` (m³→kWh_Hs), `.../iot/{malo_id}` — idempotent on `session_id` |
| Bulk | `POST /api/v1/meter-reads/{malo_id}/bulk` — up to 50 000 intervals, validated and written in one statement |
| Zählerstandsgang | `POST\|GET /api/v1/zaehlerstandsgang/{malo_id}` |
| Kafka | Optional `[kafka_ingest]` consumer for head-end systems |
| Reading orders | INSRPT 23001–23009 and GPKE 55001/55004/55007 open orders automatically |

**An ingest door answers by whether a retry could work.** `5xx` for a transient
store failure, so the fan-out redelivers; **422** for a delivery the store
*refused* — an overlapping span, a value restated under an existing version, a
second network operator on one reading. A refusal never succeeds on a retry, so
answering `5xx` would redeliver a poison message for its whole retry budget.

An unrecognised `sparte` or `source` on the Kafka path is refused, not coerced:
read as STROM/MSCONS, a mislabelled gas batch stores as electricity with EDIFACT
provenance. Without the optional per-message HMAC the **topic ACL is the trust
boundary** — restrict produce rights to the head-end system.

**Overlap is a constraint.** Per-partition `EXCLUDE USING gist` (`btree_gist`)
refuses a delivery whose range overlaps a stored reading rather than
double-counting it.

---

## Validation and quality

`store_reads` takes a **`ValidatedReads`**, not a slice — its only constructor
runs V01–V09/V11/V12 and its field is private, so no ingest path can persist
unvalidated data, and taking it **by value** stops a caller keeping the raw
batch. It annotates, never rejects: billability is a separate decision from
storage (§ 147 AO / GoBD).

One pass — `domain::validation::findings` — is shared by every door, the MCP
`validate_timeseries` tool and the § 60 Abs. 2 substitute path. The batch splits
by **(Sparte, OBIS register)** first: V01/V02 are statements about a single
series, and a prosumer MeLo delivering import beside export at one slot would
otherwise trip V02 on every interval. Thresholds come from
`QualityConfig::for_sparte` and the cadence is **observed**, so an hourly gas
series is not judged as a broken quarter-hour one.

**V12 needs a ceiling from the caller.** `ImplausiblePower` is impossible against
nothing, and edmd holds no master data, so `max_plant_power_kw` is optional on
the push, bulk, IoT and Kafka bodies, the MSCONS payload and the MCP tool.
Absent, the rule stays off — an invented ceiling would block billing on a reading
that is merely large. Every response names the rules that did **not** run
(`skipped_rules`, `rules_evaluated`/`rules_skipped`), so a clean verdict states
what stands behind it.

`compute_quality(samples, sparte, from, to)` scores each register on its own and
folds worst-first — Hampel filter with the commodity's window/σ/σ-floor, grades
A/B/C/F, coverage against the requested window. It needs more than 2×window
intervals; a shorter series is scored without outlier detection rather than with
a claim the data cannot bear. Retroactive:
`POST /api/v1/quality-score/{malo_id}`. Every verdict lands in
`quality_assessments` under the batch's own `IngestionSource` — the § 147 AO
history is only as complete as its least-covered path.

---

## Registers, not series

**A MaLo is a set of registers.** meterstore reads span channels, so every path
that folds readings into a figure goes through `domain::register`.

- `energy_intervals(reads, direction)` — for anything that **sums**. Drops
  non-billable qualities, kvarh/kW/fault registers and the other direction, and
  uses the total register *instead of* the HT/NT intervals it overlaps
  (`1.8.0 = 1.8.1 + 1.8.2`; summing all three bills the consumption twice) while
  summing tariff intervals no total covers. Per interval, not per window: a meter
  reconfigured mid-month reports the total for part of it and the split for the
  rest.
- `register_groups(reads)` — for anything judging a series' *shape*, where mixed
  registers would make the cadence a median across channels and multiply
  coverage.

A reading is stored **and labelled** in its Sparte's billing unit. Gas converts
to kWh_Hs at ingest (§ 25 Nr. 4 MessEV / DVGW G 685); only water stays a volume.

---

## Zählerstandsgang

`§ 2 Satz 1 Nr. 27 MsbG` makes the ZSG what an iMSys measures — viertelstündig
ermittelte Zählerstände für Strom, stündlich für Gas — and **BK6-24-174**, whose
own subject is „Übermittlung von Zählerstandsgängen", puts the differencing at
the MSB. edmd is the MSB, so both halves are stored and both are tiered: the
register readings in the `meter_readings` **point** table (§ 146 Abs. 4 AO — a
stored difference cannot reproduce the values it came from) and the derived
intervals relabelled onto the Lastgang register (`1-0:1.8.0` is a Zählerstand,
`1-0:1.29.0` is the Lastgang).

Two consequences. A Zählerstand is stored **unconverted** — § 25 Nr. 4 MessEV
converts the *difference*, so a gas register stays m³ while its interval is
kWh_Hs. And the ZSG is keyed by **Messlokation** as well as register, because a
Marktlokation may be measured by several meters carrying the same OBIS code at
the same instants.

Where no honest difference exists — a backwards step no `register_digits`
explains, a jump beyond `max_plant_power_kw` — **no interval is emitted** and the
reason goes to `zsg_conversion_log`. The hole surfaces as a V01 gap and the § 60
Abs. 2 substitute path fills it with its own row, so the two logs together say
"this quarter-hour is an Ersatzwert *because* the register went backwards here".

---

## Substitutes, corrections and confirmations

A § 60 Abs. 2 substitute is filed **under the register it fills** — the request's
`obis_code`, or the point's dominant energy register. An unlabelled reading *is*
the canonical total register, so on a dual-tariff point reporting only HT and NT
an unlabelled substitute would make the month read as its own decomposition.
`interval_secs` defaults to the register's observed cadence, not a flat 900 s.

**Authored writes supersede.** A delivery may legitimately be shadowed by a newer
one; a value edmd *authors* may not. An operator correction and an Ersatzwert
carry no MSCONS version, so `store::append_superseding` re-asserts them above the
version that actually holds — taken from the store's own displacement report —
and errors after four contested attempts.

Every value-changing overwrite — corrections **and** redeliveries, on every
transport — leaves an immutable `meter_read_corrections` row (§ 147 Abs. 1 AO /
§ 146 Abs. 4 AO), and `?as_of=` reconstructs prior knowledge states.

Every stored ESTIMATED/SUBSTITUTED interval opens an obligation in
`estimated_read_confirmations`, discharged when a MEASURED/CORRECTED value for
the slot arrives. A daily worker escalates stale ones to UEBERFAELLIG
(`deadline_weeks = 8`, aligned with the MaBiS BKA correction window — no statute
fixes a number); `GET /api/v1/confirmations?status=` lists them.

MSCONS **13006** is *Messwert Storno*: the receipt is recorded and the payload is
**not** stored, because booking withdrawn quantities as freshly measured ones is
the opposite of what the message says.

---

## Delivery surveillance

Every V-rule and the Hampel scorer judge data that *arrived*; silence triggers
nothing. An hourly sweep asks which measuring points have **not** delivered —
`SILENT` (nothing for `silent_after_hours`, default 36) or `UNDER_COVERED` (under
`min_coverage_pct` of the window) — and emits
`de.messwert.reading.delivery.overdue` / `.resumed` on the transitions. Coverage
is a *duration* ratio, so a point legitimately moving from ¼h to hourly is not a
finding; a point that never delivered is `marktd`'s question, not edmd's.
`GET /api/v1/surveillance/delivery` · `POST …/scan`.

The **ESA Typ-2** stream is swept separately — own threshold, own rows
(`stream = 'TYP2'`), own events — and keyed per **(Meldepunkt, subscription,
register)**, because one Meldepunkt may carry several subscriptions and two
sharing an OBIS register would mask each other going silent.

---

## ESA Typ-2 is not billing data

ESA-delivered „Werte nach Typ 2" (MSCONS **13027**) are **non-authoritative**
(Codeliste 1.4 Kap. 4.6 · WiM Strom Teil 2 § 4): no bearing on Netznutzungs-,
Bilanzkreis- or Mehr-/Mindermengenabrechnung. They land in `esa_typ2_reads`,
never `meter_reads`, so no billing query can reach them by omission — no
correction, substitution, reconciliation or billing-period participation.

Kapitel 4.6 has **two** delivery paths: 4.6.1 arrives as MSCONS 13027 over AS4
through `makod`; 4.6.2 comes as XML straight from the iMS over SM-PKI, and
`POST /api/v1/esa/typ2/{malo_id}` is that second door (`write-esa-typ2`, its own
Cedar action, so it cannot reach the authoritative store). Both Typ-2 relations
are **refused by name** on `POST /api/v1/query/sql`: the tables share a
DataFusion session, and free-form SQL is the one surface where the separation
would otherwise be a naming convention rather than a type.

---

## SMGW and § 14a

A daily worker sweeps every certificate in `smgw_sessions` and emits
`de.messwert.smgw.cert.expiry-warning` at **90 / 30 / 7 days** before `valid_to`,
once per tier per certificate. The ladder is **operational, not statutory** —
BSI TR-03109-4 binds certificate *runtimes* while the renewal lead time lives in
the Root-CP — so it is configurable. An expired cert silently ends § 14a
Fernsteuerbarkeit.

`cls_compliance_issues` is a register of **what is wrong now**, keyed on the
identity of the fault rather than when it was noticed: events fire on the
transitions, not once per sweep, or a gateway on an expired certificate would
emit a CloudEvent a day forever. The duty is **§ 25 MsbG** — the GWA's monitoring
and maintenance responsibility — not § 21c (which does not exist) or § 29 (the
rollout obligation).

---

## Reads

| Path | Purpose |
|---|---|
| `GET /api/v1/energy/{malo_id}?direction=` | **The canonical projected series** — one direction through `domain::register`, with quality, duration-ratio coverage at the observed cadence and `billable_pct` for the § 60 Abs. 2 gate |
| `GET /api/v1/deliveries/{malo_id}` | BO4E `Energiemenge` for ERP billing import |
| `GET /api/v1/lastgang/{malo_id}` · `/zeitreihe/{malo_id}` | API-Webdienste Strom shapes; Arrow IPC on `Accept: application/vnd.apache.arrow.stream` |
| `GET /api/v1/billing-period/{malo_id}` · `/billing-periods` | `MeterBillingPeriod` for `netzbilanzd`; the plural answers from the **readings**, not the cache, and reports `truncated` |
| `GET /api/v1/imbalance/{malo_id}/{year}/{month}?bilanziert_kwh=` | Mehr-/Mindermengen |
| `GET /api/v1/netzverlust?from=&to=` | § 22 EnWG indicative grid-loss balance |
| `GET /api/v1/sharing/{community_id}/allocation` | § 42c allocation — `community_id` is the shared plant's MeLo |
| `GET /api/v1/archive/…` · `POST /api/v1/query/sql` | Cold tier over DataFusion, JSON or Arrow IPC |
| `GET /api/v1/iceberg/v1/…` | Read-only Iceberg REST catalog; DuckDB / Spark / Trino / PyIceberg attach and read Parquet with their own credentials |
| `GET /api/v1/compliance/jahresablesung/{year}` | Only `AUSGEFUEHRT` discharges the § 40b Abs. 1 EnWG obligation |

Every materialising read defaults to the **last 31 days** and refuses a window
wider than **732 days**; a malformed `?from=`/`?to=` is a `400`, not a silent
fallback. Bulk history goes over Arrow IPC, SQL or Iceberg — the three paths that
stream instead of materialising. `quantity_kwh` is `Decimal128(18,5)` over Arrow,
matching the storage column, because binary floating point cannot represent
0.1 kWh.

**Mehr-/Mindermengen needs both halves and edmd holds one.** `?bilanziert_kwh=`
is required; without it the endpoint answers `422` rather than comparing the
measured total against itself. Under the profile is a **Mehr**menge the NB
credits (GPKE Teil 1 Kap. 8.4 Nr. 3), and the measured half is the **Bezug**,
register-projected.

**Balancing day.** Electricity balances on the calendar day, **gas on the Gastag
(06:00–06:00)** — GaBi Gas, Art. 3 Nr. 6 VO (EU) 312/2014. `?sparte=` selects it;
aggregating gas over calendar days misbooks six hours into the neighbouring
Bilanzierungstag every day. The long and short Gastag is the one named after the
**Saturday**, because the clocks change before 06:00.

`§ 40 Abs. 2 Nr. 6 EnWG`: the invoice's opening and closing Zählerstand is the
last reading **at or before** each period bound — one dated after the period end
did not hold at the period end.

---

## Reading orders

`POST/GET /api/v1/reading-orders` — Ablesesteuerung for LF/MSB/NB/ESA (an ESA may
order value delivery, § 60 Abs. 1 MsbG), with `/complete`, `/cancel` and `/fail`
(Ablesehindernis). INSRPT PID 23001 auto-creates an `INSRPT_STOERUNG` order.

**A completed Ablesung files its Zählerstand into `meter_readings`** — for an
**SLP** point, with no interval metering at all, the year-on-year register
difference is the entire billing path. The order names its `sparte`, `melo_id`
and `obis_code`: a reading belongs to one register of one meter, and a
Zählerstand in the wrong dimension is refused rather than filed.

---

## GDPR

`DELETE /api/v1/gdpr/erasure/{malo_id}` — Art. 17 pseudonymisation in one
transaction. It destroys the MaLo's subject mapping in meterstore's registry,
which unlinks **both** reading stores at once (non-authoritative is a statement
about settlement, not about personal data); **rewrites `malo_id` to that subject
reference** in the Buchungsbeleg tables it may not delete (§ 147 Abs. 1 AO,
Art. 17 Abs. 3 lit. b DSGVO); and deletes the derived, operational and device
tables outright.

---

## CloudEvents emitted

`de.messwert.reading.direct.stored` · `.delivery.overdue` / `.resumed` ·
`.quality.warning` (Hampel grade C/F **or** any V-rule finding, from every ingest
door — the same predicate as the `202` status, so the two cannot disagree) ·
`.order.failed` · `de.messwert.cls.compliance-issue` / `-resolved` ·
`de.messwert.smgw.cert.expiry-warning`

---

## Quick Start

```bash
edmd --config edmd.toml
```

Migrations run automatically at startup from `migrations/0001_schema.sql`.
The schema is designed for a fresh install — no incremental migration state is maintained.

---

## Configuration

All settings live in `edmd.toml`, loaded by `mako_service::run` — the same
loader every mako daemon uses, so `EDMD_CONFIG` names the file and
`EDMD_<SECTION>__<KEY>` env vars override individual keys.

| Flag | Env var | Description |
|---|---|---|
| `--check` | — | Probe the **already-running** instance's `/health/ready` on loopback and exit 0/1 |
| — | `EDMD_CONFIG` | Path to the configuration file (default `edmd.toml`) |
| — | `RUST_LOG` | Log level (default `info`) |

`--check` is the container `HEALTHCHECK`: it needs no shell and no curl, which
suits the distroless image.

### Sections

```toml
# Required unless an [oidc] section is present. Without token verification every
# request is admitted as `dev-admin` with all market roles.
allow_insecure_no_auth = false

[http]
addr = "0.0.0.0:8380"

[database]
url       = "env:EDMD_DATABASE_URL"
pool_size = 10

[identity]
tenant = "9900357000004"          # BDEW Codenummer

[marktd]
url     = "http://marktd:8180"
api_key = "env:EDMD_MARKTD_API_KEY"

[subscription]
subscriber_id = "edmd"
webhook_url   = "http://edmd:8380/webhook"

[webhook]
inbound_secret   = "env:EDMD_INBOUND_SECRET"   # verifies webhook-signature
erp_webhook_url  = "http://erp:9000/events"    # outbound CloudEvents

[oidc]
issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
audience = "api://mako-edmd"

# Service-to-service keys accepted alongside OIDC JWTs. Internal callers
# (einsd/billingd/vertragd/portald) send an opaque Bearer key, not a JWT — a
# match authenticates as that service principal (this deployment's tenant, the
# listed roles). Without this, those callers get 401 against a secured edmd.
[[oidc.service_keys]]
name  = "einsd"                       # → principal sub
key   = "env:EDMD_SERVICE_KEY_EINSD"  # must equal the caller's edmd_api_key
roles = ["NB", "MSB"]

[rate_limit]
requests_per_second            = 500    # global sustained
burst                          = 1000   # ingest is bursty by nature
per_tenant_requests_per_second = 100

[mcp]
api_key = "env:EDMD_MCP_API_KEY"

# §14a SMGW/CLS compliance sweeps. These were function parameters every call
# site passed 30 and 2 to, while the docs called them configurable.
[smgw]
enabled                    = true
cert_warning_days          = 30     # operational, not a TR deadline
comm_fault_threshold_hours = 2
sweep_interval_secs        = 86400

# Delivery surveillance — which measuring points have gone quiet.
[surveillance]
enabled              = true
silent_after_hours   = 36     # a daily cadence plus a retry window
min_coverage_pct     = 95.0
coverage_window_days = 7
sweep_interval_secs  = 3600
max_events_per_sweep = 500    # one broken head-end must not emit a fleet
typ2_enabled             = true  # watch the ESA "Werte nach Typ 2" stream too
typ2_silent_after_hours  = 36    # its own clock — see below

# Optional: high-throughput reading intake from a Kafka topic.
[kafka_ingest]
enabled           = true
bootstrap_servers = "kafka-1:9092,kafka-2:9092"
topic             = "edmd.meter-reads"   # default
group_id          = "edmd-ingest"        # default
```

### Archive configuration (`[archive]` in `edmd.toml`)

meterstore is a library edmd links in-process (the published `meterstore` crate)
and reads no config of its own, so its tiering knobs are supplied here:

```toml
[archive]
enabled             = true
storage_uri         = "s3://my-bucket/edmd/warehouse"   # scheme picks the backend
region              = "eu-central-1"
# access_key_id / secret_access_key optional — omit to use the instance-role chain.
# endpoint_url      = "http://minio:9000"   # S3-compatible (MinIO/Ceph/R2 → path-style)
settlement_lag_days = 7    # age at which an interval settles hot → cold (mutable below this)
archival_step_days  = 1    # watermark advance per sweep, and the partition granularity
cold_file_target_mib = 512 # target Parquet file size
maintenance_interval_secs = 3600  # tiering loop cycle period (archives due windows)
ddl_lock_timeout_secs = 3  # how long DDL waits for its lock before giving up
```

`enabled` turns the cold tier on; the `storage_uri` **scheme** selects the
warehouse backend — `file://`, `memory://` (dev), `s3://` (and S3-compatible
`minio://` / `r2://`), `gs://`, `abfss://`. For S3, credentials come from `region`
+ the optional `access_key_id`/`secret_access_key` (prefer `"env:…"` refs), or —
recommended in production — are **omitted** so the EC2/IRSA instance-role chain
supplies them. GCS and Azure authenticate through their platform chains (ADC /
managed identity). There is no retention window or archival interval to tune —
meterstore reclaims the hot tier through its tiering watermark and owns the Iceberg
format, partitioning and layout; `settlement_lag_days` is simply how long a reading
stays correctable in the hot tier before it settles.

---

## REST API

### `GET /api/v1/deliveries/{malo_id}`

Returns all typed meter reads for a Marktlokation within the given time range.

Query parameters: `from`, `to` (RFC 3339). Defaults to the last 31 days; the
maximum window is 732 days, and a malformed value is a `400`.

```bash
curl "http://localhost:8380/api/v1/deliveries/51238696012?from=2025-10-01T00:00:00Z&to=2026-10-01T00:00:00Z"
```

Response:

```json
[
  {
    "malo_id":      "51238696012",
    "melo_id":      "DE0001234567890123456789012345678",
    "dtm_from":     "2025-10-01T00:00:00Z",
    "dtm_to":       "2025-10-01T01:00:00Z",
    "quantity_kwh": "123.456",
    "quality":      "MEASURED",
    "pid":          13025
  }
]
```

### `GET /api/v1/archive/olap/{malo_id}`

MMM aggregation over meterstore's version-resolved, tier-split series (hot window
+ settled Iceberg history). Typical use case: MMM Jahresabrechnung spanning 3+
billing years.

Query parameters: `from`, `to` (ISO 8601).

```bash
curl "http://localhost:8380/api/v1/archive/olap/51238696012?from=2022-01-01T00:00:00Z&to=2024-12-31T23:59:59Z" \
  -H "Authorization: Bearer <token>"
```

Response:

```json
{
  "malo_id":    "51238696012",
  "total_kwh":  "98765.43200",
  "read_count": 105120,
  "from":       "2022-01-01 00:00:00 +00:00:00",
  "to":         "2024-12-31 23:59:59 +00:00:00"
}
```

### `GET /api/v1/imbalance/{malo_id}/{year}/{month}?bilanziert_kwh=`

The Mehr-/Mindermengensaldo for a single billing month.

`bilanziert_kwh` is **required**: the saldo compares the measured quantity
against the profile-allocated one, and only the first is a metering figure. The
bilanzierte Menge is what the Bilanzkreis was charged from the load profile — a
commercial figure from the Bilanzkreisabrechnung. Omit it and the endpoint
answers `422`, because the alternative is comparing the measured total against
itself and reporting a delta of zero.

Naming is from the **network operator's** side, which inverts the intuitive
reading (GPKE Teil 1 Kap. 8.4 Nr. 3): consuming *under* the profile leaves
surplus the NB absorbed — a **Mehr**menge, which the NB credits. Consuming over
it is a **Minder**menge, which the NB invoices. Only one is ever positive.

`gemessen_kwh` is the **Bezug**, projected onto one canonical register set
(`domain::register`). A measuring point reports several registers and the read
spans all of them, so the raw sum would add a prosumer's Einspeisung to its grid
draw and count a dual-tariff meter's consumption twice — `1-0:1.8.0` *is*
`1-0:1.8.1 + 1-0:1.8.2`. `interval_count` counts the projected series, and
`quality` is its worst contributor.

```bash
curl "http://localhost:8380/api/v1/imbalance/51238696012/2026/07?bilanziert_kwh=1000"
```

Response:

```json
{
  "malo_id":         "51238696012",
  "period_from":     "2026-07-01",
  "period_to":       "2026-07-31",
  "sparte":          "STROM",
  "gemessen_kwh":    "962.5",
  "bilanziert_kwh":  "1000",
  "mehrmenge_kwh":   "37.5",
  "mindermenge_kwh": "0",
  "delta_kwh":       "-37.5",
  "delta_pct":       "-3.75",
  "quality":         "MEASURED",
  "interval_count":  2976,
  "richtung":        "MEHRMENGE — Netzbetreiber vergütet dem Lieferanten",
  "legal_basis":     "GPKE (BK6-24-174) Teil 1 Kap. 8.4 (Strom) · GaBi Gas 2.1 (BK7-24-01-008) Ziff. 3a (Gas)"
}
```

`?sparte=gas` switches the period to the 06:00 Gastag.

---

## Database Schema

`migrations/0001_schema.sql` is the single authoritative DDL — the schema is
designed for a fresh install, so no incremental migration state is maintained.

| Area | Tables |
|---|---|
| Metered data receipts | `meter_data_receipts` · `meter_billing_periods` (billing-period cache) |
| Corrections & substitution | `meter_read_corrections` · `substitute_value_log` |
| Quality | `quality_assessments` |
| Surveillance | `delivery_surveillance` (open-issue register: which points stopped, keyed `(tenant, stream, malo_id, obis_code, subscription_ref)` — `stream` separates Typ-1 from ESA Typ-2, `subscription_ref` separates two ESA subscriptions at one Meldepunkt) |
| Reading orders | `ablese_auftraege` |
| Ingest sessions | `direct_push_sessions` |
| Gas | `gas_quality_data` |
| Virtual meters (§42b/§42c EnWG) | `virtual_meter_configs` |
| Devices | `smgw_sessions` · `cls_compliance_issues` (open-issue register) · `smgw_cert_expiry_alerts` — Gerätewechsel is `marktd`'s (master data), so edmd declares no `meter_exchange_events` |
| GDPR | `gdpr_deletions` (erasure requests) — the subject-mapping tables (`meterstore_subject_map` / `meterstore_erasures`) live in the same database but are meterstore's |

The authoritative `meter_reads` store and the non-authoritative `esa_typ2_reads`
store are **meterstore** tables (hot PostgreSQL + cold Iceberg), created and
owned by `store.create_tables()` — not declared in edmd's DDL. meterstore owns
their partitioning, version resolution and the hot-tier overlap exclusion.

All edmd tables carry `tenant TEXT NOT NULL` for multi-tenant isolation — the
operator's MP-ID (BDEW/DVGW Codenummer, not a UUID) — and every query filters
on it. edmd also declares `tenant` as meterstore's non-nullable identity column,
so two tenants' readings for one measuring point never merge.

---

## Event Routing

`edmd` subscribes to `de.mako.process.completed` events from `marktd` where `makopid`
is in the MSCONS PID set (`edmd::domain::MSCONS_PIDS`). On receipt:

1. Verifies the Standard Webhooks signature (if configured)
2. Parses `data` into a `MeterDataReceipt`
3. Upserts the receipt row (idempotent on `process_id`)
4. Stores typed interval reads

---

## Relationship to Other Services

```
marktd :8180
  │  POST /webhook  (de.mako.process.completed · MSCONS PIDs)
  ▼
edmd :8380
  │  GET /api/v1/deliveries/{malo_id}           (meterstore hot PostgreSQL window)
  │  GET /api/v1/archive/olap/{malo_id}          (meterstore cold tier — Iceberg V2 on S3)
  │  GET /api/v1/imbalance/{malo_id}/{year}/{month}
  ├──► invoicd :8280       — MMM imbalance input for selbstausgestellt INVOIC
  ├──► netzbilanzd :8680   — MeterBillingPeriod (HT/NT kwh) for NNE / §14a ToU billing
  └──► ERP / operator dashboard — historical reads and billing data
```

## Testing

Two suites run on every plain `cargo test`, no external services:

- `cedar_policy` — pins the authorization gates: who may read, write meter reads,
  mutate series, run reading orders, and request GDPR erasure.
- `schema_code_guard` — pins the schema↔code contracts. Beyond the enum⇄CHECK
  lists it now walks every SQL literal in `src/` and asserts that each column
  named against an edmd table exists, and that every `ON CONFLICT` target has a
  matching unique key. Both catch a class of bug that is otherwise only a runtime
  500: a `SELECT pid FROM gas_quality_data` whose column is `source_pid`, and an
  `ON CONFLICT ON CONSTRAINT` naming a `CREATE UNIQUE INDEX` (which PostgreSQL
  rejects — it silently disabled the billing-period cache entirely).

Storage integration runs against a real PostgreSQL + a filesystem Iceberg
warehouse via testcontainers — `#[ignore]`d by default (Docker required), run with
`just test-edmd-db`:

- `meterstore_integration` — ingest → version-resolved read-back (Sparte survives,
  not defaulted to Strom; gas is stored in its billing unit), latest-read,
  correction supersession with the § 147 AO audit row **and** the billing-period
  cache invalidation that makes it reach an invoice, the Mehr-/Mindermengensaldo
  in both directions, Gasbeschaffenheit round-trip, GDPR Art. 17 subject-mapping
  erasure, per-tenant push-session isolation, and tenant isolation (the same MaLo
  under two tenants never merges on read).

The hot/cold tiering, watermark and cold-tier archival internals are meterstore's
own real-PostgreSQL tests; edmd exercises them through the `store` handle it
configures.

## See Also

- [edmd operator guide](https://hupe1980.github.io/mako/docs/services/edmd/)
- `edmd::domain` (in-crate) — `MeterDataReceipt`, `TimeSeriesRepository`/`Typ2Repository`, MSCONS PID set
- [meterstore](https://crates.io/crates/meterstore) — hot/cold tiered store (PostgreSQL + Apache Iceberg) backing `meter_reads` / `esa_typ2_reads`
- [marktd](../marktd/README.md) — event source
