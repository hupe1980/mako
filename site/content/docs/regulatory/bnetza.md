+++
title = "BNetzA Regulatory Reference"
description = "Complete BNetzA ruling index for German energy market communication: BK6 GPKE/WiM/MaBiS, BK7 GeLi Gas, current rulings, and Fristen."
weight = 10
+++
# BNetzA Regulatory Reference

Reference document for Bundesnetzagentur rulings that govern German energy market
communication (MaKo). Extracted from official BNetzA pages as of 2026-07-23.

Sources:
- BK6 Netzzugang/Messwesen: <https://www.bundesnetzagentur.de/DE/Beschlusskammern/BK06/BK6_83_Zug_Mess/NetzZ.html>
- BK7 Erdgas / Lieferantenwechsel: <https://www.bundesnetzagentur.de/DE/Beschlusskammern/BK07/BK7_04_Erdgas/BK7_45_LieferW_Messw/BK7_LieferantenW_Messw.html>

---

## Beschlusskammer 6 — Electricity (Strom)

BK6 is responsible for electricity network access, metering, and market communication.
All GPKE, WiM, and MaBiS processes are regulated under BK6.

### GPKE — Geschäftsprozesse zur Kundenbelieferung mit Elektrizität

**Page:** <https://www.bundesnetzagentur.de/DE/Beschlusskammern/BK06/BK6_83_Zug_Mess/831_gpke/gpke_node.html>

**Current ruling:**
- **BK6-24-174** (Beschluss 24.10.2024) — GPKE Teil 1–3 + WiM + MaBiS
- **BK6-22-024** (Beschluss 21.03.2024) — **LFW24-Festlegung** („beschleunigter werktäglicher
  Lieferantenwechsel in 24 Stunden", statutory anchor **§20a EnWG**): re-issued GPKE Teil 2
  und Teil 4 and absorbed the MPES processes into the GPKE (effective 06.06.2025).
  Go-live 06.06.2025 — postponed from 04.04.2025 by Mitteilung Nr. 4 vom 06.12.2024
- **Gültig seit: 06.06.2025**

**Process documents (Lesefassungen):**

| Document | Content |
|---|---|
| Anlage 1a, GPKE Teil 1 | Einführende Prozessbeschreibung |
| Anlage 1b, GPKE Teil 2 | Fokus Zuordnungsprozesse |
| Anlage 1c, GPKE Teil 3 | (Lesefassung) |
| Anlage 1d, GPKE Teil 4 | Fokus Stammdatenprozesse (via BK6-22-024) |

**Scope — electricity market only:**
- Lieferantenwechsel Strom (UTILMD E, PIDs 55001–55006, 55016–55018)
  — 55001 Anmeldung / 55004 Abmeldung (Anfragen des LF); 55002/55003 Bestätigung/Ablehnung
    Anmeldung; 55005/55006 Bestätigung/Ablehnung Abmeldung; 55016 Kündigung Lieferbeginn
    (LFN → LFA); 55017/55018 Bestätigung/Ablehnung Kündigung
- Neuanlage MaLo (UTILMD E, PIDs 55600–55605)
- Sperrauftrag / Entsperrauftrag Strom (ORDERS, PIDs 17115–17117)
- Anfrage Daten der individuellen Bestellung (UTILMD, PID 55555) — GPKE Teil 4 data request
- Konfigurationseinrichtung Rollenzuordnung MSB (ORDERS/ORDRSP, PIDs 17134–17135, 19001–19002) — via BK6-22-024 GPKE Teil 4
- Abschlagsrechnung / NN-Rechnung Netz (INVOIC, PIDs 31001–31002)
- Stornorechnung Netz (INVOIC, PID 31004)
- **Mehr-/Mindermengen Strom** (INVOIC, PIDs 31005–31006) — see Mitteilung Nr. 72 below
- **Mehr-/Mindermengen Gas** (INVOIC, PIDs 31007–31008; NB → MGV, Gas-only) — belongs to GaBi Gas (`mako-gabi-gas`)

**APERAK Frist (GPKE):** **45 Minuten** an einem Werktag für UTILMD und ORDERS
(APERAK AHB 1.0 §2.4.1); Samstagseingang bis Sonntag 12:00 Uhr, alles übrige bis
12:00 Uhr des nächsten Werktags. Nicht die Antwortfrist des Geschäftsprozesses —
siehe unten.

**Laufende Verfahren:**

| Az. | Gegenstand | Eröffnet |
|---|---|---|
| BK6-24-210 | Festlegungsverfahren MaBiS-Hub (Aggregation und Abrechnung bilanzierungsrelevanter Daten) | 02.10.2024 |

**Selected Mitteilungen:**

| Nr. | Gegenstand | Datum |
|---|---|---|
| 73 | Az. BK6-06-009 — Energy Sharing (**§42c EnWG**) läuft über das **Dienstleistungsmodell** innerhalb der bestehenden Lieferanten-/Bilanzkreiszuordnung: keine neuen MaKo-Prozesse, keine GPKE-Änderungen; Fristen 01.06.2026 / 01.06.2028 bekräftigt | 07.07.2026 |
| 72 | Empfehlung zur Anwendung der neuen BDEW-Anwendungshilfe „Ermittlung des Mehr-/Mindermengenpreises **Strom**" (auf Basis der BDEW-2025-Standardlastprofile) | 05.02.2026 |
| 71 | Empfehlung zur Anwendung „Marktprozesse Netzbetreiberwechsel Strom" | 01.07.2024 |
| 66 | Empfehlung zur Anwendung „Prozesse zur Ermittlung und Abrechnung von Mehr-/Mindermengen Strom und Gas" (superseded by Mitteilung 72 for Strom) | 27.01.2020 |
| 46 | Prozesse zur Ermittlung der Abrechnung von Mehr-/Mindermengen Strom und Gas (nicht mehr aktuell, s. Mitteilung 66) | 22.01.2015 |
| 4 | Einführung Änderungsmanagement, Umsetzung INVOIC/REMADV, GPKE-Auslegungsgrundsätze | 28.11.2007 |

> **Key boundary:** Mitteilung Nr. 72 (05.02.2026) explicitly refers to „Mehr-/Mindermengenpreises **Strom**"
> only, confirming that PIDs 31005–31006 are Strom GPKE processes.
> PIDs 31007–31008 (Aggreg. MMM-Rechnung, NB → MGV) are Gas-only (MGV is a Gas-domain role)
> and belong to GaBi Gas (`mako-gabi-gas` `gabi-gas-invoic`), not GPKE.

---

### MPES — Marktprozesse für erzeugende Marktlokationen (Strom)

**Status:** merged into GPKE.

- **BK6-20-160** (standalone MPES) — gültig bis **05.06.2025**
- Seit **06.06.2025** sind die MPES-Prozesse in **GPKE Teil 2** aufgegangen (via BK6-22-024 / LFW24)
- Erzeugende-MaLo PIDs **55077/55078/55080** und **55601** live in der GPKE

---

### MaBiS — Marktregeln für die Bilanzkreisabrechnung Strom

**Page:** <https://www.bundesnetzagentur.de/DE/Beschlusskammern/BK06/BK6_83_Zug_Mess/833_mabis/mabis_node.html>

**Current ruling:**
- **BK6-24-174** (24.10.2024); Mitteilung Nr. 4 vom 06.12.2024
- **Gültig seit: 06.06.2025**

**Document:** Anlage 3, MaBiS (Lesefassung, 9 MB PDF)

**Scope:**
- Bilanzkreisabrechnung Strom between Bilanzkreisverantwortliche (BKV) and Übertragungsnetzbetreiber (ÜNB)
- MSCONS, PID 13003 (Bilanzkreisabrechnung Summenzeitreihe)
- Not supplier-switch; not network billing

**Laufende Verfahren:**

| Az. | Gegenstand | Eröffnet |
|---|---|---|
| BK6-24-210-1 | MaBiS-Hub — Messwertverarbeitung / Pseudonymisierung | 02.10.2024 |
| BK6-24-210-2 | MaBiS-Hub — Abrechnung | 02.10.2024 |

> **MaBiS-Hub:** no Beschluss yet (the H1-2026 target slipped; -1 consultation closed 17.11.2025); Hub go-live still planned H2 2028.

**Selected Mitteilungen:**

| Nr. | Gegenstand | Datum |
|---|---|---|
| 10 | Veröffentlichung BDEW-Anwendungshilfe „Fallsammlung MaBiS" | 09.05.2019 |
| 8 | MaBiS Geschäftsprozesse, Version 2.0 | 04.06.2013 |
| 3 | MaBiS Geschäftsprozesse, Version 1.0 | 28.04.2010 |

---

### WiM — Wechselprozesse im Messwesen

**Page:** <https://www.bundesnetzagentur.de/DE/Beschlusskammern/BK06/BK6_83_Zug_Mess/834_wim/BK6_WiM_node_neu.html>

**Current ruling:**
- **BK6-22-024** — WiM was **not** reissued under BK6-24-174; that decision
  covers GPKE and MaBiS. Cite BK6-22-024 for every WiM process.

**Process documents:**

| Document | Content |
|---|---|
| Anlage 2a, WiM Teil 1 | Fokus Basis Prozesse (Lesefassung) |
| Anlage 2b, WiM Teil 2 | Fokus Übermittlung von Werten (Lesefassung) |

**Scope:**
- Messstellenbetreiberwechsel (UTILMD, PIDs 55039, 55042, 55051, 55168)
  — 55039 Kündigung MSB, 55042 Anmeldung MSB, 55051 Ende MSB, 55168 Verpflichtungsanfrage;
    legacy PIDs 11001–11003 are superseded and not in the current AHB
- Geräteübernahme ORDERS (PIDs 17001, 17002, 17009)
- Stammdaten ORDERS (PIDs 17101–17135; 17101 inbound Anforderung, 17102–17135 outbound Übermittlung)
- Stornierung ORDCHG (PID 39000; 39001–39002 outbound responses)
- WiM-Rechnung (INVOIC, PID 31003) — Abrechnung von Dienstleistungen im Messwesen, beide Sparten
- MSB-Rechnung (INVOIC, PID 31009) — Messstellenbetrieb an NB, LF oder ESA

**Fristen (WiM Strom):** die fachliche Antwort ist **je Prozess** befristet — Kündigung (55039) **3 WT**, Anmeldung (55042) **5 WT**, Abmeldung (55051) **7 WT**, Verpflichtungsanfrage (55168) **1 WT** (BK6-22-024 Anlage 2a, Kap. 2.2.2 / 2.3.2 / 2.4.2 / 2.5.2). Samstage, Sonntage und gesetzliche Feiertage sind keine Werktage.

> Davon zu unterscheiden ist die **APERAK**-Eingangsbestätigung: für UTILMD Strom **45 Minuten** an Werktagen (APERAK AHB §2.4.1) — eine eigene, deutlich kürzere Frist.

**Laufende Verfahren:**

| Az. | Gegenstand | Eröffnet |
|---|---|---|
| BK6-24-210 | Festlegungsverfahren MaBiS-Hub | 02.10.2024 |

**Selected Mitteilungen:**

| Nr. | Gegenstand | Datum |
|---|---|---|
| 3 | Erweiterung Aufgabenumfang MSB: Pflicht zur Übermittlung von Messwerten an ESA | 07.02.2024 |
| 2 | Ergänzung Wertetabelle aufgrund EEG 2021 | 02.07.2021 |
| 1 | Fehlerkorrekturen | 19.01.2017 |

---

### Redispatch 2.0 — Koordination von Einspeisungen

**Page:** <https://www.bundesnetzagentur.de/DE/Beschlusskammern/1_GZ/BK6-GZ/2023/BK6-23-241/BK6-23-241_beschluss.html>

**Current rulings:**

| Az. | Topic | Effective |
|---|---|---|
| **BK6-20-059** | `AcknowledgementDocument` (6 h), `StatusRequest` (24 h) | 2021-10-01 |
| **BK6-20-060** | `Stammdaten` forwarding (1 Werktag), Activation (5 min) | 2021-10-01 |
| **BK6-20-061** | `Kostenblatt` submission (15th of following month) | 2021-10-01 |
| **BK6-23-241** | Fortentwicklung der Bilanzierung von Redispatch-Maßnahmen (Beschluss 07.05.2026) — staged transition from Prognosemodell to Planwertmodell in Verteilnetzen | staged |

**Scope:**
- All German TSOs (ÜNB) and DSOs (VNB) plus connected asset operators (ANB)
- Redispatch 2.0 uses **CIM/IEC 62325 XML** documents — not EDIFACT (except IFTSTA)
- IFTSTA status confirmations: PIDs 21037 (Ansicht NB) and 21038 (Ansicht BTR)
- Handled by the `mako-redispatch` + `redispatch-xml` crates
- XML document types are catalogued in the [PID reference](@/docs/regulatory/pid-reference.md#redispatch-2-0-xml-document-types-not-edifact-pids)

**Regulatory context:** Mandatory since NABEG 2019, § 13 ff. EnWG.
Covers renewable (EE) and combined heat-and-power (KWK) plants ≥ 100 kW,
plus all installations permanently remote-controllable by a grid operator.

**Bilanzieller Ausgleich (§14 EnWG, amended late 2025):** until **31.12.2031** the
**BKV** performs the bilanzielle Ausgleich of DSO redispatch measures (with DSO
compensation); from **2032** the Netzbetreiber-Ausgleich returns.

---

## Beschlusskammer 7 — Gas (Erdgas)

BK7 is responsible for gas network access, metering, balancing, and market communication.
GeLi Gas (Lieferantenwechsel Gas) is regulated under BK7. GaBi Gas (balancing) is also BK7.

### GeLi Gas — Geschäftsprozesse Lieferantenwechsel Gas

**Page:** <https://www.bundesnetzagentur.de/DE/Beschlusskammern/BK07/BK7_04_Erdgas/BK7_45_LieferW_Messw/451_LieferW/BK7_LieferantenW.html>

**Current ruling:**
- **GeLi Gas 3.0** — **BK7-24-01-009** (Beschluss 12.09.2025, abgeschlossen 24.09.2025)
  - [Beschluss PDF](https://www.bundesnetzagentur.de/DE/Beschlusskammern/1_GZ/BK7-GZ/2024/BK7-24-0009/Anlagen/BK7-24-01-0009_Beschluss_Download_BF.pdf)
  - [Anlage PDF](https://www.bundesnetzagentur.de/DE/Beschlusskammern/1_GZ/BK7-GZ/2024/BK7-24-0009/Anlagen/BK7-24-01-0009_Anlage_Download_BF.pdf)
  - **Anwendung:** Tenor gilt ab **01.01.2026** (Ziff. 18) — mit Ausnahme von Ziff. 13–16:
    der Widerruf von BK7-17-026 und der neue **Messstellenbetreiberrahmenvertrag Gas**
    (§9 Abs. 1 Ziff. 3 MsbG) werden zum **01.10.2026** wirksam bzw. fällig

**Previous rulings:**

| Az. | Gegenstand | Datum |
|---|---|---|
| BK7-19-001 | Anpassung GeLi Gas inkl. Messstellenbetreiberrahmenvertrag | 22.11.2023 |
| BK7-16-142 | Anpassung an Erfordernisse Digitalisierung der Energiewende | 20.12.2016 |
| BK7-11-075 | Anpassung „GeLi Gas" | 28.10.2011 |
| BK7-06-067 | Festlegung einheitlicher Geschäftsprozesse und Datenformate „GeLi Gas" (Ursprungsfestlegung) | 20.08.2007 |

**Scope — gas supplier switching only:**
- Lieferbeginn Gas / Lieferende Gas (UTILMD G, PIDs 44001–44018)
  — 44001 Anmeldung NN / 44004 Abmeldung NN (LFN → GNB); 44002/44003 Bestätigung/Ablehnung
    Anmeldung; 44005/44006 Bestätigung/Ablehnung Abmeldung; 44007–44012 Abmeldung durch den
    GNB; 44013–44015 Anmeldung/Zuordnung EoG; 44016–44018 Kündigung Lieferbeginn (LFN ↔ LFA)
- Sperr-/Entsperrprozess Gas (ORDERS, PIDs 17115–17117) — same PID numbers as
  Strom Sperrung; implemented in `mako-geli-gas` as `geli-gas-sperrung-lf` (LF role)
  and `geli-gas-sperrung-nb` (GNB role) under BK7-24-01-009
- APERAK / CONTRL acknowledgements
- **Does NOT cover** INVOIC billing or Mehr-/Mindermengen Gas — these belong to GaBi Gas (BK7 Bilanzierung)

**APERAK Frist (GeLi Gas):** **10 Werktage** (Samstag, Sonntag und gesetzliche Feiertage zählen nicht)

**Sonstiges / Gemeinsame Mitteilungen:**
- Gemeinsame Mitteilungen zu Datenformaten (BK6 + BK7 joint): <https://www.bundesnetzagentur.de/DE/Beschlusskammern/BK06/BK6_83_Zug_Mess/835_mitteilungen_datenformate/Datenformate-node.html>
- Mitteilung Nr. 1 zu BK7-19-001: AS4-Kommunikation

---

### BK7 Messwesen Gas

**Page:** <https://www.bundesnetzagentur.de/DE/Beschlusskammern/BK07/BK7_04_Erdgas/BK7_45_LieferW_Messw/452_Messw/BK7_Messw.html>

**Scope:**
- Messwesen für Gas (MSCONS, various PIDs for Zeitreihen/Mengendaten)

---

### BK7 Bilanzierung und Konvertierung (GaBi Gas context)

**Page:** <https://www.bundesnetzagentur.de/DE/Beschlusskammern/BK07/BK7_04_Erdgas/BK7_41_Bilanz_Konvert/BK7_Bilanz_Konvert.html>

**Current ruling:**
- **GaBi Gas 2.1** — **BK7-24-01-008**, wirksam seit **01.01.2026**; ersetzt die
  Bilanzierungsregeln der GasNZV (GasNZV am 31.12.2025 außer Kraft getreten)

**Scope:**
- Bilanzierung in Gasbilanzkreisen (Allokation, Nominierung)
- Regelenergie Gas
- Mehr-/Mindermengenbilanzierung Gas
- Konvertierung im qualitätsübergreifenden Gasmarktgebiet
- INVOIC Gas billing: Kapazitätsrechnung (PID 31010)
- DVGW message types: ALOCAT, NOMINT, NOMRES

---

## Mitteilungen zu den Datenformaten (BK6 + BK7 gemeinsam)

**Page:** <https://www.bundesnetzagentur.de/DE/Beschlusskammern/BK06/BK6_83_Zug_Mess/835_mitteilungen_datenformate/Datenformate-node.html>

| Nr. | Gegenstand | Datum |
|---|---|---|
| 56 | Finale Datenformate, verbindlich zum **01.10.2026**: UTILMD Strom 2.2, UTILMD Gas 1.2, MSCONS 3.2, ORDERS/ORDRSP 1.1b, EBD 4.3; neues Attribut „fernsteuerbar" (§10b EEG) auf der TR | 01.04.2026 |
| 55 | Konsultation der Datenformate zum **01.10.2026** | 02.02.2026 |
| 54 | Datenformate verbindlich zum **01.04.2026** | 01.10.2025 |
| 53 | Konsultation Konzept API-Webdienste | 01.09.2025 |
| 51 | Datenformate verbindlich zum **01.10.2025** | — |
| 50 | Aussetzung der Pflicht zur Content-Verschlüsselung (Gas) | 26.03.2025 |

---

## APERAK Fristen Summary

| Process family | Frist | Shape |
|---|---|---|
| GPKE (Strom) | **11:00 / 06:00 / 05:00 / 09:00 Uhr des 1. WT nach dem ÜT**, je Prüfidentifikator | `FristShape::WerktagAt` |
| GPKE Neuanlage (55600/55601) | **00:00 Uhr des 61. WT nach dem ÜT** — der tägliche Prüflauf nach `E_0608` läuft 60 WT | `FristShape::WerktagAt` |
| GPKE Sperrung (17115/17117/39000) | **spätester ÜT ist der 1. WT nach dem ÜT** | `FristShape::WerktageAtCutoff` |
| GPKE Teil 4 Stammdaten-Rückmeldung | **2. WT nach dem ÜT**; die *Bestellung* 10 WT | `FristShape::WerktageAtCutoff` |
| WiM (Strom) | **3 / 5 / 7 / 1 Werktage** je PID | `FristShape::WerktageAtCutoff` |
| GeLi Gas | **Ablauf des 4. / 3. / 2. Werktags** je Prozess | `FristShape::EndOfWerktag` |

Alle vier Familien stehen in **einer** Tabelle, `mako_fristen::antwort` — `makod`
registriert daraus die Prozessfrist, `processd` bemisst die Operator-Queue,
`obsd` meldet die Verletzung. Eine PID ohne veröffentlichte Frist liefert `None`:
**unbekannt**, nie *unbefristet*.

> GPKE Teil 2 nennt jede Antwortfrist als Uhrzeit auf einem Werktag, nie als
> Dauer: eine Freitagnachmittag eingegangene Nachricht ist bis Montag früh zu
> beantworten, eine am Dienstagabend eingegangene hat keine sechzehn Stunden. Die
> 10-Werktage-Zahl bei GeLi Gas ist die **Vorlauffrist des Lieferanten**, nicht
> die Antwortfrist des Netzbetreibers.

> **Werktag rule:** Saturdays, Sundays and public holidays are not Werktage (GPKE Teil 1). 24.12. and 31.12. count as holidays.

> **Redispatch 2.0 deadlines are separate** — they use UTC wall-clock hours, not Werktage:
> 6 h (`AcknowledgementDocument`), 24 h (`StatusRequest`), and 5 min (Activation response).
> The full deadline table is in the [PID reference](@/docs/regulatory/pid-reference.md#redispatch-2-0-xml-document-types-not-edifact-pids).

---

## Domain Boundary Summary

| Domain | BK | Crate | INVOIC billing? |
|---|---|---|---|
| GPKE Lieferantenwechsel + MMM Strom | BK6 | `mako-gpke` | PIDs 31001–31002, 31004–31006 ✅ |
| MaBiS Bilanzkreisabrechnung | BK6 | `mako-mabis` | PID 13003 (MSCONS) |
| WiM Messwesen / MSB | BK6 | `mako-wim` | PIDs 31003, 31009 ✅ |
| GeLi Gas Lieferantenwechsel + AWH Sperrprozesse | BK7 | `mako-geli-gas` | PID 31011 (Rechnung sonstige Leistung, NB → LF) ✅ |
| GaBi Gas Bilanzierung / MMM Gas | BK7 | `mako-gabi-gas` | PIDs 31007–31008 (Aggreg. MMM, NB → MGV) + PID 31010 (Kapazitätsrechnung) ✅ |
| Netzbetreiberwechsel Strom | BK6 | `mako-nbw` | ❌ PARTIN only |

> INVOIC 31011 (Rechnung sonstige Leistung, AWH Sperrprozesse Gas) is billed by the GNB/VNB
> to the LFN/LFA for AWH performed during the Sperrprozess — it belongs to GeLi Gas (BK7-24-01-009),
> not GaBi Gas. INVOIC 31010 (Kapazitätsrechnung, NB → BKV) is GaBi Gas (BK7-24-01-008).
