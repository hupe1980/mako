# mako-nne

> Role-neutral NNE/KA/MMM/MSB invoice calculation library for German energy market
> communication (BDEW MaKo).

[![Crates.io](https://img.shields.io/crates/v/mako-nne?label=mako-nne&color=f59e0b&logo=rust)](https://crates.io/crates/mako-nne)

## What this crate does

`mako-nne` computes BDEW INVOIC billing positions for:
- **NNE** (Netznutzungsentgelt) — flat-rate or §14a Modul 2 ToU (HT/NT split)
- **KA** (Konzessionsabgabe) — §17 StromNZV, included as separate position
- **MMM** (Mehr-/Mindermengensaldo) — actual vs. SLP profile deviation
- **MSB-Rechnung** — metering service fee (NB → MSB)

All calculations are **pure functions** — zero I/O, zero async, no side effects.
All monetary arithmetic uses `EuroAmount` = `i64 × 10⁻⁵ EUR` — no `f64` anywhere in the billing path.

### Who uses this library

| Consumer | Role | Use case |
|---|---|---|
| `netzbilanzd` | **NB** | Generate INVOIC 31001/31002/31005/31009/31011 to send to LF/MSB/LFG |
| `invoicd` | **LF** | §20 MessZV selbstausstellen PID 31006 — LF runs the same formula the NB would use |

This dual-role usage is intentional: under §20 MessZV *selbstausgestellt* invoicing,
the LF runs the identical formula independently. The calculation is symmetric.

## Generated invoice types

| PID | Description | Direction | `billing_type` in netzbilanzd |
|---|---|---|---|
| 31001 | NNE Strom | NB → LF | `nne_strom` |
| 31002 | MMM Strom / Gas | NB → LF · GNB → LFG | `mmm_strom`, `mmm_gas` |
| 31005 | NNE Gas | GNB → LFG | `nne_gas` |
| 31006 | Selbst ausgestellte NNE | LF → LF internal | `invoicd` only |
| 31009 | MSB-Rechnung | NB → MSB | `msb_31009` |
| 31011 | AWH Sperrprozesse Gas | GNB → LFG | `nne_gas_awh_31011` (PID override) |

## Design invariants

| Invariant | Detail |
|---|---|
| **No floating-point money** | All amounts use `EuroAmount` (`i64 × 10⁻⁵ EUR`). No `f64` in billing path. |
| **Self-validating** | Generated invoices pass `invoic-checker` checks 1–3 by construction. |
| **Decimal-only tariff input** | All tariff rates are `rust_decimal::Decimal` via `from_str_exact`. Never `Decimal::try_from(f64)`. |
| **Pure functions** | `calculate_nne_invoice`, `calculate_mmm_invoice`, `calculate_msb_invoice` are synchronous with no side effects. |

## Quick start

```toml
[dependencies]
mako-nne     = { version = "0.9" }
rust_decimal = "1"
time         = "0.3"
```

### NNE flat-rate (SLP)

```rust
use mako_nne::{NneInput, calculate_nne_invoice};
use rust_decimal::Decimal;
use time::macros::date;

fn d(s: &str) -> Decimal { Decimal::from_str_exact(s).unwrap() }

let result = calculate_nne_invoice(&NneInput {
    malo_id:                 "51238696780".into(),
    nb_mp_id:                "9900357000004".into(),
    lf_mp_id:                "9900012345678".into(),
    rechnungsnummer:         "NNE-2026-001".into(),
    period_from:             date!(2026-01-01),
    period_to:               date!(2026-01-31),
    invoice_date:            date!(2026-02-01),
    due_date:                date!(2026-03-03),
    arbeitsmenge_kwh:        d("1500"),
    arbeitspreis_ct_per_kwh: d("3.50"),
    arbeitsmenge_ht_kwh:     None,       // set for §14a Modul 2 ToU
    arbeitspreis_ht_ct_per_kwh: None,
    arbeitsmenge_nt_kwh:     None,
    arbeitspreis_nt_ct_per_kwh: None,
    spitzenleistung_kw:      None,       // Some(d("12.5")) for RLM customers
    leistungspreis_eur_per_kw: None,
    ka_satz_ct_per_kwh:      Some(d("1.32")),  // §17 StromNZV residential rate
}).expect("valid billing input");

assert_eq!(result.pid, 31001);
// 1500 × 3.50 ct + 1500 × 1.32 ct = 52.50 + 19.80 = 72.30 EUR
```

### §14a Modul 2 — Time-of-Use NNE (mandatory for controllable loads since 01.01.2024)

Provide `arbeitsmenge_ht_kwh` + `arbeitsmenge_nt_kwh` to generate separate HT and NT positions:

```rust
let result = calculate_nne_invoice(&NneInput {
    // … common fields …
    arbeitsmenge_kwh:            d("1000"),  // total for backward compat
    arbeitspreis_ct_per_kwh:     d("3.50"),  // ignored when HT/NT set
    arbeitsmenge_ht_kwh:         Some(d("600")),
    arbeitspreis_ht_ct_per_kwh:  Some(d("4.20")),  // Hochlast band
    arbeitsmenge_nt_kwh:         Some(d("400")),
    arbeitspreis_nt_ct_per_kwh:  Some(d("1.50")),  // Niedertarif band
    // … other fields …
}).expect("§14a ToU input");
// Positions: Arbeit HT + Arbeit NT + KA (3 positions)
// 600 × 4.20ct + 400 × 1.50ct = 25.20 + 6.00 = 31.20 EUR
```

### MMM Strom

```rust
use mako_nne::{MmmInput, calculate_mmm_invoice};

let result = calculate_mmm_invoice(&MmmInput {
    malo_id:                 "51238696780".into(),
    nb_mp_id:                "9900357000004".into(),
    lf_mp_id:                "9900012345678".into(),
    rechnungsnummer:         "MMM-2026-001".into(),
    period_from:             date!(2026-01-01),
    period_to:               date!(2026-01-31),
    invoice_date:            date!(2026-02-05),
    due_date:                date!(2026-03-07),
    actual_kwh:              d("1100"),  // metered consumption
    profil_kwh:              d("1000"),  // SLP forecast
    mehr_preis_ct_per_kwh:   d("3.00"),
    minder_preis_ct_per_kwh: d("2.50"),
}).expect("valid MMM input");

assert_eq!(result.pid, 31002);
// Mehrmenge: 100 kWh × 3.00 ct = 3.00 EUR
```

### MSB-Rechnung

```rust
use mako_nne::{MsbInput, calculate_msb_invoice};

let result = calculate_msb_invoice(&MsbInput {
    malo_id:                    "51238696780".into(),
    nb_mp_id:                   "9900357000004".into(),
    msb_mp_id:                  "4012345000023".into(),
    rechnungsnummer:            "MSB-2026-001".into(),
    period_from:                date!(2026-01-01),
    period_to:                  date!(2026-12-31),
    invoice_date:               date!(2026-01-15),
    due_date:                   date!(2026-02-15),
    grundgebuehr_eur_per_month: d("9.50"),
    billing_months:             12,
    messdienstleistung_eur:     Some(d("24.00")),
}).expect("valid MSB input");

assert_eq!(result.pid, 31009);
// 12 × 9.50 EUR + 24.00 EUR = 138.00 EUR
```

## Billing positions

### NNE flat-rate

| # | Position | Formula | Condition |
|---|---|---|---|
| 1 | Netznutzung Arbeit | `kwh × ct/kWh ÷ 100` | Always |
| 2 | Netznutzung Leistung | `kW × EUR/kW` | RLM only |
| 3 | Konzessionsabgabe | `kwh × ka_ct/kWh ÷ 100` | When `ka_satz_ct_per_kwh` set |

### §14a Modul 2 ToU

| # | Position | Formula | Condition |
|---|---|---|---|
| 1 | Netznutzung Arbeit HT | `ht_kwh × ht_ct/kWh ÷ 100` | HT/NT supplied |
| 2 | Netznutzung Arbeit NT | `nt_kwh × nt_ct/kWh ÷ 100` | HT/NT supplied |
| 3 | Netznutzung Leistung | `kW × EUR/kW` | RLM only |
| 4 | Konzessionsabgabe | `(ht+nt) × ka_ct ÷ 100` | When KA set |

### MMM

| # | Position | Formula | Condition |
|---|---|---|---|
| 1 | Mehrmengen | `max(0, actual−profil) × mehr_ct ÷ 100` | actual > profil |
| 2 | Mindermengen (Gutschrift) | `−max(0, profil−actual) × minder_ct ÷ 100` | profil > actual |

### MSB

| # | Position | Formula | Condition |
|---|---|---|---|
| 1 | Grundgebühr Messstellenbetrieb | `EUR/month × months` | Always |
| 2 | Messdienstleistung | flat amount | When set |

## EuroAmount precision

All arithmetic is performed in `EuroAmount` = `i64 × 10⁻⁵ EUR` (0.00001 EUR resolution).
Conversion to BO4E `Betrag.wert` uses `Decimal::round_dp(5)`.
This prevents floating-point residue errors (`0.10000000000000001`) that appear when
multiplying `ct/kWh × kWh` using `f64`.

## Regulatory basis

| Regulation | Handled |
|---|---|
| GPKE BK6-22-024 | NNE Strom billing (PID 31001) |
| §40 StromNZV | MMM Strom settlement (PID 31002) |
| §17 StromNZV | KA as separate Rechnungsposition |
| §14a EnWG (BK6-22-300) | HT/NT ToU split (mandatory for controllable loads since 01.01.2024) |
| WiM BK6-24-174 | MSB-Rechnung (PID 31009) |
| GeLi Gas BK7-24-01-009 §5.4 | AWH Sperrprozesse Gas (PID 31011, via `nne_gas` + PID override) |

## License

Licensed under either of [MIT](../../LICENSE-MIT) or [Apache 2.0](../../LICENSE-APACHE) at your option.

    malo_id:                 "51238696780".into(),
    nb_mp_id:                "9900357000004".into(),
    lf_mp_id:                "9900012345678".into(),
    rechnungsnummer:         "NNE-2025-001".into(),
    period_from:             date!(2025-01-01),
    period_to:               date!(2025-01-31),
    invoice_date:            date!(2025-02-15),
    due_date:                date!(2025-03-15),
    arbeitsmenge_kwh:        d("1500"),
    arbeitspreis_ct_per_kwh: d("3.50"),  // ct/kWh — string, never f64
    spitzenleistung_kw:      None,       // Some(d("12.5")) for RLM customers
    leistungspreis_eur_per_kw: None,
    ka_satz_ct_per_kwh:      Some(d("0.11")),
}).expect("valid billing input");

println!("Total: {:?}", result.total_eur);
println!("Positions: {}", result.rechnung.rechnungspositionen
    .as_ref().map_or(0, |v| v.len()));
```

## Billing positions

`calculate_nne_invoice` generates up to three positions:

| Pos | Description | Unit | Condition |
|---|---|---|---|
| 1 | Netznutzung Arbeit | ct/kWh × kWh | Always |
| 2 | Netznutzung Leistung | EUR/kW × kW | RLM only (`spitzenleistung_kw` set) |
| 3 | Konzessionsabgabe | ct/kWh × kWh | When `ka_satz_ct_per_kwh` is set |

## EuroAmount precision

All intermediate and final amounts are computed in `EuroAmount` = `i64 × 10⁻⁵ EUR`
(0.00001 EUR resolution). Conversion to BO4E `Betrag.wert` uses `Decimal::round_dp(5)`.
This avoids floating-point rounding errors that would otherwise appear at the 6th
decimal place when multiplying `ct/kWh × kWh` tariff amounts.

## License

Licensed under either of [MIT](../../LICENSE-MIT) or [Apache 2.0](../../LICENSE-APACHE) at your option.
