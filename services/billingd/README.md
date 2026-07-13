# billingd — Multi-Product Energy Billing Engine

`billingd` is a **pure calculation service**: no grid topology knowledge, no EDIFACT,
no customer management. It pulls product definitions from `tarifbd`, consumption from
`edmd`, and grid pass-through costs from `marktd`.

| Feature | Detail |
|---|---|
| **HTTP port** | `:9280` |
| **Database** | PostgreSQL (billing_records) |
| **Auth** | OIDC/JWT + Cedar ABAC |
| **Product categories** | 12: STROM, GAS, WAERME, SOLAR, EEG, EINSPEISUNG, WAERMEPUMPE, WALLBOX, HEMS, EMOBILITY, ENERGIEDIENSTLEISTUNG, BUNDLE |
| **§41a EPEX dynamic** | 15-min Lastgang × hourly EPEX day-ahead → `STROM` dynamic category |
| **§14a discount** | Modul 1 (capacity reduction) + Modul 3 (load-shedding) intersected with `WimSteuerungsauftrag` events |
| **§42a GGV** | `POST /api/v1/billing/ggv/{ggv_id}` — multi-tenant PV community solar, per-tenant share billing |
| **Gas H2-blend** | `gasqualitaet` field on `GasMeterInput` — annotates Rechnung as `ZusatzAttribut` for audit (per DVGW G 260, the measured Brennwert from `edmd` already reflects the blend) |
| **Korrekturrechnung** | `POST /api/v1/billing/{id}/correction` — `rechnungstyp=KORREKTURRECHNUNG` with `originalRechnungsnummer` |
| **Sammelrechnung** | `POST /api/v1/billing/sammelrechnung/{rv_id}` — B2B consolidated invoice for Rahmenvertrag |
| **XRechnung 3.0** | `GET /api/v1/billing/{id}/xrechnung` — CII XML (EN16931) |
| **ZUGFeRD 2.3** | Embedded in PDF-A/3 |
| **PEPPOL BIS 3.0** | `GET /api/v1/billing/{id}/ubl` — UBL 2.1 XML (EU Directive 2014/55/EU) |
| **XRechnung B2G** | `POST /api/v1/billing/{id}/submit-b2g` — emits `de.billing.xrechnung.b2g.ready` CloudEvent (§27 EGovG from 01.01.2027) |
| **Health** | `GET /health/live`, `GET /health/ready` |

## Billing arithmetic

All monetary amounts use `billing::Amount<5>` (`EuroAmount` — `i64 × 10⁻⁵` EUR). Never `f64`.
The billing calculator is **pure Rust with no I/O** — covered by **15 unit tests** without a database:

```bash
cargo test -p billingd --test calculator_tests
```

Tests cover all 12 product categories, MwSt override, EEG Gutschrift reduction, Zweitarif
position sums, Gas Brennwert equivalence, and Mieterstrom Aufschlag precision.

## Configuration

```toml
# billingd.toml
database_url  = "postgresql://billingd:secret@db:5432/billingd"
port          = 9280
tenant        = "9900357000004"

tarifbd_url     = "http://tarifbd:9080"
edmd_url        = "http://edmd:8380"
marktd_url      = "http://marktd:8180"
einsd_url       = "http://einsd:9180"
vertragd_url    = "http://vertragd:9780"

[rates]
stromsteuer_ct_per_kwh        = 2.05
energiesteuer_gas_ct_per_kwh  = 0.55
behg_gas_ct_per_kwh           = 0.62
mwst_rate                     = 0.19

[erp]
webhook_url = "http://erp:8000/events"
```
