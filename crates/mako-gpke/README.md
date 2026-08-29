# mako-gpke

**GPKE — Geschäftsprozesse zur Kundenbelieferung mit Elektrizität**

Process engine workflows for the German electricity market supplier-switch
and grid access billing processes. Implements the BDEW GPKE specification
and BNetzA rulings:
- **BK6-24-174** (Beschluss 24.10.2024, gültig seit 06.06.2025) — GPKE Teil 1–3 (Lieferantenwechsel, Zuordnungsprozesse)
- **BK6-22-024** (Beschluss 21.03.2024) — GPKE Teil 4 (Stammdatenprozesse, Konfigurationseinrichtung)

## Fristen — two clocks

**APERAK** (the transport acknowledgement): **45 Minuten** on a Werktag for
UTILMD and ORDERS; a Saturday arrival is due Sunday 12:00 Berlin, everything else
12:00 of the next Werktag (APERAK AHB 1.1 § 2.4.1).

**The business answer** is per Prüfidentifikator and comes from
`mako_fristen::antwort` — 11:00 / 06:00 / 05:00 / 09:00 Uhr des 1. WT nach dem ÜT
for the GPKE Teil 2 core processes, 00:00 Uhr des 61. WT for a Neuanlage, the
1. WT for a Sperr-/Entsperrauftrag, 2 WT for a Teil-4 Stammdaten-Rückmeldung.

GPKE Teil 2 states every window as a wall-clock instant on a Werktag, never as a
duration: a message arriving Friday afternoon is answerable until Monday morning,
one arriving Tuesday evening has under sixteen hours.

### There is no 24-hour message window

LFW24 (**BK6-22-024**) implements § 20a EnWG, which requires the *supplier switch
itself* to complete within 24 hours. GPKE does not express that as a per-message
deadline: it chains wall-clock instants on the first Werktag after the ÜT —
07:00, 09:00, 11:00, 12:00 — so the whole sequence fits inside the statutory
duration. **GPKE Teil 1 Kap. 7 („Fristenberechnung") defines WT, T,
Zuordnungsbeginn, ÜT and ÜZ and contains no 24-hour Frist at all.**

Reading the statutory 24 hours as a message window is wrong in both directions:
it expires a Friday-afternoon Anmeldung on Saturday, and it reports a
Tuesday-11:00 Frist as still running until Tuesday night. Every window in this
crate therefore comes from `mako_fristen::antwort`, never from a literal, and
`services/makod/tests/deadline_labels.rs` pins the registration sites to the
constants the workflows match.

### The Anmeldung is two trees, and on an assigned Marktlokation two phases

`E_0622` „Prüfen, ob Anmeldung direkt ablehnbar" is a **Vorprüfung**: every code
it publishes is an Ablehnung, and surviving it means only that the Anmeldung is
not *directly* refusable. What the NB answers comes from **`E_0623`**, and that
tree reads a fact the message does not carry — the incumbent LFA's answer to an
Anfrage zur Beendigung der Zuordnung the NB has to send first.

GPKE Teil 2 § 2.1.2 SD Lieferbeginn Nr. 1 **Prüfschritt 4** is the branch: „Ist
die Marktlokation bzw. Tranche zum Zuordnungsbeginn einem LF zugeordnet, fährt
der NB mit Prozessschritt 2 fort, ansonsten mit Prozessschritt 5." So an
unassigned Marktlokation is confirmed in one pass and an assigned one is not.

| Nr. | Message | Spätester ÜZ |
|---|---|---|
| 2 | 55036 Information über existierende Zuordnung → LFN | 07:00 Uhr des 1. WT |
| 3 | **55010 Anfrage zur Beendigung der Zuordnung → LFA** | parallel zu Nr. 2 |
| 4 | 55011 / 55012 Antwort des LFA (`E_0624`) | **09:00 Uhr des 1. WT** |
| 5/6 | 55002 / 55003 → LFN | 11:00 Uhr des 1. WT |
| 10 | 55037 Beendigung der Zuordnung → LFA | 12:00 Uhr des 1. WT |

`gpke-beendigung-zuordnung` runs **both ends**, told apart by which command opens
the process: `Anfragen` / `ReceiveAntwort` on the NB side, `ReceiveAnfrage` /
`SendAntwort` on the LFA's.

**Silence is a result, not a timeout.** „Verstreicht die Frist, ohne dass eine
Antwort beim NB eingeht, gilt dies als Bestätigung nach Fall a). Nach Ablauf der
Frist eingehende Antworten sind für den Fortlauf dieses Prozesses unerheblich."
The NB's window on the LFA therefore **completes** the Anfrage, which is why it
has its own deadline label and its own command — routing it through the ordinary
`TimeoutExpired` would reject a Lieferantenwechsel the Festlegung confirms.

`mako_pruefung::evaluate_lieferbeginn` walks `E_0623`. Four of its eight outcomes
are refusals: `A50` / `A57` (the LFA widersprochen, and not with the „bereits
abgemeldet" code `A30` / `A41`) and `A53` / `A54` (Geschäftsvorfall 3 — not
enough percentage came free). Gas states the same rule as a flat code: `G_0011`
`Z35` „Ablehnung der Abmeldeanfrage".

**`A50` and `A57` oblige a second `SG4 STS`.** `STS+Z35` „Status der Antwort des
dritten Marktbeteiligten" restates the LFA's own `E_0624` code (Bedingungen
`[356]` / `[84]`) — Nr. 6's „der NB gibt zusätzlich den Grund der Ablehnung des
LFA an", on the wire. `makod` refuses to render the Ablehnung without it; the
55080 form additionally names which Tranche the restated answer is about, because
Geschäftsvorfall 3 has several LFA.

The Anfrage carries `SG12 NAD+Z09` „Kunde des LF" (Muss on `ZW4`/`ZAP`,
Bedingung `[279]`) — the Kundenname from the LFN's own Anmeldung (`[572]`),
which is how the LFA tells an Einzug from a Wechsel at `E_0624` Prüfschritt 30.
It is a **name**, so it rides `C080` with the DE 3045 Namensformat and not the
party-identification composite.

### Meldepflichten — obligations with no answer

Four messages the NB owes have **no Bestätigung**, so a missing one produces no
timeout and no alert:

| PID | Message | NB → | Frist |
|---|---|---|---|
| 55036 | Information über existierende Zuordnung — **die Identität des LFA** | LFN | 07:00 Uhr des 1. WT nach dem ÜT |
| 55037 | Beendigung der Zuordnung | LFA | 12:00 Uhr des 1. WT nach dem ÜT |
| 55038 | Aufhebung einer zukünftigen Zuordnung | LFZ | 12:00 Uhr des 1. WT nach dem ÜT |
| 55611 | Beendigung der Zuordnung des **MSB** zur MaLo / MeLo | MSB / MSBZ | 07:00 Uhr des 1. WT nach dem ÜT |

`gpke-zuordnungsmeldung` renders all four. **55611 is the odd one**: it belongs
to the SD „Lieferende von NB an LF" (§ 2.5.2 Nr. 11 / Nr. 13), which the NB opens
itself with a 55007 rather than answering an inbound Anmeldung — so it is
anchored on `MeldungAnchor::EigeneAnkuendigung`. It is also the only message here
that may name a **Messlokation** (`SG5 LOC+Z17`), because „der MSB ist
ausschließlich dem Objekt Messlokation zugeordnet", and the only one whose
`SG4 DTM` qualifier follows the **Grund** instead of the PID: `DTM+93` under
`ZC8`, `DTM+92` under `ZH1`.

The other three follow the Lieferbeginn. Whether one is *owed* is a
Versorgungsstatus question — GPKE Teil 2 § 2.1.2 Nr. 1 Prüfschritt 4 sends the
NB to Prozessschritt 2 only „Ist die Marktlokation bzw. Tranche zum
Zuordnungsbeginn einem LF zugeordnet" — so `processd` decides and issues
`gpke.zuordnung.informieren` / `.beenden` / `.aufheben`.

Because nothing waits for them, the guard is a test rather than an alert:
`mako_fristen::meldung` catalogues the windows and Fundstellen and
`services/makod/tests/meldepflicht_coverage.rs` cross-checks it against the PID
router.

Three wire facts the AHB fixes per PID, and the workflow refuses a message that
gets them wrong: 55036 is `BGM+E01` and carries **no** `SG4` date at all, while
55037 (`E02`) names a `DTM+93` Vertragsende and 55038 (`E02`) the originally
confirmed `DTM+92` Vertragsbeginn; the Gründe are disjoint (`Z26` · `ZC8`/`ZD9`/
`ZG6` · `ZG5`/`ZG9`/`ZH0`/`ZH1`); and `SG12 NAD+VY` names **every** Altlieferant
on a 55036 (Bedingung `[518]`, because Geschäftsvorfall 3 splits a Marktlokation
across Tranchen).

## PID Inventory

### UTILMD supplier-switch and feed-in processes (S2.1/S2.2)

> **Legend:** ✅ Implemented — full state machine, AHB-validated, production-safe.
> ↩ Derived — emitted by workflow as outbound ANTWORT, not routed as inbound.
> ❌ Removed — existed pre-LFW24; router rejects with CONTRL.

| PID   | Process name (AHB)                                    | Direction   | Status      |
|-------|-------------------------------------------------------|-------------|-------------|
| 55001 | Anmeldung verb. MaLo                                  | LF → NB     | ✅ Implemented |
| 55002 | Bestätigung Anmeldung verb. MaLo                      | NB → LF     | ↩ Derived from 55001 accept |
| 55003 | Ablehnung Anmeldung verb. MaLo                        | NB → LF     | ↩ Derived from 55001 reject |
| 55004 | Abmeldung                                             | LF → NB     | ✅ Implemented |
| 55005 | Bestätigung Abmeldung                                 | NB → LF     | ↩ Derived from 55004 accept |
| 55006 | Ablehnung Abmeldung                                   | NB → LF     | ↩ Derived from 55004 reject |
| 55007 | Abmeldung / Beendigung der Zuordnung                  | NB → LF     | ✅ Implemented (`gpke-lf-abmeldung`) |
| 55010 | Anfrage zur Beendigung der Zuordnung (NB Abmeldeanfrage) | NB → LFA | ✅ Implemented (`gpke-beendigung-zuordnung`) |
| 55011/55012 | Bestätigung / Ablehnung Beendigung der Zuordnung     | LFA → NB    | ↩ Derived from 55010 accept/reject |
| 55013 | Anmeldung / Zuordnung EOG (§36/§38 EnWG)              | NB → LF     | ✅ Implemented (`gpke-eog`, both roles) |
| 55014 | Bestätigung EOG Anmeldung                             | LF → NB     | ✅ Implemented (`gpke-eog`) |
| 55015 | Ablehnung EOG Anmeldung                               | LF → NB     | ✅ Implemented (`gpke-eog`) |
| 55016 | Kündigung Lieferbeginn                                | LFN → LFA   | ✅ Implemented |
| 55017/55018 | Bestätigung / Ablehnung Kündigung Lieferbeginn  | LFA → LFN   | ↩ Derived from 55016 accept/reject |
| 55555 | Anfrage Daten der individuellen Bestellung            | LFN → NB    | ✅ Implemented (GPKE Teil 4, BK6-22-024 Anlage 1d) |

### ORDERS/ORDRSP Konfigurationseinrichtung (GPKE Teil 4)

| PID   | Process name                                          | Direction   | Status         |
|-------|-------------------------------------------------------|-------------|----------------|
| 17134 | Einrichtung Konfiguration aufgrund Zuordnung LF (NB an MSB) | NB → MSB | ✅ Implemented |
| 17135 | Einrichtung Konfiguration aufgrund Zuordnung LF (MSB an MSB) | MSB → MSB | ✅ Implemented |
| 19001 | Bestellbestätigung (accept)                           | MSB → NB/MSB | ↩ Derived from 17134/17135 accept |
| 19002 | Ablehnung der Bestellung (reject)                     | MSB → NB/MSB | ↩ Derived from 17134/17135 reject |

### INVOIC billing processes (Netznutzungsabrechnung)

Implemented by `GpkeAbrechnungWorkflow`. Inbound INVOIC messages from the NB
spawn a new process; the `invoicd` daemon listens for
`de.mako.process.initiated` events and runs a plausibility check via
`invoic-checker`. It then calls `gpke.abrechnung.annehmen` (→ REMADV) or
`gpke.abrechnung.ablehnen` (→ COMDIS) on the Command API. Inbound REMADV and
COMDIS from the NB are handled via `ReceiveRemadv` and `ReceiveComdis` commands.

The state machine itself is not this crate's: all four INVOIC billing families
(GPKE, WiM, GaBi Gas, GeLi Gas) share `mako-invoic`, and this crate declares
only the family — its PID set, its deadline label, and which of the two roles
the deployment plays.

| PID   | Process name                                  | Status          |
|-------|-----------------------------------------------|-----------------|
| 31001 | Abschlagsrechnung (Netznutzung)               | ✅ Implemented  |
| 31002 | NN-Rechnung (Netznutzungsabrechnung)          | ✅ Implemented  |
| 31005 | MMM-Rechnung (Mehr-/Mindermengensaldo)        | ✅ Implemented  |
| 31006 | MMM-Rechnung (selbst ausgestellt)             | ✅ Implemented  |

> PIDs 31007/31008 (Aggreg. MMM-Rechnung Gas, NB → MGV) belong to `mako-gabi-gas` (BK7-24-01-008).
> PIDs 31003 (WiM-Rechnung) and 31009 (MSB-Rechnung) belong to the WiM domain.
> PID 31004 (Stornorechnung) is a Sparte-neutral, cross-process universal Storno (INVOIC AHB §3.1.2) — `invoicd` checks it Sparte-neutrally via `check_storno`, not a GPKE billing PID.

### ORDERS Sperrung Strom (GPKE Teil 4, BK6-22-024)

> The gas Sperrung equivalents of these PIDs (same PID numbers, different Sparte) belong
> to `mako-geli-gas`. Never mix Strom and Gas Sperrung in the same deployment module.

**Direction matters here and is easy to get wrong:** 17115 and 17117 travel
**LF → NB** — the Lieferant orders the grid operator to disconnect or reconnect.
17116 is the NB asking the MSB. Two workflows model the two ends.

| PID   | Process name                                     | Direction   | Workflow           | Status         |
|-------|--------------------------------------------------|-------------|--------------------|----------------|
| 17115 | Sperrauftrag                                     | LF → NB     | both               | ✅ Implemented |
| 17116 | Anfrage Sperrung (NB fragt MSB)                  | NB → MSB    | `gpke-sperrung`    | ✅ Implemented |
| 17117 | Entsperrauftrag                                  | LF → NB     | both               | ✅ Implemented |
| 39000 | Stornierung Sperr-/Entsperrauftrag (ORDCHG)      | LF → NB     | both               | ✅ Implemented |
| 19116/19117 | Bestätigung / Ablehnung (ORDRSP)           | NB → LF     | `gpke-sperrung-lf` | ✅ Implemented |
| 19128/19129 | Bestätigung / Ablehnung Stornierung        | NB → LF     | `gpke-sperrung-lf` | ✅ Implemented |
| 21039 | Auftragsstatus nach Ausführung (IFTSTA)          | NB → LF     | `gpke-sperrung-lf` | ✅ Implemented |

**ERP commands.** LF side: `gpke.sperrung.beauftragen` (17115),
`gpke.entsperrung.beauftragen` (17117), `gpke.sperrung.stornieren` (39000).
NB side: `gpke.sperrung.bestaetigen` / `gpke.sperrung.fehlgeschlagen` — both
dispatch IFTSTA 21039 and are issued automatically by `sperrd`.

### UTILMD Stornierung Zuordnungsprozess (GPKE Teil 1)

| PID   | Process name                                          | Direction            | Status         |
|-------|-------------------------------------------------------|----------------------|----------------|
| 55022 | Anfrage Stornierung Zuordnungsprozess                 | LFN/NB → NB/LFN      | ✅ Implemented |
| 55023 | Bestätigung Stornierung Zuordnungsprozess             | NB/LFN → orig.       | ↩ Derived      |
| 55024 | Ablehnung Stornierung Zuordnungsprozess               | NB/LFN → orig.       | ↩ Derived      |

### UTILMD Ankündigung / Zuordnung LF (GPKE Teil 1)

| PID   | Process name                                          | Direction    | Status         |
|-------|-------------------------------------------------------|--------------|----------------|
| 55607 | Ankündigung Zuordnung LF (NB → LFN)                   | NB → LFN     | ✅ Implemented |
| 55608 | Bestätigung Ankündigung Zuordnung LF (LFN → NB)       | LFN → NB     | ↩ Derived      |
| 55609 | Ablehnung Ankündigung Zuordnung LF (LFN → NB)         | LFN → NB     | ↩ Derived      |

### PARTIN Strom — Kommunikationsdaten (PARTIN AHB 1.0f)

| PID       | Process name                                             | Status         |
|-----------|----------------------------------------------------------|----------------|
| 37000     | Übermittlung Kommunikationsdaten Strom (Stammdaten)      | ✅ Implemented |
| 37001     | Bestätigung Übermittlung Kommunikationsdaten Strom       | ↩ Derived      |
| 37002     | Ablehnung Übermittlung Kommunikationsdaten Strom         | ↩ Derived      |
| 37003     | Übermittlung Kommunikationsdaten Strom (Korrekturen)     | ✅ Implemented |
| 37004     | Bestätigung Korrektur                                    | ↩ Derived      |
| 37005     | Ablehnung Korrektur                                      | ↩ Derived      |
| 37006     | Übermittlung Kommunikationsdaten — weiterer Typ          | ✅ Implemented |

> PIDs 37008–37014 (PARTIN Gas Kommunikationsdaten) belong to `mako-geli-gas`.

### UTILMD Neuanlage Marktlokation (GPKE Teil 1)

Workflow `gpke-neuanlage` handles Neuanlage requests where the MaLo does not yet
exist in the grid operator's system.

| PID   | Process name (AHB)                               | Direction  | Status         |
|-------|--------------------------------------------------|------------|----------------|
| 55600 | Anmeldung neue verbrauchende MaLo (LF → NB)     | LF → NB    | ✅ Implemented |
| 55601 | Anmeldung neue erzeugende MaLo (LF → NB)        | LF → NB    | ✅ Implemented |
| 55602 | Bestätigung Anmeldung neue verb. MaLo (NB → LF) | NB → LF    | ↩ Derived from 55600 accept |
| 55603 | Bestätigung Anmeldung neue erz. MaLo (NB → LF)  | NB → LF    | ↩ Derived from 55601 accept |
| 55604 | Ablehnung Anmeldung neue verb. MaLo (NB → LF)   | NB → LF    | ↩ Derived from 55600 reject |
| 55605 | Ablehnung Anmeldung neue erz. MaLo (NB → LF)    | NB → LF    | ↩ Derived from 55601 reject |

> Antwortfrist: 00:00 Uhr des 61. WT nach dem ÜT (GPKE Teil 2 § 2.2.2 — `E_0608`
> re-identifies daily for up to 60 Werktage). PIDs 55602–55605 are derived response
> PIDs; they are never routed inbound — the NB emits them outbound.

### UTILMD Lieferende von NB an LF (GPKE Teil 2 § 2.5.2)

Workflow `gpke-lf-abmeldung`: the NB announces that the supplier's assignment to
the Marktlokation ends, and the LF answers by 05:00 Uhr des 1. WT nach dem ÜT.
Not a Kündigung — the Kündigung is 55016, sent LFN → LFA without the NB.

| PID   | Process name (AHB)                                    | Direction  | Status         |
|-------|-------------------------------------------------------|------------|----------------|
| 55007 | Abmeldung / Beendigung der Zuordnung (NB an LF)       | NB → LF    | ✅ Implemented |
| 55008 | Bestätigung Abmeldung (LF an NB)                      | LF → NB    | ↩ Derived from the Antwortcode's Zustimmungs-Cluster |
| 55009 | Ablehnung Abmeldung (LF an NB)                        | LF → NB    | ↩ Derived from the Antwortcode's Ablehnungs-Cluster |

`E_0609` decides which. The three Transaktionsgründe a 55007 may carry are `Z33`
(Auszug wegen Stilllegung), `ZQ7` (fehlende Zuordnungsermächtigung nach
BKV-Deaktivierung) and `ZT0` (fehlende Zuordnungsermächtigung nach Änderung des
Zeitreihentyps); anything else has no path through the tree and escalates.

### MSCONS Messwerte Strom — Lieferant (GPKE Teil 2/4)

Workflow `gpke-messwerte` accepts inbound MSCONS messages carrying metered values
from the NB or MSB to the LF. These are read-only deliveries; no APERAK response
is required unless the message fails validation.

| PID   | Process name (AHB)                                        | Sender      |
|-------|-----------------------------------------------------------|-------------|
| 13005 | EEG-Überführungszeitreihe                                 | NB → LF     |
| 13006 | Stornierung von Messwerten                                | NB/MSB → LF |
| 13015 | Arbeit Leistungsmax. Kalenderj. vor Lieferbeginn          | NB → LF     |
| 13016 | Energiemenge u. Leistungsmax. Strom                       | NB/MSB → LF |
| 13017 | Zählerstand (Strom)                                       | MSB → LF    |
| 13018 | Lastgang Messlokation, Netzkoppelpunkt, Netzlokation      | MSB → LF    |
| 13019 | Energiemenge (Strom)                                      | NB/MSB → LF |
| 13025 | Lastgang Marktlokation, Tranche                           | MSB → LF    |
| 13027 | Werte nach Typ 2 (WiM Strom Teil 2)                       | MSB → LF    |

> All MSCONS PIDs here carry metered data. They are stateless deliveries that
> write no outbox entries on success.

### ORDERS Datenabruf — Anfrage / Ablehnung (GPKE Teil 4)

Workflow `gpke-datenabruf` handles the LF-side of data-request processes: the LF
sends an ORDERS Anfrage to the NB or MSB and waits for a response or explicit
rejection within 24 h.

| PID   | Process name (AHB)                                 | Direction   | Status         |
|-------|----------------------------------------------------|-------------|----------------|
| 17004 | Anfrage Datenabruf (allgemein)                     | LF → NB/MSB | ✅ Implemented |
| 17102 | Anfrage Übermittlung Stammdaten Strom              | LF → NB/MSB | ✅ Implemented |
| 17113 | Anfrage Übermittlung Werte                         | LF → NB/MSB | ✅ Implemented |
| 19101 | Ablehnung Anfrage Datenabruf (NB → LF)             | NB → LF     | ↩ Derived      |
| 19102 | Ablehnung Anfrage Stammdaten (NB → LF)             | NB → LF     | ↩ Derived      |
| 19114 | Ablehnung Anfrage Werte (NB → LF)                  | NB → LF     | ↩ Derived      |

> The response deadline (`gpke-datenabruf-antwort`) is **1 Werktag** —
> „Unverzüglich, jedoch spätester ÜZ ist 1 WT nach dem ÜZ von Nr. 1", GPKE Teil 4
> § 3.2 Prozessschritte 2 und 4 — and is enforced by a `DeadlineStore` entry
> registered on every outbound ORDERS Anfrage.

### ORDERS Allokationsliste — MSCONS 13014 (GPKE MSCONS Strom)

Workflow `gpke-allokationsliste` handles requests and rejections for the
Allokationsliste, exchanged between LF and NB via ORDERS and answered with MSCONS.

| PID   | Process name (AHB)                                      | Direction   | Status         |
|-------|---------------------------------------------------------|-------------|----------------|
| 17110 | Anfrage Allokationsliste (LF → NB)                      | LF → NB     | ✅ Implemented |
| 17114 | Anfrage Allokationsliste alternativ (LF → NB)           | LF → NB     | ✅ Implemented |
| 19110 | Ablehnung Anfrage Allokationsliste (NB → LF)            | NB → LF     | ↩ Derived      |
| 19115 | Ablehnung alternativ (NB → LF)                          | NB → LF     | ↩ Derived      |
| 13014 | Allokationsliste Strom (NB → LF, MSCONS)                | NB → LF     | ↩ Derived      |

> PIDs 17110/19110 here are Strom (GPKE). The same PID numbers also appear in
> `mako-gabi-gas` for the gas MMMA process (different commodity, different crate).

## EDIFACT Format Versions

| Format version   | Valid from | Valid until | Profile status                   |
|------------------|------------|-------------|----------------------------------|
| `FV2025-06-06`   | 2025-06-06 | 2025-09-30  | ✓ available (UTILMD S1.2 — LFW24 cutover) |
| `FV2025-10-01`   | 2025-10-01 | 2026-09-30  | ✓ available (UTILMD S2.1 — current) |
| `FV2026-10-01`   | 2026-10-01 | —           | ✓ available (UTILMD S2.2 — upcoming) |
| `FV2026-04-01`   | 2026-04-01 | 2026-09-30  | ✓ available (INVOIC 2.8e, REMADV 2.9f, ORDERS 1.4b) |

> INVOIC (31001–31008) and ORDERS/ORDRSP Konfiguration (17134/17135, 19001/19002)
> use their own versioned profiles (`fv20260401`), independent of the UTILMD
> Strom release cycle.

## Modules

| Rust module                 | Workflow name                    | Contents                                                            |
|-----------------------------|----------------------------------|---------------------------------------------------------------------|
| `wechselprozesse`           | `gpke-supplier-change`           | PIDs 55001/55002/55016/55077/55557 (UTILMD supplier-switch + Kündigung, NB role) + IFTSTA Vollzugs-/Statusmeldungen 21024–21028/21033/21035 |
| `stornierung`               | `gpke-stornierung`               | PIDs 55022–55024 (UTILMD Stornierung Zuordnungsprozess) + 55023/55024 derived |
| `beendigung_zuordnung`      | `gpke-beendigung-zuordnung`      | PID 55010 (NB Abmeldeanfrage) + 55011/55012 derived responses |
| `eog`                       | `gpke-eog`                       | PID 55013 (Anmeldung/Zuordnung EOG §36/§38 EnWG) + 55014/55015 derived |
| `comdis`                    | `gpke-comdis`                    | PIDs 29001/29002 (COMDIS Kaufmännisch-Bilanzielle Ausgleichsprozesse) |
| `lf_anmeldung`              | `gpke-lf-anmeldung`              | PIDs 55001/55004/55016/55077 (LF outbound) + 55002-55003/55005-55006/55017-55018/55078/55080 (LF-role receive NB ANTWORT) |
| `lf_abmeldung`              | `gpke-lf-abmeldung`              | PID 55007 (Lieferende von NB an LF) + 55008/55009 derived     |
| `stammdatenaenderung`       | `gpke-stammdatenaenderung`       | GPKE Teil 4 Stammdatenänderung 55615–55694, 55109/55110 — inbound MaLo change → apply to marktd + Rückmeldung A01/A02 (quality feedback, tacit acceptance after 2 WT) |
| `neuanlage`                 | `gpke-neuanlage`                 | PIDs 55600/55601 (Neuanlage MaLo, LF → NB) + 55602–55605 derived   |
| `messwerte`                 | `gpke-messwerte`                 | MSCONS PIDs 13005/13006/13015–13019/13025/13027 (Messwerte NB/MSB → LF) |
| `datenabruf`                | `gpke-datenabruf`                | ORDERS 17004/17102/17113 (Anfrage) + ORDRSP 19101/19102/19114 (Ablehnung) |
| `allokationsliste`          | `gpke-allokationsliste`          | ORDERS 17110/17114 + ORDRSP 19110/19115 + MSCONS 13014 (Allokationsliste Strom) |
| `anfrage_bestellung`        | `gpke-anfrage-bestellung`        | PID 55555 (Anfrage Daten der individuellen Bestellung, GPKE Teil 4)  |
| `abrechnung`                | `gpke-abrechnung`                | PIDs 31001/31002/31005/31006 (INVOIC Netznutzungsabrechnung)        |
| `abrechnungsdaten`          | `gpke-abrechnungsdaten`          | PIDs 55156/55220/55673 (Rückmeldung/Bestellung Abrechnungsdaten, LF → NB) → IFTSTA 21047 Bearbeitungsstand, `E_0595` |
| `konfiguration`             | `gpke-konfiguration`             | PIDs 17134/17135 (ORDERS outbound) + 19001/19002 (ORDRSP inbound) — GPKE Teil 4 |
| `konfiguration_aenderung`   | `gpke-konfiguration-aenderung`   | ORDERS/ORDRSP for configuration changes (NB role)                   |
| `sperrung`                  | `gpke-sperrung`                  | PIDs 17115–17117 (ORDERS Sperrung Strom, NB → MSB)                 |
| `sperrung_lf`               | `gpke-sperrung-lf`               | LF-side Sperrung: ORDERS 17115/17117 + ORDCHG 39000 outbound, ORDRSP 19116/19117 · 19128/19129 + IFTSTA 21039 inbound |
| `ankuendigung_zuordnung_lf` | `gpke-ankuendigung-zuordnung-lf` | PIDs 55607–55609 (UTILMD Ankündigung Zuordnung LF)                 |
| `partin`                    | `gpke-partin`                    | PIDs 37000–37006 (PARTIN Strom Kommunikationsdaten)                |
| `utilts`                    | `gpke-utilts`                    | UTILTS PIDs 25001/25004–25010 (Netzzustandsdaten NB → LF)          |

## Usage

```rust
use mako_gpke::wechselprozesse::{GpkeSupplierChangeWorkflow, SupplierChangeCommand};
use mako_engine::{builder::EngineBuilder, event_store::InMemoryEventStore};

// In tests (requires `testing` feature or `#[cfg(test)]`):
#[cfg(test)]
let ctx = EngineBuilder::new()
    .with_event_store(InMemoryEventStore::new())
    .build();

// In production, explicitly provide all stores:
// let ctx = EngineBuilder::with_stores(outbox, deadline, registry)
//     .with_event_store(my_slatedb_store)
//     .build();

let process = ctx.spawn::<GpkeSupplierChangeWorkflow>(tenant_id, workflow_id);
let events = process.execute(SupplierChangeCommand::ReceiveUtilmd {
    pid: 55001,
    // …
}).await?;
```

## Regulatory references

- BDEW GPKE Marktprozesse für die Belieferung mit Elektrizität
- BNetzA **BK6-24-174** (Beschluss 24.10.2024, gültig seit 06.06.2025) — GPKE Teil 1–3
- BNetzA **BK6-22-024** (Beschluss 21.03.2024) — LFW24, superseded for the
  process descriptions by BK6-24-174
- EDI@Energy UTILMD Strom AHB S2.2 (`FV2026-10-01`)
- EDI@Energy INVOIC AHB 2.8e / AHB 1.0 (`FV2025-10-01` onwards)
- EDI@Energy **APERAK AHB 1.1** (`FV2026-10-01`) — § 2.4.1 Strom, § 2.3.1 Gas.
  2.2 is the APERAK **MIG** revision; AHB and MIG carry different version numbers
  for every message type except UTILMD
