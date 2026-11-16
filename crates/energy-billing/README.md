# energy-billing

[![Crates.io](https://img.shields.io/crates/v/energy-billing?label=energy-billing&color=f59e0b&logo=rust)](https://crates.io/crates/energy-billing)

The Lieferant's retail billing engine for German energy supply: given a
product, the meter quantities and the dated statutory rates, it computes the
customer's invoice — Strom, Gas, Wärme, Wasser, § 14a EnWG modules, § 41a EnWG
dynamic tariffs, Abschläge and Storno. Pure and synchronous, with no rate
hard-coded and money in fixed-point `Amount`, never `f64`; a missing input is a
refusal, never a zero. Payments, ledger and dunning are the caller's.

## Example

```rust
use energy_billing::{BillingContext, BillingPeriod, GridInput, InvoiceType, MeterInput,
                     Product, Quantities, RegulatoryRates};
use rust_decimal::dec;
use time::macros::date;

// A productd product, discriminated by its "category"
let product: Product = serde_json::from_str(r#"{
    "category": "STROM",
    "arbeitspreis_ct_per_kwh": "32.0",
    "grundpreis_ct_per_day": "12.0"
}"#)?;

let ctx = BillingContext {
    malo_id:          "51238696012".to_owned(),
    lf_mp_id:         "9910000000002".to_owned(),
    rechnungsnummer:  "R2026-06-001".to_owned(),
    period:           BillingPeriod::new(date!(2026-06-01), date!(2026-06-30))?,
    invoice_type:     InvoiceType::Initial,
    regulatory_rates: RegulatoryRates::default(),
    ..Default::default()
};
let quantities = Quantities {
    electricity: Some(MeterInput { arbeitsmenge_kwh: dec!(312.5), ..Default::default() }),
    ..Default::default()
};

let invoice = product
    .build_engine(&GridInput::default(), &ctx.regulatory_rates)
    .bill(ctx, &quantities)?;
invoice.assert_valid();
println!("Brutto: {} EUR", invoice.brutto_eur);
```

## What it covers

`Product` has 13 variants, each deserialised by its `category` tag and refusing
a key it does not define; `build_engine()` routes it to its providers.

| `category` | Provider | Rules |
|---|---|---|
| `STROM` | `ElectricityProvider` / `DynamicElectricityProvider` | SLP/RLM, HT/NT, block tariffs; § 41a EnWG dynamic tariff (iMSys required) |
| `WAERMEPUMPE`, `WALLBOX` | `ControllableLoadProvider` | § 14a EnWG Modul 1/2/3 (BK8-22/010-A) |
| `GAS` | `GasProvider` | Brennwertkorrektur, Energiesteuer, BEHG CO₂ price |
| `WAERME` | `HeatProvider` | AVBFernwärmeV § 24 Preisgleitklausel, CO2KostAufG, § 14 WPG |
| `WASSER` | `WaterProvider` | Trinkwasser 7 % USt, gesplittete Abwassergebühr |
| `SOLAR` | `SolarProvider` | § 42a Abs. 4 EnWG Mieterstrom-Preisdeckel, § 42b EnWG GGV, § 9 Abs. 1 Nr. 3 StromStG |
| `EEG` | `EegProvider` | LF-side Gutschrift; delegates to `eeg-billing` with feature `eeg` |
| `EINSPEISUNG` | `EinspeisungProvider` | Direktvermarktung: Marktwert minus Gebühr |
| `HEMS`, `EMOBILITY`, `ENERGIEDIENSTLEISTUNG` | `HemsProvider`, `EmobilityProvider`, `ServiceProvider` | subscriptions, sessions, per-event fees |
| `SHARING` | `ElectricityProvider` + `EnergyShareProvider` | § 42c EnWG sharing credit on the shared allocation |

Around them: `BillingEngine` (`validate()`, `bill()`, `bill_batch()`), `Invoice`
(positions, VAT breakdown, `warnings`), `rates` (the dated Stromsteuer,
Energiesteuer, BEHG and Umsatzsteuer rates) and `steuer` (Befreiung,
Ermäßigung, Entlastung). A § 19 UStG Kleinunternehmer document charges and
states no Umsatzsteuer.

## Features

| Feature | Enables |
|---|---|
| `eeg` | `EegProvider` delegates to `eeg_billing::calculate_settlement` |
| `bo4e` | `Invoice::to_rechnung()` / `to_rechnung_json()` |
| `en16931` | `Invoice::to_en16931(…)`, the semantic model `en16931-formats` renders |

All three are off by default.

## Sources

- EnWG §§ 14a, 41, 41a, 42a, 42b, 42c; BNetzA BK8-22/010-A
- StromStG / StromStV, EnergieStG, BEHG, UStG
- AVBFernwärmeV, CO2KostAufG, WPG, AVBWasserV

Operator reference: [billingd](https://hupe1980.github.io/mako/docs/services/billingd/) ·
API: <https://docs.rs/energy-billing>

License: MIT OR Apache-2.0
