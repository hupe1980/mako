# energy-billing

[![Crates.io](https://img.shields.io/crates/v/energy-billing?label=energy-billing&color=f59e0b&logo=rust)](https://crates.io/crates/energy-billing)

Retail energy billing for German suppliers: given a product, the meter
quantities and the statutory rates, it computes the customer's invoice — Strom,
Gas, Wärme, Wasser, § 14a EnWG modules, § 41a dynamic tariffs, Abschläge and
Storno. Zero I/O, zero async, no hard-coded regulatory rates; money is
`Decimal`. It is the calculation core of
[`billingd`](https://hupe1980.github.io/mako/docs/services/billingd/).

```bash
cargo add energy-billing --features bo4e
```

## Example

```rust
use energy_billing::{BillingContext, BillingPeriod, GridInput, InvoiceType, MeterInput,
                     Product, Quantities, RegulatoryRates};
use rust_decimal::dec;
use time::macros::date;

// Deserialize directly from productd JSONB using the "category" discriminator
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
    electricity: Some(MeterInput {
        arbeitsmenge_kwh: dec!(312.5),
        ..Default::default()
    }),
    ..Default::default()
};

// Build and run — Product::build_engine() handles all category routing
let invoice = product
    .build_engine(&GridInput::default(), &ctx.regulatory_rates)
    .bill(ctx, &quantities)?;

invoice.assert_valid();
println!("Brutto: {} EUR", invoice.brutto_eur);

let rechnung_json: serde_json::Value = invoice.to_rechnung_json();  // feature `bo4e`
```

## Key types

| Item | Purpose |
|---|---|
| `Product` | one variant per category, deserialised from productd JSON by its `category` tag; `build_engine()` routes to the providers |
| `BillingContext`, `BillingPeriod`, `Quantities`, `RegulatoryRates` | the period (validated), identities, meter inputs and dated statutory rates |
| `BillingEngine` | `validate()` pre-flight, `bill()` → `Invoice`, `bill_batch()` for portfolios |
| `Invoice` | positions, totals, VAT breakdown and `warnings` (`has_errors()`); a missing input is a refusal, never a zero |
| `steuer` | Stromsteuer/Energiesteuer Befreiung, Ermäßigung and Entlastung |

| `Product` variant | Category string | Provider | Key features |
|---|---|---|---|
| `Strom(ElectricityProduct)` | `STROM` | `ElectricityProvider` or `DynamicElectricityProvider` | SLP/RLM; HT/NT; block tariffs; §41a EPEX |
| `Waermepumpe(ControllableLoadProduct)` | `WAERMEPUMPE` | `ControllableLoadProvider` | §14a Modul 1/2/3 |
| `Wallbox(ControllableLoadProduct)` | `WALLBOX` | `ControllableLoadProvider` | §14a Modul 1/2/3 |
| `Gas(GasProduct)` | `GAS` | `GasProvider` | Brennwertkorrektur; Energiesteuer; BEHG CO₂ |
| `Waerme(HeatProduct)` | `WAERME` | `HeatProvider` | Fernwärme; standard-rated (19 %); AVBFernwärmeV §24 Preisgleitklausel; the CO2KostAufG § 3 Abs. 1 statement + § 14 WPG Anteil |
| `Wasser(WaterProduct)` | `WASSER` | `WaterProvider` | Trinkwasser 7 % USt; gesplittete Abwassergebühr (Schmutzwasser − Absetzungen, Niederschlagswasser m²); public-law fee is EN 16931 `O`, not `Z` |
| `Solar(SolarProduct)` | `SOLAR` | `SolarProvider` | §42b EnWG GGV; Mieterstrom mit § 42a Abs. 4 EnWG 90 %-Deckel; Stromsteuer per § 9 Abs. 1 Nr. 3 StromStG, **stated** not omitted; 0 % USt if Kleinunternehmer (§19 UStG) |
| `Eeg(EegProduct)` | `EEG` | `EegProvider` | LF-side Gutschrift; `eeg` feature for §51/§52 |
| `Einspeisung(EinspeisungProduct)` | `EINSPEISUNG` | `EinspeisungProvider` | Direktvermarktung Marktwert − Gebühr |
| `Hems(HemsProduct)` | `HEMS` | `HemsProvider` | Platform subscription + events |
| `Emobility(EmobilityProduct)` | `EMOBILITY` | `EmobilityProvider` | CPO/EMSP: service + kWh + session/roaming |
| `Energiedienstleistung(ServiceProduct)` | `ENERGIEDIENSTLEISTUNG` | `ServiceProvider` | Flat fee + per-event |
| `Sharing(SharingProduct)` | `SHARING` | `ElectricityProvider` + `EnergyShareProvider` | §42c Energiegemeinschaft credit |

| Feature | Enables |
|---|---|
| `eeg` | `EegProvider` delegates to `eeg_billing::calculate_settlement()` |
| `bo4e` | `Invoice::to_rechnung()` / `to_rechnung_json()` |
| `en16931` | `Invoice::to_en16931(…)` — the semantic model `en16931-formats` renders |

All three are off by default; `billingd` enables all three. API documentation:
<https://docs.rs/energy-billing>.
