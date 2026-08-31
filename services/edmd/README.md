# edmd — Energy Data Management daemon

`edmd` stores MSCONS meter readings received from `marktd`, accepts direct iMSys/SMGW interval push, scores data quality with a Hampel filter, schedules reading orders (Ablesesteuerung), and serves BO4E typed time-series and imbalance queries — `Energiemenge` deliveries for ERP billing import, `Lastgang`/`Zeitreihe` for API-Webdienste Strom, `MeterBillingPeriod` for `netzbilanzd`, and Mehr-/Mindermengen reconciliation for `invoicd`. `meter_reads` (and the non-authoritative `esa_typ2_reads` stream) are stored through the [`meterstore`](https://github.com/hupe1980/meterstore) crate — a recent window in PostgreSQL and the settled history in **Apache Iceberg V2** on S3/GCS/Azure, split by a tiering watermark that meterstore owns.

| Feature | Detail |
|---|---|
| HTTP port | `:8380` |
| Database | PostgreSQL 17+ (sqlx 0.8, schema from `migrations/0001_schema.sql`) |
| Tiering | `meter_reads` / `esa_typ2_reads` are meterstore tables — a hot PostgreSQL window plus settled Iceberg V2 history, split by a tiering watermark (one-week settlement lag). edmd supplies a `TableConfig` (daily partition/archival steps, `tenant` identity column); meterstore owns partitioning, retention, version resolution and cold-tier archival |
| Schema | edmd business tables in its own PgPool: `meter_billing_periods` — NUMERIC aggregates, `tenant TEXT NOT NULL`; `zsg_conversion_log`, `gdpr_deletions`, `ablese_auftraege`, `direct_push_sessions`, `meter_read_corrections`, `substitute_value_log`, `quality_assessments`. `meter_reads`, **`esa_typ2_reads`** and **`meter_readings`** (the Zählerstandsgang, a meterstore **point** table) are meterstore-owned, not declared in edmd's DDL |
| ESA Typ-2 store | ESA-delivered "Werte nach Typ 2" (MSCONS **13027**) are **non-authoritative** (Codeliste 1.4 Kap. 4.6 · WiM Strom Teil 2 §4) — no bearing on Netznutzungs-/Bilanzkreis-/Mehr-/Mindermengenabrechnung. They land in a **separate `esa_typ2_reads` table**, never `meter_reads`, so no billing query can reach them by omission. No correction, substitution, reconciliation, or billing-period participation. A Typ-2 row carries the subscription that ordered it (`bestellung_ref` — `SG1 RFF+AGI`, MSCONS AHB 3.2 §11.2 hint `[574]`) and the transport it arrived on (`delivery_path`). Read via `GET /api/v1/esa/typ2/{malo_id}` (**multi-register** — an Angebot names 1–23 OBIS-Kennzahlen). Kapitel 4.6 has **two** delivery paths: 4.6.1 arrives as MSCONS 13027 over AS4 through `makod`, 4.6.2 as XML straight from the iMS over SM-PKI — `POST /api/v1/esa/typ2/{malo_id}` is that second door, where the ESA's SM-PKI gateway files what it decoded (`write-esa-typ2`, its own Cedar action so it cannot reach the authoritative store). Both Typ-2 relations are **refused by name** on `POST /api/v1/query/sql`: the tables share a DataFusion session, so free-form SQL was the one surface where the separation was a naming convention rather than a type |
| Inbound | CloudEvents from `marktd` — `de.mako.process.completed` (MSCONS billing PIDs 13005–13025 → `meter_reads`; **13027 → `esa_typ2_reads`**; GPKE 55001 → LIEFERBEGINN and 55004/55007 → LIEFERENDE reading orders), `de.mako.process.initiated` (INSRPT 23001/23003/23004/23005/23008/23009 → auto reading orders). **An ingest door answers by whether a retry could work**: `5xx` for a transient store failure, so the fan-out redelivers; **422** for a delivery the store *refused* — an overlapping span, a value restated under an existing version, a second network operator on one reading. Those never succeed on a retry, and answering `5xx` to them redelivered a poison message for as long as its retry budget allowed |
| Kafka ingest | Optional `[kafka_ingest]` consumer (krafka) for head-end systems — at-least-once, earliest offset reset, same V-rule + Hampel + audit path as REST; an unrecognised `sparte` or `source` is **refused**, not coerced (reading them as STROM/MSCONS stored a mislabelled gas batch as electricity with EDIFACT provenance); e2e-tested against an in-process `FakeBroker`. Optional per-message HMAC; without it the **topic ACL is the trust boundary** — restrict produce rights to the head-end system |
| Zählerstandsgang | `POST\|GET /api/v1/zaehlerstandsgang/{malo_id}` — **what an iMSys actually measures** (§ 2 Satz 1 Nr. 27 MsbG: viertelstündig ermittelte Zählerstände für Strom, stündlich für Gas). **BK6-24-174** — whose own subject is *„Übermittlung von Zählerstandsgängen (Datenübermittlung ZSG)"*, wirksam 06.06.2025 — puts the differencing at the MSB, and edmd is the MSB. Both halves are stored, and both are tiered: the register readings in the `meter_readings` **point** table (§ 146 Abs. 4 AO — a stored difference cannot reproduce the values it came from) and the derived intervals in the interval table, relabelled onto the **Lastgang** register (`1-0:1.8.0` is a Zählerstand, `1-0:1.29.0` is the Lastgang). A Zählerstand is stored **unconverted** — § 25 Nr. 4 MessEV converts the *difference*, so a gas register stays m³ while its interval is kWh_Hs. The ZSG is keyed by **Messlokation** as well as register: a Marktlokation may be measured by several meters, and two of them carry the same OBIS code at the same instants |
| ZSG conversion audit | Where no honest difference exists — a backwards step no `register_digits` explains, a jump beyond `max_plant_power_kw` — **no interval is emitted** and the reason goes to `zsg_conversion_log`. The hole surfaces as a V01 gap and is filled by the § 60 Abs. 2 substitute path, which writes its own row: together the two logs say "this quarter-hour is an Ersatzwert *because* the register went backwards here". A reconstructed register wrap is logged too — it is not an error, but it is the one place the conversion adds `10^digits` on the strength of a configured device property |
| § 40 Abs. 2 Nr. 6 EnWG | The invoice's opening and closing Zählerstand — the last reading **at or before** each period bound, since one dated after the period end did not hold at the period end |
| Direct push | `POST /api/v1/meter-reads/rlm/{malo_id}` (Strom), `POST /api/v1/meter-reads/gas/{malo_id}` (Gas m³→kWh_Hs) — idempotent on `session_id` |
| Validation gate | `store_reads` accepts a **`ValidatedReads`**, not a slice — its only constructor runs **V01–V09/V11/V12** and its field is private, so no ingest path can persist unvalidated data. Taken **by value**, so a caller cannot keep the raw batch and store it another way. Covers IoT push, RLM/gas push, bulk import, Kafka, and edmd's own § 60 Abs. 2 MsbG Ersatzwerte. Annotates, never rejects — billability is a separate decision from storage (§ 147 AO / GoBD) |
| Validation shape | **One pass**, `domain::validation::findings`, shared by every ingest door, the MCP `validate_timeseries` tool and the § 60 Abs. 2 substitute path — a second pass on `ValidationConfig::default()` would disagree with the door that stored the same readings. The batch is split by **(Sparte, OBIS register)** before the rules run: V01/V02 are statements about a single series, and a prosumer MeLo delivering import beside export at one slot would otherwise trip V02 (`Error`) on every interval. Thresholds come from `QualityConfig::for_sparte` and the cadence is **observed**, so an hourly gas series is not judged as a broken quarter-hour one and a vacant flat's water meter is not a stuck meter. Findings are attached to the interval they name, with its register — not the whole batch's list copied onto every row |
| V12 plant capacity | `ImplausiblePower` needs a ceiling to be impossible against, and `QualityConfig::for_sparte` sets none — so the rule was documented, surfaced as `spike_intervals`, and could not fire. edmd holds no master data, so the ceiling comes from the caller that does: optional `max_plant_power_kw` on the direct-push, bulk, IoT and Kafka bodies, on the MSCONS payload, and on the MCP `validate_timeseries` tool. Absent, the rule stays off — an invented ceiling would block billing on a reading that is merely large. Every validation response now names the rules that **did not run** (`skipped_rules` on the ingest doors, `rules_evaluated`/`rules_skipped` on the MCP tool), so a clean verdict states what stands behind it rather than reading as a guarantee no rule ever made |
| Ersatzwert register | A § 60 Abs. 2 substitute is filed **under the register it fills** — the request's `obis_code`, or the point's dominant energy register when none is named. Written back from the request instead, an unnamed substitute landed unlabelled, and an unlabelled reading *is* the canonical total register: on a dual-tariff point reporting only HT and NT, one such value made the whole month read as its own decomposition and every aggregate collapsed to the substitute alone. `interval_secs` likewise defaults to the register's **observed** cadence, not a flat 900 s |
| Register selection | **A MaLo is a set of registers, not a series** — meterstore reads span channels, so every path that folds readings into a figure goes through `domain::register`. `energy_intervals(reads, direction)` for anything that **sums**: non-billable qualities dropped, kvarh/kW/fault registers dropped, the other direction dropped, and the total register used *instead of* the HT/NT intervals it **overlaps** (`1.8.0 = 1.8.1 + 1.8.2` — summing all three bills the consumption twice), while tariff intervals no total covers are **summed**. Per interval, not per window: a meter reconfigured mid-month reports the total for part of it and the split for the rest, and a window-wide rule throws the second half away. `register_groups(reads)` for anything that judges a series' *shape*, where mixed registers make the observed cadence a median across channels, turn every same-slot pair into an overlap, and multiply coverage. Group C is a direction for electricity only, so gas/water/heat are not filtered by it |
| Quality scoring | One `compute_quality(samples, sparte, from, to)`, and **every** door runs it and records the verdict under the batch's own `IngestionSource` — the § 147 AO history is only as complete as its least-covered path. The MCP twins share it too, so no surface scores with electricity thresholds for another commodity or an assumed cadence. Each register is scored on its own and folded worst-first — Hampel filter with the commodity's own window/σ/σ-floor, reported in the response as the parameters actually used; grades A/B/C/F; coverage against the **requested** window. Values are scored **as stored** (gas already converted). **Needs more than 2×window intervals**: a shorter series is scored without outlier detection rather than with a claim the data cannot bear. Retroactive: `POST /api/v1/quality-score/{malo_id}`, which scores the stored readings with their own quality flags |
| §42c allocation | `GET /api/v1/sharing/{community_id}/allocation` — **`community_id` is the shared plant's MeLo.** A GGV rule names one `plant_melo_id` and one `tenant_melo_id`, so a `virtual_meter_configs` row is a *participant*; the community is the set of rules sharing a plant, matched with `jsonb_path_exists` and the MeLo as a bound variable (variant-agnostic, whole-value). The per-participant allocation comes from `metering::compute_ggv_allocation` whole — `consumption == allocated + net_grid_draw` holds exactly in every interval, and the §42b Abs. 5 `Pos()` cap is **reported** (`capped`, `surplus_to_grid_kwh`) rather than inferred by subtracting a re-projected consumption series; a failed source read answers 503 rather than under-allocating in silence |
| Reading orders | `POST/GET /api/v1/reading-orders` — Ablesesteuerung for LF/MSB/NB/ESA (an ESA may order value delivery, §60 Abs. 1 MsbG); `/complete`, `/cancel`, `/fail` (Ablesehindernis); auto-creates `INSRPT_STOERUNG` on INSRPT PID 23001 (WiM Störungsmeldung). **A completed Ablesung files its Zählerstand into `meter_readings`** — for an **SLP** point (no interval metering at all) the year-on-year register difference is the entire billing path. The order names its `sparte`, and a Zählerstand in the wrong dimension is refused rather than filed. Filing one also needs the order's `melo_id` and `obis_code`: a reading belongs to one register of one meter, and neither is derivable from an order that does not name it |
| Delivery surveillance | **The other half of quality.** Every V-rule and the Hampel scorer judge data that *arrived*; silence triggers nothing, so without this a broken head-end stays invisible until a settlement run comes up short. An hourly sweep asks which measuring points have **not** delivered — `SILENT` (nothing for `silent_after_hours`, default 36) or `UNDER_COVERED` (still delivering, under `min_coverage_pct` of the window) — and emits `de.messwert.reading.delivery.overdue` / `.resumed` on the transitions. Coverage is a *duration* ratio, so a point that legitimately moves from ¼h to hourly is not a finding; only billable qualities count; a point that never delivered is not reported (that is `marktd`'s question). `GET /api/v1/surveillance/delivery` · `POST …/scan`. The **ESA Typ-2** stream is swept separately — own threshold, own register rows (`stream = 'TYP2'`), own events — because a Typ-2 gap reaches no billing run that could come up short. It is keyed per **(Meldepunkt, subscription, register)**: an ESA subscription is the (Meldepunkt, Messprodukt) pair and one Meldepunkt may carry several, so the subscription (`SG1 RFF+AGI` on the delivering 13027) is part of the key — otherwise two subscriptions sharing an OBIS register share one row and one going silent is masked by the other. |
| § 60 MsbG confirmations | Every stored ESTIMATED/SUBSTITUTED interval opens an obligation in `estimated_read_confirmations`; auto-discharged when a MEASURED/CORRECTED value for the slot arrives (ingest or correction path). Daily worker (`[confirmation]`, default on, `deadline_weeks = 8` — aligned with the MaBiS BKA correction window, no statute fixes a number) escalates stale ones to UEBERFAELLIG and emits `de.messwert.reading.confirmation.overdue`; `GET /api/v1/confirmations?status=` lists them |
| Jahresablesung compliance | `GET /api/v1/compliance/jahresablesung/{year}` — only `AUSGEFUEHRT` discharges the obligation (§ 40b Abs. 1 EnWG i. V. m. GPKE Teil 1 Turnusablesung; § 40a Abs. 2 EnWG governs estimation) |
| REST API | `GET /api/v1/deliveries/{malo_id}` → `Vec<Energiemenge>` · `GET /api/v1/lastgang/{malo_id}` · `GET /api/v1/energy/{malo_id}?direction=BEZUG\|EINSPEISUNG` (**the canonical projected series** — one direction, through `domain::register`, with quality, duration-ratio coverage at the **observed** cadence, and `billable_pct` for the §60 Abs. 2 gate; consumed by einsd §51 and the EEG Einspeisemenge, mabis-syncd, and billingd §41a — folding `/lastgang`'s per-register export back into one series *is* the projection, so consumers take it made) · `GET /api/v1/zeitreihe/{malo_id}` · `GET /api/v1/billing-period/{malo_id}` · `GET /api/v1/imbalance/{malo_id}/{year}/{month}?bilanziert_kwh=` · `GET /api/v1/netzverlust?from=&to=` (§22 EnWG indicative grid-loss balance) · `GET`/`POST /api/v1/esa/typ2/{malo_id}` (ESA Typ-2 store — never billing; `POST` is the Kapitel-4.6.2 SMGW door) |
| Read windows | Every materialising read defaults to the **last 31 days** and refuses a window wider than **732 days**; a malformed `?from=`/`?to=` is a `400`, not a silent fallback. Bulk history goes over Arrow IPC, `POST /api/v1/query/sql`, or the Iceberg REST catalog — the three paths that stream instead of materialising |
| Mehr-/Mindermengen | The saldo needs both halves and edmd holds one. `?bilanziert_kwh=` (the profile-allocated quantity, from the Bilanzkreisabrechnung) is **required**; without it the endpoint answers `422` rather than comparing the measured total against itself. Arithmetic and sign convention are `metering::compute_imbalance`'s — under the profile is a **Mehr**menge the NB credits (GPKE Teil 1 Kap. 8.4 Nr. 3). The measured half is the **Bezug**, register-projected: folding in the point's Einspeisung or its HT/NT split beside the total moves the saldo, and with it the money |
| MaLo discovery | `GET /api/v1/billing-periods` answers from the **readings** (cross-MaLo `SELECT DISTINCT` over the resolved relation), not from the lazily-filled billing-period cache, and reports `truncated` when it caps — a MaLo never read through the cache would otherwise be invisible to `mabis-syncd` and never get a Summenzeitreihe |
| Arrow IPC | `Accept: application/vnd.apache.arrow.stream` on `GET /api/v1/lastgang` + `GET /api/v1/zeitreihe` — 10–50× throughput vs JSON for bulk reads. `quantity_kwh` is **`Decimal128(18,5)`**, matching the storage column: this is the path `mabis-syncd` and `billingd` read bulk history over, and binary floating point cannot represent 0.1 kWh |
| Archive OLAP | `GET /api/v1/archive/status` · `GET /api/v1/archive/olap/{malo_id}` · `GET /api/v1/archive/portfolio` · `GET /api/v1/archive/timeseries/{malo_id}` · `POST /api/v1/query/sql` (DataFusion, JSON or Arrow IPC, over meterstore's resolved relation) |
| Iceberg REST | `GET /api/v1/iceberg/v1/...` — read-only Iceberg REST catalog (meterstore's `CatalogFacade`, mounted by edmd, Cedar-gated by `read-archive-olap`; mutating routes → 405). DuckDB / Spark / Trino / PyIceberg attach for schema + table locations, then read Parquet from object storage with their own credentials |
| Balancing day | Electricity balances on the calendar day, **gas on the Gastag (06:00–06:00)** — GaBi Gas, Art. 3 Nr. 6 VO (EU) 312/2014. `?sparte=` on `GET /api/v1/billing-period/{malo_id}` and the imbalance endpoint selects it; aggregating gas over calendar days would misbook six hours into the neighbouring Bilanzierungstag every day. The long/short Gastag is the one named after the **Saturday**, because the clocks change before 06:00 |
| MaLo-IDs | `metering::MaloId` validates the BDEW check digit at the store boundary; a malformed ID answers `400` with the reason rather than failing inside a scan |
| Units | A reading is stored **and labelled** in its Sparte's *billing* unit. Gas is converted to kWh_Hs at ingest (§25 Nr. 4 MessEV / DVGW G 685), so it is kWh everywhere downstream — the BO4E `Mengeneinheit`, the cold tier, the Iceberg facade. Only water, measured and billed in m³, is a volume |
| Storno | MSCONS **13006** is *Messwert Storno* — it withdraws values delivered earlier. The receipt is recorded and the payload is **not** stored; booking withdrawn quantities as freshly measured ones is the opposite of what the message says |
| GDPR | `DELETE /api/v1/gdpr/erasure/{malo_id}` — Art. 17 pseudonymisation in one transaction: destroys the MaLo's subject mapping in meterstore's registry, which unlinks **both** reading stores at once (the authoritative one and the ESA Typ-2 stream enrol the same `(tenant, MaLo)` subject — non-authoritative is a statement about settlement, not about personal data), **rewrites `malo_id` to that subject reference** in the Buchungsbeleg tables it may not delete (`meter_read_corrections`, `substitute_value_log`, `meter_data_receipts`, `ablese_auftraege`, `gas_quality_data` — § 147 Abs. 1 AO, Art. 17 Abs. 3 lit. b DSGVO), and deletes the derived, operational and device tables outright, `virtual_meter_configs` included |
| Auth | OIDC/JWT + Cedar ABAC — reads tenant-scoped, **writes role-gated** (`write-meter-reads` → MSB/admin; series mutation, reading orders, GDPR erasure → MSB/NB/admin; LF-role tokens are read-only; gates pinned by the `cedar_policy` test suite); **service-to-service keys** via `[[oidc.service_keys]]` for internal callers (einsd/billingd/vertragd/portald send an opaque Bearer key, not a JWT); webhook Standard Webhooks (`webhook-signature`). Refuses to start without `[oidc]` unless `allow_insecure_no_auth = true` |
| Rate limiting | Per-tenant and global GCRA buckets; `429` carries `Retry-After` |
| Lifecycle | `mako_service::run` owns it — tracing, the tuned pool (`application_name = edmd`), migrations, real DB-ping readiness, `/metrics`, and graceful shutdown on **SIGINT and SIGTERM** |
| Health | `GET /health/live`, `GET /health/ready` (real DB ping) — the runner's; `GET /edmd/metrics` carries edmd's own gauges |
| MCP | `POST\|GET /mcp` — 15 tools + 5 prompts, including `get_timeseries`, `validate_timeseries` (the same V-rule pass the ingest doors run), `trigger_substitution` (§ 60 Abs. 2 MsbG Ersatzwerte), `trigger_jahresablesung`, `get_correction_history` |
| CloudEvents emitted | `de.messwert.reading.direct.stored`, `de.messwert.reading.delivery.overdue` / `.resumed`, `de.messwert.cls.compliance-resolved`, `de.messwert.reading.quality.warning` (Hampel grade C/F **or** any V-rule finding, from every ingest door — MSCONS, RLM/gas push, IoT push, bulk import, Kafka; same predicate as the `202` status, so the two cannot disagree), `de.messwert.reading.order.failed`, `de.messwert.cls.compliance-issue`, `de.messwert.smgw.cert.expiry-warning` |
| SMGW cert expiry | Daily worker sweeps every certificate in `smgw_sessions` and emits `de.messwert.smgw.cert.expiry-warning` at **90 / 30 / 7 days** before `valid_to`, once per tier per certificate (dedup in `smgw_cert_expiry_alerts`); severity INFO → WARNING → CRITICAL. The ladder is **operational, not statutory** — BSI TR-03109-4 binds certificate *runtimes* while the renewal lead time and the Zertifikatswechsel overlap live in the Root-CP — so it is configurable. An expired cert silently ends §14a Fernsteuerbarkeit; `agentd` `smgw-diagnostics-agent` escalates renewal to the MSB |
| §14a compliance register | `cls_compliance_issues` is a register of **what is wrong now**, keyed on the identity of the fault and not on when it was noticed. Events fire on the transitions (`de.messwert.cls.compliance-issue` / `.compliance-resolved`), not once per daily sweep — otherwise a gateway on an expired certificate emits a CloudEvent a day forever and the fleet list's "issues in 24 h" measures the sweep cadence. `REPLACED` gateways are excluded via the `gateway_status` column. Duty: **§ 25 MsbG** (the GWA's monitoring and maintenance responsibility), *not* §21c or §29 — §21c does not exist and §29 is the rollout obligation |
| Quality history | Every scoring path records a verdict in `quality_assessments`, under the batch's own `IngestionSource`; re-scoring supersedes rather than appends. `schema_code_guard` pins the column's CHECK against `IngestionSource::ALL` — a door naming a value outside it fails the insert, and the failure is only a warning, so its history would vanish silently |
| § 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD) audit trail | Every value-changing overwrite — corrections **and** redeliveries, on every transport — leaves an immutable `meter_read_corrections` row; `?as_of=` reconstructs prior knowledge states |
| Authored writes supersede | A delivery may legitimately be shadowed by a newer one; a value **edmd authors** may not. An operator correction and a § 60 Abs. 2 Ersatzwert carry no MSCONS version, so both fell back to the deliberately-low 13-digit timestamp and were outranked by any reading delivered with a stated version — stored, never current, while the audit row, the discharged confirmation and the cache invalidation all claimed otherwise. `store::append_superseding` re-asserts one above the version that actually holds, taken from the store's own displacement report, and errors after four contested attempts |
| Overlap exclusion | Per-partition `EXCLUDE USING gist` (`btree_gist`): a delivery whose range overlaps a stored reading is refused rather than double-counted |

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

[otel]
endpoint = "http://otel-collector:4317"

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
