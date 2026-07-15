# energy-billing

**Pure multi-product retail energy billing library for German markets.**

`energy-billing` is the calculation core of [`billingd`](../../services/billingd/) — the
Energy Billing Engine daemon for the Lieferant (LF) role. The library is **zero I/O,
zero async, zero hardcoded regulatory rates**. It answers one question:

> Given a product definition, meter readings, and statutory rates —
> what does the customer's invoice look like?

---

## Architecture

```
billingd (HTTP service)
    │   tarifbd/edmd/marktd clients · HTTP endpoints
    │   XRechnung 3.0 / ZUGFeRD 2.3 · PostgreSQL · CloudEvents
    │
    └── energy-billing (pure crate)
            │
            ├── PricingModel       — typed enum, type-safe dispatch per product
            ├── TariffInput        — product pricing from tarifbd (JSONB)
            ├── Quantities         — all meter inputs for one billing period
            ├── BillingContext     — period, IDs, invoice type, regulatory rates
            ├── BillingEngine      — composes BillingProvider instances
            ├── BillingProvider    — one implementation per product/tax type
            └── Invoice            — immutable result with positions + totals + BO4E JSON
```

The engine runs in passes:

```
Pass 1  commodity / levy providers   (ElectricityProvider, GasProvider, …)
Pass 2  tax provider                 (MwStProvider — sees all net positions)
Pass 3  Abschlag deductions          (Final invoice reconciliation)
Pass 4  Minimum invoice top-up       (B2B Mindestabnahmeverpflichtung)
Pass 5  Cancellation sign reversal   (Stornorechnung — all signs negated)
```

---

## Quick start

```rust
use energy_billing::*;
use rust_decimal_macros::dec;
use time::macros::date;

let tariff: TariffInput = serde_json::from_str(r#"{
    "category": "STROM",
    "arbeitspreis_ct_per_kwh": 32.0,
    "grundpreis_ct_per_day": 12.0
}"#)?;

let ctx = BillingContext {
    malo_id:          "51238696781".to_owned(),
    lf_mp_id:         "9910000000002".to_owned(),
    rechnungsnummer:  "R2026-06-001".to_owned(),
    period_from:      date!(2026-06-01),
    period_to:        date!(2026-06-30),
    invoice_type:     InvoiceType::Initial,
    regulatory_rates: RegulatoryRates::default(),
    ..Default::default()
};

let quantities = Quantities {
    electricity: Some(MeterInput {
        arbeitsmenge_kwh: dec!(312.5),
        ..Default::default()
    }),
    ..Default::default()
};

// Type-safe dispatch via PricingModel enum
let invoice = PricingModel::try_from(tariff)?
    .build_engine(&GridInput::default(), &ctx.regulatory_rates)?
    .bill(ctx, &quantities)?;

invoice.assert_valid();
println!("Brutto: {} EUR", invoice.brutto_eur);

let rechnung_json: serde_json::Value = invoice.to_rechnung_json();
```

---

## Product categories

| `PricingModel` variant | Category string | Key features |
|---|---|---|
| `Electricity` | `STROM` | SLP/RLM; HT/NT via `billing::TimeOfUsePricing`; block tariffs via `billing::TariffSchedule`; RLM demand charge (`leistungspreis_strom_ct_per_kw_month`); §14a Modul 1/3; EEG Gutschrift pass-through |
| `DynamicElectricity` | `STROM` + `dynamic_epex: true` | §41a per-interval EPEX; price floor; §41a Abs. 4 iMSys validation; §41a Abs. 6 annual savings |
| `HeatPump` | `WAERMEPUMPE` | §14a mandatory; same as `Electricity` |
| `Wallbox` | `WALLBOX` | §14a mandatory; same as `Electricity` |
| `Gas` | `GAS` | §10 GasGVV Brennwertkorrektur; Energiesteuer; §54 EnergieStG KWK/industrial exemption; BEHG CO₂; H2-blend annotation |
| `Heat` | `WAERME` | Fernwärme: Grundpreis + Leistungspreis + Arbeitspreis; auto-7% MwSt for renewable sources |
| `Solar` | `SOLAR` | §42b EEG Mieterstrom; §42a GGV; 0% MwSt for ≤30 kWp (§12 Abs. 3 UStG) |
| `Eeg` | `EEG` | LF-side Gutschrift; §51 contractual Negativpreisregel; full accuracy via `eeg` feature |
| `Einspeisung` | `EINSPEISUNG` | Direktvermarktung: Marktwert − Vermarktungsgebühr |
| `Hems` | `HEMS` | Platform subscription + optimization events |
| `Emobility` | `EMOBILITY` | CPO/EMSP: service fee + kWh + session/roaming |
| `Service` | `ENERGIEDIENSTLEISTUNG` | Flat fee + per-event |

---

## Pricing capabilities

| Feature | How |
|---|---|
| HT/NT Zweitarif | `billing::TimeOfUsePricing` (validated, penny-correct) |
| Block / graduated tariffs | `billing::TariffSchedule::graduated()` |
| Indexed prices (TTF, Phelix) | `IndexedPriceConfig { base_ct, spread_ct, index_value, factor }` |
| Seasonal prices | `SeasonalPriceOverride` by month range (wraps year boundary) |
| §41a EPEX dynamic | `billing::DynamicPricing` with per-interval kWh × price |
| Pro-rata Grundpreis | `ctx.prorate_days()` clips to `vertragsbeginn`/`vertragsende` |
| Minimum invoice (B2B) | Pass 4 auto-top-up to `minimum_invoice_eur_brutto` |
| Discounts / bonuses | `auf_abschlag_ct_per_kwh`, `auf_abschlag_eur_per_month`, `Bonus` category |
| MSB pass-through | `msb_gebuehr_ct_per_day` (MsbG) |
| Multi-rate MwSt | Per-position `applicable_tax_rate` → grouped `MwStProvider` |
| Auto-0% MwSt solar ≤30 kWp | `anlage_kwp ≤ 30` (§12 Abs. 3 UStG Solarpaket I) |
| Stromsteuer exemption | `industrie_stromsteuer_befreiung` (§9 Abs. 1 Nr. 4 StromStG) |

---

## Invoice types

```rust
pub enum InvoiceType {
    Initial,             // RECHNUNG — normal periodic billing
    AdvancePayment,      // ABSCHLAGSRECHNUNG — estimated advance request
    Final,               // SCHLUSSRECHNUNG — Jahresabrechnung, deducts ctx.abschlage
    CreditNote,          // GUTSCHRIFT — LF pays generator (EEG, EINSPEISUNG)
    PartialInvoice,      // TEILRECHNUNG — §41 EnWG move-in/move-out / Tarifwechsel
    Correction { original_invoice_id, reason },  // KORREKTURRECHNUNG (§22 MessZV)
    Cancellation { original_invoice_id },         // STORNORECHNUNG — all signs negated
}
```

---

## Meter inputs

```rust
pub struct MeterInput {
    pub arbeitsmenge_kwh:    Decimal,
    pub arbeitsmenge_ht_kwh: Option<Decimal>,  // HT register
    pub arbeitsmenge_nt_kwh: Option<Decimal>,  // NT register
    pub spitzenleistung_kw:  Option<Decimal>,  // peak demand (RLM)
    pub steuerung_stunden:   Option<Decimal>,  // §14a load-shedding hours
    pub zaehlernummer:       Option<String>,   // §41 EnWG — shown on invoice
    pub zaehlerstand_von:    Option<Decimal>,  // start reading
    pub zaehlerstand_bis:    Option<Decimal>,  // end reading
    pub metering_mode:       MeteringMode,     // Slp | Rlm | Imsys
    pub is_estimated:        bool,             // §17 MessZV notice on invoice
    pub zaehler_replaced:    bool,             // Zählerwechsel notice on invoice
}
```

---

## Key `TariffInput` regulatory fields

| Field | Law | Effect |
|---|---|---|
| `anlage_kwp` | §12 Abs. 3 UStG | Auto-0% MwSt when ≤ 30 kWp (Solarpaket I 2023) |
| `industrie_stromsteuer_befreiung` | §9 Abs. 1 Nr. 4 StromStG | Replaces levy with exemption notice |
| `gas_energiesteuer_befreiung` | §54 EnergieStG | KWK / industrial gas Energiesteuer exemption notice |
| `leistungspreis_strom_ct_per_kw_month` | §41 EnWG | RLM demand charge on `spitzenleistung_kw` (ct/kW/month) |
| `preisgarantie_bis` | §41 Abs. 1 Nr. 4 EnWG | Price guarantee expiry shown on invoice |
| `waerme_is_renewable` | §12 Abs. 2 Nr. 1 UStG | Auto-7% MwSt for renewable Fernwärme |
| `mwst_rate_override` | §12 UStG | Override default 19% per product |
| `sect14a_modul1_nne_reduktion_ct_per_kwh` | §14a EnWG | NNE reduction (ct/kWh) |
| `steuerungsrabatt_modul1_eur_per_kw_year` | §14a EnWG | Capacity-based NNE reduction |
| `minimum_invoice_eur_brutto` | Contract | B2B minimum consumption commitment |
| `dynamic_epex_floor_ct_kwh` | §41a EnWG | Floor on spot price pass-through |

---

## Advanced operations

### Tarifwechsel — mid-period price change

```rust
// Old tariff: Jan 1–14
let inv_old = old_engine.bill(ctx_jan1_14, &meter_old)?;
// New tariff: Jan 15–31
let inv_new = new_engine.bill(ctx_jan15_31, &meter_new)?;
// Combined January invoice via billing::merge_period_documents semantics
let merged = inv_old.merge(inv_new);
```

### Proportional cost allocation (B2B shared buildings)

```rust
// Split a shared building energy cost by floor area
let parts = building_invoice.allocate_proportionally(
    &[dec!(0.40), dec!(0.35), dec!(0.25)],
    vec![ctx_tenant_a, ctx_tenant_b, ctx_tenant_c],
)?;
// Guaranteed: parts[0].brutto + parts[1].brutto + parts[2].brutto == original.brutto
```

### §41a Abs. 6 annual savings comparison

```rust
let comparison = Sect41aAnnualComparison::compute(
    dec!(2400),   // actual kWh under dynamic tariff
    dec!(650.00), // actual EUR brutto
    dec!(40.0),   // reference fixed tariff ct/kWh
);
// Rendered as Info position on the annual invoice
```

---

## Optional features

```toml
energy-billing = { path = "…", features = ["eeg"] }   # full eeg-billing accuracy
energy-billing = { path = "…", features = ["bo4e"] }  # Invoice::to_bo4e_rechnung()
energy-billing = { path = "…", features = ["full"] }  # all optional features
```

| Feature | Enables |
|---|---|
| `eeg` | `EegProvider` delegates to `eeg_billing::calculate_settlement()` for §51/§52 |
| `bo4e` | `Invoice::to_bo4e_rechnung() -> rubo4e::current::Rechnung` |

---

## Audit trail

```rust
let ctx = BillingContext {
    billing_run_id: Some(uuid::Uuid::new_v4().to_string()),
    ..Default::default()
};
// billing_run_id propagates to:
// - Invoice.billing_run_id
// - to_rechnung_json() → ZusatzAttribut["billingRunId"]
// - to_bo4e_rechnung() → Rechnung.id + ZusatzAttribut["billingRunId"]
```

---

## Regulatory basis

| Law | Coverage |
|---|---|
| §3 StromStG | Stromsteuer 2.05 ct/kWh; `stromsteuer_for_year(year)` for retroactive corrections |
| §9 Abs. 1 Nr. 4 StromStG | Industrial Stromsteuer exemption |
| §9a Nr. 1 StromStG | Eigenverbrauch exemption ≤ 30 kWp |
| §2 EnergieStG | Erdgassteuer 0.55 ct/kWh; `energiesteuer_gas_for_year(year)` (incl. 2022 0-rate) |
| §54 EnergieStG | KWK / industrial gas Energiesteuer exemption (`gas_energiesteuer_befreiung`) |
| BEHG §10 | CO₂-Preis gas (65 EUR/t 2026; ETS2 from 2027); `behg_ct_per_kwh_for_year(year)` |
| §10 GasGVV | Brennwertkorrektur m³ → kWh_Hs |
| §12 Abs. 2 Nr. 1 UStG | Reduced 7% MwSt for renewable Fernwärme |
| §12 Abs. 3 UStG | 0% MwSt for PV ≤ 30 kWp (Solarpaket I, since 01.01.2023) |
| §14a EnWG | Controllable loads Modul 1/3 (BK6-24-174) |
| §17 Abs. 1 MessZV | Estimated reading notice |
| §40a / §40b EnWG | All-inclusive ct/kWh; structured price-comparison data |
| §41 Abs. 1 EnWG | Invoice content (Zählerstand, Netzbetreiber, Preisgarantie) |
| §41 Abs. 1 Nr. 3 EnWG | Verbrauchshistorie (prior-year + national average) |
| §41a Abs. 4 EnWG | iMSys requirement for dynamic tariffs |
| §41a Abs. 6 EnWG | Annual savings comparison |
| §42b / §42a EEG 2023 | Mieterstrom / Gemeinschaftliche Gebäudeversorgung |
| §51 EEG 2023 | Negativpreisregel (contractual LF feature) |

---

## Testing

```bash
cargo test -p energy-billing --all-features
```

**148 tests** across five suites:

| Suite | Tests | Coverage |
|---|---|---|
| `calculator_tests` | 108 | All 12 categories, §14a/§41a/GGV, seasonal, indexed, prosumer, block tariffs, RLM demand charge, multi-rate MwSt, cancellation, BO4E JSON, pro-rata, Tarifwechsel, allocation |
| Unit tests (lib) | 14 | `RegulatoryRates`, `stromsteuer_for_year`, `energiesteuer_gas_for_year`, `prorate_days`, `InvoiceType`, `PricingModel`, bridge helpers |
| `proptest_invoice` | 8 | Property-based: `brutto == netto + mwst`, cancellation sign, 0% MwSt, gas arithmetic, demand charge non-negative, StromStG year table |
| `golden_scenarios` | 6 | Golden master: SLP electricity Jan 2026; Gas + levies; EEG Gutschrift 10 kWp; RLM demand charge; gas KWK Energiesteuer exemption; 2022 historic 0-rate |
| Doc tests | 12 | Inline usage examples |
