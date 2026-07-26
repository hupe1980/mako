# mako-edm

**Energy Data Management library — MSCONS meter reads, SLP reconstruction, and Mehr-/Mindermengen imbalance.**

`mako-edm` defines the domain types and repository traits used by the
[`edmd`](../../services/edmd/) daemon. The library itself has no I/O; persistence
is implemented in `edmd` via PostgreSQL and Apache Iceberg.

---

## Core types

### `MeterRead`

A single metered interval sourced from an MSCONS message (or a direct-push /
API-import path). Keyed by `malo_id`:

```rust
pub struct MeterRead {
    pub malo_id: String,               // 11-digit Marktlokations-ID
    pub melo_id: Option<String>,       // 33-char Messlokations-ID, if available
    pub dtm_from: OffsetDateTime,      // interval start (UTC)
    pub dtm_to: OffsetDateTime,        // interval end (UTC)
    pub quantity_kwh: Decimal,
    pub quality: QualityFlag,
    pub pid: u32,                      // source PID (e.g. 13005)
    pub sparte: Sparte,
    pub obis_code: Option<String>,     // None when the source had no PIA segment
    pub tenant: String,                // data-isolation key
    pub source: IngestionSource,       // provenance (§ 60 Abs. 6 MsbG)
    pub push_session: Option<String>,  // idempotency key for direct push
    pub quality_warnings: Option<serde_json::Value>,
    pub sender_mp_id: Option<String>,  // MP-ID of the delivering MSB/system
    pub allocation_version: String,    // MaBiS AllocationVersion ("INITIAL"/…)
    pub valid_from_tx: Option<OffsetDateTime>, // row write version (tx time)
}
```

### `QualityFlag`

`QualityFlag` is re-exported from the `metering` crate
(`pub use metering::{QualityFlag, Sparte}`), which is the single source of truth.
It has eight variants, mapping to the MSCONS `MESSWERTSTATUS` / BO4E
`Messwertstatus`:

| Variant | Billable | Meaning |
|---|---|---|
| `Measured` | ✓ | Gemessener Wert / Abgelesen |
| `Estimated` | ✓ | Prognosewert — valid for advance billing (§ 60 Abs. 2 MsbG) |
| `Substituted` | ✓ | Ersatzwert (MSB replacement when measurement failed) |
| `Calculated` | ✓ | Rechenwert — derived from other measurements |
| `Corrected` | ✓ | Nachbearbeitungswert — corrected earlier value |
| `Preliminary` | ✓ | Vorläufiger Wert — may be revised later |
| `Faulty` | ✗ | Fehlerhaft — must not be billed |
| `Unknown` | ✗ | Quality not determinable — do not bill (default) |

`QualityFlag::is_billable()` returns `true` for the first six variants.

---

## Repository traits

### `TimeSeriesRepository`

```rust
pub trait TimeSeriesRepository: Send + Sync + 'static {
    async fn store_receipt(&self, receipt: &MeterDataReceipt) -> Result<(), EdmError>;
    async fn store_reads(&self, reads: &[MeterRead]) -> Result<(), EdmError>;
    async fn query(&self, q: &TimeSeriesQuery) -> Result<Vec<MeterRead>, EdmError>;
    async fn receipts(&self, malo_id: &str,
                      from: OffsetDateTime, to: OffsetDateTime,
                      tenant: &str) -> Result<Vec<MeterDataReceipt>, EdmError>;
    async fn imbalance(&self, malo_id: &str, from: Date, to: Date,
                       tenant: &str) -> Result<ImbalanceReport, EdmError>;
    async fn latest_read(&self, malo_id: &str, tenant: &str)
        -> Result<Option<MeterRead>, EdmError>;
    async fn billing_period(&self, q: &BillingPeriodQuery)
        -> Result<Option<MeterBillingPeriod>, EdmError>;
    async fn update_gas_quality(&self, tenant: &str, malo_id: &str,
                                brennwert_kwh_per_m3: Option<Decimal>,
                                zustandszahl: Option<Decimal>) -> Result<u64, EdmError>;
    async fn store_corrections(&self, records: &[CorrectionRecord])
        -> Result<Vec<Uuid>, EdmError>;
}
```

`store_reads` is idempotent: duplicate `(malo_id, dtm_from, dtm_to)` rows are
overwritten with the latest `quality` and `quantity_kwh`. `tenant` is mandatory
on every read path (`query` takes it via `TimeSeriesQuery`; the others take it
directly) — the SQL layer rejects an empty tenant.

A second trait, `Typ2Repository` (`store_typ2_reads` / `query_typ2`), stores
ESA "Werte nach Typ 2" (MSCONS PID 13027) in a separate `esa_typ2_reads` table.
Typ-2 data is non-authoritative and shares no method with the billing store.

### `MeterBillingPeriod`

Aggregated billing snapshot returned by `billing_period`:

```rust
pub struct MeterBillingPeriod {
    pub malo_id: String,
    pub period_from: Date,
    pub period_to: Date,
    pub messtyp: Messtyp,                        // SLP / RLM / iMSys
    pub sparte: Sparte,
    pub arbeitsmenge_kwh: Decimal,               // HT + NT combined
    pub arbeitsmenge_ht_kwh: Option<Decimal>,    // dual-tariff HT
    pub arbeitsmenge_nt_kwh: Option<Decimal>,    // dual-tariff NT
    pub spitzenleistung_kw: Option<Decimal>,     // RLM Strom — Leistungspreisanteil
    pub brennwert_kwh_per_m3: Option<Decimal>,   // Gas §25 Nr. 4 MessEV
    pub zustandszahl: Option<Decimal>,           // Gas §25 Nr. 4 MessEV
    pub zaehlerstand_anfang: Option<Decimal>,    // meter start reading
    pub zaehlerstand_ende: Option<Decimal>,      // meter end reading
    pub quality: QualityFlag,                    // worst flag in the period
    pub lastprofil: Option<String>,              // SLP designation (H0, G0–G6, …)
    pub profil_typ: Option<String>,              // BO4E ProfilTyp
}
```

`invoicd` and `netzbilanzd` call `billing_period` to retrieve the period
aggregate; the raw 15-min Lastgang is fetched separately via `query`, never
inlined here.

---

## Mehr-/Mindermengen imbalance

Imbalance is a repository method, not a free function. `imbalance` computes the
Mehr-/Mindermengensaldo per § 13 StromNZV for one MaLo over one billing period:

```rust
async fn imbalance(&self, malo_id: &str, from: Date, to: Date,
                   tenant: &str) -> Result<ImbalanceReport, EdmError>
```

`ImbalanceReport` compares LF-expected against NB-reported quantities:

```rust
pub struct ImbalanceReport {
    pub malo_id: String,
    pub period_from: Date,
    pub period_to: Date,
    pub lf_quantity_kwh: Decimal,   // total LF quantity in period
    pub nb_quantity_kwh: Decimal,   // total NB reported quantity in period
    pub delta_kwh: Decimal,         // lf − nb
    pub delta_pct: Decimal,         // delta as % of nb (zero when nb is zero)
    pub quality: QualityFlag,       // worst flag across the period
}
```

A positive `delta_kwh` is a Mehrmenge (LF > NB); negative is a Mindermenge.

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

- **§ 60 Abs. 6 MsbG** — Pflicht zur Aufbewahrung von Zählerstandsgängen
- **§ 13 StromNZV** — Mehr-/Mindermengensaldo-Berechnung
- **§41a EnWG** — 15-Minuten-Lastgang mandatory for iMSys customers (since 2025)
- **§25 Nr. 4 MessEV / DVGW G 685** — Brennwertkorrektur (m³ → kWh_Hs)
- **MSCONS AHB** — Meter reading message format (EDI@Energy)
