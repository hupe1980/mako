# grid-billing

[![Crates.io](https://img.shields.io/crates/v/grid-billing?label=grid-billing&color=f59e0b&logo=rust)](https://crates.io/crates/grid-billing)

Deterministic German grid settlement: Netznutzungsentgelt and
Konzessionsabgabe, Mehr-/Mindermengen, Messstellenbetrieb and the GeLi Gas
Sperrprozess fees. Every function is pure and synchronous, money is `Decimal`,
and every position carries a `CalculationTrace` with the `LegalReference`s it
applied — so a settlement can be recomputed and explained without the invoice
around it.

```bash
cargo add grid-billing rust_decimal time
```

## Example

```rust,no_run
use grid_billing::{NneInput, Sparte, SettlementType, settle_nne};
use grid_billing::types::{
    ArbeitspreisModell, MengePreis, Konzessionsabgabe, KaKundengruppe, SettlementPeriod,
};
use grid_billing::umlagen::Letztverbrauchergruppe;
use rust_decimal::Decimal;
use time::macros::date;

fn d(s: &str) -> Decimal { Decimal::from_str_exact(s).unwrap() }

let settlement = settle_nne(&NneInput {
    malo_id: "51238696012".into(),
    nb_mp_id: "9900357000004".into(),
    lf_mp_id: "9900012345678".into(),
    // The delivery period is a validated pair — inverted bounds are unrepresentable.
    period: SettlementPeriod::new(date!(2026-01-01), date!(2026-01-31)).unwrap(),
    // Letztverbrauchergruppe drives the network-levy rates (EnFG §§21 ff.).
    letztverbrauchergruppe: Letztverbrauchergruppe::A,
    // A′ has no EnFG 1-GWh boundary, so the year to date does not place it.
    enfg_jahresvorverbrauch_kwh: None,
    // Exactly one Arbeitspreis form — here a single flat rate.
    arbeitspreis: ArbeitspreisModell::Einheitlich(MengePreis {
        menge_kwh: d("1500"),
        preis_ct_per_kwh: d("3.5"),
    }),
    leistungspreis: None,   // SLP — no RLM demand charge
    grundpreis: None,       // Strom has no separate Grundpreis
    konzessionsabgabe: Some(Konzessionsabgabe {
        satz_ct_per_kwh: d("0.11"),
        klasse: KaKundengruppe::Sondervertragskunde,
        niederspannung: None,   // § 2 Abs. 7 KAV facts, when the class must be checked
        grenzpreis: None,       // § 2 Abs. 4 KAV Grenzpreisvergleich
    }),
    sparte: Sparte::Strom,
    tariff_sheet_id: Some("Preisblatt-NNE-2026-Q1".into()),
    netzebene: None,
    jahreshoechstleistung_kw: None,
    jahresarbeit_kwh: None,
    sect19: None,            // no §19 Abs. 2 individual charge
    blindarbeit: None,       // no reactive-energy charge
    gas_kapazitaet: None,
    sect19_umlage_ct_per_kwh: None,   // use the tabled rate for the delivery year/group
    offshore_umlage_ct_per_kwh: None,
    kwkg_umlage_ct_per_kwh: None,
}).expect("valid NNE input");

// The settlement carries what was settled, not a PID — invoice number, dates and
// the Prüfidentifikator are properties of InvoiceDocument. SettlementType maps to
// the standard PID:
assert_eq!(settlement.settlement_type, SettlementType::NneStrom);
assert_eq!(settlement.settlement_type.default_pid(), 31002);
// recipient_mp_id is auto-populated from lf_mp_id:
assert_eq!(settlement.recipient_mp_id, "9900012345678");

// A Strom NNE settlement also carries the three netzseitige Umlagen (§19 StromNEV,
// Offshore, KWKG) alongside the Arbeit and Konzessionsabgabe positions.
for pos in &settlement.positions {
    println!("{}: {}", pos.text, pos.trace.explanation);
    for lr in &pos.trace.legal_refs {
        println!("  → {}", lr.citation());
    }
}
```

## Key types

| Item | Purpose |
|---|---|
| `settle_nne`, `settle_mmm`, `settle_msb`, `settle_gas_awh`, `settle_abschlag` | one engine per settlement type → `SettlementResult` |
| `reverse`, `correct` | Storno and the (reversal, replacement) pair; the original is never mutated |
| `SettlementResult` / `InvoiceDocument` | what is owed and why / the document facts (number, dates, PID, cadence) |
| `ArbeitspreisModell` | exactly one Arbeitspreis form — flat, § 14a Modul 1/2/3, or spot-linked |
| `RegulatoryRegime` | which rules governed the delivery period, resolved once; a period crossing a turnover raises `REGIME_TURNOVER_IN_PERIOD` |
| `msbg`, `umlagen`, `sect19`, `netzebene`, `gas` | MsbG § 30 ceilings, EnFG levies, § 19 StromNEV, Netzebene, Gas capacity |
| feature `bo4e` | `bo4e::into_rechnung()` — the settlement as a BO4E `Rechnung` |

| PID | Description | Direction | Sparte |
|---|---|---|---|
| 31001 | Abschlagsrechnung Netznutzung | NB → LF | both |
| 31002 | NN-Rechnung Strom (Netznutzung) | NB → LF | Strom |
| 31002 | NN-Rechnung Gas (Netznutzung) | GNB → LFG | Gas (auto via `Sparte::Gas`) |
| 31005 | MMM-Rechnung (Mehr-/Mindermengensaldo) | NB → LF | both |
| 31006 | MMM Mehrmenge, selbst ausgestellt | LF | both |
| 31009 | MSB-Rechnung | **MSB → NB / LF / ESA** | Strom |
| 31011 | AWH Sperrprozesse Gas | GNB → LFG | Gas |

The `SettlementType` names the settlement; `SettlementType::default_pid()` maps
it to the Prüfidentifikator. API documentation: <https://docs.rs/grid-billing>.
`netzbilanzd` and `invoicd` are the production consumers.
