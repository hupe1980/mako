# metering

**German energy metering domain library — interval types, gas conversion, billing period aggregation,
SLP/RLM/iMSys classification, BSI TR-03109 SMGW lifecycle, virtual meters, resampling, forecasting,
and Hampel-filter quality scoring.**

`metering` is the pure domain library used by [`edmd`](../../services/edmd/),
[`mako-gabi-gas`](../mako-gabi-gas/), and [`mako-mabis`](../mako-mabis/) for all
metering arithmetic. It has no I/O, no async, and no floating-point money.

## Modules at a glance

| Module | Key types | Purpose |
|---|---|---|
| `interval` | `MeterInterval`, `QualityFlag`, `Sparte` | Core interval + quality types |
| `obis` | `ObisCode` | Typed OBIS codes (IEC 62056-21 / BDEW) with `default_resolution()` |
| `validation` | `ValidationEngine`, `ValidationIssue` (V01–V10) | Gap / overlap / spike / DST / rollover detection |
| `substitute` | `fill_gaps()`, `SubstituteMethod`, `SubstitutionReason` | §17 MessZV Ersatzwertbildung |
| `forecast` | `project_annual_consumption()`, `prior_period_substitutes()` | §17 MessZV Jahresprognose + §17 Abs. 2 prior-period gap-fill |
| `resample` | `resample()`, `ResampledBucket` | Down-sample 15-min → hourly / daily / monthly |
| `virtual_meter` | `compute_virtual_meter()`, `AggregationRule` | §42b EEG GGV community solar, Residuallast |
| `smgw` | `SmgwSession`, `ClsChannel`, `GatewayCertificate` | BSI TR-03109 gateway lifecycle, §14a CLS channels |
| `measurement_point` | `MeasurementPoint`, `MarktRolle`, `EnergyFlow` | MaLo + MeLo + OBIS + role binding |
| `measurement_series` | `MeasurementSeries`, `MeasurementSource`, `ProvenanceEntry` | §22 MessZV provenance / explainability |
| `register` | `MeterRegister`, `EnergyDirection`, `RegisterUnit` | HT/NT register + Wandlerfaktor metadata |
| `aggregation` | `aggregate()`, `BillingPeriod`, `HtNtSplit` | §2 Nr. 17 MessZV Spitzenleistung + HT/NT |
| `classification` | `classify_messtyp()`, `Messtyp` | SLP/RLM/iMSys (§3/§4 MessZV, §41a EnWG) |
| `quality` | `score_intervals()`, `QualityGrade` | Hampel-filter quality scoring (A/B/C/F) |
| `demand` | `DemandWindow`, `DemandInterval` | 15-min demand / Spitzenleistung |
| `tariff_window` | `TariffWindow`, `HtNtSchedule` | DST-aware HT/NT window classification |
| `load_profile` | `LoadProfile` | German SLP load profile types (H0/G0–G6/L0–L2 …) |
| `conversion` | `gas_m3_to_kwh_hs()`, `GasConversionParams` | §24 GasGVV / DVGW G 685 |
| `imbalance` | `compute_imbalance()`, `ImbalanceSaldo` | §27 MessZV Mehr-/Mindermengensaldo |
| `resolution` | `IntervalResolution` | Typed interval lengths (15-min / hourly / daily …) |
| `lifecycle` | `MeterExchangeEvent`, `MeterStatus` | WiM meter exchange domain events |

---

## Design constraints

| Constraint | Detail |
|---|---|
| **No I/O** | All inputs are passed as arguments. |
| **No async** | Synchronous throughout. |
| **No float money** | All energy amounts use `rust_decimal::Decimal`. |
| **Deterministic** | Same inputs always produce the same output. |

---

## Core types

### `MeterInterval`

A single timestamped energy reading:

```rust
pub struct MeterInterval {
    pub from: OffsetDateTime,
    pub to:   OffsetDateTime,
    pub value_kwh: Decimal,
    pub quality: QualityFlag,
    pub obis_code: Option<String>,
}
```

### `QualityFlag` (8 variants)

| Variant | Billable | Description |
|---|---|---|
| `Measured` | ✅ | Direct meter reading |
| `Estimated` | ✅ | Prognosewert (valid for Abschlag per §17 MessZV) |
| `Substituted` | ✅ | §17 MessZV Ersatzwert |
| `Calculated` | ✅ | Derived (e.g. Residuallast) |
| `Corrected` | ✅ | Retroactive correction |
| `Preliminary` | ✅ | Vorläufiger Wert |
| `Faulty` | ❌ | Must not be billed |
| `Unknown` | ❌ | Quality not determinable |

---

## Gas conversion: m³ → kWh_Hs

Implements the Brennwertkorrektur formula per **§24 GasGVV** / **DVGW G 685**:

```rust
// kWh_Hs = m³ × Hs × Z  (rounded to 6 dp)
let kwh = gas_m3_to_kwh_hs(dec!(100), dec!(10.55), dec!(0.9764));
```

---

## Validation engine (V01–V10)

```rust
let result = validate_intervals(&intervals, &ValidationConfig::default());
println!("Issues: {}", result.issues.len());
println!("Billing blocked: {}", result.billing_block_count());
```

| Rule | ID | Detects |
|---|---|---|
| Gap detected | V01 | Missing intervals between adjacent reads |
| Overlap detected | V02 | Two intervals covering the same timestamp |
| Negative energy | V03 | `value_kwh < 0` for import registers |
| Impossible spike | V04 | `value_kwh > spike_factor × window_median` |
| Suspicious zero run | V05 | Long sequence of zero values |
| Inconsistent interval | V06 | Mixed 15-min and 60-min in same series |
| DST ambiguity | V07 | Potential local-time leak at CEST→CET fall-back |
| Future timestamp | V08 | `from > now` |
| Non-billable quality | V09 | `Faulty` or `Unknown` intervals in billing window |
| Register rollover | V10 | Counter reset without meter-exchange event |

---

## §17 MessZV substitute value generation

```rust
// Automatic: linear for short gaps, carry-forward for long
let filled = fill_gaps(&intervals, 900, period_from, period_to);

// Prior-period averaging per §17 Abs. 2 MessZV
let filled = fill_gaps_with_config(
    &intervals, 900, period_from, period_to,
    &FillGapsConfig::prior_period(prior_week_intervals),
);

// Annual forecast (Jahresprognose)
let forecast = project_annual_consumption("MALO_ID", &intervals, None)?;
println!("Projected annual kWh: {}", forecast.projected_annual_kwh);
```

| `SubstituteMethod` | When to use |
|---|---|
| `LinearInterpolation` | Short gaps (≤ 3 intervals) with surrounding data |
| `PriorPeriodAverage` | Same time-slot from prior reference week (§17 Abs. 2) |
| `ZeroFill` | Documented plant shutdown — affirmative zero only |
| `LastValueCarryForward` | Conservative fallback |

---

## Resampling

```rust
// Down-sample 15-min RLM data to monthly totals (for MaBiS §27 MessZV)
let monthly = resample(&intervals, &ResampleConfig::to_monthly());
for bucket in &monthly {
    println!("{}: {} kWh (coverage {:.1}%)",
        bucket.from.date(), bucket.total_kwh, bucket.coverage_pct());
}
```

---

## Virtual meters (§42b EEG GGV)

```rust
let rule = AggregationRule::GgvAllocation {
    plant_malo_id: "PLANT".into(),
    tenant_fractions: vec![("TENANT_A".into(), dec!(0.4)), ("TENANT_B".into(), dec!(0.35))],
};
let virtual_series = compute_virtual_meter(&rule, &source_map)?;
```

Available rules: `Sum`, `Residual` (§42a EEG), `PvSelfConsumption`, `GgvAllocation` (§42b EEG).

---

## BSI TR-03109 SMGW lifecycle

```rust
let session = SmgwSession { device_id, status, certificates, cls_channels, .. };

// Certificate expiry (BSI TR-03109-4 §6.3 — renew ≥ 30 days before expiry)
let expiring = session.expiring_certificates(today, 30);

// §14a CLS channel compliance
assert!(session.has_section_14a_cls());

// Communication fault detection (triggers §17 MessZV substitute)
if session.is_communication_fault(2) { // 2h threshold
    // create Sonderablesung reading order
}
```

---

## Billing period aggregation

```rust
let agg = aggregate(&intervals, &AggregationConfig::rlm_strom());
println!("kWh total:    {}", agg.arbeitsmenge_kwh);
println!("kWh HT:       {:?}", agg.ht_kwh);
println!("kWh NT:       {:?}", agg.nt_kwh);
println!("Spitzenlast:  {:?} kW", agg.spitzenleistung_kw);
```

### `AggregationConfig` presets

| Preset | Messtyp | Detail |
|---|---|---|
| `rlm_strom()` | RLM | Spitzenleistung §2 Nr. 17 MessZV + DST-aware HT/NT |
| `slp_strom()` | SLP | HT/NT split only |
| `rlm_zweitarif()` | RLM | Custom HT/NT window |
| `gas()` | Gas | Single total (m³ → kWh_Hs, no tariff split) |

---

## Feature flags

| Flag | Effect |
|---|---|
| `serde` | Derive `Serialize`/`Deserialize` on all public types |

---

## Testing

```bash
cargo test -p metering --all-features
```

177 tests covering: gas conversion (DVGW G 685), aggregation (RLM/SLP/Gas), Messtyp
classification, imbalance arithmetic, V01–V10 validation engine (incl. DST transitions),
§17 MessZV substitute methods, resampling (hourly/daily/monthly), virtual meters (GGV/Residual),
BSI TR-03109 SMGW + CLS lifecycle, measurement series provenance, register + ObisCode.

---

## Regulatory basis

- **§3, §4 MessZV** — SLP/RLM classification thresholds
- **§2 Nr. 17 MessZV** — Spitzenleistung definition for RLM
- **§17 MessZV** — Ersatzwertbildung + Jahresprognose (substitute values + annual forecast)
- **§22 MessZV** — 3-year provenance retention (`MeasurementSeries`, `ProvenanceEntry`)
- **§27 MessZV** — Mehr-/Mindermengensaldo
- **§24 GasGVV / DVGW G 685** — Gas Brennwertkorrektur
- **§41a EnWG** — 15-Minuten-Lastgang and iMSys Pflichteinbau
- **§42a/§42b EEG** — Residuallast / GGV community solar virtual meters
- **§14a EnWG** — Steuerbare Verbrauchseinrichtungen (CLS channels)
- **BSI TR-03109** — Smart Meter Gateway lifecycle and certificates

---

## Design constraints

| Constraint | Detail |
|---|---|
| **No I/O** | All inputs are passed as arguments. |
| **No async** | Synchronous throughout. |
| **No float money** | All energy amounts use `rust_decimal`. |
| **Deterministic** | Same inputs always produce the same output. |

---

## Core types

### `MeterInterval`

A single timestamped energy reading:

```rust
pub struct MeterInterval {
    pub period_start: OffsetDateTime,
    pub period_end: OffsetDateTime,
    pub value_kwh: Decimal,
    pub quality: QualityFlag,
}
```

### `Sparte`

```rust
pub enum Sparte { Strom, Gas }
```

### `QualityFlag`

`Measured`, `Substituted`, `Estimated`, `Invalid` — mapped to MSCONS/UTILTS
Datenqualitätskennzeichen.

---

## Gas conversion: m³ → kWh_Hs

```rust
pub fn gas_m3_to_kwh_hs(
    m3: Decimal,
    brennwert_kwh_m3: Decimal,
    zustandszahl: Decimal,
) -> Decimal
```

Implements the Brennwertkorrektur formula per **§24 GasGVV** / **DVGW G 685**:

$$kWh_{Hs} = m^3 \times z \times H_s$$

---

## Billing period aggregation

`aggregate` computes a `BillingPeriodAggregate` from a slice of `MeterInterval`s:

```rust
let agg = aggregate(&intervals, &AggregationConfig::rlm_strom())?;
println!("kWh HT: {}", agg.kwh_ht);
println!("kWh NT: {}", agg.kwh_nt);
println!("Spitzenlast: {:?} kW", agg.spitzenleistung_kw);
```

### `AggregationConfig` presets

| Preset | Messtyp | Split |
|---|---|---|
| `rlm_strom()` | RLM | Spitzenleistung §2 Nr. 17 MessZV + HT/NT split |
| `slp_strom()` | SLP | HT/NT split (no Spitzenleistung) |
| `rlm_zweitarif()` | RLM | Custom HT/NT window |
| `gas()` | Gas | Single total (m³ → kWh_Hs, no tariff split) |

---

## SLP/RLM/iMSys classification

```rust
pub fn classify_messtyp(
    malo: &MaloId,
    jahresverbrauch_kwh: Decimal,
    sparte: Sparte,
) -> Messtyp
```

| `Messtyp` | Criteria | Basis |
|---|---|---|
| `Slp` | < 100 MWh/a (Strom) or < 1.500 MWh/a (Gas) | §3 MessZV |
| `Rlm` | ≥ 100 MWh/a (Strom) or ≥ 1.500 MWh/a (Gas) | §4 MessZV |
| `Imsys` | Pflichteinbau iMSys (§41a EnWG) | §41a EnWG |

---

## Mehr-/Mindermengensaldo

```rust
pub fn compute_imbalance(
    forecast_kwh: Decimal,
    actual_kwh: Decimal,
) -> ImbalanceResult
```

`ImbalanceResult` contains `delta_kwh` and `delta_pct`.
Positive = Mehrmenge (actual > forecast); negative = Mindermenge.
Basis: **§27 MessZV**.

---

## Hampel-filter quality scoring

`score_intervals` runs a Hampel filter over a time-ordered slice of intervals
and returns per-interval `QualityGrade`s:

```rust
pub fn score_intervals(
    intervals: &[MeterInterval],
    window_k: usize,   // half-window size (default: 3)
    threshold: f64,    // MAD threshold in sigma units (default: 3.0)
) -> Vec<QualityGrade>
```

### `QualityGrade`

| Grade | Meaning | Effect in `edmd` |
|---|---|---|
| `A` | Clean — within threshold | Billing proceeds normally |
| `B` | Slightly suspicious | Logged; billing proceeds |
| `C` | Likely outlier | `de.edmd.reading.quality.warning` CloudEvent emitted |
| `F` | Severe outlier / substituted | Billing run blocked; operator alert |

`hampel_filter` is also exposed as a low-level primitive that returns raw
outlier indices.

---

## §17 MessZV substitute value generation

When meter readings are missing or faulty, the MSB must supply substitute values
before billing. `metering` implements all four methods from §17 MessZV and BDEW
practice.

### Quick usage

```rust
use metering::{fill_gaps, fill_gaps_with_config, FillGapsConfig, SubstituteMethod};

// Automatic method selection (linear for short gaps, carry-forward for long)
let filled = fill_gaps(&intervals, 900, period_from, period_to);

// Prior-period averaging per §17 Abs. 2 MessZV (same time-slot, prior week)
let prior_week: Vec<MeterInterval> = fetch_prior_week(&malo_id);
let filled = fill_gaps_with_config(
    &intervals, 900, period_from, period_to,
    &FillGapsConfig::prior_period(prior_week),
);
```

### `SubstituteMethod` variants

| Variant | When to use | BDEW recommendation |
|---|---|---|
| `LinearInterpolation` | Short gaps (≤ 3 intervals) with surrounding data | Primary for RLM/iMSys |
| `PriorPeriodAverage` | Longer gaps; same time-slot from prior reference week | Biomass, industrial load |
| `ZeroFill` | Documented plant shutdown — affirmative zero only | Outage with evidence |
| `LastValueCarryForward` | Conservative fallback when no context available | SLP, default for longer gaps |

### `FillGapsConfig`

```rust
pub struct FillGapsConfig {
    pub method: SubstituteMethod,            // default: LinearInterpolation
    pub prior_period_intervals: Vec<MeterInterval>, // for PriorPeriodAverage
    pub short_gap_threshold: usize,          // default: 3 (auto-linear below this)
}
```

`FillGapsConfig::prior_period(prior_week_intervals)` and
`FillGapsConfig::zero_fill()` are convenience constructors.

Filled intervals carry `quality = QualityFlag::Substituted`
(billable per §17 MessZV Abs. 1).

---

## Feature flags

| Flag | Effect |
|---|---|
| `serde` | Derive `Serialize`/`Deserialize` on all public types |

---

## Testing

```bash
cargo test -p metering --all-features
```

37 tests covering gas conversion, aggregation (RLM/SLP/Gas), Messtyp
classification, imbalance arithmetic, Hampel filter edge cases, and §17 MessZV
substitute value generation (all four methods including `PriorPeriodAverage`).

---

## Regulatory basis

- **§3, §4 MessZV** — SLP/RLM classification thresholds
- **§2 Nr. 17 MessZV** — Spitzenleistung definition for RLM
- **§17 MessZV** — Ersatzwertbildung (substitute value generation)
- **§27 MessZV** — Mehr-/Mindermengensaldo
- **§24 GasGVV / DVGW G 685** — Gas Brennwertkorrektur
- **§41a EnWG** — 15-Minuten-Lastgang and iMSys Pflichteinbau
