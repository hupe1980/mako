# mako-nbw

**Netzbetreiberwechsel** — the § 46 EnWG handover of every Lokation in a grid
area from the outgoing Netzbetreiber (NBA) to the incoming one (NBN).

A NB-Wechsel is triggered by anything that changes the **MP-ID of the
Netzbetreiber at a Lokation**: a Konzessionsübergang, a Netzverkauf, an
Ausgründung einer Tochtergesellschaft. It is a bulk migration measured in months
across every Marktlokation of a Netzgebiet rather than an event-driven
per-message workflow, and this crate is its domain model: the identity of the
migration, the instant it turns on, and the ordered calendar of what must have
been sent by when.

## It has no message family of its own

There is no NBW Prüfidentifikator, no NBW Anwendungshandbuch and no NBW EDIFACT
message. The BDEW Anwendungshilfe sequences Use-Cases that already exist in
GPKE, MaBiS and WiM, and adds two exchanges that are explicitly *not*
standardised. That is what the crate encodes.

| Kapitel (Strom) | Prozess | Carried by |
|---|---|---|
| 6.1 | Kommunikationsdaten NBA ↔ NBN | Übermittlung von Informationen (GPKE Teil 4) |
| 6.2 | Liste der Lokationen NBA → NBN | **NON-EDIFACT** |
| 6.3 | Lokationsbündelstruktur und DB NBA → NBN | EDIFACT |
| 6.4 | Ergänzende Daten zum Lokationsbündel NBA → NBN | EDIFACT |
| 7.1 | Stammdatenänderung vom NB verantwortlich (ausgehend), NBA → LF/MSB/ÜNB | GPKE Teil 4 |
| 7.2 | Kommunikationsdaten NBN ↔ DB | Übermittlung von Informationen (GPKE Teil 4) |
| 7.3 | Liste der Profildefinitionen NB → LF | MaBiS |
| 7.3 | Normierte Profile und Profilscharen NB → LF | MaBiS |
| 7.3 | Liste der Profildefinitionen NB → MSB | MaBiS |
| 7.3 | Normierte Profile NB → MSB | MaBiS |
| 7.3 | Preisblatt NB → LF | GPKE Teil 2 |
| 7.4 | Information von NB an weiteren Datenberechtigten | **NON-EDIFACT**, in Textform |
| 7.5 | Abrechnungsdaten Netznutzungsabrechnung | GPKE Teil 2 |
| 7.5 | Abrechnungsdaten Bilanzkreisabrechnung | GPKE Teil 2 |
| 7.5 | Stammdaten zur Bilanzkreistreue | GPKE Teil 4 |
| 7.5 | Stammdatenänderung vom NB verantwortlich (ausgehend), NBN → LF/MSB/ÜNB | GPKE Teil 4 |
| 7.5 | Berechnungsformel | Übermittlung der Berechnungsformel (WiM Strom Teil 2) |
| 8 | Stammdatenänderung vom LF verantwortlich (ausgehend), LF → NBN | GPKE Teil 4 |
| 8 | Stammdatenänderung vom MSB verantwortlich (ausgehend), MSB → NBN | GPKE Teil 4 |
| 9.1 | Liste der Lokationen gMSBA → gMSBN | **NON-EDIFACT** |
| 9.2 | Ende Messstellenbetrieb gMSBA → NBN | WiM Strom Teil 1 |

### The wire evidence

The claim above is checkable against the routing tables, and it was checked:

- **55690** („Lokationsbündelstruktur und Datenbereitstellung", Kap. 6.3) is the
  only Prüfidentifikator the Anwendungsübersicht attributes to *AWH NBW* alone.
  It is imported, profiled and validated — and credited to **no workflow**. That
  is the correct state, not a gap.
- **55691/55692** (Kap. 6.4) ride `gpke-stammdatenaenderung` as
  `StammdatenObjekt::PaketId`; they are not an NBW family.
- The Kommunikationsdaten of Kap. 6.1 and 7.2 are **PARTIN 37000–37006 and
  37008–37014** under `gpke-partin` / `geli-gas-partin`.

Two different reasons, and conflating them would be wrong. **55691/55692 and the
PARTIN band** are already owned: minting an NBW family for them would
re-register Prüfidentifikatoren other workflows hold, which the
`DeploymentRoles` conflict guard refuses outright. **55690 is claimed by
nobody** — it is unrouted simply because `makod` registers no NBW module at all
(it has no dependency on this crate). That is the deliberate current state, and
it is recorded as such in `makod`'s `pid_reference_guard`. The architecture-level write-up, including the open questions,
is in the platform design notes; this README carries the row-by-row Fristenübersicht.

Gas is a separate Anwendungshilfe with its own numbering and seven rows:
Übergabe der Kontaktdaten der DB (4.2), Information der DB (4.3), Übergabe der
Stammdaten (4.4), Übermittlung der Stammdaten (4.5, UTILMD Gas), Übergang des
Messstellenbetriebs (4.6, WiM Gas), Werteübermittlung Gas an NB (4.8) and an DB
(4.9, WiM Gas).

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

The crate holds no clock: every date check takes the business date from the
caller. It has no I/O, no EDIFACT parsing and no workflow engine.

## The Fristen

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

## Rollen

Both Anwendungshilfen build on the BDEW-Anwendungshilfe „Rollenmodell für die
Marktkommunikation im deutschen Energiemarkt", Version 2.1.

| Sparte | Rollen |
|---|---|
| Strom (Kap. 2.1) | LF, MSB, NB, BKV, BIKO, ÜNB, Registerbetreiber (das Umweltbundesamt), EIV |
| Gas (Kap. 2.1) | LF, MSB, NB, BKV, MGV |

Derived from them: NBA/NBN, gMSBA/gMSBN (Strom), MSBA/MSBN (Gas). LV and EZ are
listed as Rollen in the Use-Case „Information von NB an weiteren
Datenberechtigten". **Datenberechtigter** is a capacity rather than a Rolle: any
of the above processing a Lokation's Abrechnungs-, Stamm- or Bewegungsdaten under
a contractual or statutory duty, possibly time-limited, possibly several per
datum at once.

Objekte: Marktlokation, Messlokation, Bilanzkreis, and — Strom only —
Netzlokation, Steuerbare Ressource, Technische Ressource. Gebiete:
Bilanzierungsgebiet and Regelzone (Strom), Marktgebiet and Netzgebiet (Gas).

## Where Gas differs

- **No Paket-ID.** NBA and NBN agree the affected Lokationen bilaterally.
- **No ordering column and no Lokationsebene column** in the Fristenübersicht.
  The prerequisites the crate carries for Gas come from the Use-Cases'
  Vorbedingung fields, and every Gas row is `Lokationsebene::NichtAusgewiesen`.
- **The Gastag.** The handover is zum Monatsersten in both Sparten, but in Gas
  deliveries until 06:00 on that day are still assigned to the NBA.
- **Two rows carry no figure at all**, naming WiM Gas or a bilateral agreement
  instead.

## Where the Anwendungshilfen leave something open

- **The Paket-ID has no stated format.** No length, no character set, no check
  digit — so the type accepts any non-empty token rather than inventing a rule
  that would refuse identifiers the issuing body may hand out.
- **§ 46 Abs. 3 EnWG, 18 Monate.** Strom Kap. 3 states it as a parenthetical
  Hinweis whose antecedent („dies") is not resolved by the text: it reads equally
  as the Antragsfrist and as the geplanter Änderungszeitpunkt, and the two differ
  by six months. The figure is published as a constant; no date is derived from
  it.
- **No minimum lead is stated as a number.** Both Anwendungshilfen say the
  Änderungszeitpunkt must respect „die in den nachfolgenden Prozessen
  beschriebenen Vorlauffristen" and leave the figure to their tables, so the
  crate derives it — 6 Monate in Strom, 4 Monate in Gas — and names the row it
  comes from.
- **Strom states no clock time** for the Änderungszeitpunkt, so none is offered
  for that Sparte.

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
  BK6-22-024 also carries WiM Strom Teil 1 und 2 (Anlagen 2a/2b), which Kap. 7.5
  and Kap. 9.2 lean on.
- **BNetzA BK6-24-174 Anlage 3** — MaBiS, the source of the Kap.-7.3
  Profil-Use-Cases.
- **BNetzA BK7-16-142 / BK7-19-001** — GeLi Gas, cited by the Gas
  Anwendungshilfe.

## Related crates

| Crate | Role |
|---|---|
| [`mako-nbw`](https://docs.rs/mako-nbw) ← **this crate** | The migration identity, the Änderungszeitpunkt, the Fristenkalender |
| [`mako-fristen`](https://docs.rs/mako-fristen) | *When* an answer is due — Werktage, the MaKo holiday calendar, the per-PID Antwortfristen |
| [`mako-gpke`](https://docs.rs/mako-gpke) | GPKE Teil 1–4 — the Use-Cases Kap. 6 and 7 sequence |
| [`mako-mabis`](https://docs.rs/mako-mabis) | MaBiS — the source of the Kap. 7.3 Profil-Use-Cases |
| [`mako-wim`](https://docs.rs/mako-wim) | WiM Strom Teil 1/2 — leaned on by Kap. 7.5 and 9.2 |
| [`makod`](https://hupe1980.github.io/mako/docs/services/makod/) | Production daemon — runs the GPKE / MaBiS / WiM Use-Cases this handover sequences |

Part of **mako**, an open-source Rust platform for German energy market
communication (Marktkommunikation). Full documentation: <https://hupe1980.github.io/mako/>

## License

MIT OR Apache-2.0
