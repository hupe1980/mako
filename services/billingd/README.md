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
