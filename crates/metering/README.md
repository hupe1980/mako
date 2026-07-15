# metering

**German energy metering domain library — interval types, gas conversion, billing period aggregation, SLP/RLM/iMSys classification, and Hampel-filter quality scoring.**

`metering` is the pure domain library used by [`edmd`](../../services/edmd/) for
all metering arithmetic. It has no I/O, no async, and no floating-point money.

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
