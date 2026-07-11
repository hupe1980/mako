# mako-nne

> Role-neutral NNE/KA/MMM invoice calculation library for German energy market
> communication (BDEW MaKo).

[![Crates.io](https://img.shields.io/crates/v/mako-nne?label=mako-nne&color=f59e0b&logo=rust)](https://crates.io/crates/mako-nne)

## What this crate does

`mako-nne` computes BDEW INVOIC billing positions for Netznutzungsentgelt (NNE),
Konzessionsabgabe (KA), and Mehr-/Mindermengen (MMM) invoices from meter readings
and tariff data. It is a **pure, zero-I/O library** — no async, no network
calls, no database access. All calculations are deterministic and self-validating.

### Who uses this library

| Consumer | Role | Use case |
|---|---|---|
| `netzbilanzd` | **NB** | Generate INVOIC 31001/31002/31005 to send to LF |
| `invoicd` | **LF** | Selbstausstellen PID 31006 (§20 MessZV) — LF independently computes the same NNE invoice the NB would have sent |

This dual-role usage is intentional: under §20 MessZV *selbstausgestellt* invoicing,
the LF runs the identical formula the NB would use. The calculation is symmetric —
only who initiates differs.

## Generated invoice types

| PID | Description | Direction |
|---|---|---|
| 31001 | MMM-Rechnung NNE Strom | NB → LF (via `netzbilanzd`) |
| 31002 | MMM-Stornorechnung NNE Strom | NB → LF (via `netzbilanzd`) |
| 31005 | MMM-Rechnung NNE Gas | NB → LF (via `netzbilanzd`) |
| 31006 | MMM-selbst ausgestellte Rechnung Strom | LF → NB (via `invoicd`) |

## Design invariants

| Invariant | Detail |
|---|---|
| **No floating-point money** | All amounts use `EuroAmount` (`i64 × 10⁻⁵ EUR`). No `f64` arithmetic anywhere in the billing path. |
| **Self-validating** | Generated invoices satisfy `invoic-checker` checks 1–3 (period validity, position arithmetic, document total) by construction. |
| **Decimal tariff input** | All tariff rates are `rust_decimal::Decimal` via `Decimal::from_str_exact`. Never `Decimal::try_from(f64)`. |
| **Pure functions** | `calculate_nne_invoice`, `calculate_mmm_invoice`, `calculate_msb_invoice` are synchronous, have no side effects, and are trivially testable in isolation. |

## Quick start

```toml
[dependencies]
mako-nne = { version = "0.8" }
rust_decimal = "1"
time = "0.3"
```

```rust
use mako_nne::{NneInput, calculate_nne_invoice};
use rust_decimal::Decimal;
use time::macros::date;

fn d(s: &str) -> Decimal { Decimal::from_str_exact(s).unwrap() }

let result = calculate_nne_invoice(&NneInput {
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
