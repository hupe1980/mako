# invoic-checker

[![Crates.io](https://img.shields.io/crates/v/invoic-checker?label=invoic-checker&color=f59e0b&logo=rust)](https://crates.io/crates/invoic-checker)

INVOIC plausibility and tariff checks over BO4E for German energy-market
invoice recipients. `InvoicCheckEngine::check` runs an eight-stage pipeline
against a `Rechnung` and returns a `CheckReport` — it cannot fail — and the
`rechnung` module turns the same invoice into the published REMADV Antwortcodes.
No I/O, no async, money in `Decimal`. `invoicd` runs it on every inbound INVOIC,
`netzbilanzd` before dispatching its own.

## Example

```rust,no_run
use invoic_checker::{CheckConfig, CheckOutcome, InMemoryPreisblattStore, InvoicCheckEngine};
use rubo4e::current::Rechnung;

let rechnung: Rechnung = serde_json::from_str(&std::fs::read_to_string("rechnung.json")?)?;
let preisblaetter = InMemoryPreisblattStore::new(); // seeded from the NB's PRICAT

let report = InvoicCheckEngine::check(
    31002,             // Prüfidentifikator of the INVOIC
    "9900357000004",   // the sender, already identity-checked by the transport
    &rechnung,
    &preisblaetter,
    mako_fristen::heute(), // the day it was received
    &CheckConfig::default(),
);

if report.outcome != CheckOutcome::Ok {
    for finding in &report.findings {
        println!("{:?} dispute={}: {}", finding.kind, finding.is_dispute, finding.message);
    }
}
```

## The checks

| # | Rule | Outcome on failure |
|---|---|---|
| 1 | **Storno reference** — `ist_storno = true` must name an `original_rechnungsnummer` | `Dispute` |
| 2 | **Period validity** — `rechnungsperiode_start < end`, both within plausible range | `Dispute` |
| 3 | **Zahlungsziel** — before `rechnungsdatum` (invalid); < 10 WT after receipt on 31001–31003, 31009 with Betrag ≥ 0 (unterschritten, GPKE Teil 2 Kap. 3.3.2 Nr. 1, `A10`); < 10 WT after the Rechnungsdatum on 31005, 31007, Gas 31001–31003 (unterschritten, INVOIC AHB 1.0b `[24]`, S. 16/39); > 10 WT on a Rückerstattung (31001, 31002, 31005, Gas 31003) and on 31006/31008 (überschritten, `[25]`, `A11`); beyond `max_zahlungsziel_days` if set (exceeded) | `Dispute` · `Dispute` · `Dispute` · `Warn` |
| 3a | **WiM 31003 send window** — a Dienstleistungsrechnung dated more than 20 Werktage after the period it bills (WiM Teil 1 Kap. 3.7.2 Nr. 1). Only PID 31003 | `Warn` |
| 4 | **Currency agreement** — the document totals, every position's `gesamtpreis` and every `steuerbetraege` entry agree on one currency. Runs before the arithmetic, which reads every amount as EUR | `Dispute` |
| 5 | **Position arithmetic** — every `Rechnungsposition` satisfies `menge × einzelpreis ≈ gesamtpreis`. An unrepresentable product is itself a finding, not a panic | `Dispute` |
| 6 | **Document total** — the positions sum to `gesamtnetto` | `Warn` |
| 7 | **Umsatzsteuer** — a rate and an amount are stated (§14 Abs. 4 Nr. 8 UStG); `gesamtbrutto = gesamtnetto + gesamtsteuer`; each breakdown entry's `steuerwert` is its rate on its `basiswert` (± one cent); a reverse-charged invoice states **no** tax | `Dispute` |
| 8 | **Tariff / Angebot** — `einzelpreis` against the published Preisblatt, or against the accepted QUOTES for an ESA invoice. **Skipped for a Stornorechnung**. For PIDs 31005–31008 this is the Mehr-/Mindermengen price from the MMMA store | `Warn` or `Dispute` |

`CheckConfig` holds the tolerances (`arithmetic_tolerance_ppm`,
`total_tolerance_ppm`, `tariff_tolerance_ppm`), `require_tariff` and the
operator's optional `max_zahlungsziel_days` ceiling — no Festlegung publishes
one, so it is unset by default. Every entry point takes the
**Eingangsdatum**, which the 10-Werktage floor counts from. The Summen- and Positions-level tolerances are the same
ones the `mako_pruefung::rechnung` walk reads, so both paths agree. PID-specific
entry points: `check_storno` (31004 and any `ist_storno`), `check_msb_rechnung`
(31009, 31003), `check_esa_rechnung`, `check_mmm_settlement` (31005–31008).

## Published Antwortcodes

`rechnung::antwort_auf_rechnung` maps the invoice plus the recipient's own facts
(`EmpfaengerFakten`) onto the tree's Prüfschritte and returns a
`RechnungsAntwort` that knows its REMADV PID (33001 Zahlungsavis, 33003
Kopf/Summe, 33004 Position); `antwort_auf_erneute_rechnung` runs the round after
a COMDIS 29001. One walk serves three families:

| Familie | Rechnung | erneut | Nicht-Zahlungsavis | Storno |
|---|---|---|---|---|
| `ESA` — WiM Teil 2 Kap. 4.5 | `E_0264` | `E_0266` | `E_0265` | `E_0267` |
| `PREISBLATT_B_LF` — AWH Kap. 9.3 | `E_0270` | `E_0276` | `E_0271` | `E_0272` |
| `PREISBLATT_B_NB` — AWH Kap. 9.4 | `E_0273` | `E_0277` | `E_0274` | `E_0275` |

The invoice answers Prüfschritt 10 (17 on a Storno) and, for a Standard-rated
position, 330 about itself (`ustg`): an `A01` names each § 14 Abs. 4 UStG
Angabe the INVOIC does not state, an `A12` a position of Steuerkategorie `S`
away from the § 12 Abs. 1 UStG rate of its period. makod's inbound adapter
carries the `RFF+FC` Steuernummer and the `TAX` DE 5305 category, which BO4E
has no field for, as `ZusatzAttribut`e named by `ustg::STEUERNUMMER_ATTRIBUT`
and `ustg::STEUERKATEGORIE_ATTRIBUT`.

An ESA invoice is priced against the accepted QUOTES 15003, not a Preisblatt;
with no offer on record the price Prüfschritte answer „not comparable" rather
than „wrong". Sources: REMADV AHB 1.0a Kap. 3; Entscheidungsbaum-Diagramme 4.3
Kap. 8.27 and 9.3/9.4.

API documentation: <https://docs.rs/invoic-checker>.
