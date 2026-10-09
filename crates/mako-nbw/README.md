# mako-nbw

[![Crates.io](https://img.shields.io/crates/v/mako-nbw?label=mako-nbw&color=f59e0b&logo=rust)](https://crates.io/crates/mako-nbw)

Netzbetreiberwechsel — the § 46 EnWG handover of every Lokation in a grid area
from the outgoing Netzbetreiber (NBA) to the incoming one (NBN). This crate is
the migration's domain model: the Paket-ID, the Änderungszeitpunkt and the
ordered calendar of what must have been sent by when, for Strom and Gas. No I/O,
no clock, no workflow engine.

A NB-Wechsel has **no message family of its own**: no Prüfidentifikator, AHB or
EDIFACT message. The Anwendungshilfen sequence existing GPKE, MaBiS and WiM
Use-Cases plus two non-standardised exchanges. 55690 (Lokationsbündelstruktur)
is the one PID attributed to the AWH NBW alone and is routed to no workflow;
`makod` registers no NBW module.

## Example

```rust
use mako_nbw::{Aenderungszeitpunkt, Fristenkalender, Sparte};
use time::macros::date;

// A future Monatserster that leaves the Sparte's minimum lead — nothing else constructs.
let zeitpunkt = Aenderungszeitpunkt::neu(date!(2027-07-01), date!(2026-10-09), Sparte::Strom)?;

for geplant in Fristenkalender::fuer(Sparte::Strom).plan(zeitpunkt)? {
    println!("{:?}  Kap. {}  {}", geplant.faellig, geplant.meilenstein.kapitel, geplant.meilenstein.prozess);
}
```

## What ships

| Item | What it is |
|---|---|
| `Sparte` | Strom or Gas — two Anwendungshilfen, not two dialects of one |
| `PaketId` | the identifier the Energie Codes & Services GmbH issues, as an opaque non-empty token |
| `PaketAntrag` | the three application cases of Strom Kap. 3, the third of them a refusal |
| `Paket`, `PaketStatus` | the lifecycle: beantragt (ohne/mit NBN) → in Umsetzung → abgeschlossen, plus the „kein NB-Wechsel" outcome |
| `Aenderungszeitpunkt` | a future Monatserster that leaves the Sparte's minimum lead; nothing else can be constructed |
| `Meilenstein`, `STROM`, `GAS` | one entry per published Fristenübersicht row, with its Kapitel, parties, lead time, prerequisites and Fundstelle |
| `Fristenkalender::plan` | dated milestones in dependency order, refusing a plan whose prerequisite falls after what depends on it |

## Fristen

Strom — Prozess- und Fristenübersicht, Kap. 5, „eine (erstmalige) Übermittlung
sollte spätestens stattfinden":

| Vor dem Änderungszeitpunkt | Kapitel |
|---|---|
| 4 Monate | 6.1, 6.2, 6.3, 7.1, 9.1 |
| 3 Monate | 7.2, 7.3 (five rows), 7.4 |
| 2 Monate | 6.4, 7.5 (five rows) |
| 1 Monat | 8 (two rows) |
| — | 9.2, whose Frist comes from the Use-Case „Ende Messstellenbetrieb" (WiM Strom Teil 1) |

Gas — Prozess- und Fristenübersicht, Kap. 4.1.2, „Frist zum initialen
Austausch":

| Vor dem Änderungszeitpunkt | Kapitel |
|---|---|
| 4 Monate | 4.2 |
| 3 Monate + 10 WT | 4.3 |
| 3 Monate | 4.4 |
| 2 Monate | 4.5 |
| 25 WT | 4.6 |
| gemäß Abstimmung NBA/NBN | 4.8 |
| gemäß WiM Gas | 4.9 |

Plus, in Gas, „unverzüglich, spätestens jedoch 3 WT nach Kenntnisnahme" for every
update after the first transmission (Kap. 4.2.2, 4.3.2, 4.4.2, 4.5.2).

The Paket-ID is applied for **6 Monate** before the planned Änderungszeitpunkt
and the NBN reported **4 Monate** before it (Strom Kap. 3).

## Regulatory sources

- **§ 46 EnWG** — the Konzessionsverfahren whose Abs. 3 Bekanntgabe starts the
  clock on a Konzessionsübergang.
- **BDEW-Anwendungshilfe „Marktprozesse Netzbetreiberwechsel Sparte Strom",
  Version 1.2 (30.10.2025)** — applicable from 01.08.2025 for a NB-Wechsel to
  01.01.2026; a Paket-ID may be applied for before that date.
- **BDEW/VKU/GEODE-Anwendungshilfe „Marktprozesse Netzbetreiberwechsel Sparte
  Gas", Version 1.0 (26.06.2026)**.
- **BDEW-Anwendungshilfe „Rollenmodell für die Marktkommunikation im deutschen
  Energiemarkt", Version 2.1**.
- **BNetzA BK6-24-174**, Anlagen 1a–1c — GPKE Teil 1 bis 3, and **BK6-22-024**
  Anlage 1d — GPKE Teil 4: the Kap.-5 sequences reference Use-Cases from both.
  BK6-24-174 also carries WiM Strom Teil 1 und 2 (Anlagen 2a/2b), which Kap. 7.5
  and Kap. 9.2 lean on.
- **BNetzA BK6-24-174 Anlage 3** — MaBiS, the source of the Kap.-7.3
  Profil-Use-Cases.
- **BNetzA BK7-16-142 / BK7-19-001** — GeLi Gas, cited by the Gas
  Anwendungshilfe.

API documentation: <https://docs.rs/mako-nbw>.
