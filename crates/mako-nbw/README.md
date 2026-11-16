# mako-nbw

[![Crates.io](https://img.shields.io/crates/v/mako-nbw?label=mako-nbw&color=f59e0b&logo=rust)](https://crates.io/crates/mako-nbw)

Netzbetreiberwechsel — the § 46 EnWG handover of every Lokation in a grid area
from the outgoing Netzbetreiber (NBA) to the incoming one (NBN), for Strom and
Gas. This crate is the migration's domain model: the Paket-ID, the
Änderungszeitpunkt and the ordered calendar of what must have been sent by
when. No I/O, no clock, no workflow: a NB-Wechsel has no Prüfidentifikator, AHB
or EDIFACT message of its own — the Anwendungshilfen sequence existing GPKE,
MaBiS and WiM Use-Cases plus two non-EDIFACT exchanges — so `makod` registers no
NBW module.

## Example

```rust
use mako_nbw::{Aenderungszeitpunkt, Fristenkalender, Sparte};
use time::{Date, Month};

let heute = Date::from_calendar_date(2026, Month::January, 15).unwrap();
let az = Aenderungszeitpunkt::neu(
    Date::from_calendar_date(2027, Month::January, 1).unwrap(),
    heute,
    Sparte::Strom,
)
.expect("ein Monatserster mit 6 Monaten Vorlauf");

let plan = Fristenkalender::strom().plan(az).expect("die Tabelle ist konsistent");
// Kap. 6.1 is due four months before — 2026-09-01.
assert_eq!(
    plan[0].faellig,
    Some(Date::from_calendar_date(2026, Month::September, 1).unwrap())
);
```

## What it covers

| Item | What it is |
|---|---|
| `Sparte` | Strom or Gas — two Anwendungshilfen with their own chapters, Fristen and Rollen; Gas has no Paket-ID |
| `PaketId`, `PaketAntrag` | the identifier the Energie Codes & Services GmbH issues; Strom Kap. 3's three application cases, the third a refusal (NBN identical to NBA) |
| `Paket`, `PaketStatus` | the lifecycle of one handover, including the „kein NB-Wechsel" outcome |
| `Aenderungszeitpunkt` | a future Monatserster that keeps the Sparte's minimum lead; nothing else constructs |
| `Meilenstein`, `STROM`, `GAS` | one entry per Fristenübersicht row — 21 Strom (Kap. 5), 7 Gas (Kap. 4.1.2) — with Kapitel, parties, lead time and prerequisites |
| `Fristenkalender::plan` | dated milestones in dependency order; refuses a table whose prerequisite falls after its dependant |

Leads before the Änderungszeitpunkt:

| Sparte | Lead | Kapitel |
|---|---|---|
| Strom | 6 Monate — Paket-ID beantragt; 4 Monate — NBN gemeldet | 3 |
| Strom | 4 / 3 / 2 / 1 Monate | 6.1–6.3, 7.1, 9.1 / 7.2–7.4 / 6.4, 7.5 / 8 |
| Gas | 4 Monate, 3 Monate + 10 WT, 3 Monate, 2 Monate, 25 WT | 4.2, 4.3, 4.4, 4.5, 4.6 |
| Gas | an update after the first transmission: 3 WT nach Kenntnisnahme | 4.2.2, 4.3.2, 4.4.2, 4.5.2 |

Strom Kap. 9.2 and Gas Kap. 4.8 / 4.9 state no figure of their own. Werktage
count on the `mako-fristen` BDEW MaKo calendar.

## Sources

- § 46 EnWG — the Konzessionsverfahren
- BDEW-Anwendungshilfe „Marktprozesse Netzbetreiberwechsel Sparte Strom",
  Version 1.2 (30.10.2025)
- BDEW/VKU/GEODE-Anwendungshilfe „Marktprozesse Netzbetreiberwechsel Sparte
  Gas", Version 1.0 (26.06.2026)
- BDEW-Anwendungshilfe „Rollenmodell für die Marktkommunikation im deutschen
  Energiemarkt", Version 2.1

API: <https://docs.rs/mako-nbw>

License: MIT OR Apache-2.0
