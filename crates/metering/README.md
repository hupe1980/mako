# metering

**German energy metering domain library — interval types, gas conversion, billing period aggregation,
SLP/RLM/iMSys classification, BSI TR-03109 SMGW lifecycle, virtual meters (§42b EnWG GGV Solarpaket I),
resampling, forecasting, and Hampel-filter quality scoring.

214 tests · zero I/O · no async · no float money**

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
| `virtual_meter` | `compute_virtual_meter()`, `AggregationRule` | §42b EnWG GGV: `GgvConstantAllocation` (CCI+ZG6) + `GgvProportionalAllocation` (variable); Residuallast (§42a) |
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
| `conversion` | `gas_m3_to_kwh_hs()`, `GasConversionParams` | §25 Nr. 4 MessEV / DVGW G 685 |
| `imbalance` | `compute_imbalance()`, `ImbalanceSaldo` | §27 MessZV Mehr-/Mindermengensaldo |
| `resolution` | `IntervalResolution` | Typed interval lengths (15-min / hourly / daily …) |
| `sharing` | §42c EnWG Energy-Sharing metering eligibility — pure capability/delivery rules over `Messtyp` and `IntervalLengthClass` |
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

### `Sparte` and `MeasurementUnit`

```rust
pub enum Sparte          { Strom, Gas, Waerme, Wasser }
pub enum MeasurementUnit { KiloWattHour, CubicMetre }
```

A Sparte has **two** units, and conflating them is a correctness bug:

| `Sparte` | `measured_unit()` | `billing_unit()` | `requires_conversion()` |
|---|---|---|---|
| `Strom` | kWh | kWh | no |
| `Gas` | **m³** | **kWh** | **yes** |
| `Waerme` | kWh | kWh | no |
| `Wasser` | m³ | m³ | no |

A gas meter registers volume; its energy content is derived from Brennwert and
Zustandszahl (`gas_m3_to_kwh_hs`). `requires_conversion()` lets an ingest path
require those parameters before storing a value in an energy column.

`Wasser` is the one Sparte billed in a volume — water has no calorific value, so
the gas conversion does not apply to it. For the heat share of warm water see
[HeizkostenV §9 Abs. 2](#warm-water--heat-energy-heizkostenv-9-abs-2).

### Wire units vs storage units

Storage is canonical — exactly two units reach a database, so no consumer has to
know the table below exists. The **wire** is liberal, because real devices are:

```rust
MeasurementUnit::parse("MWh")         // None — would need rescaling
MeasurementUnit::parse_scaled("MWh")  // Some(UnitScale { KiloWattHour, 1000, 1 })
```

| Accepted | Canonical | Factor |
|---|---|---|
| `kWh`, `kWh_th`, `kWh_Hs` | kWh | 1 |
| `Wh` / `MWh` / `GWh` | kWh | 1/1000 · 1000 · 10⁶ |
| `GJ` | kWh | **2500/9** |
| `MJ` | kWh | **5/18** |
| `m³`, `m3`, `cbm` | m³ | 1 |
| `l`, `ltr`, `liter` | m³ | 1/1000 |

**No German law prescribes a unit for heat meters.** MID Annex VI (MI-004) has no
units clause, and EN 1434-1 cl. 6.3.1 permits *"Joules, Watt-hours or decimal
multiples of those units"* — so a GJ meter is exactly as compliant as a kWh one.
German heat meters ship with kWh, MWh or GJ registers depending on the ordered
variant (ista sensonic 3 is sold in both kWh and GJ; Zenner multidata WR3 offers
MJ), and water submeters commonly report litres. The register unit is therefore
**device metadata, not a constant**.

UN/ECE Rec 20 codes are also accepted, and their mnemonics do not follow the unit
symbols:

| Rec 20 | Means | Trap |
|---|---|---|
| `MTQ` | cubic metre | not `M3` |
| `GV` | gigajoule | not `GJ` |
| `3B` | megajoule | `MJ` is not a Rec 20 code |
| `WHR` / `JOU` / `KJO` | watt hour / joule / kilojoule | |

Rec 20 also assigns `GJ` to gram per millilitre. This crate reads `GJ` as
gigajoule, since no Sparte modelled here carries a density; callers emitting Rec 20
codes should send `GV`.

The GJ and MJ factors are held as **exact rationals**, not decimals: 1 GJ is
277.7… kWh, so a `Decimal` factor would lose precision on every reading.
`UnitScale::apply` multiplies before dividing, making `3.6 GJ` exactly `1000 kWh`.

`parse` rejects anything needing a rescale, so a caller must go through
`parse_scaled` to obtain a factor.

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

Implements the Brennwertkorrektur formula per **§25 Nr. 4 MessEV** / **DVGW G 685**:

```rust
// kWh_Hs = m³ × Hs × Z  (rounded to 6 dp)
let kwh = gas_m3_to_kwh_hs(dec!(100), dec!(10.55), dec!(0.9764));
```

---

## Warm water → heat energy (HeizkostenV §9 Abs. 2)

```rust
warm_water_heat_kwh(volume_m3, mean_temp_c, WarmWaterAdjustments::NONE)
warm_water_heat_kwh_unmetered(flaeche_m2, adjustments)
```

```text
Q [kWh/a] = 2.5 × V [m³] × (t_w [°C] − 10)      // Satz 2, metered volume
Q [kWh/a] = 32 × A_Wohn [m²]                    // Satz 4, floor area
```

Both are fallbacks. **§9 Abs. 2 Satz 1 requires a Wärmezähler**; Satz 2 applies
only where measurement is possible *"nur mit einem unzumutbar hohen Aufwand"*, and
Satz 4 only *"in Ausnahmefällen"* where **neither** the heat quantity **nor** the
volume can be measured.

These are *Zahlenwertgleichungen* — numerical-value equations, not dimensionally
consistent — so the constants carry no unit, and they bundle **different** things:

| Constant | Covers (§9 Abs. 2 Satz 3/5) |
|---|---|
| 2.5 | Erzeugeraufwandszahl, mittlere spezifische Wärmekapazität des Wassers, Wärmeverluste für Warmwasserspeicher, Verteilung einschließlich Zirkulation, Messdatenerhebungen |
| 32 | Nutzwärmebedarf für Warmwasser, Erzeugeraufwandszahl, Messdatenerhebungen — **no** Speicher-, Verteilungs- oder Zirkulationsverluste |

Because the Erzeugeraufwandszahl sits inside both constants, **Q is generator-input
heat, not delivered useful heat**.

`t_w` is *"die gemessene oder geschätzte mittlere Temperatur"* — an estimate is
permitted and no default or cap is prescribed. `A_Wohn` is the *"Wohn- oder
Nutzfläche"*, not living area alone.

### `WarmWaterAdjustments`

| Field | Effect | Trigger |
|---|---|---|
| `brennwert_erdgas` | × 1.11 | brennwertbezogene Abrechnung von Erdgas |
| `eigenstaendige_gewerbliche_waermelieferung` | ÷ 1.15 | **eigenständige** gewerbliche Wärmelieferung |
| `monovalente_waermepumpe` | × 0.30 | Betrieb einer monovalenten Wärmepumpe |

A struct of flags rather than an enum: §9 Abs. 2 Satz 6 applies these to the result
of *"den Zahlenwertgleichungen in Satz 2 oder 4"* and does not make them exclusive,
so a heat-pump system under eigenständige gewerbliche Wärmelieferung takes both.
`eigenständig` is a term of art (cf. §1 Abs. 1 Nr. 2); ordinary commercial heat
supply does not qualify.

A warm-water meter therefore carries **two quantities**: the m³ billed as water,
and the kWh this apportions out of the building's heating bill. The metered and
floor-area forms are separate functions rather than one `Option` parameter, because
they are different evidentiary categories.

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

## Virtual meters (§42b EnWG GGV — Solarpaket I)

Both GGV variants compute the tenant's **net grid draw after PV allocation**,
satisfying §42b Abs. 5 EnWG sentence 2: the allocated PV energy can never exceed
the tenant's actual consumption in any 15-minute interval. This is enforced by the
`Pos()` = `max(0, x)` operator per the BDEW "Anwendungshilfe Berechnungsformeln
Solarpaket 1" (v1.0, 25.01.2024).

### Formula overview

**Constant allocation** (BDEW Beispiel 1 — UTILTS CCI+ZG6):
```
net_grid_draw_i[t] = max(0, tenant_consumption_i[t] - fraction_i × plant_generation[t])
```

**Proportional allocation** (BDEW Beispiel 3 — variable ratio):
```
ratio_i[t]         = tenant_consumption_i[t] / Σ all_tenant_consumption_j[t]
                     (0 if denominator = 0 — zero-division protected)
net_grid_draw_i[t] = max(0, tenant_consumption_i[t] - ratio_i[t] × plant_generation[t])
```

### Constant allocation (Beispiel 1 — UTILTS CCI+ZG6)

```rust
// Tenant receives 10 % of plant generation; result = net grid draw
let rule = AggregationRule::GgvConstantAllocation {
    plant_melo_id: "MELO_PLANT".into(),
    tenant_melo_id: "MELO_T2".into(),
    fraction: dec!(0.10),
};
let net_grid_draw = compute_virtual_meter(&rule, &source_map)?;
// Each interval: max(0, tenant_consumption - 0.10 × plant_generation)
// Examples:
//   gen=10, consumption=5  → max(0, 5 - 1) = 4  (1 kWh covered by PV)
//   gen=10, consumption=0.5 → max(0, 0.5 - 1) = 0 (§42b cap: no negative draw)
```

### Proportional allocation (Beispiel 3 — variable ratio)

```rust
let rule = AggregationRule::GgvProportionalAllocation {
    plant_melo_id: "MELO_PLANT".into(),
    tenant_melo_id: "MELO_T2".into(),
    all_tenant_melo_ids: vec!["MELO_T2".into(), "MELO_T3".into()],
};
let net_grid_draw = compute_virtual_meter(&rule, &source_map)?;
// ratio = T2_consumption / (T2 + T3); net = max(0, T2 - ratio × generation)
// zero-division protected: if all consumptions are 0 → net = 0
```

### Energy balance check (plant feed-in)

The residual plant feed-in (grid export from PV) equals:
```
plant_feedin[t] = plant_generation[t] - Σ (tenant_consumption_i[t] - net_grid_draw_i[t])
               = plant_generation[t] - Σ pv_allocated_i[t]
```

Available rules: `Sum`, `Residual` (§42a EEG), `PvSelfConsumption`,
`GgvConstantAllocation`, `GgvProportionalAllocation` (§42b EnWG Solarpaket I).

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

## OBIS medium (value group A)

`ObisCode::is_heat`, `is_water` and `is_heat_cost_allocator` follow the
DLMS/COSEM Blue Book media list that OMS Spec Vol. 2 adopts:

| A | Medium | Predicate |
|---|---|---|
| 1 | Electricity | `is_electricity()` |
| 4 | Heizkostenverteiler | `is_heat_cost_allocator()` |
| 5 / 6 | Cooling / heat | `is_heat()` |
| 7 | Gas | `is_gas()` |
| 8 / 9 | Cold / hot water | `is_water()` |

**A = 8 is water, not heat.** An HCA (A = 4) reports dimensionless
*Verbrauchseinheiten* and carries no Eichfrist — HeizkostenV §5 Abs. 1 Satz 3
admits it as an apportionment device precisely because it measures no unit.

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

### Media-aware thresholds

```rust
QualityConfig::for_sparte(Sparte::Wasser)
```

| `Sparte` | `max_zero_run_allowed` | `min_sigma` |
|---|---|---|
| `Strom` | 2 | 0.0 |
| `Gas` | 48 | 0.01 |
| `Waerme` | 720 | 0.05 |
| `Wasser` | 720 | 0.001 |

`min_sigma` guards **MAD implosion**: across a flat window the median absolute
deviation is 0, so `t × sigma` is 0 and every nonzero value scores as an outlier.
`hampel_filter_with_floor` exposes the same guard as a primitive.

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

## Feature flags

| Flag | Effect |
|---|---|
| `serde` | Derive `Serialize`/`Deserialize` on all public types |

---

## Testing

```bash
cargo test -p metering --all-features
```

214 tests covering: gas conversion (DVGW G 685), aggregation (RLM/SLP/Gas), Messtyp
classification, imbalance arithmetic, V01–V10 validation engine (incl. DST transitions and
V07 DST ambiguity), §17 MessZV substitute methods, resampling (hourly/daily/monthly),
§42b EnWG GGV virtual meters (Beispiel 1 constant + Beispiel 3 proportional, Pos() cap,
zero-division guard), §42a Residuallast, BSI TR-03109 SMGW + CLS lifecycle,
measurement series provenance, register + ObisCode.

---

## Regulatory basis

- **§3, §4 MessZV** — SLP/RLM classification thresholds
- **§2 Nr. 17 MessZV** — Spitzenleistung definition for RLM
- **§17 MessZV** — Ersatzwertbildung + Jahresprognose (substitute values + annual forecast)
- **§22 MessZV** — 3-year provenance retention (`MeasurementSeries`, `ProvenanceEntry`)
- **§27 MessZV** — Mehr-/Mindermengensaldo
- **§25 Nr. 4 MessEV / DVGW G 685** — Gas Brennwertkorrektur
- **§41a EnWG** — 15-Minuten-Lastgang and iMSys Pflichteinbau
- **§42a/§42b EEG** — Residuallast / GGV community solar virtual meters
- **§14a EnWG** — Steuerbare Verbrauchseinrichtungen (CLS channels)
- **BSI TR-03109** — Smart Meter Gateway lifecycle and certificates
