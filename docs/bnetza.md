---
layout: default
title: BNetzA Regulatory Reference
nav_order: 40
parent: Regulatory
description: >
  Complete BNetzA ruling index for German energy market communication:
  BK6 GPKE/WiM/MaBiS, BK7 GeLi Gas, current rulings, Fristen, and
  MPES dissolution timeline.
---

# BNetzA Regulatory Reference

Reference document for Bundesnetzagentur rulings that govern German energy market
communication (MaKo). Extracted from official BNetzA pages as of 2026-06-28.

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
- **BK6-22-024** (Beschluss 21.03.2024) — GPKE Teil 4 (Stammdatenprozesse); Mitteilung Nr. 4
  vom 06.12.2024 konkretisiert Fristen
- **Gültig seit: 06.06.2025**

**Process documents (Lesefassungen):**
| Document | Content |
|---|---|
| Anlage 1a, GPKE Teil 1 | Einführende Prozessbeschreibung |
| Anlage 1b, GPKE Teil 2 | Fokus Zuordnungsprozesse |
| Anlage 1c, GPKE Teil 3 | (Lesefassung) |
| Anlage 1d, GPKE Teil 4 | Fokus Stammdatenprozesse (via BK6-22-024) |

**Scope — electricity market only:**
- Lieferantenwechsel Strom (UTILMD E, PIDs 55001–55006, 55017–55018)
  — 55001/55002 Anfrage Lieferbeginn/Lieferende; 55003–55006 Bestätigung/Ablehnung;
    55017 Kündigung Lieferbeginn; 55018 Bestätigung Kündigung
- Sperrauftrag / Entsperrauftrag Strom (ORDERS, PIDs 17115–17117)
- Anfrage Daten der individuellen Bestellung (UTILMD, PID 55555) — GPKE Teil 4 data request
- Einspeisestelle ex-MPES (UTILMD E, PIDs 56001–56004) — transferred from MPES per BK6-22-024 (LFW24), effective 06.06.2025
- Konfigurationseinrichtung Rollenzuordnung MSB (ORDERS/ORDRSP, PIDs 17134–17135, 19001–19002) — via BK6-22-024 GPKE Teil 4
- Abschlagsrechnung / NN-Rechnung Netz (INVOIC, PIDs 31001–31002)
- Stornorechnung Netz (INVOIC, PID 31004)
- **Mehr-/Mindermengen Strom** (INVOIC, PIDs 31005–31008) — see Mitteilung Nr. 72 below

**APERAK Frist (GPKE):** **24 Stunden** (wall-clock hours) — festgelegt in BK6-22-024

**Laufende Verfahren:**
| Az. | Gegenstand | Eröffnet |
|---|---|---|
| BK6-24-210 | Festlegungsverfahren MaBiS-Hub (Aggregation und Abrechnung bilanzierungsrelevanter Daten) | 02.10.2024 |

**Selected Mitteilungen:**
| Nr. | Gegenstand | Datum |
|---|---|---|
| 72 | Empfehlung zur Anwendung der BDEW-Anwendungshilfe „Ermittlung des Mehr-/Mindermengenpreises **Strom**" | 05.02.2026 |
| 71 | Empfehlung zur Anwendung „Marktprozesse Netzbetreiberwechsel Strom" | 01.07.2024 |
| 66 | Empfehlung zur Anwendung „Prozesse zur Ermittlung und Abrechnung von Mehr-/Mindermengen Strom und Gas" (superseded by Mitteilung 72 for Strom) | 27.01.2020 |
| 46 | Prozesse zur Ermittlung der Abrechnung von Mehr-/Mindermengen Strom und Gas (nicht mehr aktuell, s. Mitteilung 66) | 22.01.2015 |
| 4 | Einführung Änderungsmanagement, Umsetzung INVOIC/REMADV, GPKE-Auslegungsgrundsätze | 28.11.2007 |

> **Key boundary:** Mitteilung Nr. 72 (05.02.2026) explicitly refers to „Mehr-/Mindermengenpreises **Strom**"
> only, confirming that PIDs 31005–31008 are electricity-only GPKE processes.
> Gas MMM billing is not part of GPKE.

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
| BK6-24-210 | Festlegungsverfahren MaBiS-Hub | 02.10.2024 |

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
- **BK6-24-174** (24.10.2024); Mitteilung Nr. 4 vom 06.12.2024
- **Gültig seit: 06.06.2025**

**Process documents:**
| Document | Content |
|---|---|
| Anlage 2a, WiM Teil 1 | Fokus Basis Prozesse (Lesefassung) |
| Anlage 2b, WiM Teil 2 | Fokus Übermittlung von Werten (Lesefassung) |

**Scope:**
- Gerätewechsel / Messstellenbetreiberwechsel (UTILMD, PIDs 11001–11003)
  — 11001 Anmeldung (nMSB → NB), 11002 Abmeldung (NB → aMSB), 11003 Stammdatenänderung;
    range 11001–11099 reserved by BK6-24-174 but only 11001–11003 are defined in the current AHB
- Geräteübernahme ORDERS (PIDs 17001–17002, 17005, 17009–17011)
- Stammdaten ORDERS (PIDs 17101–17135; 17101 inbound Anforderung, 17102–17135 outbound Übermittlung)
- Stornierung ORDCHG (PID 39000; 39001–39002 outbound responses)
- WiM-Rechnung (INVOIC, PID 31003) — MSB billing
- MSB-Rechnung (INVOIC, PID 31009)

**APERAK Frist (WiM):** **5 Werktage** (Samstag zählt als Werktag; Sonntag und gesetzliche Feiertage nicht)

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

## Beschlusskammer 7 — Gas (Erdgas)

BK7 is responsible for gas network access, metering, balancing, and market communication.
GeLi Gas (Lieferantenwechsel Gas) is regulated under BK7. GaBi Gas (balancing) is also BK7.

### GeLi Gas — Geschäftsprozesse Lieferantenwechsel Gas

**Page:** <https://www.bundesnetzagentur.de/DE/Beschlusskammern/BK07/BK7_04_Erdgas/BK7_45_LieferW_Messw/451_LieferW/BK7_LieferantenW.html>

**Current ruling:**
- **GeLi Gas 3.0** — **BK7-24-01-009** (Beschluss 12.09.2025, abgeschlossen 24.09.2025)
  - [Beschluss PDF](https://www.bundesnetzagentur.de/DE/Beschlusskammern/1_GZ/BK7-GZ/2024/BK7-24-0009/Anlagen/BK7-24-01-0009_Beschluss_Download_BF.pdf)
  - [Anlage PDF](https://www.bundesnetzagentur.de/DE/Beschlusskammern/1_GZ/BK7-GZ/2024/BK7-24-0009/Anlagen/BK7-24-01-0009_Anlage_Download_BF.pdf)

**Previous rulings:**
| Az. | Gegenstand | Datum |
|---|---|---|
| BK7-19-001 | Anpassung GeLi Gas inkl. Messstellenbetreiberrahmenvertrag | 22.11.2023 |
| BK7-16-142 | Anpassung an Erfordernisse Digitalisierung der Energiewende | 20.12.2016 |
| BK7-11-075 | Anpassung „GeLi Gas" | 28.10.2011 |
| BK7-06-067 | Festlegung einheitlicher Geschäftsprozesse und Datenformate „GeLi Gas" (Ursprungsfestlegung) | 20.08.2007 |

**Scope — gas supplier switching only:**
- Lieferbeginn Gas / Lieferende Gas (UTILMD G, PIDs 44001–44006, 44017–44018)
  — 44001–44002 Anmeldung/Abmeldung (LFN → GNB); 44003–44006 Bestätigung/Ablehnung;
    44017–44018 Kündigung Lieferbeginn (LFN ↔ LFA); PIDs 44007–44016 do not exist
- Anweisung Sperrung Gas (UTILMD G, PID 44555)
- APERAK / CONTRL acknowledgements
- **Does NOT cover** INVOIC billing or Mehr-/Mindermengen Gas — these belong to GaBi Gas (BK7 Bilanzierung)

**APERAK Frist (GeLi Gas):** **10 Werktage** (Samstag zählt als Werktag; Sonntag und gesetzliche Feiertage nicht)

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

**Scope:**
- Bilanzierung in Gasbilanzkreisen (Allokation, Nominierung)
- Regelenergie Gas
- Mehr-/Mindermengenbilanzierung Gas
- Konvertierung im qualitätsübergreifenden Gasmarktgebiet
- INVOIC Gas billing: Kapazitätsrechnung (PID 31010), Rechnung sonstige Leistung (PID 31011)
- DVGW message types: ALLOCAT, NOMINT, NOMRES

---

## APERAK Fristen Summary

| Process family | Crate | Frist | Calculation |
|---|---|---|---|
| GPKE (Strom) | `mako-gpke` | **24 Stunden** (wall-clock) | `fristen::add_hours(t, 24)` |
| WiM (Strom) | `mako-wim` | **5 Werktage** | `fristen::add_werktage(d, 5, BdewMaKo)` |
| GeLi Gas | `mako-geli-gas` | **10 Werktage** | `fristen::add_werktage(d, 10, BdewMaKo)` |

> **Werktag rule:** Saturday counts as a Werktag. Sunday and public holidays do not.

---

## Domain Boundary Summary

| Domain | BK | Crate | INVOIC billing? |
|---|---|---|---|
| GPKE Lieferantenwechsel + MMM Strom | BK6 | `mako-gpke` | PIDs 31001–31002, 31004–31008 ✅ |
| MaBiS Bilanzkreisabrechnung | BK6 | `mako-mabis` | PID 13003 (MSCONS) |
| WiM Messwesen / MSB | BK6 | `mako-wim` | PIDs 31003, 31009 ✅ |
| GeLi Gas Lieferantenwechsel | BK7 | `mako-geli-gas` | ❌ No INVOIC |
| GaBi Gas Bilanzierung / MMM Gas | BK7 | `mako-gabi-gas` | PIDs 31010–31011 (placeholder) |
| Netzbetreiberwechsel Strom | BK6 | `mako-nbw` | ❌ PARTIN only |

> Gas Mehr-/Mindermengen billing (PIDs 31010–31011) is NOT part of GeLi Gas.
> It falls under BK7 Bilanzierung (GaBi Gas domain), governed separately from the
> supplier-switch process.
