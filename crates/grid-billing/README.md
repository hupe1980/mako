# grid-billing

[![Crates.io](https://img.shields.io/crates/v/grid-billing?label=grid-billing&color=f59e0b&logo=rust)](https://crates.io/crates/grid-billing)

Deterministic German grid settlement for the network and metering side:
Netznutzungsentgelt with Konzessionsabgabe and netzseitige Umlagen,
Mehr-/Mindermengen, Messstellenbetrieb and the GeLi Gas Sperrprozess fees.
Every function is pure and synchronous, money is `Decimal` range-checked as
fixed-point EUR, and every position carries a `CalculationTrace` with the
`LegalReference`s it applied. A settlement states what is owed and why; invoice
number, dates and Prüfidentifikator belong to the `InvoiceDocument` around it.

## Example

```rust,no_run
use grid_billing::{NneInput, Sparte, SettlementType, settle_nne};
use grid_billing::types::{
    ArbeitspreisModell, Grenzpreisvergleich, MengePreis, Konzessionsabgabe, KaKundengruppe,
    SettlementPeriod,
};
use grid_billing::umlagen::Letztverbrauchergruppe;
use rust_decimal::Decimal;
use time::macros::date;

fn d(s: &str) -> Decimal { Decimal::from_str_exact(s).unwrap() }

let settlement = settle_nne(&NneInput {
    malo_id: "51238696012".into(),
    nb_mp_id: "9900357000004".into(),
    lf_mp_id: "9900012345678".into(),
    period: SettlementPeriod::new(date!(2026-01-01), date!(2026-01-31)).unwrap(),
    letztverbrauchergruppe: Letztverbrauchergruppe::A,
    enfg_jahresvorverbrauch_kwh: None,
    // Exactly one Arbeitspreis form — here a single flat rate.
    arbeitspreis: ArbeitspreisModell::Einheitlich(MengePreis {
        menge_kwh: d("1500"),
        preis_ct_per_kwh: d("3.5"),
    }),
    leistungspreis: None,
    grundpreis: None,
    konzessionsabgabe: Some(Konzessionsabgabe {
        satz_ct_per_kwh: d("0.11"),
        klasse: KaKundengruppe::Sondervertragskunde,
        niederspannung: None,
        // § 2 Abs. 4 KAV Grenzpreisvergleich — required for a Sondervertragskunde
        grenzpreis: Some(Grenzpreisvergleich {
            durchschnittspreis_ct_per_kwh: d("24.80"),
            grenzpreis_ct_per_kwh: d("18.20"),
        }),
    }),
    sparte: Sparte::Strom,
    tariff_sheet_id: Some("Preisblatt-NNE-2026-Q1".into()),
    netzebene: None,
    jahreshoechstleistung_kw: None,
    jahresarbeit_kwh: None,
    sect19: None,
    blindarbeit: None,
    gas_kapazitaet: None,
    // None: the tabled Umlage rate for the delivery year and group
    sect19_umlage_ct_per_kwh: None,
    offshore_umlage_ct_per_kwh: None,
    kwkg_umlage_ct_per_kwh: None,
}).expect("valid NNE input");

assert_eq!(settlement.settlement_type, SettlementType::NneStrom);
assert_eq!(settlement.settlement_type.default_pid(), 31002);
assert_eq!(settlement.recipient_mp_id, "9900012345678");

for pos in &settlement.positions {
    println!("{}: {}", pos.text, pos.trace.explanation);
    for lr in &pos.trace.legal_refs {
        println!("  → {}", lr.citation());
    }
}
```

## What it covers

| Item | Purpose |
|---|---|
| `settle_nne`, `settle_mmm`, `settle_msb`, `settle_gas_awh`, `settle_abschlag` | one engine per settlement type → `SettlementResult` |
| `reverse`, `correct` | Storno and the (reversal, replacement) pair; the original is never mutated |
| `ArbeitspreisModell` | exactly one Arbeitspreis form: flat, § 14a EnWG Modul 1, 2 or 3, or spot-linked |
| `RegulatoryRegime` | the rules in force for the delivery period, resolved once; a period crossing a turnover warns `REGIME_TURNOVER_IN_PERIOD` |
| `umlagen` | § 19 StromNEV-, Offshore- and KWKG-Umlage per year; an untabled year is refused (`UmlageRateMissing`) |
| `msbg`, `sect19`, `netzebene`, `gas` | § 30 MsbG Preisobergrenzen, § 19 Abs. 2 StromNEV individual charges, Netzebene, Gas capacity |
| `redispatch` | § 13a Abs. 2 EnWG Redispatch compensation, on a required `AusfallarbeitBasis` |

`SettlementType::default_pid()` maps a settlement to its INVOIC
Prüfidentifikator:

| PID | Settlement | Direction |
|---|---|---|
| 31001 | Abschlagsrechnung Netznutzung | NB → LF |
| 31002 | NN-Rechnung, Strom and Gas | NB → LF |
| 31005 | MMM-Rechnung, Strom and Gas | NB → LF |
| 31006 | MMM Mehrmenge, selbst ausgestellt | NB → LF |
| 31009 | MSB-Rechnung | MSB → NB / LF / ESA |
| 31011 | AWH Sperrprozesse Gas | NB → LF |

## Features

| Feature | Enables |
|---|---|
| `bo4e` | `bo4e::into_rechnung(&InvoiceDocument)`, the invoice as a BO4E `Rechnung` |

## Sources

- StromNEV §§ 17, 19; GasNEV § 15; KAV § 2; MsbG § 30; EnFG; KWKG
- EnWG §§ 14a, 13a, 20 Abs. 3; BK6-24-174; BK7-24-01-009

Operator reference: [netzbilanzd](https://hupe1980.github.io/mako/docs/services/netzbilanzd/) ·
API: <https://docs.rs/grid-billing>

License: MIT OR Apache-2.0
