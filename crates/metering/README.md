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
classification, imbalance arithmetic, and Hampel filter edge cases.

---

## Regulatory basis

- **§3, §4 MessZV** — SLP/RLM classification thresholds
- **§2 Nr. 17 MessZV** — Spitzenleistung definition for RLM
- **§27 MessZV** — Mehr-/Mindermengensaldo
- **§24 GasGVV / DVGW G 685** — Gas Brennwertkorrektur
- **§41a EnWG** — 15-Minuten-Lastgang and iMSys Pflichteinbau
