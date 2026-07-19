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
            ├── Product                — typed enum with 12 per-category variants
            │     ├── Strom(ElectricityProduct)
            │     ├── Waermepumpe/Wallbox(ControllableLoadProduct)   §14a
            │     ├── Gas(GasProduct)
            │     ├── Waerme(HeatProduct)
            │     ├── Solar(SolarProduct)
            │     ├── Eeg(EegProduct)
            │     ├── Einspeisung(EinspeisungProduct)
            │     ├── Hems/Emobility/Energiedienstleistung(…)
            │     └── Sharing(SharingProduct)                        §42c
            │
            ├── Quantities             — all meter inputs for one billing period
            ├── BillingContext         — period, IDs, invoice type, regulatory rates
            ├── BillingEngine          — composes BillingProvider instances
            │     ├── validate()       — pre-flight regulatory check (no positions)
            │     ├── bill(&self, …)   — pure function → Result<Invoice, BillingError>
            │     └── bill_batch(…)    — portfolio billing
            ├── BillingProvider        — one implementation per product/tax type
            └── Invoice                — result with positions + totals + warnings + BO4E JSON
                  ├── warnings: Vec<BillingWarning>    — regulatory compliance notices
                  ├── has_errors()                     — any Error-severity warning?
                  └── to_rechnung_json()               — BO4E JSONB for accountingd
```

The engine runs in passes:

```
Pass 0  validate_warnings()      §41b iMSys guard · regulatory pre-checks
Pass 1  commodity / levy providers   (ElectricityProvider, GasProvider, …)
Pass 2  tax provider                 (MwStProvider — sees all net positions)
Pass 3  Abschlag deductions          (Final invoice reconciliation)
Pass 4  Minimum invoice top-up       (B2B Mindestabnahmeverpflichtung)
Pass 5  Cancellation sign reversal   (Stornorechnung — all signs negated)
```

---

## Quick start

```rust
use energy_billing::{Product, BillingContext, InvoiceType, MeterInput, Quantities,
                     RegulatoryRates, GridInput};
use rust_decimal_macros::dec;
use time::macros::date;

// Deserialize directly from tarifbd JSONB using the "category" discriminator
let product: Product = serde_json::from_str(r#"{
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

// Build and run — Product::build_engine() handles all category routing
let invoice = product
    .build_engine(&GridInput::default(), &ctx.regulatory_rates)
    .bill(ctx, &quantities)?;

invoice.assert_valid();
println!("Brutto: {} EUR", invoice.brutto_eur);

let rechnung_json: serde_json::Value = invoice.to_rechnung_json();
```

---

## Product enum

`Product` is the typed dispatch enum that replaces the old flat `TariffInput` god-struct.
Each category has its own struct with only the relevant fields — no silent field confusion.

```rust
// Deserializes via #[serde(tag = "category")] from flat tarifbd JSONB:
// {"category":"STROM","arbeitspreis_ct_per_kwh":28.5} → Product::Strom(ElectricityProduct{...})
// {"category":"WAERMEPUMPE","sect14a_modul1_nne_reduktion_ct_per_kwh":1.5,...} → Product::Waermepumpe(...)
// {"category":"GAS","gas_arbeitspreis_ct_per_kwh_hs":7.5,...} → Product::Gas(GasProduct{...})
```

| `Product` variant | Category string | Provider | Key features |
|---|---|---|---|
| `Strom(ElectricityProduct)` | `STROM` | `ElectricityProvider` or `DynamicElectricityProvider` | SLP/RLM; HT/NT; block tariffs; §41a EPEX |
| `Waermepumpe(ControllableLoadProduct)` | `WAERMEPUMPE` | `ControllableLoadProvider` | §14a Modul 1/3 mandatory |
| `Wallbox(ControllableLoadProduct)` | `WALLBOX` | `ControllableLoadProvider` | §14a Modul 1/3 mandatory |
| `Gas(GasProduct)` | `GAS` | `GasProvider` | Brennwertkorrektur; Energiesteuer; BEHG CO₂ |
| `Waerme(HeatProduct)` | `WAERME` | `HeatProvider` | Fernwärme; auto-7% MwSt renewable |
| `Solar(SolarProduct)` | `SOLAR` | `SolarProvider` | §42b GGV; §42a Mieterstrom; 0% MwSt ≤30 kWp |
| `Eeg(EegProduct)` | `EEG` | `EegProvider` | LF-side Gutschrift; `eeg` feature for §51/§52 |
| `Einspeisung(EinspeisungProduct)` | `EINSPEISUNG` | `EinspeisungProvider` | Direktvermarktung Marktwert − Gebühr |
| `Hems(HemsProduct)` | `HEMS` | `HemsProvider` | Platform subscription + events |
| `Emobility(EmobilityProduct)` | `EMOBILITY` | `EmobilityProvider` | CPO/EMSP: service + kWh + session/roaming |
| `Energiedienstleistung(ServiceProduct)` | `ENERGIEDIENSTLEISTUNG` | `ServiceProvider` | Flat fee + per-event |
| `Sharing(SharingProduct)` | `SHARING` | `ElectricityProvider` + `EnergyShareProvider` | §42c Energiegemeinschaft credit |

`ControllableLoadProduct` composes `ElectricityProduct` (via `#[serde(flatten)]`) plus §14a fields — the standard electricity billing is delegated to `ElectricityProvider` then §14a credits are appended.

---

## Pricing capabilities

| Feature | How |
|---|---|
| HT/NT Zweitarif | `billing::TimeOfUsePricing` (validated, penny-correct) |
| Block / graduated tariffs | `billing::TariffSchedule::graduated()` |
| Indexed prices (TTF, Phelix, NCG) | `IndexedPriceConfig { base_ct, spread_ct, index_value, factor }` |
| Gas indexed price | `gas_indexed_price: Option<IndexedPriceConfig>` in `GasProduct` |
| Seasonal prices | `SeasonalPriceOverride` by month range (wraps year boundary) |
| §41a EPEX dynamic | `billing::DynamicPricing` with per-interval kWh × price |
| §41b iMSys guard | Hard error when `dynamic_epex=true` and `MeteringMode != Imsys` |
| Pro-rata Grundpreis | `ctx.prorate_days()` clips to `vertragsbeginn`/`vertragsende` |
| Minimum invoice (B2B) | Pass 4 auto-top-up to `minimum_invoice_eur_brutto` |
| Discounts / bonuses | `auf_abschlag_ct_per_kwh`, `auf_abschlag_eur_per_month`, `Bonus` category |
| MSB pass-through | `msb_gebuehr_ct_per_day` (MsbG) |
| Multi-rate MwSt | Per-position `applicable_tax_rate` → grouped `MwStProvider` |
| Auto-0% MwSt solar ≤30 kWp | `anlage_kwp ≤ 30` (§12 Abs. 3 UStG Solarpaket I) |
| Stromsteuer exemption | `StromsteuerBefreiung` typed enum (§9 Nr. 1-5 + §9a) |
| Gas RLM Leistungspreis | `gas_leistungspreis_ct_per_kw_month` in `GasProduct` |
| §42 Energiemix | `EnergieQuellen` struct with `co2_g_per_kwh` (mandatory §42 Abs. 2 Nr. 2 EnWG) |

---

## Regulatory compliance

### §41b EnWG — iMSys guard for dynamic tariffs

Dynamic tariffs (`Product::Strom(p)` where `p.dynamic_epex = true`) require an intelligent
metering system. `BillingEngine::bill()` rejects with `BillingError::InvalidInput` when
`quantities.electricity.metering_mode != MeteringMode::Imsys`:

```rust
// Pre-flight check: validate without generating positions
let warnings = engine.validate(&ctx, &quantities);
for w in &warnings {
    if w.severity == WarningSeverity::Error {
        eprintln!("[{}] {}", w.code, w.message);
    }
}
// §41b violations produce BillingWarning { code: "SECT41B_IMSYS_REQUIRED", severity: Error }
```

### §9 StromStG — typed Stromsteuer exemption

`StromsteuerBefreiung` is a typed enum covering all §9 StromStG exemption grounds:

```rust
pub enum StromsteuerBefreiung {
    Keine,                     // Standard Stromsteuer applies
    Bahnstrom,                 // §9 Nr. 1 — rail traction
    NachweisErneuerbarer,      // §9 Nr. 2 — certified renewable
    KwkSelbstverbrauch,        // §9 Nr. 3 — CHP < 2 MW
    IndustrieProduktionesGewerbe, // §9 Nr. 4 — industry > 2 GWh/year
    LandForstwirtschaft,       // §9 Nr. 5 — agricultural
    SolarEigenverbrauch,       // §9a Nr. 1 — PV self-consumption ≤ 30 kWp
}
```

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

## Key regulatory fields per product

### `ElectricityProduct` / `ControllableLoadProduct`

| Field | Law | Effect |
|---|---|---|
| `anlage_kwp` | §12 Abs. 3 UStG | Auto-0% MwSt when ≤ 30 kWp (Solarpaket I 2023) |
| `stromsteuer_befreiung` | §9 StromStG | Typed enum; replaces levy with exemption notice |
| `industrie_stromsteuer_befreiung` | §9 Nr. 4 StromStG | Legacy bool; prefer `stromsteuer_befreiung` |
| `leistungspreis_strom_ct_per_kw_month` | §41 EnWG | RLM demand charge (ct/kW/month) |
| `preisgarantie_bis` | §41 Abs. 1 Nr. 4 EnWG | Price guarantee expiry on invoice |
| `mwst_rate_override` | §12 UStG | Override 19% per product |
| `dynamic_epex` | §41a EnWG | EPEX spot billing (requires `MeteringMode::Imsys`) |
| `dynamic_epex_floor_ct_kwh` | §41a EnWG | Price floor for spot pass-through |
| `energiequellen` | §42 Abs. 2 Nr. 2 EnWG | Typed fuel mix with CO₂ label |

### `ControllableLoadProduct` (§14a extras)

| Field | Law | Effect |
|---|---|---|
| `sect14a_modul1_nne_reduktion_ct_per_kwh` | §14a EnWG | Per-kWh NNE credit |
| `steuerungsrabatt_modul1_eur_per_kw_year` | §14a EnWG | Capacity NNE reduction |
| `sect14a_modul3_entschaedigung_ct_per_kwh` | §14a EnWG | Per-kWh Entschädigung |
| `steuerungsrabatt_modul3_eur_per_kw_year` | §14a EnWG | Capacity Entschädigung |

### `GasProduct`

| Field | Law | Effect |
|---|---|---|
| `gas_energiesteuer_befreiung` | §54 EnergieStG | KWK / industrial exemption notice |
| `gas_leistungspreis_ct_per_kw_month` | §41 EnWG | RLM demand charge for large gas customers |
| `gas_indexed_price` | §41 Abs. 3 EnWG | B2B TTF/NCG indexed price (alias: `indexed_price`) |

---

## Advanced operations

### Tarifwechsel — mid-period price change

```rust
// Old tariff: Jan 1–14
let inv_old = old_product.build_engine(&grid, &rates).bill(ctx_jan1_14, &meter_old)?;
// New tariff: Jan 15–31
let inv_new = new_product.build_engine(&grid, &rates).bill(ctx_jan15_31, &meter_new)?;
// Combined January invoice
let merged = inv_old.merge(inv_new);
```

### Portfolio billing

```rust
let engine = product.build_engine(&grid, &rates);
let results: Vec<Result<Invoice, BillingError>> = engine.bill_batch(
    customers.into_iter().map(|(ctx, quantities)| (ctx, quantities)).collect()
);
```

### Regulatory pre-flight

```rust
let engine = product.build_engine(&grid, &rates);
let warnings = engine.validate(&ctx, &quantities);
if invoice.has_errors() {
    // Block dispatch — Error-severity regulatory violation
}
```

### Proportional cost allocation (B2B shared buildings)

```rust
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
energy-billing = { path = "…", features = ["full"] }  # all optional features
```

| Feature | Enables |
|---|---|
| `eeg` | `EegProvider` delegates to `eeg_billing::calculate_settlement()` for §51/§52/§36k |

> **Note:** The `bo4e` / `rubo4e` dependency has been removed from `energy-billing`.
> `Invoice::to_rechnung_json()` produces BO4E-compatible JSON without any external dependency.
> For typed `rubo4e::current::Rechnung` output, convert the JSON in `billingd`'s service layer.

---

## Audit trail and explainability

Every `BillingPosition` carries a `PositionTrace` with the full calculation audit:

```rust
pub struct PositionTrace {
    pub formula: String,              // "500.000 kWh × 0.30000 EUR/kWh = 150.00000 EUR"
    pub input_quantity: Decimal,
    pub input_unit_price_eur: Decimal,
    pub gross_eur: Decimal,
    pub regulatory_basis: Vec<String>, // ["§3 StromStG", "§41 EnWG"]
    pub tariff_source: Option<String>, // product sheet ID from tarifbd
    pub pro_rata_fraction: Option<Decimal>,
}
```

The `BillingWarning` field on `Invoice` carries regulatory compliance notices:

```rust
// Check for dispatch-blocking violations
if invoice.has_errors() {
    for w in invoice.warnings.iter().filter(|w| w.severity == WarningSeverity::Error) {
        // e.g. { code: "SECT41B_IMSYS_REQUIRED", message: "§41b Abs. 2 EnWG: …" }
    }
}
```

---

## Regulatory basis

| Law | Coverage |
|---|---|
| §3 StromStG | Stromsteuer 2.05 ct/kWh; `stromsteuer_for_year(year)` for retroactive corrections |
| §9 StromStG | All 5 exemption grounds + §9a via typed `StromsteuerBefreiung` enum |
| §2 EnergieStG | Erdgassteuer 0.55 ct/kWh; `energiesteuer_gas_for_year(year)` (incl. 2022 0-rate) |
| §54 EnergieStG | KWK / industrial gas Energiesteuer exemption |
| BEHG §10 | CO₂-Preis H-Gas (65 EUR/t 2026) + L-Gas factor; `behg_ct_per_kwh_for_year(year)` |
| §25 Nr. 4 MessEV | Brennwertkorrektur m³ → kWh_Hs |
| §12 Abs. 2 Nr. 1 UStG | Reduced 7% MwSt for renewable Fernwärme |
| §12 Abs. 3 UStG | 0% MwSt for PV ≤ 30 kWp (Solarpaket I, since 01.01.2023) |
| §14a EnWG | Controllable loads Modul 1/3 (BK6-24-174) via `ControllableLoadProvider` |
| §17 Abs. 1 MessZV | Estimated reading notice on invoice |
| §40a / §40b EnWG | Mandatory ct/kWh; structured price-comparison data in JSON |
| §41 Abs. 1 EnWG | Invoice content (Zählerstand, Netzbetreiber, Preisgarantie, Energiemix) |
| §41 Abs. 1 Nr. 3 EnWG | Verbrauchshistorie (prior-year + national average) |
| §41a / §41b EnWG | §41a EPEX per-interval; §41b iMSys guard enforced as hard error |
| §42 Abs. 2 Nr. 2 EnWG | CO₂ emissions label via typed `EnergieQuellen.co2_g_per_kwh` |
| §42b / §42a EEG 2023 | Mieterstrom / Gemeinschaftliche Gebäudeversorgung |
| §42c EnWG | Energiegemeinschaft sharing credit via `SharingProduct` |
| §51 EEG 2023 | Negativpreisregel (contractual LF feature via `eeg` feature) |

---

## Testing

```bash
cargo test -p energy-billing --all-features
```

**160 tests** across five suites:

| Suite | Tests | Coverage |
|---|---|---|
| Unit tests (lib) | 18 | `RegulatoryRates`, levy lookups, `prorate_days`, `InvoiceType`, `Product` enum roundtrip, `StromsteuerBefreiung`, tariff deserialization |
| `calculator_tests` | 108 | All 12 categories, §14a/§41a/§41b, GGV, seasonal, indexed, prosumer, block tariffs, RLM demand charge, multi-rate MwSt, cancellation, BO4E JSON, pro-rata, Tarifwechsel, `bill_batch`, `validate` |
| `golden_scenarios` | 11 | Golden master: SLP electricity; gas + levies; EEG Gutschrift; RLM demand charge; §54 KWK exemption; 2022 0-rate; §41b rejection; §40a ct/kWh; §41 mandatory fields; §42c sharing; §9 exemption |
| `proptest_invoice` | 8 | Property-based: `brutto == netto + mwst`, cancellation sign, 0% MwSt, gas arithmetic, demand charge non-negative, StromStG year table |
| Doc tests | 15 | Inline usage examples |
