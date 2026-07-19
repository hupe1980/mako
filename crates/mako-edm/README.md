# mako-edm

**Energy Data Management library — MSCONS meter reads, SLP reconstruction, and Mehr-/Mindermengen imbalance.**

`mako-edm` defines the domain types and repository traits used by the
[`edmd`](../../services/edmd/) daemon. The library itself has no I/O; persistence
is implemented in `edmd` via PostgreSQL and Apache Iceberg.

---

## Core types

### `MeterInterval`

A single timestamped energy reading for one `MaLoId`:

```rust
pub struct MeterInterval {
    pub malo_id: MaloId,
    pub obis_code: ObisCode,
    pub period_start: OffsetDateTime,
    pub period_end: OffsetDateTime,
    pub value_kwh: Decimal,
    pub quality: QualityFlag,
    pub session_id: Option<String>,   // idempotency key for iMSys direct push
}
```

### `QualityFlag`

Mapped to the MSCONS/UTILTS Datenqualitätskennzeichen:

| Variant | BDEW code | Meaning |
|---|---|---|
| `Measured` | `67` | Gemessener Wert |
| `Substituted` | `Z43` | Ersatzwert |
| `Estimated` | `Z44` | Schätzwert |
| `Invalid` | `Z45` | Ungültiger Wert |

---

## Repository traits

### `TimeSeriesRepository`

```rust
#[async_trait]
pub trait TimeSeriesRepository: Send + Sync {
    async fn upsert(&self, interval: &MeterInterval) -> Result<(), EdmError>;
    async fn fetch(&self, malo_id: &MaloId, obis_code: &ObisCode,
                   from: OffsetDateTime, to: OffsetDateTime)
        -> Result<Vec<MeterInterval>, EdmError>;
    async fn fetch_billing_period(&self, malo_id: &MaloId,
                                  period_start: Date, period_end: Date)
        -> Result<MeterBillingPeriod, EdmError>;
}
```

The `obis_code` parameter scopes reads to a single OBIS code — required for
dual-tariff (HT/NT) meters and Gas (`1-1:111.8.0*255`).

### `MeterBillingPeriod`

Aggregated billing snapshot returned by `fetch_billing_period`:

```rust
pub struct MeterBillingPeriod {
    pub malo_id: MaloId,
    pub period_start: Date,
    pub period_end: Date,
    pub kwh_ht: Decimal,
    pub kwh_nt: Decimal,
    pub spitzenleistung_kw: Option<Decimal>,   // RLM §2 Nr. 17 MessZV
    pub brennwert_kwh_m3: Option<Decimal>,     // Gas §25 Nr. 4 MessEV
    pub zustandszahl: Option<Decimal>,          // Gas §25 Nr. 4 MessEV
}
```

`billingd` calls `fetch_billing_period` to retrieve the period aggregate; it never
reads raw intervals directly.

---

## Mehr-/Mindermengen imbalance

`compute_imbalance` computes the Mehr-/Mindermengensaldo per §27 MessZV:

```rust
pub fn compute_imbalance(
    forecast_kwh: Decimal,
    actual_kwh: Decimal,
) -> ImbalanceResult
```

`ImbalanceResult` contains `delta_kwh` and `delta_pct`.  A positive delta is a
Mehrmenge (actual > forecast); negative is a Mindermenge.

---

## Testing feature

Enable `testing` to use in-memory implementations:

```toml
[dev-dependencies]
mako-edm = { path = "../crates/mako-edm", features = ["testing"] }
```

```rust
use mako_edm::testing::InMemoryTimeSeriesRepository;
```

Never enable `testing` in production builds.

---

## Regulatory basis

- **§22 MessZV** — Pflicht zur Aufbewahrung von Zählerstandsgängen
- **§27 MessZV** — Mehr-/Mindermengensaldo-Berechnung
- **§41a EnWG** — 15-Minuten-Lastgang mandatory for iMSys customers (since 2025)
- **§25 Nr. 4 MessEV / DVGW G 685** — Brennwertkorrektur (m³ → kWh_Hs)
- **MSCONS AHB** — Meter reading message format (EDI@Energy)
