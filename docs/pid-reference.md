---
layout: default
title: PID Reference
nav_order: 41
parent: Regulatory
description: >
  Complete Prüfidentifikator (PID) reference for all German energy market
  processes: GPKE, WiM, GeLi Gas, MABIS, Messwesen, NBW, and GaBi Gas.
  Covers BDEW PID 3.3 (FV2025-10-01).
---

# Prüfidentifikator (PID) Reference

**Source document:** BDEW EDI@Energy — *Anwendungsübersicht der Prüfidentifikatoren*  
**Version:** 3.3 (FV2025-10-01), published 01.10.2025  
**PDF:** [`docs/pdfs/bdew-mako/PID_3_3_20251001.pdf`](pdfs/bdew-mako/PID_3_3_20251001.pdf)

A Prüfidentifikator (PID) identifies a specific EDIFACT message use case within a
business process. Each PID is bound to one EDIFACT format (UTILMD, MSCONS, INVOIC, …)
and one business context (GPKE, WiM, GeLi Gas, …). The routing layer
(`mako_engine::pid_router::PidRouter`) dispatches inbound messages to the correct
workflow by PID.

> **Legend** — *PID 3.3 column*  
> ✅ = present in BDEW PID 3.3 (FV2025-10-01)  
> ⚠️ = **not** in PID 3.3; introduced by a separate BNetzA decision or still uses an
>       older AHB PID number (see [Discrepancies](#discrepancies))

---

## Table of contents

1. [GPKE — Lieferantenwechsel Strom (55001–55018)](#gpke--lieferantenwechsel-strom)
2. [GPKE — Sperrung / Entsperrung Strom (55555, 55557–55692)](#gpke--sperrung--entsperrung-strom)
3. [GPKE — Einspeisestelle Strom (56001–56004)](#gpke--einspeisestelle-strom)
4. [GPKE — INVOIC Abrechnung Strom (31001–31008)](#gpke--invoic-abrechnung-strom)
5. [GPKE — ORDERS/ORDRSP Konfiguration Strom (17134–17135, 19001–19002)](#gpke--ordersordrsp-konfiguration-strom)
6. [WiM Strom — Messstellenbetrieb UTILMD (55039–55053, 55168–55170)](#wim-strom--messstellenbetrieb-utilmd)
7. [WiM Strom — Geräteübernahme ORDERS (17001–17011)](#wim-strom--geräteubernahme-orders)
8. [WiM Strom — Stammdaten / Übermittlung ORDERS (17132, 17102–17133)](#wim-strom--stammdaten--übermittlung-orders)
9. [WiM Strom — Stornierung ORDCHG (39000–39002)](#wim-strom--stornierung-ordchg)
10. [WiM Strom — Legacy device-change UTILMD (11001–11003)](#wim-strom--legacy-device-change-utilmd-11001--11003)
11. [WiM Gas — Messstellenbetrieb Gas (44022–44053)](#wim-gas--messstellenbetrieb-gas)
12. [GeLi Gas — Lieferbeginn/-ende Gas (44001–44018)](#geli-gas--lieferbeginn-ende-gas)
13. [GeLi Gas — Sperrung / Entsperrung Gas (44555)](#geli-gas--sperrung--entsperrung-gas)
14. [Messwesen / MSCONS (13002–13028)](#messwesen--mscons-13002--13028)
15. [MABIS (13003)](#mabis-13003)
16. [NBW — Netzbetreiberwechsel PARTIN (15001–15005)](#nbw--netzbetreiberwechsel-partin)
17. [GaBi Gas — INVOIC Billing Gas (31010–31011)](#gabi-gas--invoic-billing-gas)
18. [Discrepancies](#discrepancies)

---

## GPKE — Lieferantenwechsel Strom

**Crate:** `mako-gpke`  
**Workflow:** `GpkeLfAnmeldungWorkflow` / `gpke-supplier-change`  
**Format:** UTILMD AHB Strom  
**Regulatory basis:** BK6-22-024 (LFW24)

| PID   | Description                             | Direction  | PID 3.3 |
|-------|-----------------------------------------|------------|---------|
| 55001 | Anfrage Lieferbeginn (LF → NB)          | inbound    | ✅      |
| 55002 | Anfrage Lieferende (LF → NB)            | inbound    | ✅      |
| 55003 | Bestätigung Lieferbeginn (NB → LF)      | response   | ✅      |
| 55004 | Ablehnung Lieferbeginn (NB → LF)        | response   | ✅      |
| 55005 | Bestätigung Lieferende (NB → LF)        | response   | ✅      |
| 55006 | Ablehnung Lieferende (NB → LF)          | response   | ✅      |
| 55017 | Kündigung Lieferbeginn (LF → LFA)       | inbound    | ✅      |
| 55018 | Bestätigung Kündigung (LFA → LF)        | response   | ✅      |

**APERAK Frist:** 24 wall-clock hours (`fristen::add_hours(t, 24)`).

---

## GPKE — Sperrung / Entsperrung Strom

**Crate:** `mako-gpke`  
**Workflow:** `GpkeSperrungWorkflow` / `gpke-sperrung`  
**Format:** UTILMD AHB Strom  
**Regulatory basis:** BK6-22-024

| PID   | Description                                     | Direction  | PID 3.3 |
|-------|-------------------------------------------------|------------|---------|
| 55555 | Anweisung Sperrung Strom (NB → MSB)             | inbound    | ✅      |
| 55557 | Anweisung Entsperrung Strom (NB → MSB)          | inbound    | ✅      |

> The full Sperrung/Entsperrung range (55557–55692) contains many sub-process PIDs
> for follow-up messages (Bestätigung, Ablehnung, information flows). Only 55555 and
> 55557 are the primary workflow triggers registered in `SPERRUNG_PIDS`.

---

## GPKE — Einspeisestelle Strom

**Crate:** `mako-gpke`  
**Workflow:** `SupplierChangeWorkflow` / `gpke-supplier-change` (shared)  
**Format:** UTILMD AHB Strom  
**Regulatory basis:** BK6-22-024 (LFW24), transferred from former MPES effective **2025-06-06**  
**AHB profiles:** `fv20250606`, `fv20251001`, `fv20261001`

| PID   | Description                                         | Direction  | PID 3.3 |
|-------|-----------------------------------------------------|------------|---------|
| 56001 | Einspeisung Anmeldung (LFE → NB)                    | inbound    | ⚠️      |
| 56002 | Einspeisung Abmeldung / Kündigung (LFE → NB)        | inbound    | ⚠️      |
| 56003 | Einspeisung Bestätigung, fristgerecht (NB → LFE)    | response   | ⚠️      |
| 56004 | Einspeisung Ablehnung (NB → LFE)                    | response   | ⚠️      |

> ⚠️ **Not in PID 3.3.** PIDs 56001–56004 were transferred from the former MPES domain
> into GPKE per BK6-22-024 and are defined in the **UTILMD AHB Strom** (LFW24 annex),
> not in the PID overview document. They are present in the project's UTILMD profiles
> starting from `fv20250606`. Former PIDs 56005–56010 were never in any current AHB.

---

## GPKE — INVOIC Abrechnung Strom

**Crate:** `mako-gpke`  
**Workflow:** `GpkeAbrechnungWorkflow` / `gpke-abrechnung`  
**Format:** INVOIC AHB  
**Regulatory basis:** BK6-22-024

| PID   | Description                                        | Direction  | PID 3.3 |
|-------|----------------------------------------------------|------------|---------|
| 31001 | Rechnung (LF → NB or NB → LF)                     | inbound    | ✅      |
| 31002 | Stornorechnung                                     | inbound    | ✅      |
| 31004 | Mahnung                                            | inbound    | ✅      |
| 31005 | Antwort auf Rechnung — Bestätigung                 | response   | ✅      |
| 31006 | Antwort auf Rechnung — Ablehnung                   | response   | ✅      |
| 31007 | Antwort auf Stornorechnung — Bestätigung           | response   | ✅      |
| 31008 | Antwort auf Stornorechnung — Ablehnung             | response   | ✅      |

---

## GPKE — ORDERS/ORDRSP Konfiguration Strom

**Crate:** `mako-gpke`  
**Workflow:** `GpkeKonfigurationWorkflow` / `gpke-konfiguration`  
**Format:** ORDERS / ORDRSP AHB  
**Regulatory basis:** BK6-22-024

| PID   | Description                                        | Format  | Direction  | PID 3.3 |
|-------|----------------------------------------------------|---------|------------|---------|
| 17134 | Anfrage Konfiguration Strom (NB → LF)              | ORDERS  | inbound    | ✅      |
| 17135 | Bestätigung/Anpassung Konfiguration (LF → NB)      | ORDERS  | inbound    | ✅      |
| 19001 | Bestätigung Konfigurationsanfrage (NB → LF)        | ORDRSP  | outbound   | ✅      |
| 19002 | Ablehnung Konfigurationsanfrage (NB → LF)          | ORDRSP  | outbound   | ✅      |

---

## WiM Strom — Messstellenbetrieb UTILMD

**Crate:** `mako-wim` *(future — see [discrepancy note](#wim-strom--legacy-device-change-utilmd-11001--11003))*  
**Format:** UTILMD AHB Strom  
**Regulatory basis:** BK6-18-032 / BK6-22-082 (WiM Strom Teil 1)

These are the **current** AHB PIDs for WiM Strom Messstellenbetrieb as of FV2025-10-01.
They supersede the legacy 11001–11003 numbering used in older AHB versions.

### Beginn Messstellenbetrieb (MSB Anmeldung)

| PID   | Description                              | Direction        | PID 3.3 |
|-------|------------------------------------------|------------------|---------|
| 55042 | Anmeldung MSB (MSBN → NB)                | inbound          | ✅      |
| 55043 | Bestätigung Anmeldung MSB (NB → MSBN)    | response         | ✅      |
| 55044 | Ablehnung Anmeldung MSB (NB → MSBN)      | response         | ✅      |
| 55039 | Kündigung MSB (MSBN → NB)                | inbound          | ✅      |
| 55040 | Bestätigung Kündigung MSB (NB → MSBN)    | response         | ✅      |
| 55041 | Ablehnung Kündigung MSB (NB → MSBN)      | response         | ✅      |

### Ende Messstellenbetrieb (MSB Abmeldung)

| PID   | Description                              | Direction        | PID 3.3 |
|-------|------------------------------------------|------------------|---------|
| 55051 | Ende MSB / Abmeldung (NB → MSBN)         | inbound          | ✅      |
| 55052 | Bestätigung Ende MSB (MSBN → NB)         | response         | ✅      |
| 55053 | Ablehnung Ende MSB (MSBN → NB)           | response         | ✅      |
| 55168 | Verpflichtungsanfrage / Aufforderung     | inbound          | ✅      |
| 55169 | Bestätigung Verpflichtungsanfrage        | response         | ✅      |
| 55170 | Ablehnung Verpflichtungsanfrage          | response         | ✅      |

---

## WiM Strom — Geräteübernahme ORDERS

**Crate:** `mako-wim`  
**Workflow:** `WimGeraeteubernahmeWorkflow` / `wim-geraeteubernahme`  
**Format:** ORDERS AHB  
**Regulatory basis:** BK6-18-032

| PID   | Description                                              | Direction  | PID 3.3 |
|-------|----------------------------------------------------------|------------|---------|
| 17001 | Anfrage Geräteübernahmeangebot (nMSB → NB / aMSB)       | inbound    | ✅      |
| 17002 | Bestellung Geräteübernahme (nach separatem Angebot)      | inbound    | ✅      |
| 17005 | Bestellung Geräteübernahme (Follow-up)                   | inbound    | ✅      |
| 17009 | Stornierung Anfrage Geräteübernahmeangebot               | inbound    | ✅      |
| 17011 | Stornierung Bestellung Geräteübernahme                   | inbound    | ✅      |

**APERAK / ORDRSP Frist:** 5 Werktage (`fristen::add_werktage(d, 5, BdewMaKo)`).
Saturday counts as a Werktag; Sunday and public holidays do not.

---

## WiM Strom — Stammdaten / Übermittlung ORDERS

**Crate:** `mako-wim`  
**Workflow:** `WimStammdatenWorkflow` / `wim-stammdaten`  
**Format:** ORDERS AHB  
**Regulatory basis:** BK6-18-032  
**AHB source:** ORDERS AHB fv20251001

| PID(s)       | Description                                                      | Direction  | PID 3.3 |
|--------------|------------------------------------------------------------------|------------|---------|
| 17132        | Anfrage zur Übermittlung von Stammdaten **Strom** (NB → MSB)     | inbound    | ✅      |
| 17102–17133  | Übermittlung Stammdaten responses (MSB → NB, Nb role only)       | inbound    | ✅      |

> **Note:** PID 17101 is "Anfrage zur Übermittlung von Stammdaten **Gas**" (GeLi Gas / WiM Gas domain)
> and is NOT registered in `mako-wim`. PIDs 17134/17135 are GPKE Konfiguration PIDs
> ("Einrichtung Konfiguration aufgrund Zuordnung LF") registered in `mako-gpke`, not here.

---

## WiM Strom — Stornierung ORDCHG

**Crate:** `mako-wim`  
**Workflow:** `WimStornierungWorkflow` / `wim-stornierung`  
**Format:** ORDCHG AHB  
**Regulatory basis:** BK6-18-032

| PID   | Description                             | Direction  | PID 3.3 |
|-------|-----------------------------------------|------------|---------|
| 39000 | Stornierung (any party)                 | inbound    | ✅      |
| 39001 | Bestätigung Stornierung (outbox only)   | outbound   | ✅      |
| 39002 | Ablehnung Stornierung (outbox only)     | outbound   | ✅      |

---

## WiM Strom — Legacy device-change UTILMD (11001–11003)

**Crate:** `mako-wim`  
**Workflow:** `WimDeviceChangeWorkflow` / `wim-device-change`  
**Format:** UTILMD AHB Strom *(legacy)*

| PID   | Description (legacy name)                          | Direction  | PID 3.3 |
|-------|----------------------------------------------------|------------|---------|
| 11001 | Gerätewechsel Anmeldung / Anmeldung MSB (nMSB → NB)| inbound    | ⚠️      |
| 11002 | Gerätewechsel Abmeldung / Kündigung MSB (NB → aMSB)| inbound    | ⚠️      |
| 11003 | Stammdatenänderung (NB ↔ MSB)                      | inbound    | ⚠️      |

> ⚠️ **Not in PID 3.3 (FV2025-10-01) and not in any current UTILMD AHB profile.**
> These PIDs appear to predate the WiM Strom Teil 1 reform. The current AHB uses
> **55042–55044** (Anmeldung MSB) and **55051–55053** (Ende MSB) instead.
> The `WimDeviceChangeWorkflow` needs to be updated to register the current PIDs.
> See [Discrepancies](#discrepancies) for tracking.

---

## WiM Gas — Messstellenbetrieb Gas

**Crate:** `mako-wim-gas` *(placeholder)*  
**Format:** UTILMD AHB Gas  
**Regulatory basis:** BK7-24-01-009 (GeLi Gas 3.0, WiM Gas component)

Key WiM Gas UTILMD PIDs appearing in PID 3.3 (process NB ↔ MSBA/gMSB):

| PID   | Description                                        | PID 3.3 |
|-------|----------------------------------------------------|---------|
| 44022 | Kündigung MSB Gas (NB → MSBA)                      | ✅      |
| 44023 | Bestätigung Kündigung MSB Gas                      | ✅      |
| 44024 | Ablehnung Kündigung MSB Gas                        | ✅      |
| 44039 | Anmeldung MSB Gas (MSBN → NB)                      | ✅      |
| 44040 | Bestätigung Anmeldung MSB Gas                      | ✅      |
| 44041 | Ablehnung Anmeldung MSB Gas                        | ✅      |
| 44042 | Ende MSB Gas (NB → MSBN)                           | ✅      |
| 44043 | Bestätigung Ende MSB Gas                           | ✅      |
| 44044 | Ablehnung Ende MSB Gas                             | ✅      |
| 44051 | Vorläufige Abmeldebestätigung MSB Gas (NB → MSBA)  | ✅      |
| 44052 | Bestätigung Ende MSB (NB → MSBA)                   | ✅      |
| 44053 | Ablehnung Ende MSB (NB → MSBA)                     | ✅      |
| 44168 | Verpflichtungsanfrage Aufforderung (NB → gMSB)     | ✅      |
| 44169 | Bestätigung Verpflichtungsanfrage                  | ✅      |
| 44170 | Ablehnung Verpflichtungsanfrage                    | ✅      |

**APERAK Frist:** 10 Werktage (`fristen::add_werktage(d, 10, BdewMaKo)`).

---

## GeLi Gas — Lieferbeginn/-ende Gas

**Crate:** `mako-geli-gas`  
**Workflow:** `GeliGasSupplierChangeWorkflow` / `geli-gas-supplier-change`  
**Format:** UTILMD AHB Gas  
**Regulatory basis:** BK7-24-01-009 (GeLi Gas 3.0, Beschluss 12.09.2025, abgeschlossen 24.09.2025)

| PID   | Description                                         | Direction  | PID 3.3 |
|-------|-----------------------------------------------------|------------|---------|
| 44001 | Anfrage Lieferbeginn Gas (LFN → NB)                 | inbound    | ✅      |
| 44002 | Bestätigung Lieferbeginn Gas (NB → LFN)             | response   | ✅      |
| 44003 | Ablehnung Lieferbeginn Gas (NB → LFN)               | response   | ✅      |
| 44004 | Anfrage Lieferende Gas (LFN → NB)                   | inbound    | ✅      |
| 44005 | Bestätigung Lieferende Gas (NB → LFN)               | response   | ✅      |
| 44006 | Ablehnung Lieferende Gas (NB → LFN)                 | response   | ✅      |
| 44017 | Kündigung Lieferbeginn Gas (LFN → LFA)              | inbound    | ✅      |
| 44018 | Bestätigung Kündigung Lieferbeginn Gas (LFA → LFN)  | response   | ✅      |

**APERAK Frist:** 10 Werktage (`fristen::add_werktage(d, 10, BdewMaKo)`).

> GeLi Gas 3.0 scope is **UTILMD G only** (PIDs 44001–44018, 44555).
> Gas MMM billing (INVOIC 31010–31011) belongs to **GaBi Gas** (`mako-gabi-gas`),
> not GeLi Gas.

---

## GeLi Gas — Sperrung / Entsperrung Gas

**Crate:** `mako-geli-gas`  
**Workflow:** `GeliGasSperrungWorkflow` / `geli-gas-sperrung`  
**Format:** UTILMD AHB Gas  
**Regulatory basis:** BK7-24-01-009 (GeLi Gas 3.0); UTILMD profile `fv20241001_gas`+

| PID   | Description                                       | Direction  | PID 3.3 |
|-------|---------------------------------------------------|------------|---------|
| 44555 | Anweisung Sperrung Gas (NB → MSB)                 | inbound    | ⚠️      |

> ⚠️ **Not in PID 3.3.** PID 44555 is defined in the **UTILMD AHB Gas** directly and
> appears in the project's gas profiles (`fv20241001_gas`, `fv20251001_gas`,
> `fv20261001_gas`) but was not extracted from the PID overview document.
> This is likely a PDF table-column extraction limitation, not a genuine absence —
> the PID is confirmed present in the AHB profiles.

---

## Messwesen / MSCONS (13002–13028)

**Crates:** cross-domain (not routed to a single MaKo workflow)  
**Format:** MSCONS AHB  
**Regulatory basis:** various (BK6/BK7 measurement data exchange)

Selected PIDs appearing in PID 3.3 across multiple process contexts:

| PID   | Description                              | Context             | PID 3.3 |
|-------|------------------------------------------|---------------------|---------|
| 13002 | Zählerstand (Gas / Strom)                | WiM Gas, Messwesen  | ✅      |
| 13003 | Energiemenge (Strom)                     | Messwesen Strom     | ✅      |
| 13005 | Lastgang (Strom, RLM)                    | Messwesen Strom     | ✅      |
| 13006 | Zählerstand (Strom)                      | GPKE / WiM Strom    | ✅      |
| 13007 | Gasbeschaffenheit                        | WiM Gas / Messwesen | ✅      |
| 13008 | Lastgang (Gas, RLM)                      | WiM Gas / Messwesen | ✅      |
| 13009 | Energiemenge (Gas)                       | WiM Gas / Messwesen | ✅      |
| 13015 | Zählpunkt-Summenzeitreihe (Strom)        | Messwesen Strom     | ✅      |
| 13016 | Lastgang (Strom, SLP)                    | Messwesen Strom     | ✅      |
| 13017 | Energiemenge SLP synthetisch (Strom)     | Messwesen Strom     | ✅      |
| 13019 | Statusmeldung Messwerte                  | Messwesen Strom     | ✅      |
| 13025 | Lastgang (Strom, iMSys)                  | Messwesen Strom     | ✅      |
| 13026 | Bilanzierungsrelevante Messwerte         | Messwesen Strom     | ✅      |
| 13027 | Sonderausspielung                        | WiM Strom / AWH     | ✅      |
| 13028 | Lastgang (Strom, SLP/Pegel)              | WiM Strom / GPKE    | ✅      |

PIDs 13002–13028 are Messwesen PIDs; they are **not** MABIS PIDs.

---

## MABIS (13003)

**Crate:** `mako-mabis`  
**Workflow:** `MabisBillingWorkflow` / `mabis-billing`  
**Format:** MSCONS  
**Regulatory basis:** BK6-06-013 (MABIS Bilanzkreisabrechnung Strom, BKV ↔ ÜNB)

| PID   | Description                                      | PID 3.3 |
|-------|--------------------------------------------------|---------|
| 13003 | Summenzeitreihen und Ausfallarbeitssummen (MSCONS) | ✅     |

> ℹ️ **PID 13001 does not exist** in any MSCONS AHB version.
> The MABIS Bilanzkreisabrechnung is identified by PID **13003** in the MSCONS AHB.
> PIDs 13002 and 13004–13028 are Messwesen-PIDs (meter data exchange) and do **not**
> belong to MABIS. See [copilot-instructions.md](../.github/copilot-instructions.md)
> domain rules for the full classification.

---

## NBW — Netzbetreiberwechsel PARTIN

**Crate:** `mako-nbw` *(placeholder)*  
**Format:** PARTIN AHB  
**Regulatory basis:** BDEW NBW process

| PID   | Description                          | PID 3.3 |
|-------|--------------------------------------|---------|
| 15001 | PARTIN Stammdaten NB (bulk transfer) | ✅      |
| 15002 | PARTIN Bestätigung                   | ✅      |
| 15003 | PARTIN Ablehnung                     | ✅      |
| 15004 | PARTIN Änderung                      | ✅      |
| 15005 | PARTIN Löschung                      | ✅      |

---

## GaBi Gas — INVOIC Billing Gas

**Crate:** `mako-gabi-gas` *(placeholder)*  
**Format:** INVOIC AHB  
**Regulatory basis:** GaBi Gas (BK7-06-067 / BK7-24-01-009 MMM)

| PID   | Description                           | PID 3.3 |
|-------|---------------------------------------|---------|
| 31010 | Rechnung Gas MMM (NB / BKV → LF)      | ✅      |
| 31011 | Stornorechnung Gas MMM                | ✅      |

> Gas MMM billing PIDs 31010–31011 belong to **GaBi Gas** (`mako-gabi-gas`), not
> GeLi Gas. GeLi Gas 3.0 explicitly excludes INVOIC billing from its scope.

---

## Discrepancies

The following discrepancies were identified by cross-checking the project source against
**BDEW PID 3.3 (FV2025-10-01)** and the UTILMD AHB profiles.

### D-1 — WiM Strom: legacy PIDs 11001–11003 not in current AHB

| | |
|---|---|
| **Severity** | High |
| **Crate** | `mako-wim` |
| **Current state** | `WimDeviceChangeWorkflow` registers PIDs 11001, 11002, 11003 |
| **Expected** | Current UTILMD AHB Strom (FV2025-10-01) uses **55042–55044** (Anmeldung/Bestätigung/Ablehnung MSB) and **55051–55053** (Ende MSB / Abmeldung) |
| **Evidence** | PIDs 11001–11003 are absent from all UTILMD AHB profiles (`fv20241001` through `fv20261001`) and from PID 3.3 |
| **Action** | Update `geraetewechsel.rs` to register 55042, 55051 as trigger PIDs; map old PID constants to new AHB values; update `UTILMD_PIDS` routing slice |

### D-2 — GPKE Einspeisestelle: PIDs 56001–56004 absent from PID 3.3

| | |
|---|---|
| **Severity** | Informational |
| **Crate** | `mako-gpke` |
| **Current state** | PIDs 56001–56004 registered; present in UTILMD profiles `fv20250606`+ |
| **Expected** | PID 3.3 (FV2025-10-01) does not list 56001–56004 |
| **Reason** | Introduced by BK6-22-024 (LFW24) effective 2025-06-06; defined in UTILMD AHB annex, not yet reflected in the PID overview document |
| **Action** | No code change needed. Verify inclusion in the next PID overview revision (expected FV2026-10-01) |

### D-3 — GeLi Gas Sperrung: PID 44555 absent from PID 3.3

| | |
|---|---|
| **Severity** | Informational |
| **Crate** | `mako-geli-gas` |
| **Current state** | PID 44555 registered; present in UTILMD Gas profiles (`fv20241001_gas`+) |
| **Expected** | PID 3.3 (FV2025-10-01) does not list 44555 |
| **Reason** | Likely a PDF table extraction artefact or the PID is defined directly in the UTILMD AHB Gas without appearing in the process overview table |
| **Action** | Verify against the published UTILMD AHB Gas PDF. No code change pending confirmation |

### D-4 — MABIS PID 13003 correctly registered; historic 13001 confusion resolved

| | |
|---|---|
| **Severity** | Informational (resolved) |
| **Crate** | `mako-mabis` |
| **Current state** | `router.register(13003, "mabis-billing")` — correct MSCONS AHB PID |
| **History** | Earlier versions used 13001 as an engine-internal routing key; that was incorrect. PID 13001 does not exist in any MSCONS AHB. |
| **Action** | No change needed. PID 13003 is the correct BDEW Prüfidentifikator for Summenzeitreihen und Ausfallarbeitssummen. |

---

*Last updated: FV2025-10-01 — cross-checked against BDEW PID 3.3 (01.10.2025)*
