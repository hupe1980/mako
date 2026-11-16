# invoic-checker

[![Crates.io](https://img.shields.io/crates/v/invoic-checker?label=invoic-checker&color=f59e0b&logo=rust)](https://crates.io/crates/invoic-checker)

INVOIC plausibility and tariff checks over BO4E for the recipient of a German
energy-market invoice — LF, NB or ESA. `InvoicCheckEngine::check` runs eight
stages against a `Rechnung` and returns a `CheckReport` (`Ok`, `Warn`,
`Dispute`); it cannot fail. These findings are the operator's, for every INVOIC
PID; the published REMADV Antwortcodes come from `mako-pruefung`'s walk, which
the `rechnung` module feeds from the same invoice. No I/O, no async.

## Example

```rust,no_run
use invoic_checker::{CheckConfig, CheckOutcome, InMemoryPreisblattStore, InvoicCheckEngine};
use rubo4e::current::Rechnung;

let rechnung: Rechnung = serde_json::from_str(&std::fs::read_to_string("rechnung.json")?)?;
let preisblaetter = InMemoryPreisblattStore::new(); // seeded from the NB's PRICAT

let report = InvoicCheckEngine::check(
    31002,                 // Prüfidentifikator of the INVOIC
    "9900357000004",       // the sender, identity-checked by the transport
    &rechnung,
    &preisblaetter,
    mako_fristen::heute(), // the Eingangsdatum
    &CheckConfig::default(),
);

if report.outcome != CheckOutcome::Ok {
    for finding in &report.findings {
        println!("{:?} dispute={}: {}", finding.kind, finding.is_dispute, finding.message);
    }
}
```

## What it covers

| # | Stage | On failure |
|---|---|---|
| 1 | Storno names its `original_rechnungsnummer` (INVOIC AHB 1.0b Kap. 3.1.2) | `Dispute` |
| 2 | Rechnungsperiode valid | `Dispute` |
| 3 | Zahlungsziel: not before the Rechnungsdatum, the published 10-WT floor and ceiling, an optional operator ceiling | `Dispute`; operator ceiling `Warn` |
| 3a | PID 31003 sent within 20 Werktage (WiM Teil 1 Kap. 3.7.2 Nr. 1) | `Warn` |
| 4 | One currency across totals, positions and tax entries | `Dispute` |
| 5 | `menge × einzelpreis ≈ gesamtpreis` per position | `Dispute` |
| 6 | Positions sum to `gesamtnetto` | `Warn` |
| 7 | Umsatzsteuer stated (§ 14 Abs. 4 Nr. 8 UStG) and consistent; none under reverse charge | `Dispute` |
| 8 | Price against the Preisblatt, the accepted QUOTES for an ESA, the MMM price for 31005–31008; skipped on a Storno | `Warn` or `Dispute` |

The 10-WT floor counts from the Eingangsdatum for a payable 31001–31003 or
31009 (GPKE Teil 2 Kap. 3.3.2 Nr. 1 and the WiM process descriptions) and from
the Rechnungsdatum where INVOIC AHB 1.0b `[24]` sets it. `CheckConfig` holds the tolerances and the optional
`max_zahlungsziel_days`, unset by default because no Festlegung publishes one.
PID-specific entry points: `check_storno`, `check_msb_rechnung`,
`check_esa_rechnung`, `check_mmm_settlement`.

`rechnung::antwort_auf_rechnung` and `antwort_auf_erneute_rechnung` (after a
COMDIS 29001) take the invoice plus the recipient's `EmpfaengerFakten` and
return an answer that knows its REMADV PID (33001, 33003 or 33004):

| Family | Rechnung | erneut | Nicht-Zahlungsavis | Storno |
|---|---|---|---|---|
| ESA — WiM Teil 2 Kap. 4.5 | `E_0264` | `E_0266` | `E_0265` | `E_0267` |
| Preisblatt B ↔ LF — AWH Änderung der Technik Kap. 9.3 | `E_0270` | `E_0276` | `E_0271` | `E_0272` |
| Preisblatt B ↔ NB — AWH Änderung der Technik Kap. 9.4 | `E_0273` | `E_0277` | `E_0274` | `E_0275` |

The invoice answers Prüfschritt 10 (17 on a Storno) and 330 about itself
(`ustg`): the § 14 Abs. 4 UStG Angaben and the § 12 Abs. 1 UStG rate of a
Standard-rated position. A fact the records do not hold leaves the walk open
rather than answering it.

## Sources

- INVOIC AHB 1.0b; REMADV AHB 1.0a Kap. 3
- Entscheidungsbaum-Diagramme und Codelisten 4.3, Kap. 8.27 and 9.3/9.4
- GPKE Teil 2 (BK6-24-174 Anlage 1b); WiM Strom Teil 1 and Teil 2
- BDEW AWH Prozesse zur Änderung der Technik an Lokationen V1.1
- UStG §§ 12, 14

Operator reference: [invoicd](https://hupe1980.github.io/mako/docs/services/invoicd/) ·
API: <https://docs.rs/invoic-checker>

License: MIT OR Apache-2.0
