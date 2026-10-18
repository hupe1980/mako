+++
title = "BNetzA Regulatory Reference"
description = "Complete BNetzA ruling index for German energy market communication: BK6 GPKE/WiM/MaBiS, BK7 GeLi Gas, current rulings, and Fristen."
weight = 10
+++
The Bundesnetzagentur rulings that govern German market communication (MaKo),
from the official BNetzA pages as of 2026-07-23; Mitteilung Nr. 58, the MiSpeL
Festlegung and KoV XV as of 2026-10-05.

Sources:
- BK6 Netzzugang/Messwesen: <https://www.bundesnetzagentur.de/DE/Beschlusskammern/BK06/BK6_83_Zug_Mess/NetzZ.html>
- BK7 Erdgas / Lieferantenwechsel: <https://www.bundesnetzagentur.de/DE/Beschlusskammern/BK07/BK7_04_Erdgas/BK7_45_LieferW_Messw/BK7_LieferantenW_Messw.html>

---

## Beschlusskammer 6 — Electricity (Strom)

### GPKE — Geschäftsprozesse zur Kundenbelieferung mit Elektrizität

**Page:** <https://www.bundesnetzagentur.de/DE/Beschlusskammern/BK06/BK6_83_Zug_Mess/831_gpke/gpke_node.html>

**Current ruling:**
- **BK6-24-174** (Beschluss 24.10.2024) — GPKE Teil 1–3 (Anlagen 1a–1c), MaBiS
  (Anlage 3) and, through Tenorziffer 2, the amendments to **WiM** Teil 1 and
  Teil 2 (Anlagen 2a/2b) — see [WiM](#wim-wechselprozesse-im-messwesen) below.
  WiM keeps its own Aktenzeichen, BK6-09-034.
- **BK6-22-024** (Beschluss 21.03.2024) — **LFW24-Festlegung** („beschleunigter werktäglicher
  Lieferantenwechsel in 24 Stunden", statutory anchor **§20a EnWG**): re-issued GPKE Teil 2
  und Teil 4, which carry the MPES processes.
- **Gültig seit: 06.06.2025** (Mitteilung Nr. 4 vom 06.12.2024)

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
- Sperrauftrag / Entsperrauftrag Strom (ORDERS, PIDs 17115–17117; Stornierung ORDCHG 39000/39001)
- Anfrage Daten der individuellen Bestellung (UTILMD, PID 55555) — GPKE Teil 4 data request
- Konfigurationseinrichtung Rollenzuordnung MSB (ORDERS/ORDRSP, PIDs 17134–17135, 19001–19002) — via BK6-22-024 GPKE Teil 4
- Abschlagsrechnung / NN-Rechnung Netz (INVOIC, PIDs 31001–31002)
- **Mehr-/Mindermengen Strom** (INVOIC, PIDs 31005–31006) — see Mitteilung Nr. 72 below
- **Mehr-/Mindermengen Gas** (INVOIC, PIDs 31007–31008; NB → MGV, Gas-only) — belongs to GaBi Gas (`mako-gabi-gas`)

**APERAK Frist (GPKE):** **45 Minuten** für UTILMD und ORDERS; ein
Samstagseingang bis Sonntag 12:00 Uhr; alles übrige bis 12:00 Uhr des nächsten
Werktags (APERAK AHB 1.1 § 2.4.1). Das ist nicht die Antwortfrist des
Geschäftsprozesses — siehe [unten](#antwortfristen-summary).

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

> Mitteilung Nr. 72 concerns the „Mehr-/Mindermengenpreis **Strom**" only:
> 31005–31006 are GPKE; 31007–31008 (NB → MGV) are GaBi Gas
> (`mako-gabi-gas` `gabi-gas-invoic`).

---

### MPES — Marktprozesse für erzeugende Marktlokationen (Strom)

The MPES processes are part of **GPKE Teil 2** (BK6-22-024); the standalone
MPES Festlegung BK6-20-160 was gültig bis 05.06.2025. The erzeugende-MaLo PIDs
**55077/55078/55080** and **55601** are GPKE PIDs.

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

> **MaBiS-Hub:** no Beschluss. The BK6-24-210-1 consultation took Stellungnahmen
> until 17.11.2025. Its Anwendungsfälle are to be **EDIFACT**, not new API-Webdienste:
> „Bestehende Anwendungsfälle bleiben in EDIFACT und neue Anwendungsfälle aus der
> ausstehenden Festlegung ‚MaBiS-Hub' werden ebenfalls in EDIFACT ausgeprägt werden"
> ([Mitteilung Nr. 58](https://www.bundesnetzagentur.de/DE/Beschlusskammern/BK06/BK6_83_Zug_Mess/835_mitteilungen_datenformate/Mitteilung_58/Mitteilung_Nr_58.html), 01.10.2026, „Weitere Ergebnisse").

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
- **BK6-09-034** (Beschluss 09.09.2010) is the WiM Festlegung and keeps its own
  Aktenzeichen. Its Anlage 1 — Teil 1 „Fokus Basis-Prozesse" und Teil 2 „Fokus
  Übermittlung von Werten" — is amended by **BK6-24-174 Tenorziffer 2** through
  that Beschluss's Anlagen 2a/2b. Cite the Teil and chapter, and BK6-24-174 for
  the current text.

**Process documents:**

| Document | Content |
|---|---|
| Anlage 2a, WiM Teil 1 | Fokus Basis Prozesse (Lesefassung) |
| Anlage 2b, WiM Teil 2 | Fokus Übermittlung von Werten (Lesefassung) |

**Scope:**
- Messstellenbetreiberwechsel (UTILMD, PIDs 55039, 55042, 55051, 55168)
  — 55039 Kündigung MSB, 55042 Anmeldung MSB, 55051 Ende MSB, 55168 Verpflichtungsanfrage;
    11001–11003 are not in the current AHB
- Geräteübernahme ORDERS (PIDs 17001, 17002, 17009)
- Stornierung der Bestellung (ORDCHG, PID 39002, ESA → MSB, WiM Teil 2)
- WiM-Rechnung (INVOIC, PID 31003) — Abrechnung von Dienstleistungen im Messwesen, beide Sparten
- Stornorechnung (INVOIC, PID 31004, Sparte-neutral; hosted by `wim-invoic`)
- MSB-Rechnung (INVOIC, PID 31009) — Messstellenbetrieb an NB, LF oder ESA

**Fristen (WiM Strom):** die fachliche Antwort ist **je Prozess** befristet — Kündigung (55039) **3 WT**, Anmeldung (55042) **5 WT**, Abmeldung (55051) **7 WT**, Verpflichtungsanfrage (55168) **1 WT** (BK6-24-174 Anlage 2a, Kap. 2.2.2 / 2.3.2 /
2.4.2 / **2.4.2 Nr. 4** — the Verpflichtungsanfrage is Prozessschritt 3 of the
*Ende Messstellenbetrieb*, not of Kap. 2.5 „Verpflichtung gMSB"). Samstage, Sonntage und gesetzliche Feiertage sind keine Werktage.

> Davon zu unterscheiden ist die **APERAK**-Eingangsbestätigung: für UTILMD Strom **45 Minuten** (APERAK AHB 1.1 § 2.4.1).

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

**Current ruling — and what it repealed:**

| Az. | Topic | Status |
|---|---|---|
| **BK6-23-241** | BilAReM — Fortentwicklung der Bilanzierung von Redispatch-Maßnahmen (Beschluss 07.05.2026) | in force, staged; 2026-07-01 for the ÜNB |
| **BK6-20-059** | Datenformate und Übermittlungswege | TZ 1 **repealed** by BK6-23-241 TZ 1 with the end of 30.06.2026; TZ 2 survives until the new EDI@Energy documents apply |
| **BK6-20-060** | Netzbetreiberkoordinierung — `Stammdaten` forwarding (1 Werktag), Activation (5 min) | **repealed** by BK6-23-241 TZ 4 |
| **BK6-20-061** | Informationsbereitstellung — `Kostenblatt` (15th of the following month) | **repealed** by BK6-23-241 TZ 3 |

BK6-23-241 TZ 7 tells the ÜNB to develop bundesweit einheitliche
Prozessbeschreibungen; until they are published, the windows above are operator
configuration, not a Festlegung (`crates/mako-redispatch/src/fristen.rs`).

The one published Redispatch deadline is in the format documentation: the
`AcknowledgementDocument` is due „unverzüglich, jedoch spätestens **3 Minuten**
nach Erhalt der Übertragungsdatei" (EDI@Energy `AcknowledgementDocument`
Formatbeschreibung 1.0g, Stand 01.10.2025).

**Next binding documents (01.04.2027):** [Mitteilung Nr. 58](https://www.bundesnetzagentur.de/DE/Beschlusskammern/BK06/BK6_83_Zug_Mess/835_mitteilungen_datenformate/Mitteilung_58/Mitteilung_Nr_58.html)
(01.10.2026) publishes **ActivationDocument AWT 1.1g** — „lediglich ein minimaler
Anpassungsbedarf aufgrund einer veränderten Anforderung im Bahnstrom-Umfeld" —
and **Regelungen zum Übertragungsweg 1.11**, whose changes „betreffen
insbesondere die Zertifikatsnutzung für den Datenaustausch mittels mTLS". Both
are „verbindlich ab dem 01.04.2027"; AWT 1.1f applies before that date.

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

### MiSpeL — Marktintegration von Speichern und Ladepunkten (EEG-Aufsicht)

**Ruling:** Festlegung **Az. 618-25-02**, entschieden am **01.10.2026**
([Tenor mit Begründung](https://www.bundesnetzagentur.de/DE/Fachthemen/ElektrizitaetundGas/ErneuerbareEnergien/EEG_Aufsicht/MiSpeL/DL/MiSpeL_TenorMitBegruendung.pdf?__blob=publicationFile)).

- **Wirksam mit der Bekanntgabe** (Tenorziffer 8, S. 2); published on the
  MiSpeL page on 01.10.2026 and in the Amtsblatt der Bundesnetzagentur on
  07.10.2026 (Begründung, S. 7).
- **Übergang bis 30.09.2027:** the Abgrenzungs- and Pauschaloption apply only
  „im Einverständnis mit dem jeweiligen Netzbetreiber und Messstellenbetreiber"
  (Tenorziffer 9 a, S. 3). The Pauschaloption (Tenorziffern 2 and 4) applies at
  the earliest from the month after the Commission's state-aid approval of
  § 19 Abs. 3c EEG (Tenorziffer 9 b).
- **NB and MSB must enable** the Umlageprivilegien, the Marktprämienzahlungen and
  the Zuordnungen (Tenorziffer 7). The Marktkommunikation for it is BK6's to
  set, not part of this Festlegung (Begründung 3.8, S. 92).

**In mako:** `einsd` records whether a plant claims under the § 19 Abs. 3b/3c
EEG Abgrenzungs- or Pauschaloption, and `eeg-billing::marktwertserie` moves such
a plant onto the Jahresmarktwert (Anlage 1 Nr. 2 Satz 3 EEG).

---

## Beschlusskammer 7 — Gas (Erdgas)

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

**APERAK Frist (Gas):** Gas kennt nur die **Verarbeitbarkeitsfehlermeldung** —
**nächster Werktag 12:00 Uhr** bei einem Folgeprozess, **3 Werktage** bei einem
Initialprozess (APERAK AHB 1.1 § 2.3.1); auf jede APERAK folgt eine CONTRL.
Samstag, Sonntag und gesetzliche Feiertage zählen nicht.

**Antwortfristen (GeLi Gas 3.0 Kap. 3.1–3.3):** Anmeldung **4 WT**, Abmeldung /
Kündigung / Abmeldeanfrage **3 WT**, Ersatz-/Grundversorgung **2 WT** — jeweils
bis zum *Ablauf* des Werktags nach Eingang. Die bekannten **10 Werktage** sind
die **Vorlauffrist des Lieferanten** vor Aufnahme der Belieferung (Kap. 3.2.3),
keine Antwortfrist; die Abmeldung hat dafür **7 WT** (Kap. 3.2.2).

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
- **Kooperationsvereinbarung Gas (KoV XV)** — Änderungsfassung vom 27.03.2026,
  **in Kraft seit 01.10.2026** (KoV XV Haupttext, Deckblatt: „Inkrafttreten am
  1. Oktober 2026"). Its Anlage 8 is the Messstellenbetreiberrahmenvertrag Gas
  that BK7-24-01-009 Tenorziffer 14 required by 01.10.2026 (BDEW Energie-Info
  zur KoV XV, Kap. 3.8); the gas allocation deadlines are §§ 46/47

**Scope:**
- Bilanzierung in Gasbilanzkreisen (Allokation, Nominierung)
- Regelenergie Gas
- Mehr-/Mindermengenbilanzierung Gas
- Konvertierung im qualitätsübergreifenden Gasmarktgebiet
- INVOIC Gas billing: Kapazitätsrechnung (PID 31010)
- DVGW message types: ALOCAT, NOMINT, NOMRES, SSQNOT

---

## Mitteilungen zu den Datenformaten (BK6 + BK7 gemeinsam)

**Page:** <https://www.bundesnetzagentur.de/DE/Beschlusskammern/BK06/BK6_83_Zug_Mess/835_mitteilungen_datenformate/Datenformate-node.html>

| Nr. | Gegenstand | Datum |
|---|---|---|
| [58](https://www.bundesnetzagentur.de/DE/Beschlusskammern/BK06/BK6_83_Zug_Mess/835_mitteilungen_datenformate/Mitteilung_58/Mitteilung_Nr_58.html) | Datenformate verbindlich zum **01.04.2027**: nur ActivationDocument AWT 1.1g und Regelungen zum Übertragungsweg 1.11. Keine neuen EDIFACT-, API-Webdienste- oder EBD-Dokumente. API-Webdienste bleiben auf MaLo-Ident und Steuern beschränkt; die neu konsultierten APIs (Lokationsbündel, Berechnungsformel) werden zurückgezogen, UTILTS bleibt; MaBiS-Hub-Anwendungsfälle in EDIFACT | 01.10.2026 |
| 56 | Finale Datenformate, verbindlich zum **01.10.2026**: UTILMD Strom 2.2, UTILMD Gas 1.2, MSCONS 3.2, ORDERS/ORDRSP 1.1b, EBD 4.3; neues Attribut „fernsteuerbar" (§10b EEG) auf der TR | 01.04.2026 |
| 55 | Konsultation der Datenformate zum **01.10.2026** | 02.02.2026 |
| 54 | Datenformate verbindlich zum **01.04.2026** | 01.10.2025 |
| 53 | Konsultation Konzept API-Webdienste | 01.09.2025 |
| 51 | Datenformate verbindlich zum **01.10.2025** | — |
| 50 | Aussetzung der Pflicht zur Content-Verschlüsselung (Gas) | 26.03.2025 |

---

## Antwortfristen summary

| Process family | Frist | Shape |
|---|---|---|
| GPKE (Strom) | **11:00 / 06:00 / 05:00 / 09:00 Uhr des 1. WT nach dem ÜT**, je Prüfidentifikator | `FristShape::WerktagAt` |
| GPKE Neuanlage (55600/55601) | **00:00 Uhr des 61. WT nach dem ÜT** — der tägliche Prüflauf nach `E_0608` läuft 60 WT | `FristShape::WerktagAt` |
| GPKE Sperrung (17115/17117/39000) | **spätester ÜT ist der 1. WT nach dem ÜT** | `FristShape::EndOfWerktag` |
| GPKE Teil 4 Stammdaten-Rückmeldung | **2. WT nach dem ÜT**; die *Bestellung* 10 WT | `FristShape::EndOfWerktag` |
| WiM (Strom) | **3 / 5 / 7 / 1 Werktage** je PID | `FristShape::EndOfWerktag` |
| GeLi Gas | **Ablauf des 4. / 3. / 2. Werktags** je Prozess | `FristShape::EndOfWerktag` |

Alle Zeilen stehen in **einer** Tabelle, `mako_fristen::antwort` — `makod`
registriert daraus die Prozessfrist, `processd` bemisst die Operator-Queue
und meldet die Verletzung. Eine PID ohne veröffentlichte Frist liefert `None`:
**unbekannt**, nie *unbefristet*.

> **Werktag rule:** Saturdays, Sundays and public holidays are not Werktage (GPKE Teil 1). 24.12. and 31.12. count as holidays.

> **Redispatch 2.0 deadlines are separate** and run in wall-clock time, not
> Werktage: the `AcknowledgementDocument` is due within **3 Minuten**
> (Formatbeschreibung 1.0g); the `StatusRequest` and Activation windows are
> operator configuration (see [Redispatch 2.0](#redispatch-2-0-koordination-von-einspeisungen)).
> The full table is in the [PID reference](@/docs/regulatory/pid-reference.md#redispatch-2-0-xml-document-types-not-edifact-pids).

---

## Domain Boundary Summary

| Domain | BK | Crate | INVOIC billing? |
|---|---|---|---|
| GPKE Lieferantenwechsel + MMM Strom | BK6 | `mako-gpke` | PIDs 31001–31002, 31005–31006 ✅ |
| MaBiS Bilanzkreisabrechnung | BK6 | `mako-mabis` | PID 13003 (MSCONS) |
| WiM Messwesen / MSB | BK6 | `mako-wim` | PIDs 31003, 31004 (Storno), 31009 ✅ |
| GeLi Gas Lieferantenwechsel + AWH Sperrprozesse | BK7 | `mako-geli-gas` | PID 31011 (Rechnung sonstige Leistung, NB → LF) ✅ |
| GaBi Gas Bilanzierung / MMM Gas | BK7 | `mako-gabi-gas` | PIDs 31007–31008 (Aggreg. MMM, NB → MGV) + PID 31010 (Kapazitätsrechnung) ✅ |

> INVOIC 31011 is billed by the GNB/VNB to the LFN/LFA for AWH during the
> Sperrprozess (GeLi Gas, BK7-24-01-009); INVOIC 31010 (NB → KN — INVOIC AHB 1.0b
> Kap. 3.1.4) is GaBi Gas (BK7-24-01-008).
