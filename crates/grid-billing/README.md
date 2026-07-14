# grid-billing

> Role-neutral NNE/KA/MMM/MSB grid invoice calculation library for German energy market
> communication (BDEW MaKo).

[![Crates.io](https://img.shields.io/crates/v/grid-billing?label=grid-billing&color=f59e0b&logo=rust)](https://crates.io/crates/grid-billing)

## What this crate does

`grid-billing` computes BDEW INVOIC billing positions for:

- **NNE** (Netznutzungsentgelt) — flat-rate or §14a Modul 2 ToU (HT/NT split)
- **KA** (Konzessionsabgabe) — §17 StromNZV, included as separate position
- **MMM** (Mehr-/Mindermengensaldo) — actual vs. SLP profile deviation, credit when Mindermengen dominate
- **MSB-Rechnung** — metering service fee (NB → MSB, PID 31009)

All calculations are **pure functions** — zero I/O, zero async, no side effects.
All monetary arithmetic uses `EuroAmount` = `i64 × 10⁻⁵ EUR` — no `f64` anywhere in the billing path.

## Architecture: domain types, no BO4E dependency

`grid-billing` returns `GridInvoice` — a **pure domain type** with no dependency on `rubo4e`.
The service layer (`netzbilanzd`, `invoicd`) owns the `into_rechnung()` conversion to
`rubo4e::current::Rechnung` for EDIFACT serialization and `invoic-checker` validation.

```
grid-billing::calculate_nne_invoice(NneInput) → GridInvoice { positions, total_eur, pid, … }
                                                        │
                                                        ▼
                                          netzbilanzd::into_rechnung(&invoice) → Rechnung
                                                        │
                                                        ▼
                                          InvoicCheckEngine::check(pid, &rechnung, …)
                                                        │
                                                        ▼
                                          invoice_drafts (PostgreSQL) → AS4 dispatch
```

This separation makes `grid-billing` publishable to crates.io without pulling in the internal
`rubo4e` crate.

## Who uses this library

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
| 31011 | AWH Sperrprozesse Gas | GNB → LFG | `nne_gas_awh_31011` (PID override on `GridInvoice.pid`) |

## Output type: `GridInvoice`

```rust
pub struct GridInvoice {
    pub pid: u32,              // BDEW Prüfidentifikator (caller may override for 31005/31011)
    pub rechnungsnummer: String,
    pub invoice_date: time::Date,
    pub due_date: time::Date,
    pub period_from: time::Date,
    pub period_to: time::Date,
    pub nb_mp_id: String,      // Sender MP-ID (for invoic-checker tariff lookups)
    pub positions: Vec<InvoicePosition>,
    pub total_eur: Decimal,    // Net total, rounded to 2 decimal places
}

pub struct InvoicePosition {
    pub number: u32,
    pub text: String,
    pub quantity: Decimal,
    pub unit: QuantityUnit,    // Kwh | Kw | Monat
    pub unit_price_eur: Decimal,
    pub net_eur: Decimal,      // quantity × unit_price_eur, rounded to 5 dp
}
```

## Design invariants

| Invariant | Detail |
|---|---|
| **No floating-point money** | All amounts use `EuroAmount` (`i64 × 10⁻⁵ EUR`). No `f64` in billing path. |
| **No rubo4e dependency** | Returns `GridInvoice` domain types. Service layer owns `into_rechnung()`. |
| **PID mutable** | `GridInvoice.pid` is `pub` — caller sets `31005` for Gas, `31011` for AWH Gas. |
| **Decimal-only tariff input** | All tariff rates are `rust_decimal::Decimal` via `from_str_exact`. Never `Decimal::try_from(f64)`. |
| **Pure functions** | `calculate_nne_invoice`, `calculate_mmm_invoice`, `calculate_msb_invoice` are synchronous with no side effects. |

## Quick start

```toml
[dependencies]
grid-billing = { version = "0.9" }
rust_decimal = "1"
time         = "0.3"
```

### NNE flat-rate (SLP)

```rust
use grid_billing::{NneInput, calculate_nne_invoice};
use rust_decimal::Decimal;
use time::macros::date;

fn d(s: &str) -> Decimal { Decimal::from_str_exact(s).unwrap() }

let invoice = calculate_nne_invoice(&NneInput {
    malo_id: "51238696780".into(),
    nb_mp_id: "9900357000004".into(),
    lf_mp_id: "9900012345678".into(),
    rechnungsnummer: "NNE-2026-01-0001".into(),
    period_from: date!(2026-01-01),
    period_to:   date!(2026-01-31),
    invoice_date: date!(2026-02-15),
    due_date:    date!(2026-03-17),  // 30-day Zahlungsziel
    arbeitsmenge_kwh: d("1500"),
    arbeitspreis_ct_per_kwh: d("28.50"),
    ka_satz_ct_per_kwh: Some(d("0.11")),  // Konzessionsabgabe
    ..NneInput::default()
}).expect("valid NNE input");

// invoice.total_eur == "435.15" (1500 × 0.285 + 1500 × 0.0011)
assert_eq!(invoice.positions.len(), 2);  // Arbeit + KA
```

### §14a Modul 2 ToU (HT/NT split)

```rust
use grid_billing::{NneInput, calculate_nne_invoice};

let invoice = calculate_nne_invoice(&NneInput {
    // … identity fields …
    arbeitsmenge_kwh: d("0"),          // ignored when ToU is active
    arbeitspreis_ct_per_kwh: d("0"),   // ignored when ToU is active
    arbeitsmenge_ht_kwh: Some(d("900")),
    arbeitspreis_ht_ct_per_kwh: Some(d("32.00")),  // HT band
    arbeitsmenge_nt_kwh: Some(d("600")),
    arbeitspreis_nt_ct_per_kwh: Some(d("18.00")),  // NT reduced rate
    ..NneInput::default()
}).unwrap();

// Two separate Arbeit positions: "Netznutzung Arbeit HT (§14a Modul 2)" + NT
assert_eq!(invoice.positions.len(), 2);
```

### MMM settlement (Mehr-/Mindermengen)

```rust
use grid_billing::{MmmInput, calculate_mmm_invoice};

let mut invoice = calculate_mmm_invoice(&MmmInput {
    // … identity fields …
    actual_kwh: d("1600"),   // measured
    profil_kwh: d("1500"),   // SLP forecast
    mehr_preis_ct_per_kwh:   d("4.0"),
    minder_preis_ct_per_kwh: d("2.0"),
    ..MmmInput::default()
}).unwrap();

// Override PID for Gas MMM:
// invoice.pid = 31002 by default; set 31005 for Gas if needed via outer context
assert_eq!(invoice.total_eur, d("4.00"));  // 100 kWh Mehrmengen × 0.04 EUR/kWh

// Mindermengen: negative total = NB owes LF
// actual = 1400, profil = 1500 → total = -2.00 EUR (credit)
```

### MSB-Rechnung (PID 31009)

```rust
use grid_billing::{MsbInput, calculate_msb_invoice};

let invoice = calculate_msb_invoice(&MsbInput {
    // … identity fields …
    grundgebuehr_eur_per_month: d("9.50"),
    billing_months: 12,
    messdienstleistung_eur: Some(d("24.00")),  // optional flat fee
    ..MsbInput::default()
}).unwrap();

// 12 × 9.50 + 24.00 = 138.00 EUR
assert_eq!(invoice.total_eur, d("138.00"));
assert_eq!(invoice.pid, 31009);
```

### Service-layer conversion to BO4E Rechnung

The caller (netzbilanzd, invoicd) does the mapping — `grid-billing` stays BO4E-free:

```rust
// In netzbilanzd/src/billing.rs:
use grid_billing::{GridInvoice, QuantityUnit, calculate_nne_invoice};
use rubo4e::current::{Betrag, Menge, Mengeneinheit, Preis, Rechnungsposition, Rechnung, Zeitraum};

fn into_rechnung(invoice: &GridInvoice) -> Rechnung {
    let lz = Zeitraum { startdatum: Some(invoice.period_from), enddatum: Some(invoice.period_to), ..Default::default() };
    let positions = invoice.positions.iter().map(|p| {
        let einheit = match p.unit {
            QuantityUnit::Kwh => Some(Mengeneinheit::Kwh),
            QuantityUnit::Kw  => Some(Mengeneinheit::Kw),
            QuantityUnit::Monat => Some(Mengeneinheit::Monat),
        };
        Rechnungsposition {
            positionsnummer: Some(p.number as i64),
            positionstext: Some(p.text.clone()),
            lieferungszeitraum: Some(lz.clone()),
            positions_menge: Some(Menge { wert: Some(p.quantity), einheit, ..Default::default() }),
            einzelpreis: Some(Preis { wert: Some(p.unit_price_eur.round_dp(6)), ..Default::default() }),
            gesamtpreis: Some(Betrag { wert: Some(p.net_eur.round_dp(5)), ..Default::default() }),
            ..Default::default()
        }
    }).collect();
    Rechnung {
        rechnungsnummer: Some(invoice.rechnungsnummer.clone()),
        rechnungsdatum: Some(invoice.invoice_date),
        faelligkeitsdatum: Some(invoice.due_date),
        rechnungsperiode: Some(lz),
        gesamtnetto: Some(Betrag { wert: Some(invoice.total_eur), ..Default::default() }),
        rechnungspositionen: Some(positions),
        ..Default::default()
    }
}

// Then validate:
let rechnung = into_rechnung(&invoice);
let report = InvoicCheckEngine::check(invoice.pid, &invoice.nb_mp_id, &rechnung, &store, &config);
```

## Billing position table

| Position | Label | Unit | Condition |
|---|---|---|---|
| 1 | `Netznutzung Arbeit` | kWh | SLP / RLM flat-rate |
| 1 + 2 | `Netznutzung Arbeit HT/NT (§14a Modul 2)` | kWh each | When `arbeitsmenge_ht_kwh` + `arbeitspreis_ht_ct_per_kwh` are both set |
| next | `Netznutzung Leistung` | kW | When `spitzenleistung_kw` + `leistungspreis_eur_per_kw` are both set (RLM) |
| last | `Konzessionsabgabe` | kWh | When `ka_satz_ct_per_kwh` is set |

All amounts are rounded to 5 decimal places per position; total rounded to 2 decimal places.

## See also

- [`invoic-checker`](../invoic-checker/README.md) — validates the generated `Rechnung` in the service layer
- [`netzbilanzd`](../../services/netzbilanzd/README.md) — NB billing service that calls `grid-billing`
- [`invoicd`](../../services/invoicd/README.md) — LF service using `grid-billing` for selbstausstellen
