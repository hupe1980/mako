# billingd — Multi-Product Energy Billing Engine

`billingd` is a **pure calculation service**: no grid topology knowledge, no EDIFACT,
no customer management. It pulls product definitions from `tarifbd`, consumption from
`edmd`, and grid pass-through costs from `marktd`.

| Feature | Detail |
|---|---|
| **HTTP port** | `:9280` |
| **Database** | PostgreSQL (billing_records) |
| **Auth** | OIDC/JWT + Cedar ABAC |
| **Product API** | `Product` typed enum — `#[serde(tag="category")]` deserialization from tarifbd JSONB |
| **Categories** | 13: STROM, GAS, WAERME, SOLAR, EEG, EINSPEISUNG, WAERMEPUMPE, WALLBOX, HEMS, EMOBILITY, ENERGIEDIENSTLEISTUNG, SHARING, BUNDLE |
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
The billing calculator is in the **pure `energy-billing` crate** — **160 tests** with no I/O:

```bash
cargo test -p energy-billing --all-features
```

Tests cover all 13 product categories, §41b iMSys guard, §9 StromStG typed exemptions,
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
stromsteuer_ct_per_kwh        = 2.05   # §3 StromStG, since 01.07.2023
energiesteuer_gas_ct_per_kwh  = 0.55   # §2 Nr. 3 EnergieStG
behg_gas_ct_per_kwh           = 1.310  # BEHG §10, 65 EUR/t × 0.20160 kg/kWh (2026)
mwst_rate                     = 0.19

# Outbound ERP CloudEvents. `erp_hmac_secret` signs them (X-Mako-Signature,
# HMAC-SHA256) so the receiver can verify the origin.
erp_webhook_url = "http://erp:8000/events"
erp_hmac_secret = "env:BILLINGD_ERP_HMAC_SECRET"
```
