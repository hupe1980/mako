# billingd — Multi-Product Energy Billing Engine

`billingd` is a **pure calculation service**: no grid topology knowledge, no EDIFACT,
no customer management. It pulls product definitions from `tarifbd`, consumption from
`edmd`, and grid pass-through costs from `marktd`.

| Feature | Detail |
|---|---|
| **HTTP port** | `:9280` |
| **Database** | PostgreSQL (billing_records) |
| **Auth** | OIDC/JWT on every business route — fail-closed at startup (`allow_insecure_no_auth` for dev) |
| **Product API** | `Product` typed enum — `#[serde(tag="category")]` deserialization from tarifbd JSONB |
| **Categories** | 12: STROM, GAS, WAERME, SOLAR, EEG, EINSPEISUNG, WAERMEPUMPE, WALLBOX, HEMS, EMOBILITY, ENERGIEDIENSTLEISTUNG, SHARING |
| **§41a EPEX dynamic** | 15-min Lastgang × hourly EPEX day-ahead → `STROM` dynamic category |
| **§41b iMSys guard** | Hard error when `dynamic_epex=true` and `MeteringMode != Imsys` |
| **§14a discount** | `ControllableLoadProvider` Modul 1 (capacity reduction) + Modul 3 (load-shedding) |
| **§42a GGV** | `POST /api/v1/billing/ggv/{ggv_id}` — multi-tenant PV community solar, per-tenant share billing |
| **§42c Sharing** | `Product::Sharing(SharingProduct)` — community energy allocation credit via `EnergyShareProvider` |
| **Gas H2-blend** | `gasqualitaet` field on `GasMeterInput` — annotates Rechnung as `ZusatzAttribut` (per DVGW G 260, measured Brennwert already reflects blend) |
| **Gas RLM Leistungspreis** | `gas_leistungspreis_ct_per_kw_month` in `GasProduct` — demand charge for large gas customers |
| **Korrekturrechnung** | `POST /api/v1/billing/{id}/correction` — `rechnungstyp=KORREKTURRECHNUNG` with `originalRechnungsnummer` |
| **Sammelrechnung** | `POST /api/v1/billing/sammelrechnung/{rv_id}` — B2B consolidated invoice for Rahmenvertrag |
| **XRechnung 3.0** | `GET /api/v1/billing/{id}/xrechnung` — CII XML (EN16931) |
| **ZUGFeRD 2.3** | Embedded in PDF-A/3 |
| **PEPPOL BIS 3.0** | `GET /api/v1/billing/{id}/ubl` — UBL 2.1 XML (EU Directive 2014/55/EU) |
| **XRechnung B2G** | `POST /api/v1/billing/{id}/submit-b2g` — emits `de.billing.xrechnung.b2g.ready` CloudEvent (§27 EGovG from 01.01.2027) |
| **MCP** | 12 tools at `/mcp`: list/get/preview/calculate, `validate_tariff_config`, `explain_invoice_position` |
| **Health** | `GET /health/live`, `GET /health/ready` |

## Record store integrity

`insert_billing_record` shipped with two independent faults — it omitted the
NOT-NULL `tenant` column and its `ON CONFLICT` named five of the partial unique
index's six columns with no predicate — so **it failed on every call**, and
nothing noticed because nothing tested `pg.rs`. All three insert paths now
supply the tenant, the upsert names the full index identity and repeats its
predicate, and a re-run may replace a **draft only**: once dispatched, the
stored Rechnung is what the counterparty received, and a conflicting re-run is
refused with a pointer at the correction path.

`tests/schema_code_guard.rs` pins these rules textually on every `cargo test`;
`just test-billingd-db` proves them against a real PostgreSQL.

## Every document through the engine

No handler assembles BO4E invoice JSON by hand:

- **VPP** (webhook auto-billing and `POST /billing/vpp/:id`): positions plus
  the engine's tax provider plus `to_rechnung_json`. The previous inline VAT
  block hardcoded `UST_19` even when the contract overrode the rate.
- **GGV and Sammelrechnung aggregates** (`build_aggregate_invoice`): the
  per-MaLo engine runs stay stored as calculation records; the consolidated
  document strips their tax positions and recomputes VAT **once** over the
  combined base per rate. At the BG-23 breakdown (cent-rounded per BT-117)
  this matters: three sub-invoices of 10.01 EUR each show 1.90 apiece, the
  combined base 30.03 correctly shows 5.71. Each rendered position carries
  the `marktlokationsId` it came from; rechnungsdatum is derived, not
  wall-clock.

## Typed engine errors on the wire

Engine failures answer with a structured body, not prose:

```json
{ "error": { "code": "VALIDATION_BLOCKED", "context": "51238696781",
             "message": "…", "warnings": [{ "code": "MODUL2_AND_FLAT_NNE", … }] } }
```

`code` is `EngineError::code()` — stable, machine-readable; `warnings` carries
the full set behind a blocked validation.

## §40 contract facts from vertragd

`dispatch_invoice` resolves the active contract behind the MaLo via
`GET vertragd /api/v1/vertraege/by-malo/{malo_id}` and puts the §40 Abs. 1
EnWG facts on the invoice: Vertragsdauer, Kündigungsfrist, the next possible
Kündigungstermin (computed by vertragd, including the §309 Nr. 9 BGB one-month
cap after an automatic renewal) and the next Abrechnungstermin (same cadence,
next period). The contract dates also set `vertragsbeginn`/`vertragsende`, so
first and last invoices pro-rate to the actual contract days (§41 EnWG). The
dependency is soft: an unreachable vertragd degrades to an invoice without the
facts, logged.

## §40c and §41a

Invoice generation checks the §40c EnWG six-week deadline (generation time vs
period end) and attaches `SECT40C_DEADLINE_EXCEEDED` when late — the engine is
clock-free by design, so the deadline lives here, where a clock legitimately
exists. The §41a dynamic path now fetches hourly EPEX prices from tarifbd; it
was a stub returning an empty map, which priced every dynamic interval at
nothing while the working client function sat as dead code.

## Billing arithmetic

All monetary amounts use `billing::Amount<5>` (`EuroAmount` — `i64 × 10⁻⁵` EUR). Never `f64`.
The billing calculator is in the **pure `energy-billing` crate** — **191 tests** with no I/O:

```bash
cargo test -p energy-billing --all-features
```

Tests cover all 13 product categories (incl. §42c SHARING and municipal WASSER), §41b iMSys guard, §9 StromStG typed exemptions,
`EnergieQuellen` CO₂ label, MwSt override, EEG Gutschrift, HT/NT ToU, gas Brennwert,
Mieterstrom, Tarifwechsel merge, proportional allocation, batch billing, and pre-flight validation.

## Configuration

```toml
# billingd.toml
database_url  = "postgresql://billingd:secret@db:5432/billingd"
port          = 9280
tenant        = "9900357000004"

tarifbd_url     = "http://tarifbd:9080"
edmd_url        = "http://edmd:8380"
marktd_url      = "http://marktd:8180"
vertragd_url    = "http://vertragd:9780"

[rates]
stromsteuer_ct_per_kwh        = 2.05   # §3 StromStG
energiesteuer_gas_ct_per_kwh  = 0.55   # §2 Abs. 3 Nr. 4 EnergieStG (constant since 2003)
behg_gas_ct_per_kwh           = 1.310  # BEHG §10, 65 EUR/t × 0.20160 kg/kWh (2026)
mwst_rate                     = 0.19

# §40 Abs. 2 Nr. 1 EnWG — supplier identity as shown on invoices. The
# statutory consumer hints (Schlichtungsstelle Energie §111b EnWG, BNetzA
# Verbraucherservice, Energieberatung, §41c Wechselhinweis) are emitted into
# every Rechnung automatically as `verbraucherinformationen`.
seller_name    = "Stadtwerke Musterstadt GmbH"
seller_vat_id  = "DE123456789"
seller_address = "Musterstraße 1, 12345 Musterstadt"
seller_contact = "Tel. 0800 1234567, service@stadtwerke-musterstadt.de"

# OIDC token verification for the HTTP API. billingd refuses to start
# without it unless `allow_insecure_no_auth = true` (dev only) — an open
# billing API accepts calculate/correction/mutation calls from anyone.
[oidc]
issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
audience = "api://mako-billingd"

# Outbound ERP CloudEvents. `erp_hmac_secret` signs them (X-Mako-Signature,
# HMAC-SHA256) so the receiver can verify the origin.
erp_webhook_url = "http://erp:8000/events"
erp_hmac_secret = "env:BILLINGD_ERP_HMAC_SECRET"
```

## Layered billing quality assurance

The platform implements the state-of-the-art layered model — deterministic
where regulation demands auditability, ML-ready where statistics end:

1. **Rule engine (blocking)** — `energy-billing`'s validation pass: an
   Error-severity violation (§41b iMSys, missing EPEX prices, §14a
   double-billing, Ersatzversorgung > 3 months) means the invoice **never
   exists** (`VALIDATION_BLOCKED`); `assert_valid` pins the arithmetic
   invariants; the DB uniqueness guard prevents double-billing a period; and
   metering's V01–V10 + Hampel grades (F blocks billing) guard the inputs.
2. **Deterministic risk gate (`[risk]`, default on)** — every calculated
   invoice is scored 0–100 from coded findings: content checks
   (Σ Steuerbeträge vs gesamtsteuer, valid German VAT rates, negative/zero
   consumption), engine warnings (estimated readings, Vorjahr deviation
   > 50 %, USt-Stichtag, §40c lateness, Preisgarantie), and history checks
   (rolling-baseline deviation, **cross-invoice period overlap/gap**,
   **≥ 3 consecutive estimate-based invoices**). Bands: 0–19 auto-release,
   20–49 sample, 50–79 review, **80–100 HELD — not dispatched** until
   `POST /api/v1/billing/{id}/release`. `GET /api/v1/billing/review-queue`
   is the analyst work list. Every point on the score is a coded,
   human-readable finding persisted in `billing_records.risk_findings` —
   explainability by construction, no post-hoc SHAP needed.
3. **Statistical/ML analytics (external by design)** — the industry pattern:
   edmd's Iceberg/S3 archive, Arrow IPC streams and DataFusion SQL are the
   feed for external ML platforms (Isolation Forests, autoencoders,
   time-series models); their verdicts can flow back as analyst reviews.
   No ML runtime lives in the billing core — determinism is the product.
4. **AI-assisted investigation** — agentd's `billing-anomaly-agent` triages
   every `de.billing.rechnung.erstellt` event from the persisted
   `risk_findings` first, then the rolling baseline
   (`check_billing_anomaly`), and escalates with root-cause taxonomy;
   `billing-regulatory-guard-agent` independently re-checks §40a/§41/§42.

Inbound invoices get the mirror image: `invoic-checker` recomputes NNE
invoices line-by-line against PRICAT price sheets (digital-twin
reconciliation) with Ok/Warn/Dispute outcomes driving
`gpke.abrechnung.annehmen|ablehnen`.

## §40b EnWG scheduled billing runs

The config-gated `[billing_runs]` worker sweeps daily (after `run_hour_utc`,
default 04 UTC): it pulls active contracts + their `abrechnungszyklus`
(MONATLICH/VIERTELJAEHRLICH/HALBJAEHRLICH/JAEHRLICH) from vertragd
(`GET /api/v1/vertraege/billing-candidates`), computes each contract's most
recently completed period (calendar-aligned; JAEHRLICH rolls on the
`vertragsbeginn` anniversary), clips it to the supply window, and bills every
period without an existing `billing_records` row through the same
dispatch→persist→emit pipeline as `POST …/calculate`. Each calendar month's
sweeps accumulate one `billing_run_log` audit row; a failed sweep marks the
month `failed` until an operator looks.

For **iMSys** MaLos the worker additionally delivers the free monthly
**Abrechnungsinformation** (§40b Abs. 2 EnWG) as a
`de.billing.abrechnungsinformation.monatlich` CloudEvent (a preview
calculation, never a persisted invoice), logged once per MaLo and month in
`abrechnungsinfo_log`.

```toml
[billing_runs]
enabled              = true
run_hour_utc         = 4
abrechnungsinformation = true
```

All outbound CloudEvents (invoices and monthly infos) are HMAC-signed with
`erp_hmac_secret` (`X-Mako-Signature`).

## Rounding

All monetary rounding uses **kaufmännisches Runden** (DIN 1333, half away
from zero), and the mode has a single authority: the `billing` arithmetic
core's `RoundingStrategy::MidpointAwayFromZero` — the same strategy every
`billing::Amount` conversion, multiplication and division applies
internally. `energy_billing::round_money` / `.round_kfm(dp)` delegate to it
for runtime-precision rounding on raw `Decimal`s; statutory precisions go
through the typed core (`EuroAmount = Amount<5>` for unit prices,
`Amount<2>` for cents). `Decimal::round_dp` (banker's rounding) is not used
anywhere in the billing path; a grep for `round_dp(` finding only the
helper is the invariant.

Sum-exact money splitting also comes from the core: GGV tenant allocation
uses `billing::proportional_split` (largest remainder), and
`Abschlagsplan::monthly_uniform` distributes the annual estimate via
`Amount::distribute` — any 12 consecutive instalments sum to exactly the
annual amount, instead of drifting up to 6 ct/year from naïve
`round(annual/12)`.

## §40–§40c EnWG invoice compliance

- **Zahlungsziel (§40c):** every Rechnung carries `zahlungsziel` = issue + 14
  days; XRechnung (BT-9), UBL `DueDate` and the MCP tool all render it —
  payment is never implied due before the statutory two weeks after receipt.
- **Fristen (§40c):** invoice generation warns (`SECT40C_DEADLINE_EXCEEDED`)
  when issued later than six weeks after the period end — three weeks for
  monthly periods.
- **Schlussrechnung (§40c):** `POST …/calculate` with `"schlussrechnung":
  true` renders `rechnungsart = SCHLUSSRECHNUNG`; paid advances passed as
  `"abschlaege": [{datum, betrag_eur, ust_satz}]` are itemised and settled
  against the Zahlbetrag (each at the VAT rate it was invoiced at,
  §14 Abs. 5 UStG).
- **Verbraucherinformationen (§40 Abs. 2):** supplier contact plus the
  statutory Schlichtungsstelle/BNetzA/Energieberatung/Wechsel hints are part
  of every `rechnung_json`.
- **Historic VAT is commodity-aware:** gas/Fernwärme carried 7 % from
  01.10.2022 to 31.03.2024 (§28 Abs. 5/6 UStG) and 16 % in H2/2020; periods
  straddling a boundary produce a `MWST_STICHTAG_IM_ZEITRAUM` warning — split
  at the Stichtag (Tarifwechsel pattern) and merge.
- **Rechnungsnummern (§14 Abs. 4 Nr. 4 UStG):** auto-generated numbers embed
  the product code (`BILL-{malo}-{product}-{from}`), and a second correction
  of the same original is refused with `409` so `KORR-{nr}` stays einmalig.
- **Zählerstände + Zählernummer (§40 Abs. 2 Nr. 6):** start/end register
  readings and the aggregate quality flag come from edmd's billing-period
  response (`zaehlerstand_anfang/ende`, `quality`, `messtyp`); estimated or
  substituted values render the §40a estimation notice and an
  `ESTIMATED_READING` warning. The Zählernummer is resolved from the marktd
  device registry (MaLo → Lokationszuordnung → MeLo → Zähler).
- **Vorjahresvergleich + Vergleichsgruppe (§40 Abs. 2 Nr. 7/8):** the
  prior-year consumption is fetched from edmd (same window one year
  earlier); the comparable-customer-group value comes from
  `vergleichsgruppe_kwh_pro_jahr` / `vergleichsgruppe_label` (Stromspiegel/
  BDEW reference data), pro-rated to the billing period. Rendered as
  machine-readable ZusatzAttribute so the invoice renderer can chart them
  (the law asks for graphical display).
