---
layout: default
title: Process Catalog
nav_order: 15
parent: Reference
mermaid: true
description: >
  Business-level catalog of all German energy market communication processes —
  GPKE, WiM Strom, MaBiS, GeLi Gas, WiM Gas, GaBi Gas, and PARTIN.
  For each process: initiating role, message exchange, APERAK deadline,
  regulatory basis, and implementation status.
---

# Process Catalog

This page is the **business-level** companion to the [PID Reference](pid-reference).
Where the PID Reference lists every individual EDIFACT message type, the Process
Catalog groups related messages into **complete end-to-end processes** — the unit
of work from the business perspective and the unit of implementation in the
`mako-*` domain crates.

> **Role coverage.** All market roles are equally supported: Lieferant (LF/LFN/LFA),
> Netzbetreiber (NB/GNB), Messstellenbetreiber (MSB/gMSB), Bilanzkreisverantwortlicher (BKV),
> Übertragungsnetzbetreiber (ÜNB), and others. Each process table lists all participating
> roles and marks which crate implements each side of the exchange.

> **Commodity isolation.** Strom and Gas are fully independent deployment units.
> A makod instance for Strom loads only `mako-gpke` + `mako-wim` + `mako-mabis`.
> A makod instance for Gas loads only `mako-geli-gas` + `mako-wim-gas` + `mako-gabi-gas`.
> Running separate instances per commodity is explicitly supported and common in
> production. A combined instance is equally valid. Each section in this catalog
> is documented as **self-contained** — no cross-commodity knowledge required.

**Both format versions coexist simultaneously:**

| Format version | Valid period |
|---|---|
| `FV2025-10-01` | 2025-10-01 – 2026-09-30 (current production) |
| `FV2026-10-01` | from 2026-10-01 (next release — profiles already deployed) |

**Status legend:**

| Symbol | Meaning |
|---|---|
| ✅ | Full state machine + AHB rule enforcement, production-safe |
| ⚠️ | PID registered, partial handling — accepts message, limited state transitions |
| 🔄 | Placeholder crate — not yet implemented |
| — | Not registered; inbound messages are dead-lettered |

**APERAK Frist legend:**

| Domain | Frist |
|---|---|
| GPKE | 24 Stunden (wall-clock) |
| WiM Strom | 5 Werktage |
| GeLi Gas | 10 Werktage |
| WiM Gas | 10 Werktage |

Saturday counts as a Werktag; Sundays and public holidays do not.
Deadline arithmetic uses German local time (CET/CEST) — an off-by-one-hour error
at DST transitions constitutes a regulatory deadline violation.

---

## Process Overview

Quick reference across all process families. Each row is a top-level domain.

| Domain | Sparte | Crate | Key PIDs | APERAK Frist | Basis |
|---|:---:|---|---|---|---|
| **GPKE Lieferantenwechsel (NB-Sicht)** | ⚡ | `mako-gpke` `gpke-supplier-change` | UTILMD 55001–55018, 55022–55024 | 24 h | BK6-24-174 |
| **GPKE Lieferantenwechsel (LF-Sicht)** | ⚡ | `mako-gpke` `gpke-lf-anmeldung` | UTILMD 55001/55002/55016/55077 (out) · 55003–55006 (in) | 24 h | BK6-24-174 |
| **GPKE Neuanlage MaLo** | ⚡ | `mako-gpke` `gpke-neuanlage` | UTILMD 55600/55601 → 55602–55605 | 24 h | BK6-24-174 |
| **GPKE Abmeldung LF** | ⚡ | `mako-gpke` `gpke-lf-abmeldung` | UTILMD 55007 → 55008/55009 | 24 h | BK6-24-174 |
| **GPKE Ankündigung Zuordnung LF** | ⚡ | `mako-gpke` `gpke-ankuendigung-zuordnung-lf` | UTILMD 55607 → 55608/55609 | 24 h | BK6-24-174 |
| **GPKE Sperrung/Entsperrung (NB)** | ⚡ | `mako-gpke` `gpke-sperrung` | ORDERS 17115/17117 → ORDRSP 19116/19117 | 24 h | BK6-22-024 |
| **GPKE Sperrung/Entsperrung (LF-Sicht)** | ⚡ | `mako-gpke` `gpke-sperrung-lf` | ORDERS 17115/17117 (out) · ORDCHG 39000 (out) · ORDRSP 19116/19117 · 19128/19129 · IFTSTA 21039 | 24 h | BK6-22-024 |
| **GPKE Abrechnung (INVOIC)** | ⚡ | `mako-gpke` `gpke-abrechnung` | INVOIC 31001/31002/31005/31006; REMADV; COMDIS | 24 h | BK6-24-174 |
| **GPKE Datenabruf** | ⚡ | `mako-gpke` `gpke-datenabruf` | ORDERS 17004/17102/17113 → ORDRSP rejection | 24 h | BK6-22-024 |
| **GPKE Anfrage Bestellung (55555)** | ⚡ | `mako-gpke` `gpke-anfrage-bestellung` | UTILMD 55555 | 24 h | BK6-22-024 |
| **GPKE Allokationsliste Strom** | ⚡ | `mako-gpke` `gpke-allokationsliste` | ORDERS 17110/17114 · ORDRSP 19110/19115 · MSCONS 13014 | 24 h | BK6-24-174 |
| **GPKE Messwerte (MSCONS)** | ⚡ | `mako-gpke` `gpke-messwerte` | MSCONS 13005/13006/13015–13019/13025/13027 | 24 h | BK6-24-174 |
| **GPKE UTILTS** | ⚡ | `mako-gpke` `gpke-utilts` | UTILTS 25001/25004–25010 | 24 h | BK6-24-174 |
| **GPKE Konfiguration** | ⚡ | `mako-gpke` `gpke-konfiguration` | ORDERS 17134/17135 → ORDRSP 19001/19002 | 24 h | BK6-22-024 |
| **GPKE Konfiguration Änderung** | ⚡ | `mako-gpke` `gpke-konfiguration-aenderung` | ORDERS/ORDRSP config changes | 24 h | BK6-22-024 |
| **PARTIN Strom Kommunikationsdaten** | ⚡ | `mako-gpke` `gpke-partin` | PARTIN 37000–37006 | — | PARTIN AHB 1.0f |
| **WiM Strom MSB-Wechsel** | ⚡ | `mako-wim` `wim-device-change` | UTILMD 55039/55042/55051/55168 (out+in) · 55040/55041 · 55043/55044 · 55052/55053 · 55169/55170 (Antwort) | 3/5/7/1 WT — see below | BK6-24-174 |
| **WiM Strom Geräteübernahme** | ⚡ | `mako-wim` `wim-geraeteubernahme` | ORDERS 17001–17011 · ORDRSP 19001/19002 | 5 WT | BK6-24-174 |
| **WiM Strom Abrechnung** | ⚡ | `mako-wim` `wim-rechnung` | INVOIC 31009 | 5 WT | BK6-24-174 |
| **WiM Strom INSRPT** | ⚡ | `mako-wim` `wim-insrpt` | INSRPT 23001/23003/23004/23008 | 5 WT | BK6-24-174 |
| **MaBiS Bilanzkreisabrechnung** | ⚡ | `mako-mabis` `mabis-billing` | MSCONS 13003; IFTSTA 21000–21005 | 1 WT (§13.8) | BK6-24-174 |
| **MaBiS Clearingliste** | ⚡ | `mako-mabis` `mabis-clearingliste` | UTILMD 55065/55069/55070 | — | BK6-24-174 |
| **GeLi Gas Lieferantenwechsel** | 🔥 | `mako-geli-gas` `geli-gas-supplier-change` | UTILMD G 44001–44021 | 10 WT | BK7-24-01-009 |
| **GeLi Gas Lieferbeginn (LF-Sicht)** | 🔥 | `mako-geli-gas` `geli-gas-lf-anmeldung` | UTILMD G 44001 (out) · 44003/44004 (in) | 10 WT | BK7-24-01-009 |
| **GeLi Gas Stornierung (GNB-Sicht)** | 🔥 | `mako-geli-gas` `geli-gas-stornierung` | UTILMD G 44022 (Nb-only inbound) | 10 WT | BK7-24-01-009 |
| **GeLi Gas Stornierung (LF-Sicht)** | 🔥 | `mako-geli-gas` `geli-gas-stornierung-lf` | UTILMD G 44023/44024 (Lf-only inbound) | 10 WT | BK7-24-01-009 |
| **GeLi Gas Sperrung (LF-Sicht)** | 🔥 | `mako-geli-gas` `geli-gas-sperrung-lf` | ORDERS 17115/17117 · ORDCHG 39000 | 10 WT | BK7-24-01-009 |
| **GeLi Gas Sperrung (GNB-Sicht)** | 🔥 | `mako-geli-gas` `geli-gas-sperrung-nb` | ORDERS 17115–17117 · ORDCHG 39000/39001 · ORDRSP 19118/19119 | 10 WT | BK7-24-01-009 |
| **GeLi Gas AWH-Abrechnung** | 🔥 | `mako-geli-gas` `geli-gas-sperrprozesse-invoic` | INVOIC 31011 | — | BK7-24-01-009 |
| **GeLi Gas Messdaten (MSCONS)** | 🔥 | `mako-geli-gas` `geli-gas-mscons` | MSCONS 13002/13007/13008/13009 | — | BK7-24-01-009 |
| **GeLi Gas Datenabruf** | 🔥 | `mako-geli-gas` `geli-gas-datenabruf` | ORDERS 17103/17104 → ORDRSP 19103/19104 | 10 WT | BK7-24-01-009 |
| **PARTIN Gas Kommunikationsdaten** | 🔥 | `mako-geli-gas` `geli-gas-partin` | PARTIN 37008–37014 | — | PARTIN AHB 1.0f |
| **WiM Gas MSB-Wechsel** | 🔥 | `mako-wim-gas` | UTILMD G 44039–44053/44168–44170 | 10 WT | BK7-24-01-009 |
| **WiM Gas Stornierung** | 🔥 | `mako-wim-gas` `wim-gas-stornierung` | UTILMD G 44022–44024 (Msb/Nmsb role) | 10 WT | BK7-24-01-009 |
| **WiM Gas INSRPT** | 🔥 | `mako-wim-gas` `wim-gas-insrpt` | INSRPT 23005/23009 (Gas-only) | 10 WT | BK7-24-01-009 |
| **WiM Gas Abrechnung** | 🔥 | `mako-wim-gas` `wim-gas-invoic` | INVOIC 31003/31004 | — | BK7-24-01-009 |
| **GaBi Gas Abrechnung** | 🔥 | `mako-gabi-gas` `gabi-gas-invoic` | INVOIC 31007/31008/31010 | — | BK7-24-01-008 |
| **GaBi Gas Allokationsliste (MMMA)** | 🔥 | `mako-gabi-gas` `gabi-gas-mmma` | ORDERS 17110 · ORDRSP 19110 · MSCONS 13013 | — | BK7-24-01-008 |
| **GaBi Gas ALOCAT** | 🔥 | `mako-gabi-gas` `gabi-gas-allocation` | Synthetic PIDs 90001–90003 | — | DVGW ALOCAT 5.11a |
| **GaBi Gas NOMINT/NOMRES** | 🔥 | `mako-gabi-gas` `gabi-gas-nomination` | Synthetic PIDs 90011/90012/90021/90022 | — | DVGW NOMINT 4.6 FK |
| **GaBi Gas SCHEDL** | 🔥 | `mako-gabi-gas` `gabi-gas-schedl` | Synthetic PIDs | — | DVGW G685/G2000 |
| **GaBi Gas IMBNOT** | 🔥 | `mako-gabi-gas` `gabi-gas-imbnot` | Synthetic PIDs | — | DVGW IMBNOT 5.7a |
| **GaBi Gas TRANOT** | 🔥 | `mako-gabi-gas` `gabi-gas-tranot` | Synthetic PIDs | — | DVGW TRANOT 5.8b |
| **GaBi Gas DELORD/DELRES** | 🔥 | `mako-gabi-gas` `gabi-gas-delivery-order` | Synthetic PIDs | — | DVGW DELORD 4.5 FK |
| **Redispatch 2.0** | ⚡ | `mako-redispatch` | IFTSTA 21037/21038; XML documents | — | BK6-20-059/060/061 |

---

## Table of Contents

1. [GPKE — Kundenbelieferung Elektrizität](#gpke--kundenbelieferung-elektrizität)
   - [Lieferantenwechsel Strom](#lieferantenwechsel-strom)
   - [Sperrung / Entsperrung Strom](#sperrung--entsperrung-strom)
   - [INVOIC Strom Abrechnung](#invoic-strom-abrechnung)
   - [Datenabruf und Stammdatenprozesse](#datenabruf-und-stammdatenprozesse)
   - [UTILTS — Berechnungsformeln und Zählzeitdefinitionen](#utilts--berechnungsformeln-und-zählzeitdefinitionen)
   - [MSCONS — Zählerstandsübermittlung](#mscons--zählerstandsübermittlung)
   - [GPKE IFTSTA — Vollzugsmeldungen, Statusmeldungen, EnFG](#gpke-iftsta--vollzugsmeldungen-statusmeldungen-enfg-gpke-teil-234)
2. [WiM Strom — Messstellenbetrieb](#wim-strom--messstellenbetrieb)
   - [MSB-Wechsel Strom](#msb-wechsel-strom)
   - [Geräteübernahme und Stammdaten](#geräteübernahme-und-stammdaten)
   - [WiM-Abrechnung](#wim-abrechnung)
   - [Technik-Änderung und Gerätekonfiguration](#technik-änderung-und-gerätekonfiguration)
   - [Preisanfrage, Angebote und Preislisten](#preisanfrage-angebote-und-preislisten)
   - [Steuerungsauftrag (API-Webdienste Strom)](#steuerungsauftrag-api-webdienste-strom)
   - [IFTSTA Status (WiM Strom)](#iftsta-status-wim-strom)
   - [INSRPT — Störungsmeldungen (WiM Strom)](#insrpt--störungsmeldungen-wim-strom)
3. [MaBiS — Bilanzkreisabrechnung Strom](#mabis--bilanzkreisabrechnung-strom)
4. [GeLi Gas — Lieferantenwechsel Gas](#geli-gas--lieferantenwechsel-gas)
   - [Lieferantenwechsel Gas](#lieferantenwechsel-gas)
     - [LF-seitige Einreichung (geli-gas-lf-anmeldung)](#lf-seitige-einreichung-geli-gas-lf-anmeldung)
   - [Sperrung / Entsperrung Gas](#sperrung--entsperrung-gas)
   - [Gas Abrechnung — Billing Scope](#gas-abrechnung--billing-scope)
   - [Gas Datenabruf](#gas-datenabruf)
   - [MSCONS Gas — Messwert- und Energiemengenübermittlung](#mscons-gas--messwert--und-energiemengenübermittlung)
   - [Process Symmetry: GPKE ↔ GeLi Gas](#process-symmetry-gpke--geli-gas)
5. [WiM Gas — Messstellenbetrieb Gas](#wim-gas--messstellenbetrieb-gas)
   - [WiM Gas Abrechnung](#wim-gas-abrechnung)
   - [WiM Gas — INSRPT Störungsmeldungen](#wim-gas--insrpt-störungsmeldungen)
6. [GaBi Gas — Kapazitätsabrechnung Gas](#gabi-gas--kapazitätsabrechnung-gas)
7. [PARTIN — Stammdaten Marktpartner](#partin--stammdaten-marktpartner)
8. [Redispatch 2.0](#redispatch-20)
9. [DVGW — Gas Transport](#dvgw--gas-transport)

---

## GPKE — Kundenbelieferung Elektrizität

**Regulatory basis:** BK6-24-174 (Beschluss 24.10.2024, gültig ab 06.06.2025) +
BK6-22-024 (GPKE Teil 4, Stammdaten und Konfiguration)

**APERAK Frist:** **24 wall-clock hours** from receipt of the triggering message.

---

### Lieferantenwechsel Strom

The supplier-switch process (GPKE Teil 2) is the highest-volume process in the
German electricity market. The incoming supplier (LFN) initiates the registration
and simultaneously cancels the outgoing supplier's (LFA) contract. Both the
NB and LFA must respond within 24 h.

**Process timing rules (BK6-22-024 / BK6-24-174):**

| Scenario | Mindestvorlauffrist | Notes |
|---|---|---|
| Standardwechsel | 7 Werktage vor Lieferbeginn | Eingang bei NB und LFA auf demselben Kalendertag |
| Schneller Lieferantenwechsel | nächster Werktag | Eingang bis 12:00 Uhr des Vortages |
| Neuanlage MaLo | keine Mindestfrist | Lieferbeginn = Tag der Fertigstellung |
| Stornierung Zuordnung | bis 24 h vor Lieferbeginn | Nur der ursprüngliche Sender darf stornieren |

> **Einreichungstag rule:** UTILMD 55001 (LFN → NB) and UTILMD 55016 (LFN → LFA)
> must be submitted on the **same calendar day**. The NB coordinates the transition;
> the actual LFA disconnection follows automatically at Lieferbeginn-Datum.

**Grund- und Ersatzversorgung (GEV / EOG):** When a customer has no active
supplier (e.g. after LFA exit, insolvency), the NB activates the basic supplier
(Grundversorger). The NB sends an End-of-Contract / EOG notification to the
outgoing LF and registers the basic supplier automatically. Standard GPKE PIDs
apply (55007–55015 range) via the `gpke-supplier-change` workflow.

| Process | Initiator → Responder | Anfrage PID | Antwort OK | Antwort NG | Crate |
|---|---|---|---|---|---|
| Anmeldung / Lieferbeginn (LF-AN) | LFN → NB | UTILMD **55001** | 55003 | 55004 | `mako-gpke` ✅ |
| Lieferende / Abmeldung (LFN → NB) | LFN → NB | UTILMD **55002** | 55005 | 55006 | `mako-gpke` ✅ |
| Anmeldung erz. MaLo (LF-AN) | LFN → NB | UTILMD **55077** | 55078 | 55080 | `mako-gpke` ✅ |
| Neuanlage verb. MaLo | LF → NB | UTILMD **55600** | 55602 | 55604 | `mako-gpke` ✅ |
| Neuanlage erz. MaLo | LF → NB | UTILMD **55601** | 55603 | 55605 | `mako-gpke` ✅ |
| Kündigung Lieferbeginn | LFN → LFA | UTILMD **55016** | 55017 | 55018 | `mako-gpke` ✅ |
| Abmeldung (NB-initiiert) | NB → LFA | UTILMD **55007** | 55008 | 55009 | `mako-gpke` ✅ |
| Änderung MSB-Abrechnungsdaten der MaLo | LFN ↔ NB | UTILMD **55557** | — | — | `mako-gpke` ✅ |
| Ankündigung Zuordnung LF | NB → LFN | UTILMD **55607** | 55608 | 55609 | `mako-gpke` ✅ |
| Stornierung Zuordnungsprozess | orig. → orig. | UTILMD **55022** | 55023 | 55024 | `mako-gpke` ✅ |

> **Lieferbeginn = T.** Both UTILMD 55001 (LFN → NB) and 55016 (LFN → LFA) are sent
> on the same day, referencing the same `Lieferbeginn`-date. The NB coordinates
> the transition; the actual disconnection of LFA follows automatically when the
> Lieferbeginn date is reached.

**Message flow — Lieferantenwechsel Strom (LF-AN):**

```mermaid
sequenceDiagram
    participant LFN as Neuer LF (LFN)
    participant NB  as Netzbetreiber (NB)
    participant LFA as Alter LF (LFA)

    Note over LFN,LFA: T = Lieferbeginn-Datum
    LFN->>NB:  UTILMD 55001 (Anmeldung / Lieferbeginn)
    LFN->>LFA: UTILMD 55016 (Kündigung Lieferbeginn)

    alt Bestätigung
        NB-->>LFN:  UTILMD 55003 (Bestätigung Lieferbeginn)
        LFA-->>LFN: UTILMD 55017 (Bestätigung Kündigung)
    else Ablehnung durch NB
        NB-->>LFN:  UTILMD 55004 (Ablehnung Lieferbeginn)
    else Ablehnung durch LFA
        LFA-->>LFN: UTILMD 55018 (Ablehnung Kündigung)
    end

    Note over LFN,LFA: Zum Lieferbeginn-Datum T
    NB->>LFA: UTILMD 55007 (Abmeldung / Beendigung Zuordnung)
    LFA-->>NB: UTILMD 55008 (Bestätigung) oder 55009 (Ablehnung)

    Note over LFN,NB: Nach Lieferbeginn (separate Prozesse)
    NB-->>LFN: MSCONS (Zählerstandsübermittlung — eigener Prozess)
    NB-->>LFN: INVOIC 31001/31002 (Netznutzungsrechnung — eigener Prozess)
```

> **Note:** MSCONS and INVOIC arrive after Lieferbeginn as independent processes with
> their own process IDs and APERAK windows. They are shown here only to indicate the
> downstream billing relationship.

---

### Sperrung / Entsperrung Strom

The LF can order a disconnection (Sperrung) or reconnection (Entsperrung) of a
market location via ORDERS. The NB forwards the order to the MSB (metering point
operator). After physical execution, the NB confirms back to the LF via ORDRSP
and sends an IFTSTA status update.

PIDs 17115 and 17117 are shared between **GPKE Strom** (NB-role inbound) and
**GeLi Gas** (LF-role outbound). Routing is determined by market context
(Sparte Strom vs. Gas) at the protocol level.

| Process | Initiator → Responder | Anfrage PID | Antwort OK | Antwort NG | Crate |
|---|---|---|---|---|---|
| Sperrauftrag LF-initiiert (Strom) | LF → NB | ORDERS **17115** | ORDRSP 19116 | ORDRSP 19117 | `mako-gpke` ✅ |
| Entsperrauftrag LF-initiiert (Strom) | LF → NB | ORDERS **17117** | ORDRSP 19116 | ORDRSP 19117 | `mako-gpke` ✅ |
| Anfrage Sperrung (NB → MSB) | NB → MSB | ORDERS **17116** | ORDRSP 19118 | ORDRSP 19119 | `mako-gpke` ✅ |
| Auftragsstatus Sperren | NB → LF/MSB/ÜNB | — | IFTSTA **21039** | — | `mako-gpke` ✅ |
| Info Entsperrauftrag | NB → MSB | — | IFTSTA **21040** | — | — |
| Stornierung Sperrauftrag | LF → NB | ORDCHG **39000** | ORDRSP 19128 | ORDRSP 19129 | `mako-gpke` ✅ |
| Weiterleitung Stornierung | NB → MSB | ORDCHG **39001** | — | — | — |

**Message flow — Sperrauftrag Strom (LF-initiiert):**

```mermaid
sequenceDiagram
    participant LF
    participant NB  as Netzbetreiber (NB)
    participant MSB as Messstellenbetreiber (MSB)

    LF->>NB:  ORDERS 17115 (Sperrauftrag)
    NB->>MSB: ORDERS 17116 (Anfrage Sperrung)
    MSB-->>NB: ORDRSP 19118 (Bestätigung) oder 19119 (Ablehnung)

    alt Bestätigung
        NB-->>LF: ORDRSP 19116 (Bestätigung Sperrauftrag)
        NB->>LF:  IFTSTA 21039 (Auftragsstatus — Sperrung ausgeführt)
    else Ablehnung
        NB-->>LF: ORDRSP 19117 (Ablehnung Sperrauftrag)
    end

    opt Stornierung
        LF->>NB:  ORDCHG 39000 (Stornierung Sperrauftrag)
        NB-->>LF: ORDRSP 19128 (Bestätigung) oder 19129 (Ablehnung)
    end
```

---

### INVOIC Strom Abrechnung

Network billing messages from the NB to the LF. The LF is the passive receiver;
acknowledgement is via APERAK within 24 h.

#### INVOIC — Netznutzungs- und Mehr-/Mindermengenabrechnung

| Process | Sender → Empfänger | INVOIC PID | Content | Sparte | Crate |
|---|---|---|---|---|---|
| Abschlagsrechnung | NB → LF | INVOIC **31001** | Netznutzung Abschlag (StromNEV §21) | ⚡ | `mako-gpke` ✅ |
| NN-Rechnung / MMM | NB → LF | INVOIC **31002** | Mehr-/Mindermengen Strom (MMM) | ⚡ | `mako-gpke` ✅ |
| NNE Gas | NB → LF | INVOIC **31005** | Netznutzungsentgelt Gas (GasNEV §14) | 🔥 | `netzbilanzd` ✅ |
| NNE selbstausgestellt | NB+LF same entity | INVOIC **31006** | Selbst ausgestellte NNE | ⚡ | `netzbilanzd` ✅ |
| WiM Gas Rechnung | gMSB → NB | INVOIC **31003** | MSB-Gerätewechsel Gas | 🔥 | `mako-wim-gas` ⚠️ |
| Stornorechnung WiM Gas | gMSB → NB | INVOIC **31004** | Storno MSB-Rechnung Gas | 🔥 | `mako-wim-gas` ⚠️ |
| MSB-Rechnung Strom | MSB → LF | INVOIC **31009** | WiM Messstellenbetriebsabrechnung | ⚡ | `mako-wim` ✅ |
| MMM Gas aggregiert | NB → MGV | INVOIC **31007** | Aggreg. MMM-Rechnung Gas | 🔥 | `mako-gabi-gas` ✅ |
| MMM Gas selbstausgestellt | MGV | INVOIC **31008** | Selbst ausgest. MMM-Rechnung Gas | 🔥 | `mako-gabi-gas` ✅ |
| AWH Sperrprozesse Gas | GNB/VNB → LF | INVOIC **31011** | Sonstige Leistung Sperrung Gas | 🔥 | `mako-geli-gas` ✅ |
| Kapazitätsabrechnung Gas | GNB → KN | INVOIC **31010** | Kapazitätsabrechnung Gas | 🔥 | `mako-gabi-gas` ✅ |

#### REMADV / COMDIS — Zahlungsabwicklung

| Message | Sender → Empfänger | PID | Meaning | Crate |
|---|---|---|---|---|
| Zahlungsavis (vollständige Zahlung) | LF → NB | REMADV **33001** | Full payment confirmation | `mako-gpke` ✅ |
| Zahlungsavis (Ablehnung Zahlung) | LF → NB | REMADV **33002** | Payment rejected | `mako-gpke` ✅ |
| Zahlungsavis (Teilzahlung Netznutzung) | LF → NB | REMADV **33003** | Partial payment NNA | `mako-gpke` ✅ |
| Zahlungsavis (Teilzahlung MMM) | LF → NB | REMADV **33004** | Partial payment MMM | `mako-gpke` ✅ |
| Ablehnung Zahlungsavis | NB → LF | COMDIS **29001** | Invoicer disputes REMADV | `mako-gpke` ✅ |

---

### Datenabruf und Stammdatenprozesse

Data requests and configuration processes under GPKE Teil 4 (BK6-22-024).

> **Multi-crate processes:** Some PIDs appear in more than one crate when the
> direction or Marktrolle differs. For example, ORDERS 17132 (Stammdaten MeLo)
> is handled by `mako-wim` because it uses WiM-specific MeLo semantics, while
> all other GPKE datenabruf PIDs live in `mako-gpke`.

#### Datenabruf

| Process | Initiator → Responder | Anfrage PID | Antwort | Crate |
|---|---|---|---|---|
| Anfrage Daten der individuellen Bestellung | LF → NB | UTILMD **55555** | UTILMD 55553 | `mako-gpke` ✅ |
| Anfrage Werte (GPKE) | LF → MSB/NB | ORDERS **17004** | ORDRSP 19101 | `mako-gpke` ✅ |
| Anfrage Stammdaten MaLo (Strom) | LF/NB → NB | ORDERS **17102** | ORDRSP 19102 | `mako-gpke` ✅ |
| Anfrage Stammdaten NNE/NLPV | LF → NB | ORDERS **17113** | ORDRSP 19114 | `mako-gpke` ✅ |
| Anfrage Stammdaten Messlokation | LF/MSB → NB | ORDERS **17132** | ORDRSP | `mako-wim` ✅ |
| Anforderung Allokationsliste | LF → NB | ORDERS **17110** | ORDRSP 19110 | `mako-gpke` ✅ |
| Anforderung bilanzierte Menge | NB/LF → ÜNB | ORDERS **17114** | ORDRSP 19115 | `mako-gpke` ✅ |

#### Konfigurationseinrichtung (NB-outbound)

These ORDERS messages are **generated by the NB workflow** as part of its
post-Lieferbeginn configuration obligation (GPKE Teil 4 §3). They are dispatched
via the NB's outbox after UTILMD 55001 is accepted — they are **not** inbound
routing PIDs. The MSB responds with ORDRSP 19001 (Bestätigung) or 19002 (Ablehnung)
which are routed back to the `gpke-konfiguration` workflow.

| Process (BDEW AHB name) | NB sends to | ORDERS PID | MSB-Antwort | Crate |
|---|---|---|---|---|
| Einrichtung Konfiguration aufgrund Zuordnung LF (NB an MSB) | NB → MSB | **17134** | ORDRSP 19001/19002 | `mako-gpke` ✅ |
| Einrichtung Konfiguration aufgrund Zuordnung LF (MSB an MSB) ¹ | NB → MSB | **17135** | ORDRSP 19001/19002 | `mako-gpke` ✅ |

> ¹ Despite the name "MSB an MSB", ORDERS 17135 is **sent by the NB** (via its outbox)
> to coordinate configuration between two MSBs. The NB workflow (`gpke-konfiguration`)
> is the owner; no MSB system invokes this directly.

#### Konfigurationsänderung (LF/NB-initiated)

| Process | Initiator → Responder | ORDERS PID | Antwort | Crate |
|---|---|---|---|---|
| Änderung Prognosegrundlage | LF → NB | **17120** | ORDRSP 19121 | `mako-gpke` ✅ |
| Änderung Konfiguration (NB → MSB) | NB → MSB | **17121** | ORDRSP | `mako-gpke` ✅ |
| Änderung Lastprofilzuordnung | LF/NB → NB | **17122/17123** | ORDRSP | `mako-gpke` ✅ |
| Änderung iMS-Pflichteinbau | NB → MSB | **17128–17131** | ORDRSP | `mako-gpke` ✅ |
| Änderung Netzengpassmanagement | LF → NB | **17133** | ORDRSP | `mako-gpke` ✅ |

---

### UTILTS — Berechnungsformeln und Zählzeitdefinitionen

**Workflow:** `gpke-utilts` — Inbound-only receive-and-store. No APERAK is triggered
by the LF for UTILTS; the NB expects acknowledgement via CONTRL at transport level.

| PID | Description | Sender → Empfänger |
|---|---|---|
| 25001 | Berechnungsformel | NB → LF |
| 25004 | Übermittlung Übersicht Zählzeitdefinitionen | NB/MSB → LF |
| 25005–25010 | Weitere UTILTS-Varianten (Messwertparameter, etc.) | NB/MSB → LF |

UTILTS is used by the NB to distribute tariff formula structures and meter-reading
time-zone definitions to all connected suppliers. The LF stores these for billing
calculation and pass-through to the ERP system.

**§42b EnWG Solarpaket I — GGV community solar allocation formulas (CCI+ZG6):**
UTILTS PID 25001 is also used to transmit the community solar allocation fractions
for Gemeinschaftliche Gebäudeversorgung (GGV) under §42b Abs. 5 EnWG (Solarpaket I,
2024). The segment `CCI+ZG6` (Aufteilungsfaktor Energiemenge) carries the fraction
parameter for each tenant MaLo. `edmd` evaluates these formulas via the
`metering::AggregationRule::GgvConstantAllocation` and
`metering::AggregationRule::GgvProportionalAllocation` variants — see the
[edmd operator guide](../edmd#virtual-meters-42b-engw-ggv--solarpaket-i) for details
on the computation and the §42b Abs. 5 `Pos()` cap.

---

### MSCONS — Zählerstandsübermittlung

**Workflow:** `gpke-messwerte`

MSCONS messages carry meter readings, load profiles, and interval metered values.
The NB sends MSCONS to the LF at defined reporting intervals and at Lieferbeginn/
Lieferende. The LF acknowledges with APERAK within 24 h.

| Context | Sender → Empfänger | Trigger |
|---|---|---|
| Turnusablesung | NB → LF | Annual or quarterly meter read |
| Lieferbeginn | NB → LF | At or shortly after Lieferbeginn-Datum |
| Lieferende | NB → LF | Final reading at Lieferende |
| Nachlieferung | NB → LF | Late-arriving corrected values |

---

### GPKE IFTSTA — Vollzugsmeldungen, Statusmeldungen, EnFG (GPKE Teil 2/3/4)

IFTSTA messages in the GPKE family carry supplier-change execution confirmations
(Vollzugsmeldungen), Konfigurationsänderung responses, and EnFG-related status
notifications (privilege information and billing status under the
Energiefinanzierungsgesetz, 2023). All are routed to the relevant `mako-gpke`
workflow for correlation; no separate receipt-only workflow exists.

| IFTSTA PID | Description (IFTSTA AHB) | Sender → Empfänger | Crate |
|---|---|---|---|
| 21024 | Vollzugsmeldung Lieferantenwechsel | NB → LF | `mako-gpke` `gpke-supplier-change` ✅ |
| 21025 | Vollzugsmeldung Einzug | NB → LF | `mako-gpke` `gpke-supplier-change` ✅ |
| 21026 | Vollzugsmeldung Auszug | NB → LF | `mako-gpke` `gpke-supplier-change` ✅ |
| 21027 | Vollzugsmeldung Netznutzung | NB → LF | `mako-gpke` `gpke-supplier-change` ✅ |
| 21028 | Vollzugsmeldung | NB → LF | `mako-gpke` `gpke-supplier-change` ✅ |
| 21033 | Statusmeldung Kündigung | MSB → NB/LF | `mako-gpke` `gpke-supplier-change` ✅ |
| 21035 | Rückmeldung an Lieferstelle (GPKE Teil 2) | MSB → LF | `mako-gpke` `gpke-supplier-change` ✅ |
| 21043 | Bestellungsantwort / -mitteilung (GPKE Teil 3) | NB → LF · MSB → MSB · MSB → NB · MSB → LF | `mako-gpke` `gpke-konfiguration-aenderung` ✅ |
| 21044 | Bestellungsbeendigung (GPKE Teil 3) | MSB → NB · MSB → LF | `mako-gpke` `gpke-konfiguration-aenderung` ✅ |
| 21045 | EnFG Informationen (GPKE Teil 4) | LF → NB | `mako-gpke` `gpke-supplier-change` ✅ |
| 21047 | Bearbeitungsstandsmeldung (GPKE Teil 2/4) | NB → LF · NB → ÜNB · MSB → NB · MSB → LF | `mako-gpke` `gpke-supplier-change` ✅ |

> PID 21042 (Privilegierungsinformation EnFG, NB → LF) is a WiM Strom Teil 2
> message (MSB/ESA domain). It is not registered in `mako-gpke`.

> **Why are 17134/17135/17121/17128–17131 NB→MSB PIDs in GPKE, not WiM?**
> GPKE governs *what metering configuration is required* after a supplier change
> and *who can authorize disconnection* (BK6-22-024). WiM governs *which company
> provides the metering service* (BK6-24-174). These are orthogonal obligations:
> GPKE Teil 3/4 obligates the NB to configure the MSB after confirming
> `Lieferbeginn`; WiM Teil 1 governs the MSB-Wechsel process itself. A combined
> Stadtwerke NB+MSB operator implements both crates simultaneously.

---

## WiM Strom — Messstellenbetrieb

**Regulatory basis:** BK6-24-174 (Beschluss 24.10.2024, gültig ab 06.06.2025)

**APERAK Frist:** **5 Werktage** (Samstag = Werktag)

WiM regulates the competitive metering point market. The key processes from the LF
perspective are: MSB-Wechsel (when the customer switches their metering service
provider) and receiving WiM-Rechnungen for metering services.

### MSB-Wechsel Strom

| Process | Initiator → Responder | UTILMD PID | Antwort OK | Antwort NG | Frist | Crate |
|---|---|---|---|---|---|---|
| Kündigung MSB (neuer MSB initiiert) | MSBN → MSBA | **55039** | 55040 | 55041 | **3 WT** | `mako-wim` ✅ |
| Anmeldung MSB beim NB | MSBN → NB | **55042** | 55043 | 55044 | **5 WT** | `mako-wim` ✅ |
| Ende MSB (alter MSB → NB) | MSBA → NB | **55051** | 55052 | 55053 | **7 WT** | `mako-wim` ✅ |
| Verpflichtungsanfrage / Aufforderung | NB → gMSB | **55168** | 55169 | 55170 | **1 WT** | `mako-wim` ✅ |

The **Antwortfrist differs per process** (BK6-24-174 WiM Teil 1 Kap. 2.2.2 / 2.3.2 /
2.4.2) and is distinct from the APERAK window, which is 45 minutes for UTILMD in
Strom (APERAK AHB §2.4.1). `geraetewechsel::antwort_frist_werktage(pid)` is the
single source for these values.

The Kündigung (55039) runs on the **contract layer between the two MSB** and never
reaches the NB. Per Kap. 2.1.3 it is explicitly *non-constitutive*: the switch is
effected solely by a successful Anmeldung MSBN → NB, so 55042 must never be gated
on a 55040 Bestätigung.

### Geräteübernahme und Stammdaten

| Process | Initiator → Responder | ORDERS PID | Antwort | Crate |
|---|---|---|---|---|
| Anzeige Gerätewechselabsicht | MSBN → MSBA | ORDERS **17009** | ORDRSP 19015/19016 | `mako-wim` ✅ |
| Bestellung Angebot Änderung Technik | NB/LF → MSB | ORDERS **17011** | — | `mako-wim` ✅ |
| Stammdaten Messlokation (Strom) | LF/MSB → NB | ORDERS **17132** | — | `mako-wim` ✅ |
| Geräteübernahme Bestellung | MSBN → MSBA | ORDERS **17001/17002** | ORDRSP | `mako-wim` ✅ |

### WiM-Abrechnung

| Process | Sender → Empfänger | INVOIC PID | Content | Crate |
|---|---|---|---|---|
| MSB-Rechnung | MSB → LF | INVOIC **31009** | Messstellenbetriebsabrechnung | `mako-wim` ✅ |
| Stornierung WiM | ESA → MSB | ORDCHG **39002** | Storno Bestellung von Werten | `mako-wim` ✅ |

### Technik-Änderung und Gerätekonfiguration

**Workflow:** `wim-technik-aenderung`

ORDERS-based requests for device or configuration changes defined in WiM Strom
Teil 1 and AWH Änderung Technik (BK6-24-174). APERAK Frist: **5 Werktage**.

| Process | Initiator → Responder | ORDERS PID | Antwort | Crate |
|---|---|---|---|---|
| Beauftragung Änderung Technik (Gas / MeLo) | LF → MSB | **17003** | ORDRSP 19005/19006 | `mako-wim` ✅ |
| Bestellung Werte ESA | LF/NB → MSB | **17007** | ORDRSP 19011/19012 | `mako-wim` ✅ |
| Abbestellung Werte ESA | ESA → MSB | **17008** | ORDRSP 19007 (Ablehnung) | `mako-wim` ✅ |
| Konfigurationsänderung (MSB → MSB) | MSB → MSB | **17118** | ORDRSP 19003/19004 | `mako-wim` ✅ |


> **ORDRSP semantics:** 19005 = Auftragsbestätigung Änderung Technik · 19006 = Ablehnung ·
> 19011 = Bestätigung Ab-/Bestellung Werte ESA · 19012 = Ablehnung ·
> 19007 = Ablehnung Anforderung Messwerte · 19003 = Fortführungsbestätigung (MSB→MSB) ·
> 19004 = Ablehnung Fortführung. All ORDRSP 19003–19012 route to `wim-technik-aenderung`.
### Preisanfrage, Angebote und Preislisten

Allows market participants to request and receive price offers (REQOTE/QUOTES)
and price lists (PRICAT) for MSB services before committing to a device takeover
or configuration change. **Workflow:** `wim-preisanfrage` / `wim-preisliste`.

**Preisanfrage / Angebot:**

| Process | Initiator → Responder | PID | Crate |
|---|---|---|---|
| Anfrage Geräteübernahmeangebot | MSBN → MSBA | REQOTE **35001** | `mako-wim` ✅ |
| Anfrage Rechnungsabwicklung MSB | LF → MSB | REQOTE **35002** | `mako-wim` ✅ |
| Anfrage Werte | ESA → MSB | REQOTE **35003** | `mako-wim` ✅ |
| Anfrage Konfigurationsangebot | NB/LF → MSB | REQOTE **35004** | `mako-wim` ✅ |
| Anfrage Angebot Änderung Technik | NB/LF → MSB | REQOTE **35005** | `mako-wim` ✅ |
| Angebot Geräteübernahme | MSBA → MSBN | QUOTES **15001** | `mako-wim` ✅ |
| Angebot Rechnungsabwicklung MSB | MSB → LF | QUOTES **15002** | `mako-wim` ✅ |
| Angebot Werte | MSB → ESA | QUOTES **15003** | `mako-wim` ✅ |
| Angebot Konfiguration | MSB → NB/LF | QUOTES **15004** | `mako-wim` ✅ |
| Angebot Änderung Technik | MSB → NB/LF | QUOTES **15005** | `mako-wim` ✅ |

**Preislisten (PRICAT):**

| Process | Sender → Empfänger | PID | Content | Crate |
|---|---|---|---|---|
| Ausgleichsenergiepreis | BIKO → BKV | PRICAT **27001** | Settlement energy price | `mako-wim` ✅ |
| Preisblätter MSB-Leistungen | MSB → NB/LF | PRICAT **27002** | MSB service price list | `mako-wim` ✅ |
| Preisblätter NB-Leistungen | NB → LF | PRICAT **27003** | NB service price list (incl. Sperrprozesse) | `mako-wim` ✅ |

### Steuerungsauftrag (API-Webdienste Strom)

**Workflow:** `wim-steuerungsauftrag` — **REST-based, not EDIFACT/AS4.**

The Steuerungsauftrag handles remote load control commands
(`controlMeasuresV1`) via HTTPS using the **BDEW API-Webdienste Strom**
interface (API-Guideline 1.0a, BK6-18-032). APERAK Frist: **5 Werktage**.

| Step | Sender → Empfänger | Transport | Description |
|---|---|---|---|
| Konfiguration / InitialZustand | NB/LF → MSB | REST JSON | Command dispatch |
| Sofortquittung | MSB → NB/LF | REST 202 Accepted | Immediate receipt |
| Vorläufige Antwort | MSB → NB/LF | REST JSON | Feasibility confirmed |
| Endantwort (positiv/negativ) | MSB → NB/LF | REST JSON | Execution result |

> This workflow has no EDIFACT Prüfidentifikator and is not listed in the BDEW
> PID overview. It is implemented as an event-sourced workflow over the REST
> channel; AS4 is not involved.

### IFTSTA Status (WiM Strom)

IFTSTA messages carry status updates that the NB or MSB sends to inform the LF or
the outgoing MSB about the progress of an ongoing WiM process. The LF receives
these passively — no workflow state change is required on the LF side.

| IFTSTA PID | Description | Sender → Empfänger |
|---|---|---|
| 21007 | Statusmeldung Gerätewechsel | NB → MSBA / NB → LF |
| 21009 | Statusmeldung MSB-Wechsel nach MsbG an LF | NB → LF |
| 21010 | Statusmeldung MSB-Wechsel nach MsbG an NB | MSB alt → NB |
| 21011 | Statusmeldung MSB-Wechsel nach MsbG an NB | MSB neu → NB |
| 21012 | Statusmeldung MSB-Wechsel nach MsbG an BKV | NB → BKV |
| 21013 | Statusmeldung MSB-Wechsel nach MsbG an ÜNB | NB → ÜNB |
| 21015 | Statusmeldung Einbau iMS | wMSB → gMSB |
| 21018 | Statusmeldung Anforderung Datenzugang | MSB → LF |
| 21029 | Vorabinformation iMS-Einbau | wMSB → NB |
| 21030 | iMS-Ersteinbauzustand | wMSB → gMSB |
| 21031 | Bestandssituation / Eigenausbau iMS | wMSB → gMSB |
| 21032 | Antwort auf das Angebot | LF → MSB |

All PIDs above are routed to `mako-wim` `wim-device-change` ✅.

### INSRPT — Störungsmeldungen (WiM Strom)

**Workflow:** `wim-insrpt` — APERAK Frist: **5 Werktage**

Fault and interruption reports sent to the MSB when a supply-point problem is
detected. The MSB responds with a confirmation or rejection within 5 Werktage.
PIDs 23011/23012 (Strom-only Ergebnisbericht variants) are registered exclusively
in `mako-wim`; PIDs 23005/23009 (Gas-only variants) belong to `mako-wim-gas`.

| PID | Process | Sender → Empfänger | Crate |
|---|---|---|---|
| 23001 | Störungsmeldung | LF/NB/Melder → MSB | `mako-wim` `wim-insrpt` ✅ |
| 23003 | Ablehnung Störungsmeldung | MSB → LF/NB/Melder | `mako-wim` `wim-insrpt` ✅ |
| 23004 | Bestätigung Störungsmeldung | MSB → LF/NB/Melder | `mako-wim` `wim-insrpt` ✅ |
| 23008 | Ergebnisbericht (gemeinsam) | MSB → LF/NB/Melder | `mako-wim` `wim-insrpt` ✅ |
| 23011 | Ergebnisbericht Strom (Variante 1) | MSB → LF/NB/Melder | `mako-wim` `wim-insrpt` ✅ |
| 23012 | Ergebnisbericht Strom (Variante 2) | MSB → LF/NB/Melder | `mako-wim` `wim-insrpt` ✅ |

> PIDs 23005 and 23009 (Gas-only Ablehnung/Ergebnisbericht variants) are handled
> by `mako-wim-gas` `wim-gas-insrpt` with a **10-Werktage** deadline. See
> [WiM Gas — INSRPT Störungsmeldungen](#wim-gas--insrpt-störungsmeldungen).

---

## MaBiS — Bilanzkreisabrechnung Strom

**Regulatory basis:** BK6-24-174 (Anlage 3 MaBiS, gültig ab 06.06.2025)

**Architecture note:** MaBiS is a **batch projection**, not a per-MaLo saga.
`mako-mabis` uses `ProjectionRunner::catch_up_persistent` to aggregate metering
data across all MaLo streams for a billing period, then produces MSCONS output.
There is no per-process deadline (Frist) — the submission windows are calendar-based.

| Process | Roles | Message | PID | Crate |
|---|---|---|---|---|
| Summenzeitreihe (BKV-Abrechnung) | ÜNB → BKV | MSCONS | **13003** | `mako-mabis` ✅ |
| Statusmeldung BKV | LF/NB/BKV → NB/ÜNB | IFTSTA | **21000–21005** | `mako-mabis` ✅ |
| Clearingliste DZR | BIKO → NB/ÜNB | UTILMD | **55069** | `mako-mabis` ✅ |
| Clearingliste BAS | BIKO → BKV | UTILMD | **55070** | `mako-mabis` ✅ |
| Lieferantenclearingliste | NB → LF | UTILMD | **55065** | `mako-mabis` ✅ |

---

## GeLi Gas — Lieferantenwechsel Gas

**Regulatory basis:** BK7-24-01-009 (Beschluss 12.09.2025, gültig ab 24.09.2025)
Supersedes BK7-19-001 and BK7-06-067.

**APERAK Frist:** **10 Werktage** (longest Frist across all MaKo process families)

**Key difference from electricity:** Gas uses **MaLo** (not MeLo) as the supply
object. The grid operator is called **GNB** (Gasnetzbetreiber), not NB.

---

### Lieferantenwechsel Gas

**Process timing rules (BK7-24-01-009):**

| Scenario | Mindestvorlauffrist | Notes |
|---|---|---|
| Standardwechsel | 10 Werktage vor Lieferbeginn | Eingang bei GNB und LFA auf demselben Werktag |
| Schneller Wechsel | **nicht anwendbar** | Gas kennt keinen schnellen Lieferantenwechsel |
| Neuanlage | keine Mindestfrist | Lieferbeginn = frühestmöglicher Termin |
| Stornierung | bis 24 h vor Lieferbeginn | Nur der ursprüngliche Sender darf stornieren |

> **GNB-role note:** In the gas market the grid operator is always called **GNB**
> (Gasnetzbetreiber). Messages are addressed to the GNB by EIC/GLN. The GNB
> coordinates with the outgoing LF (LFA) automatically after receiving the LFN's
> Anmeldung.

| Process | Initiator → Responder | UTILMD PID | Antwort OK | Antwort NG | Crate |
|---|---|---|---|---|---|
| Lieferbeginn Gas (LF-AN) | LFN → GNB | UTILMD G **44001** | 44003 | 44004 | `mako-geli-gas` ✅ |
| Lieferende Gas | LFN → GNB | UTILMD G **44002** | 44005 | 44006 | `mako-geli-gas` ✅ |
| Abmeldung NN (GNB → LFN) | GNB → LFN | UTILMD G **44007** | 44008 | 44009 | `mako-geli-gas` ✅ |
| Abmeldungsanfrage (GNB → LFA) | GNB → LFA | UTILMD G **44010** | 44011 | 44012 | `mako-geli-gas` ✅ |
| EoG Anmeldung (GNB → LF) | GNB → LF | UTILMD G **44013** | 44014 | 44015 | `mako-geli-gas` ✅ |
| Kündigung beim alten Lieferanten | LFN → LFA | UTILMD G **44016** | 44017 | 44018 | `mako-geli-gas` ✅ |
| Bestandsliste (GNB → LF) | GNB → LF | UTILMD G **44019** | — | — | `mako-geli-gas` ✅ |
| Änderungsmeldung zur Bestandsliste | LF → GNB | UTILMD G **44020** | 44021 | — | `mako-geli-gas` ✅ |
| Stornierung (GNB-side, inbound) | LFN/LFA → GNB | UTILMD G **44022** | — | — | `mako-geli-gas` ✅ |
| Stornierung (LF-side, inbound) | GNB → LFN/LFA | UTILMD G **44023/44024** | — | — | `mako-geli-gas` ✅ |

> **PIDs 44022–44024** (Stornierung) are multi-domain: GeLi Gas 2.0 (supply
> cancellation by LFN/LFA ↔ GNB) and WiM Gas (MSB-change cancellation by gMSB).
> Role-conditional routing is implemented in `mako-geli-gas`:
> - `Nb`-only: PID 44022 → `geli-gas-stornierung` (GNB receives Anfrage)
> - `Lf`-only: PIDs 44023/44024 → `geli-gas-stornierung-lf` (LF receives GNB response)
> - `Msb`/`Nmsb`/`all()`: `mako-wim-gas` `wim-gas-stornierung` handles all three

---

#### LF-seitige Einreichung (geli-gas-lf-anmeldung)

When makod is deployed in the **LF role**, the LF initiates the Lieferbeginn Gas by sending
UTILMD G 44001 outbound to the GNB. The response arrives inbound as 44003 (Bestätigung)
or 44004 (Ablehnung). This mirrors the GPKE `gpke-lf-anmeldung` workflow for Strom.

**Workflow:** `geli-gas-lf-anmeldung` — APERAK Frist: **10 Werktage**

| Direction | Message | PID | Role |
|---|---|---|---|
| Outbound (LFN → GNB) | Anmeldung Lieferbeginn | UTILMD G **44001** | LFN initiates |
| Inbound (GNB → LFN) | Bestätigung Lieferbeginn | UTILMD G **44003** | GNB confirms |
| Inbound (GNB → LFN) | Ablehnung Lieferbeginn | UTILMD G **44004** | GNB rejects |
| Outbound (LFN → LFA) | Kündigung beim alten LF | UTILMD G **44016** | Concurrent with 44001 |
| Inbound (LFA → LFN) | Bestätigung Kündigung | UTILMD G **44017** | LFA confirms |
| Inbound (LFA → LFN) | Ablehnung Kündigung | UTILMD G **44018** | LFA rejects |

> **Einreichungstag rule (Gas):** Like GPKE Strom, UTILMD G 44001 (LFN → GNB) and
> UTILMD G 44016 (LFN → LFA) must be submitted on the **same Werktag**.
> The Mindestvorlauffrist for a Standardwechsel is **10 Werktage** — significantly
> longer than the GPKE 7-Werktage window. Gas has **no fast-switch equivalent**
> (`Schneller Lieferantenwechsel` does not exist in Gas; BK7-24-01-009 §2.1).

```mermaid
sequenceDiagram
    participant LFN as Neuer LF (LFN)
    participant GNB as Gasnetzbetreiber (GNB)
    participant LFA as Alter LF (LFA)

    Note over LFN,LFA: Einreichungstag = gleichzeitig
    LFN->>GNB: UTILMD G 44001 (Anmeldung Lieferbeginn)
    LFN->>LFA: UTILMD G 44016 (Kündigung beim alten LF)

    Note over LFN,GNB: Frist: 10 Werktage (keine Express-Option)

    alt GNB bestätigt
        GNB-->>LFN: UTILMD G 44003 (Bestätigung Lieferbeginn)
        LFA-->>LFN: UTILMD G 44017 (Bestätigung Kündigung)
    else GNB lehnt ab
        GNB-->>LFN: UTILMD G 44004 (Ablehnung Lieferbeginn)
    else LFA lehnt ab
        LFA-->>LFN: UTILMD G 44018 (Ablehnung Kündigung)
    end

    Note over LFN,LFA: Zum Lieferbeginn-Datum
    GNB->>LFA: UTILMD G 44007 (Abmeldung NN)
    LFA-->>GNB: UTILMD G 44008 (Bestätigung) oder 44009 (Ablehnung)
```

---

**Message flow — Lieferbeginn Gas (GNB-Sicht):**

```mermaid
sequenceDiagram
    participant LFN as Neuer LF (LFN)
    participant GNB as Gasnetzbetreiber (GNB)
    participant LFA as Alter LF (LFA)

    LFN->>GNB: UTILMD G 44001 (Anmeldung Lieferbeginn Gas)
    LFN->>LFA: UTILMD G 44016 (Kündigung beim alten Lieferanten)

    alt Bestätigung
        GNB-->>LFN: UTILMD G 44003 (Bestätigung Lieferbeginn)
        LFA-->>LFN: UTILMD G 44017 (Bestätigung Kündigung)
    else Ablehnung durch GNB
        GNB-->>LFN: UTILMD G 44004 (Ablehnung Lieferbeginn)
    else Ablehnung durch LFA
        LFA-->>LFN: UTILMD G 44018 (Ablehnung Kündigung)
    end

    Note over LFN,LFA: Zum Lieferbeginn-Datum
    GNB->>LFA: UTILMD G 44007 (Abmeldung NN)
    LFA-->>GNB: UTILMD G 44008 (Bestätigung) oder 44009 (Ablehnung)
```

---

### Sperrung / Entsperrung Gas

The gas disconnection / reconnection process (LF-initiated) follows the same PID
numbers as the Strom Sperrung, but runs between the LF and the GNB on a Gas MaLo
and is governed by BK7-24-01-009 with a **10-Werktage deadline** instead of 24 h.

**LF-Seite** — `geli-gas-sperrung-lf` (LF initiates, awaits GNB response)

| Process | Initiator → Responder | Anfrage PID | Antwort OK | Antwort NG | Crate |
|---|---|---|---|---|---|
| Gas-Sperrauftrag senden | LF → GNB | ORDERS **17115** | ORDRSP 19116 | ORDRSP 19117 | `mako-geli-gas` `geli-gas-sperrung-lf` ✅ |
| Gas-Entsperrauftrag senden | LF → GNB | ORDERS **17117** | ORDRSP 19116 | ORDRSP 19117 | `mako-geli-gas` `geli-gas-sperrung-lf` ✅ |
| Stornierung Sperrauftrag senden | LF → GNB | ORDCHG **39000** | ORDRSP 19128 | ORDRSP 19129 | `mako-geli-gas` `geli-gas-sperrung-lf` ✅ |

**GNB-Seite** — `geli-gas-sperrung-nb` (GNB receives, forwards to gMSB, confirms to LF)

| Process | Initiator → Responder | Anfrage PID | Antwort OK | Antwort NG | Crate |
|---|---|---|---|---|---|
| Sperrauftrag empfangen (GNB) | LF → GNB | ORDERS **17115** | ORDRSP 19116 | ORDRSP 19117 | `mako-geli-gas` `geli-gas-sperrung-nb` ✅ |
| Entsperrauftrag empfangen (GNB) | LF → GNB | ORDERS **17117** | ORDRSP 19116 | ORDRSP 19117 | `mako-geli-gas` `geli-gas-sperrung-nb` ✅ |
| Anfrage Sperrung an gMSB | GNB → gMSB | ORDERS **17116** | ORDRSP **19118** | ORDRSP **19119** | `mako-geli-gas` `geli-gas-sperrung-nb` ✅ |
| Stornierung empfangen (GNB) | LF → GNB | ORDCHG **39000** | ORDRSP 19128 | ORDRSP 19129 | `mako-geli-gas` `geli-gas-sperrung-nb` ✅ |
| Weiterleitung Stornierung (GNB → gMSB) | GNB → gMSB | ORDCHG **39001** | — | — | `mako-geli-gas` `geli-gas-sperrung-nb` ✅ |

> **Same PIDs, different market.** ORDERS 17115 and 17117 are used for both
> **Strom Sperrung** (routed to `mako-gpke`) and **Gas Sperrung** (routed to
> `mako-geli-gas`). The routing is determined at dispatch time by the commodity
> field in the ORDERS message header and the deployment role of the receiving party.

**Message flow — Gas-Sperrauftrag (LF-initiiert):**

**LF-Sicht** (LF initiates, `geli-gas-sperrung-lf`):

```mermaid
sequenceDiagram
    participant LF
    participant GNB as Gasnetzbetreiber (GNB)

    LF->>GNB: ORDERS 17115 (Gas-Sperrauftrag, LF → GNB)
    Note over LF,GNB: GNB hat 10 Werktage Zeit (BK7-24-01-009)

    alt Bestätigung
        GNB-->>LF: ORDRSP 19116 (Bestätigung Gas-Sperrauftrag)
    else Ablehnung
        GNB-->>LF: ORDRSP 19117 (Ablehnung Gas-Sperrauftrag)
    end

    opt Stornierung (vor GNB-Antwort)
        LF->>GNB:  ORDCHG 39000 (Stornierung Gas-Sperrauftrag)
        GNB-->>LF: ORDRSP 19128 (Bestätigung) oder 19129 (Ablehnung)
    end
```

**GNB-Sicht** (GNB receives, forwards to gMSB, `geli-gas-sperrung-nb`):

```mermaid
sequenceDiagram
    participant LF
    participant GNB as Gasnetzbetreiber (GNB)
    participant gMSB

    LF->>GNB:  ORDERS 17115 (Gas-Sperrauftrag)
    GNB->>gMSB: ORDERS 17116 (Anfrage Sperrung)
    gMSB-->>GNB: ORDRSP 19118 (Bestätigung) oder 19119 (Ablehnung)

    alt gMSB bestätigt
        GNB-->>LF: ORDRSP 19116 (Bestätigung Gas-Sperrauftrag)
    else gMSB lehnt ab
        GNB-->>LF: ORDRSP 19117 (Ablehnung Gas-Sperrauftrag)
    end

    opt Stornierung
        LF->>GNB:  ORDCHG 39000 (Stornierung)
        GNB->>gMSB: ORDCHG 39001 (Weiterleitung Stornierung)
        GNB-->>LF: ORDRSP 19128 (Bestätigung) oder 19129 (Ablehnung)
    end
```

---

### Gas Abrechnung — Billing Scope

| INVOIC PID | Content | Sender → Empfänger | Crate |
|---|---|---|---|
| **31005** | Netznutzungsentgelt Gas (GasNEV §14) | NB → LF | `netzbilanzd` ✅ |
| **31011** | AWH Sperrprozesse Gas | GNB/VNB → LF | `mako-geli-gas` ✅ |
| **31003** | WiM Gas Rechnung (Gerätewechsel) | gMSB → NB | `mako-wim-gas` ⚠️ |
| **31004** | Stornorechnung WiM Gas | gMSB → NB | `mako-wim-gas` ⚠️ |
| **31007/31008** | Aggreg. MMM-Rechnung Gas | NB → MGV | `mako-gabi-gas` ✅ |
| **31010** | Kapazitätsabrechnung Gas | GNB → KN | `mako-gabi-gas` ✅ |

---

### Gas Datenabruf

Data retrieval processes for gas-specific values. The positive response is the
actual data (MSCONS or similar); ORDRSP is sent only for rejections.

| Process | Initiator → Responder | ORDERS PID | Ablehnung PID | Crate |
|---|---|---|---|---|
| Anfrage Abrechnungsbrennwert / Zustandszahl | LF → GNB/MSB | ORDERS **17103** | ORDRSP 19103 | `mako-geli-gas` ✅ |
| Anfrage MSB Gas an NB Strom (Messwerte) | MSB Gas → NB Strom | ORDERS **17104** | ORDRSP 19104 | `mako-geli-gas` ✅ |
| Anfrage Stammdaten MaLo Gas | LF → GNB | ORDERS **17101** | ORDRSP 19101 | — |
| Anfrage Stammdaten MeLo Gas | MSB → GNB | ORDERS **17126** | — | — |

### MSCONS Gas — Messwert- und Energiemengenübermittlung

**Workflow:** `geli-gas-mscons`

Gas meter readings, load profiles, energy quantities, and gas quality values
delivered via MSCONS by the GNB or MSB to the LF. The LF acknowledges
with APERAK within **10 Werktage** (BK7-24-01-009).

| PID | Content | Sender → Empfänger |
|---|---|---|
| **13002** | Zählerstand Gas | MSBA/MSBN → GNB · GNB → LF |
| **13007** | Gasbeschaffenheit (Brennwert, Zustandszahl) | GNB → LF · MSBA → GNB |
| **13008** | Lastgang Gas | GNB → LF · MSBA → GNB |
| **13009** | Energiemenge Gas | MSBA/MSBN → GNB · GNB → LF |

> **PIDs 13013 and 13014** are listed here for cross-reference only.
> **13013** (Allokationsliste Gas, MMMA) belongs to `mako-gabi-gas` (`gabi-gas-mmma`) — GaBi Gas
> billing domain (BK7-24-01-008). **13014** (Bilanzierte Menge Gas/Strom) is a GaBi Gas/ÜNB process.
> Neither is registered under `mako-geli-gas`; Gas-only deployments that do not load `mako-gabi-gas`
> will dead-letter these PIDs.

| PID | Content | Sender → Empfänger | Crate |
|---|---|---|---|
| **13002** | Zählerstand Gas | MSBA/MSBN → GNB · GNB → LF | `mako-geli-gas` ✅ |
| **13007** | Gasbeschaffenheit (Brennwert, Zustandszahl) | GNB → LF · MSBA → GNB | `mako-geli-gas` ✅ |
| **13008** | Lastgang Gas | GNB → LF · MSBA → GNB | `mako-geli-gas` ✅ |
| **13009** | Energiemenge Gas | MSBA/MSBN → GNB · GNB → LF | `mako-geli-gas` ✅ |
| **13013** | Allokationsliste Gas (MaLo-scharf, MMMA) | GNB → MGV | **`mako-gabi-gas`** `gabi-gas-mmma` — GaBi Gas domain |
| **13014** | Bilanzierte Menge Gas/Strom (MaLo-scharf) | ÜNB → GNB · GNB → LF | **`mako-gabi-gas`** — GaBi Gas domain |

---

### Process Symmetry: GPKE ↔ GeLi Gas

> **Why doesn't GeLi Gas have every GPKE process?**
>
> GPKE (Strom) and GeLi Gas (Gas) share the same *business goals* — supplier switching,
> disconnection, billing, data retrieval — but have structurally different regulatory frameworks.
> The asymmetry is real and intentional, not a documentation or implementation gap.

| GPKE Process (Strom) | GeLi Gas Equivalent | Notes |
|---|---|---|
| Lieferantenwechsel (NB-Sicht) — UTILMD 55001–55018 | Lieferantenwechsel (GNB-Sicht) — UTILMD G 44001–44021 | ✅ Direct equivalent. Gas has no fast-switch option (10 WT only) |
| Lieferantenwechsel (LF-Sicht) — `gpke-lf-anmeldung` | Lieferbeginn (LF-Sicht) — `geli-gas-lf-anmeldung` (44001 out, 44003/44004 in) | ✅ Direct equivalent |
| Abmeldung NB-initiiert — UTILMD 55007–55009 | Abmeldung NN (GNB → LFN) — UTILMD G 44007–44009 | ✅ Direct equivalent |
| Stornierung — UTILMD 55022–55024 | Stornierung — UTILMD G 44022–44024 | ✅ Direct equivalent (role-conditional routing) |
| Sperrung/Entsperrung — ORDERS 17115–17117 | Sperrung/Entsperrung — ORDERS 17115–17117 | ✅ **Same PIDs**, different market; routed by commodity |
| INVOIC NNE Strom — 31001 | **INVOIC 31005** — NNE Gas (NB → LF, GasNEV §14) | ✅ Direct equivalent. `netzbilanzd` `billing_type: "nne_gas"` generates PID 31005 via `SettlementType::NneGas`; same calculation as Strom, legal refs switch to `GasNEV §14` |
| INVOIC MMM Strom — 31002 (NB → LF) | **GaBi Gas** INVOIC 31007/31008 (`mako-gabi-gas`) | ⚠️ Equivalent exists but **different counterparty**: Gas MMM (Aggreg. MMM-Rechnung) flows **NB → MGV** (Marktgebietsverantwortlicher), not NB → LF as in Strom. `invoicd` handles 31007/31008 with MMMA Gas (THE) price check |
| **Neuanlage MaLo** — UTILMD 55600–55605 | Embedded in UTILMD G 44001 (Lieferbeginn) | ⚠️ Gas has no separate "Neuanlage" PID set; new connections use the same 44001 PID as supplier changes |
| **Ankündigung Zuordnung LF** — UTILMD 55607–55609 | ❌ No equivalent | Strom-only balancing group notification (§14a EnWG / iMSys demand response) |
| **UTILTS** — 25001/25004–25010 | ❌ No equivalent | UTILTS carries Zählzeitdefinitionen (HT/NT tariff clocks) and Berechnungsformeln — concepts that don't exist in Gas regulation |
| **Allokationsliste Strom** — ORDERS 17110 · MSCONS 13014 | **GaBi Gas** Allokationsliste — MSCONS 13013 (`mako-gabi-gas`) | Different crate/domain: Gas allocation belongs to GaBi Gas (BK7-24-01-008), not GeLi Gas |
| **Konfiguration / iMSys** — ORDERS 17134/17135 | **WiM Gas** — UTILMD G 44039–44053 | Handled by `mako-wim-gas`; MSB gateway configuration is a WiM concern in both Strom and Gas |
| **GPKE Anfrage Bestellung** — UTILMD 55555 | ❌ No equivalent | Strom-only Stammdaten process for special metering configurations |
| **MSCONS Zählerstand** — 13005/13006 | MSCONS Gas Zählerstand — 13002/13008/13009 | ✅ Equivalent function; Gas uses separate PID range due to Gas-specific Brennwert/Zustandszahl fields |
| Datenabruf — ORDERS 17004/17102 | Datenabruf Gas — ORDERS 17103/17104 | ✅ Direct equivalent (Gas-specific fields: Abrechnungsbrennwert, Zustandszahl) |
| PARTIN Strom — 37000–37006 | PARTIN Gas — 37008–37014 | ✅ Direct equivalent — separate PID ranges for separate partner data schemas |
| IFTSTA 21039 (Sperrung Vollzug) | IFTSTA 21039 (Gas Sperrung Vollzug) | ✅ Same PID, routed by Sparte |

**Key Gas-only processes (no GPKE equivalent):**

| GeLi Gas Process | Reason |
|---|---|
| MSCONS 13007 (Gasbeschaffenheit: Brennwert, Zustandszahl) | Gas physical properties required for billing conversion (m³ → kWh_Hs per DVGW G 685); no Strom analogue |
| INVOIC 31011 (AWH Sperrprozesse) | Gas Sperrung involves separate gMSB layer; GNB bills LF for AWH. Strom Sperrung costs are handled via INVOIC 31001/31002 |
| GaBi Gas ALOCAT/NOMINT/NOMRES/SCHEDL/IMBNOT/TRANOT/DELORD/DELRES (DVGW) | Gas balancing and transport nomination — no Strom equivalent (Strom uses redispatch and BKV processes) |
| Datenabruf 17103/17104 (Brennwert/Zustandszahl) | Gas-specific physical data required for settlement |

---

## WiM Gas — Messstellenbetrieb Gas

**Regulatory basis:** BK7-24-01-009 (Beschluss 12.09.2025, gültig ab 24.09.2025)

**APERAK Frist:** **10 Werktage**

**Implementation status:** Core switching workflows implemented. INVOIC stub
(records receipt, settlement state machine pending). See module status below.

| Process | Initiator → Responder | UTILMD PID | Status | Crate |
|---|---|---|---|---|
| Kündigung MSB Gas (neuer MSB) | gMSBN → gMSBA | UTILMD G **44039–44041** | ✅ | `mako-wim-gas` |
| Anmeldung neuer MSB Gas beim GNB | gMSBN → GNB | UTILMD G **44042–44044** | ✅ | `mako-wim-gas` |
| Ende MSB Gas / Vorläufige Abmeldung | gMSBA → GNB | UTILMD G **44051–44053** | ✅ | `mako-wim-gas` |
| Verpflichtungsanfrage gMSB | GNB → gMSB | UTILMD G **44168–44170** | ✅ | `mako-wim-gas` |
| Stornierung LF/MSB Gas ¹ | orig. → orig. | UTILMD G **44022–44024** | ✅ | `mako-wim-gas` / `mako-geli-gas` |
| Weitere MSB-Wechsel Varianten | gMSBN/GNB | UTILMD G **44045–44050** | 🔄 | `mako-wim-gas` |

> ¹ PIDs 44022–44024 are **multi-domain** per BDEW PID overview. Routing is
> role-conditional:
>
> | Role | PID | Workflow | Crate |
> |---|---|---|---|
> | `Nb`-only (GNB) | 44022 inbound (Anfrage) | `geli-gas-stornierung` | `mako-geli-gas` |
> | `Lf` (LFN/LFA) | 44023/44024 inbound (Bestätigung/Ablehnung) | `geli-gas-stornierung-lf` | `mako-geli-gas` |
> | `Msb`/`Nmsb`/`all()` | 44022–44024 | `wim-gas-stornierung` | `mako-wim-gas` |
>
> For `Nb`-only deployments, PIDs 44023/44024 are outbound (GNB dispatches them
> via outbox) and do not require inbound PID-router registration. For `Lf`
> deployments, PID 44022 is ERP-initiated outbound via `POST /api/v1/commands`
> and also does not need inbound registration.

### WiM Gas Abrechnung

> **Key difference from WiM Strom:** In Strom the MSB bills the **LF** directly
> (INVOIC 31009: MSB → LF). In Gas the gMSB bills the **NB** (INVOIC 31003/31004:
> gMSB → NB). The NB is the contractual counterpart for gas MSB services.

| Message | Sender → Empfänger | PID | Content | Status | Crate |
|---|---|---|---|---|---|
| WiM Gas Rechnung | gMSB → NB | INVOIC **31003** | MSB-Rechnung Gerätewechsel | ⚠️ | `mako-wim-gas` |
| Stornorechnung WiM Gas | gMSB → NB | INVOIC **31004** | Storno MSB-Rechnung | ⚠️ | `mako-wim-gas` |
| Zahlungsavis (NB → gMSB) | NB → gMSB | REMADV **33001/33002** | Payment confirmation/rejection | ⚠️ | `mako-wim-gas` |
| Ablehnung Zahlungsavis | gMSB → NB | COMDIS **29001** | Invoicer disputes REMADV | ⚠️ | `mako-wim-gas` |

### WiM Gas — INSRPT Störungsmeldungen

**Workflow:** `wim-gas-insrpt` — APERAK Frist: **10 Werktage**

Gas fault and interruption reports use the same base INSRPT message type as
WiM Strom, but with Gas-specific qualifier codes and a **10-Werktage deadline**
(BK7-24-01-009). Gas-only PIDs (23005, 23009) are exclusively registered in
`mako-wim-gas`. Shared PIDs (23001, 23003, 23004, 23008) are registered by
`mako-wim` (via `register_with_sparte(pid, Sparte::Strom)`) and by `mako-wim-gas`
(via `register_with_sparte(pid, Sparte::Gas)`) in both standalone and combined deployments.

| PID | Process | Sender → Empfänger | Deployment | Crate |
|---|---|---|---|---|
| 23001 | Störungsmeldung | LF/NB/Melder → MSB | Combined: `route_with_sparte(Strom)→wim-insrpt` · Gas-only: `wim-gas-insrpt` | `wim-insrpt` / `wim-gas-insrpt` |
| 23003 | Ablehnung Störungsmeldung | MSB → LF/NB/Melder | Combined: `route_with_sparte(Strom)→wim-insrpt` · Gas-only: `wim-gas-insrpt` | `wim-insrpt` / `wim-gas-insrpt` |
| 23004 | Bestätigung Störungsmeldung | MSB → LF/NB/Melder | Combined: `route_with_sparte(Strom)→wim-insrpt` · Gas-only: `wim-gas-insrpt` | `wim-insrpt` / `wim-gas-insrpt` |
| 23005 | Ablehnung Gas-Variante | MSB → NB/MSB | **Gas-only** | `mako-wim-gas` `wim-gas-insrpt` ✅ |
| 23008 | Ergebnisbericht (gemeinsam) | MSB → LF/NB/Melder | Combined: `route_with_sparte(Strom)→wim-insrpt` · Gas-only: `wim-gas-insrpt` | `wim-insrpt` / `wim-gas-insrpt` |
| 23009 | Ergebnisbericht Gas-Variante | MSB → NB/MSB | **Gas-only** | `mako-wim-gas` `wim-gas-insrpt` ✅ |

> In a **combined Strom+Gas** `makod` instance, `PidRouter::route_with_sparte` selects the workflow
> by commodity: `Sparte::Strom → wim-insrpt` (5 WT) and `Sparte::Gas → wim-gas-insrpt` (10 WT).
> PIDs 23005/23009 always route to `wim-gas-insrpt` (10 WT) regardless of deployment topology.

---

## GaBi Gas — Kapazitätsabrechnung Gas

**Regulatory basis:** BK7 (Kapazitätsabrechnung Gas / AWH Sperrprozesse Gas) + DVGW G685/G2000

**Implementation status:** `mako-gabi-gas` ✅ for all processes — BK7 billing (INVOIC 31010), DVGW nomination cycle (NOMINT/NOMRES), allocation (ALOCAT), and the full DVGW transport suite (SCHEDL, IMBNOT, TRANOT, DELORD/DELRES).

> **Crate layering:** `dvgw-edi` is the **format library** (parses NOMINT, NOMRES,
> ALOCAT, SCHEDL, …) — analogous to `edi-energy` for EDI@Energy messages.
> `mako-gabi-gas` is the **process layer** built on top of it, handling both
> DVGW transport workflows (nominations, allocations) and BK7 billing (INVOIC
> 31010) — analogous to `mako-gpke` sitting on top of `edi-energy`.

### Gas balancing process flow

```mermaid
sequenceDiagram
    autonumber
    participant BKV as BKV
    participant FNB as FNB / MGV
    participant VNB as VNB

    Note over BKV,FNB: D-1 (deadline 13:00 CET per KoV §3.2)
    BKV->>FNB: NOMINT 90011/90012
    FNB-->>BKV: NOMRES 90021/90022

    Note over BKV,FNB: Day D intraday
    BKV->>FNB: DELORD 90061
    FNB-->>BKV: DELRES 90062
    FNB->>BKV: SCHEDL 90031

    Note over FNB,BKV: After day D (KoV §6.4)
    FNB->>BKV: ALOCAT 90001 (Initial)
    FNB->>BKV: ALOCAT 90001 (Correction 1..n)
    FNB->>BKV: ALOCAT 90001 (Final — binding)
    FNB->>BKV: IMBNOT 90041

    Note over VNB,FNB: Sub-daily
    VNB->>FNB: ALOCAT 90003
    FNB->>BKV: TRANOT 90051
```

### Domain model

`mako-gabi-gas` provides a gas-specific domain vocabulary in `src/domain.rs` and `src/portfolio.rs`.
All energy quantities use `Decimal` — no float arithmetic.

| Type | Description | Key method |
|---|---|---|
| `GasDay` | Typed gas market day. Starts 06:00 CET (DST-aware). | `start_utc()`, `duration_hours()` (23/24/25), `nomination_deadline_utc()` |
| `GasBeschaffenheit` | Brennwert Hs/Hu + Zustandszahl. DVGW G 685/G 260. | `to_kwh_hs(m3)` = m³ × Hs × Z, rounded to 3 dp |
| `GasQuantity` | Gas energy in kWh_Hs with optional m³ context. | `from_m3(vol, beschaffenheit)`, `from_kwh(kwh)` |
| `NominationQuantity` | Submitted / accepted / curtailed breakdown. | `accept_partial(kwh, reason)`, `is_curtailed()` |
| `AllocationVersion` | Initial / Correction(n) / Final per KoV §6.4. | `is_revision()` |
| `GasMarketRole` | Typed BKV/FNB/VNB/MGV/LF/Händler classification. | `submits_nominations()`, `has_imbalance_obligation()` |
| `GasImbalanceSaldo` | Nomination − allocation imbalance. | `direction()` → Mehr / Minder / Balanced |
| `GasPortfolioBalance` | BKV portfolio across all Bilanzkreise. | `net_imbalance_kwh()`, `open_imbalance_count()` |

**DVGW transport processes** (see [DVGW — Gas Transport](#dvgw--gas-transport) for the full PID/message table):

| Process | Roles | Format | Workflow | Crate |
|---|---|---|---|---|
| Nomination / Renomination | BKV → FNB/MGV | NOMINT / NOMRES | `gabi-gas-nomination` | `mako-gabi-gas` ✅ |
| Allocation (Initial/Correction/Final) | FNB/MGV/VNB → BKV | ALOCAT | `gabi-gas-allocation` | `mako-gabi-gas` ✅ |
| Day-ahead schedule | sender → receiver | SCHEDL | `gabi-gas-schedl` | `mako-gabi-gas` ✅ |
| Imbalance notification | FNB/MGV → BKV | IMBNOT | `gabi-gas-imbnot` | `mako-gabi-gas` ✅ |
| Transport notification | FNB/VNB → BKV/GH/MGV | TRANOT | `gabi-gas-tranot` | `mako-gabi-gas` ✅ |
| Delivery order / response | BKV/GH ↔ FNB/MGV | DELORD / DELRES | `gabi-gas-delivery-order` | `mako-gabi-gas` ✅ |

**BK7 billing processes:**

| Process | Sender → Empfänger | INVOIC PID | Content | Crate |
|---|---|---|---|---|
| Kapazitätsrechnung | GNB → KN (Kapazitätsnutzer) | INVOIC **31010** | Kapazitätsabrechnung Gas | `mako-gabi-gas` ✅ |

> **PID 31011** (Rechnung sonstige Leistung / AWH Sperrprozesse Gas, NB → LF) belongs to
> `mako-geli-gas` (BK7-24-01-009), not GaBi Gas. See
> [Gas Abrechnung — Billing Scope](#gas-abrechnung--billing-scope).

---

## PARTIN — Stammdaten Marktpartner

PARTIN messages carry trading-partner master data (GLN, AS4 endpoint, email).
They are not part of any process saga — they update the durable `PartnerStore`
directly on receipt.

**Inbound PARTIN auto-upsert:** Any PARTIN message with a PID in the
37000–37014 range is automatically parsed and merged into the partner store.
No ERP webhook is triggered. A more recent `valid_from` always wins; a
config-bootstrapped record (no `valid_from`) is overwritten by inbound PARTIN data.

| PID range | Description | Commodity | Crate |
|---|---|---|---|
| **37000–37006** | LF, NB, MSB, BKV, BIKO, ÜNB, ESA Kommunikationsdaten | Strom | `mako-gpke` `gpke-partin` ✅ |
| **37008–37014** | LF, GNB, gMSB, MGV, ÜNB, spartenübergreifend Kommunikationsdaten Gas | Gas | `mako-geli-gas` `geli-gas-partin` ✅ |

**COM segment qualifier for AS4:**

The AS4 endpoint URL is carried in the `COM` segment with qualifier `"AK"`
(PARTIN AHB 1.0f, DE 3155). The `PartnerStore` stores this as
`CommunicationChannel { qualifier: "AK", address: "<URL>" }`.

**REST admin endpoints** (see also [makod Operator Guide](makod#partner-management-adminpartners)):

| Method | Path | Description |
|---|---|---|
| `GET` | `/admin/partners` | List all trading-partner records |
| `GET` | `/admin/partners/{mp_id}` | Retrieve a single partner record |
| `PUT` | `/admin/partners/{mp_id}` | Create or update a partner record |
| `DELETE` | `/admin/partners/{mp_id}` | Remove a partner record |
| `POST` | `/admin/partners/import` | Bulk-import from a raw PARTIN interchange |

---

## Redispatch 2.0

**Regulatory basis:** BNetzA BK6 (Beschluss BK6-20-160 und Folgebeschlüsse)

Redispatch 2.0 uses **XML-based messages** (not EDIFACT) alongside IFTSTA status
messages. The `mako-redispatch` crate handles the IFTSTA-based status workflow;
the XML document formats are parsed by `redispatch-xml`.

| Process | Roles | Format | IFTSTA PID | Crate |
|---|---|---|---|---|
| Aktivierungsauftrag | NB → BTR | ActivationDocument (XML) | IFTSTA **21037/21038** | `mako-redispatch` ✅ |
| Kaskade | NB → NB | Kaskade (XML) | — | `redispatch-xml` ✅ |
| Stammdaten | NB → BTR | Stammdaten (XML) | — | `redispatch-xml` ✅ |
| Netzrestriktion | NB → BTR | NetworkConstraintDocument (XML) | — | `redispatch-xml` ✅ |
| PlannedResourceSchedule | NB → NB | PlannedResourceScheduleDocument (XML) | — | `redispatch-xml` ✅ |

**Message flow — Redispatch Aktivierung:**

```mermaid
sequenceDiagram
    participant NB  as Netzbetreiber (NB)
    participant BTR as Betreiber techn. Ressource (BTR)

    NB->>BTR:  ActivationDocument (XML) — Redispatch-Auftrag
    BTR-->>NB: IFTSTA 21038 (Ansicht BTR — Annahme/Ablehnung)
    NB-->>BTR: IFTSTA 21037 (Ansicht NB — Bestätigung)
```

---

## DVGW — Gas Transport

DVGW EDIFACT messages handle gas transport nominations, allocations, and schedules
between FNBs (Fernleitungsnetzbetreiber), BKVs, and MGVs. They carry **no BGM
Prüfidentifikator**; routing uses synthetic PIDs (90000–90999) derived from
`(message_type, role_qualifier)`.

> **Crate layering.** `dvgw-edi` is the **format library** — it parses and
> validates DVGW EDIFACT messages (NOMINT, NOMRES, ALOCAT, SCHEDL, …),
> analogous to `edi-energy` for EDI@Energy. `mako-gabi-gas` is the
> **process layer** on top of it, implementing GABi Gas workflows (both
> DVGW transport and BK7 billing) — analogous to `mako-gpke` on top of
> `edi-energy`. See [GaBi Gas — Kapazitätsabrechnung Gas](#gabi-gas--kapazitätsabrechnung-gas)
> for the full process table.

See [DVGW EDI](dvgw) for the full regulatory basis and parsing architecture.

| Synthetic PID | Message | Direction | Description | Workflow |
|---|---|---|---|---|
| 90001 | ALOCAT | FNB → BKV | Daily allocation | `gabi-gas-allocation` ✅ |
| 90002 | ALOCAT | MGV → BKV | Monthly allocation | `gabi-gas-allocation` ✅ |
| 90003 | ALOCAT | VNB → FNB | Sub-daily allocation | `gabi-gas-allocation` ✅ |
| 90011 | NOMINT | BKV → FNB | Nomination | `gabi-gas-nomination` ✅ |
| 90012 | NOMINT | BKV → MGV | Nomination | `gabi-gas-nomination` ✅ |
| 90021 | NOMRES | FNB → BKV | Nomination response | `gabi-gas-nomination` ✅ |
| 90022 | NOMRES | MGV → BKV | Nomination response | `gabi-gas-nomination` ✅ |
| 90031 | SCHEDL | FNB/BKV/MGV → receiver | Day-ahead transport schedule | `gabi-gas-schedl` ✅ |
| 90041 | IMBNOT | FNB/MGV → BKV | Imbalance notification | `gabi-gas-imbnot` ✅ |
| 90051 | TRANOT | FNB/VNB → BKV/GH/MGV | Transport notification | `gabi-gas-tranot` ✅ |
| 90061 | DELORD | BKV/GH → FNB/MGV | Delivery order | `gabi-gas-delivery-order` ✅ |
| 90062 | DELRES | FNB/MGV → BKV/GH | Delivery response | `gabi-gas-delivery-order` ✅ |

---

## Cross-Process Notes

### APERAK — Universal acknowledgement

Every EDIFACT message exchange has an APERAK acknowledgement layer. The sender
expects an APERAK within the applicable Frist. An APERAK can carry:

- **Acceptance** (`Z01`) — message syntactically and semantically valid
- **Functional rejection** (`Z04`) — AHB rule violation
- **Technical rejection** (`Z07`) — message could not be processed

The APERAK does not signal process acceptance/rejection — that is done by the
substantive response (e.g. UTILMD 55002/55003). APERAK is purely the
**technical receipt acknowledgement**.

### CONTRL — Syntactic Transport Acknowledgement

CONTRL is distinct from APERAK. It operates at the **transport/interchange level**
(between AS4 Message Service Handlers) and confirms that the EDIFACT interchange
was syntactically parseable. CONTRL is exchanged automatically by the AS4 MSH and
is never exposed to the workflow layer.

| Level | Message | Scope | Who handles it |
|---|---|---|---|
| Transport | CONTRL | Interchange syntax | AS4 MSH (`mako-as4`) |
| Application | APERAK | Functional / AHB rules | Domain workflow (`mako-gpke`, etc.) |

Implementors must not confuse a CONTRL acknowledgement with APERAK compliance:
a CONTRL-accepted message may still be rejected by an APERAK with code `Z04`.

### ERP Integration

All process events are forwarded to the ERP system via outbound webhooks.
The INVOIC/REMADV/COMDIS messages in particular drive downstream accounting
workflows. See [ERP Integration Guide](erp-integration) for the full webhook
payload schema and retry semantics.

### Shared PID numbers across commodities

The BDEW ORDERS/ORDRSP AHB reuses some PID numbers across both Strom and Gas
because the underlying message structure is identical — only the commodity context
differs. **No cross-commodity coupling exists in the code.** Each crate registers
only the PIDs it owns; a Strom-only instance never loads any Gas crate and vice versa.

| PID | Strom usage | Gas usage | Routing |
|---|---|---|---|
| 17115 (Sperrauftrag) | Inbound NB receives from LF (`mako-gpke` `gpke-sperrung`) | **Outbound** LF→GNB (`geli-gas-sperrung-lf`) · **Inbound** GNB receives from LF (`geli-gas-sperrung-nb`) | Commodity + `DeploymentRoles` |
| 17117 (Entsperrauftrag) | Inbound NB receives from LF (`mako-gpke` `gpke-sperrung`) | **Outbound** LF→GNB (`geli-gas-sperrung-lf`) · **Inbound** GNB receives from LF (`geli-gas-sperrung-nb`) | Commodity + `DeploymentRoles` |
| 17116 (Anfrage Sperrung) | NB→MSB outbox (`mako-gpke` `gpke-sperrung`) · **Inbound** MSB→NB response via 19118/19119 | GNB→gMSB outbox (`geli-gas-sperrung-nb`) · **Inbound** gMSB→GNB response via 19118/19119 | Commodity |
| 19118 (Best. Anfrage Sperr.) | Inbound NB receives from MSB (`mako-gpke` `gpke-sperrung`) | Inbound GNB receives from gMSB (`geli-gas-sperrung-nb`) | Commodity + `DeploymentRoles` |
| 19119 (Abl. Anfrage Sperr.) | Inbound NB receives from MSB (`mako-gpke` `gpke-sperrung`) | Inbound GNB receives from gMSB (`geli-gas-sperrung-nb`) | Commodity + `DeploymentRoles` |
| 19116 (Bestätigung Sperrung) | Inbound LF receives from NB (`mako-gpke`) | Inbound LF receives from GNB (`mako-geli-gas`) | `DeploymentRoles` / `Marktrolle` |
| 19117 (Ablehnung Sperrung) | Inbound LF receives from NB (`mako-gpke`) | Inbound LF receives from GNB (`mako-geli-gas`) | `DeploymentRoles` / `Marktrolle` |
| 19128/19129 (Storno ORDRSP) | `mako-gpke` | `mako-geli-gas` | `DeploymentRoles` / `Marktrolle` |
| 19001/19002 (ORDRSP Bestätigung/Ablehnung) | `mako-gpke` (NB-role only) + `mako-wim` | not used by GeLi/WiM Gas | ORDERS correlation ID |
| 23001/23003/23004/23008 (INSRPT shared) | `mako-wim` `wim-insrpt` (5 WT) | `mako-wim-gas` `wim-gas-insrpt` (10 WT) | `PidRouter::route_with_sparte(pid, Sparte)` |

> **Inbound disambiguation:** When the same PID is registered by two crates in a
> combined Strom+Gas instance, `PidRouter` dispatches using:
>
> - **ORDERS/ORDRSP Sperrung** (17115, 17117, 19116–19129): by `DeploymentRoles` / `Marktrolle` (EIC prefix).
> - **INSRPT** (23001/23003/23004/23008): by `Sparte` via `route_with_sparte(pid, Sparte)` — the sender sets Strom/Gas Sparte, routing selects the correct deadline (5 WT vs. 10 WT).
>
> In commodity-separated instances (separate makod per Sparte), no disambiguation
> is needed — only one crate registers the PID.

### Format versions and process transitions

A process started under `FV2025-10-01` continues under those AHB rules until it
completes, even after the `FV2026-10-01` cutover on 2026-10-01. Both format versions
coexist simultaneously in the same engine instance.
`WorkflowVersionPolicy::ForwardCompatible` is the mandatory default for all MaKo
workflows. See [Schema Versioning](schema-versioning) for details.
