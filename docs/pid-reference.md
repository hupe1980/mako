---
layout: default
title: PID Reference
nav_order: 11
parent: Regulatory
description: >
  Complete Prüfidentifikator (PID) reference for all German energy market
  processes. Covers BDEW PID 3.3 (FV2025-10-01, Fehlerkorrektur 27.03.2026),
  PID 4.0 (FV2026-10-01), and DVGW synthetic PIDs (90000–90999).
  Includes communication roles (Von → An), response-trigger PIDs (Reaktion),
  and the Rust domain crate that routes each PID.
---

# Prüfidentifikator (PID) Reference

**Source documents:**
- BDEW EDI@Energy — *Anwendungsübersicht der Prüfidentifikatoren*:
  PID 3.3 (FV2025-10-01, Fehlerkorrektur 27.03.2026) · PID 4.0 (FV2026-10-01, published 01.04.2026)
- DVGW EDI-DVGW — synthetic PIDs 90000–90999 for GaBi Gas routing

A Prüfidentifikator (PID) identifies a specific EDIFACT message use case within a
business process. Each PID is bound to one EDIFACT format (UTILMD, MSCONS, INVOIC, …)
and one business context (GPKE, WiM, GeLi Gas, …). The routing layer
(`mako_engine::pid_router::PidRouter`) dispatches inbound messages to the correct
workflow by PID.

**Legend — columns**

| Column | Meaning |
|--------|---------|
| **Von → An** | Communication direction from BDEW xlsx. Multi-occurrence PIDs (same PID, different process contexts) show all unique role pairs separated by ` · `. |
| **Reaktion** | PID that this message _reacts to_ (i.e. is a response/follow-up to). `—` if the column is empty in the source xlsx. |
| **⚡ / 🔥** | Sparte: ✅ = covered; — = not applicable. |
| **3.3 / 4.0** | ✅ = present in BDEW PID overview for that format version; ⚠️ = absent (sunset or not-yet-added). |
| **Crate / Workflow** | The `mako-*` crate and `workflow-name` that registers this PID in `PidRouter`. `—` = not yet implemented. ⁽ᴺᴮ⁾ = NB-role conditional registration only. Multiple entries separated by ` · ` = same PID registered independently in different crates (commodity-isolated; each crate is loaded only in the relevant Strom or Gas deployment). |

**Commodity isolation:** Strom crates (`mako-gpke`, `mako-wim`, `mako-mabis`)
and Gas crates (`mako-geli-gas`, `mako-wim-gas`, `mako-gabi-gas`) are fully
independent. A Strom-only makod instance loads only Strom crates; a Gas-only
instance loads only Gas crates. Running separate instances per commodity is
a standard and supported deployment topology.

**Multi-commodity PIDs:** Where the same BDEW PID number is used in both Strom
and Gas contexts (e.g. ORDERS 17115/17117, ORDRSP 19116/19117), each commodity
crate handles it independently. In a combined Strom+Gas instance the `PidRouter`
dispatches by `DeploymentRoles` / `Marktrolle`.

Source: BDEW PID 3.3 xlsx (Fehlerkorrektur 27.03.2026) and PID 4.0 xlsx (01.04.2026).

---

## Table of contents

1. [UTILMD AHB Strom](#utilmd-ahb-strom)
2. [UTILMD AHB Gas](#utilmd-ahb-gas)
3. [ORDERS AHB](#orders-ahb)
4. [ORDRSP AHB](#ordrsp-ahb)
5. [ORDCHG AHB](#ordchg-ahb)
6. [IFTSTA AHB](#iftsta-ahb)
7. [MSCONS AHB](#mscons-ahb)
8. [INVOIC AHB](#invoic-ahb)
9. [REMADV AHB](#remadv-ahb)
10. [PARTIN AHB](#partin-ahb)
11. [REQOTE AHB](#reqote-ahb)
12. [QUOTES AHB](#quotes-ahb)
13. [PRICAT AHB](#pricat-ahb)
14. [INSRPT AHB](#insrpt-ahb)
15. [UTILTS AHB](#utilts-ahb)
16. [COMDIS AHB](#comdis-ahb)
17. [SSQNOT AHB](#ssqnot-ahb)
18. [DVGW Synthetic PIDs (range 90000–90999)](#dvgw-synthetic-pids)

---

## DVGW Synthetic PIDs

DVGW EDIFACT messages (ALOCAT, NOMINT, NOMRES, …) carry **no BGM Prüfidentifikator**.
Routing uses the combination of message type and direction qualifier from NAD+MS/MR.
To keep the PID router uniform, the range `90000–90999` is reserved for DVGW
synthetic PIDs, derived from `(message_type, role_qualifier)`.

These PIDs are **not** defined by BDEW and do not appear in PID 3.3 or PID 4.0.
They are workspace-internal routing keys used by `DvgwMessageType::synthetic_pid()`
and `AnyDvgwMessage::detect_pid()` in the `dvgw-edi` crate.

| PID   | Message | Role qualifier | Direction |
|-------|---------|----------------|-----------|
| 90001 | ALOCAT  | Z15 / none     | FNB → BKV (daily allocation) |
| 90002 | ALOCAT  | Z16            | MGV → BKV (monthly allocation) |
| 90003 | ALOCAT  | Z17            | VNB → FNB (sub-daily allocation) |
| 90011 | NOMINT  | Z01 / none     | BKV → FNB (nomination) |
| 90012 | NOMINT  | Z02            | BKV → MGV (nomination) |
| 90021 | NOMRES  | Z01 / none     | FNB → BKV (nomination response) |
| 90022 | NOMRES  | Z02            | MGV → BKV (nomination response) |
| 90031 | SCHEDL  | —              | FNB → BKV (schedule) |
| 90041 | IMBNOT  | —              | MGV → BKV (intraday imbalance) |
| 90051 | TRANOT  | —              | FNB → BKV (transport notification) |
| 90061 | DELORD  | —              | BKV → FNB (delivery order) |
| 90062 | DELRES  | —              | FNB → BKV (delivery response) |

See [DVGW EDI](dvgw) for the full regulatory basis and parsing architecture.

---

## UTILMD AHB Strom

| PID | Beschreibung | Prozess | Von → An | Reaktion | ⚡ | 🔥 | 3.3 | 4.0 | Crate / Workflow |
|-----|--------------|---------|----------|----------|---|---|-----|-----|------------------|
| 55001 | Anmeldung verb. MaLo | GPKE Teil 2 | LFN → NB | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-supplier-change` |
| 55002 | Bestätigung Anmeldung verb. MaLo | GPKE Teil 2 | NB → LFN | 55001 | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-supplier-change` |
| 55003 | Ablehnung Anmeldung verb. MaLo | GPKE Teil 2 | NB → LFN | 55001 | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-lf-anmeldung` |
| 55004 | Abmeldung | GPKE Teil 2 | LF → NB | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-lf-anmeldung` |
| 55005 | Bestätigung Abmeldung | GPKE Teil 2 | NB → LF | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-lf-anmeldung` |
| 55006 | Ablehnung Abmeldung | GPKE Teil 2 | NB → LF | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-lf-anmeldung` |
| 55007 | Abmeldung / Beendigung der Zuordnung | GPKE Teil 2 | NB → LF | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-lf-abmeldung` |
| 55008 | Bestätigung Abmeldung | GPKE Teil 2 | LF → NB | — | ✅ | — | ✅ | ✅ | — |
| 55009 | Ablehnung Abmeldung | GPKE Teil 2 | LF → NB | — | ✅ | — | ✅ | ✅ | — |
| 55010 | Anfrage zur Beendigung der Zuordnung | GPKE Teil 2 | NB → LFA | — | ✅ | — | ✅ | ✅ | — |
| 55011 | Bestätigung Beendigung der Zuordnung | GPKE Teil 2 | LFA → NB | — | ✅ | — | ✅ | ✅ | — |
| 55012 | Ablehnung Beendigung der Zuordnung | GPKE Teil 2 | LFA → NB | — | ✅ | — | ✅ | ✅ | — |
| 55013 | Anmeldung / Zuordnung EOG | GPKE Teil 2 | NB → LF | — | ✅ | — | ✅ | ✅ | — |
| 55014 | Bestätigung EOG Anmeldung | GPKE Teil 2 | LF → NB | — | ✅ | — | ✅ | ✅ | — |
| 55015 | Ablehung EOG Anmeldung | GPKE Teil 2 | LF → NB | — | ✅ | — | ✅ | ✅ | — |
| 55016 | Kündigung | GPKE Teil 2 | LFN → LFA | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-supplier-change` |
| 55017 | Bestätigung Kündigung | GPKE Teil 2 | LFA → LFN | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-lf-anmeldung` |
| 55018 | Ablehnung Kündigung | GPKE Teil 2 | LFA → LFN | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-lf-anmeldung` |
| 55022 | Anfrage nach Stornierung | GPKE Teil 4 | orig. → orig. | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-stornierung` |
| 55023 | Bestätigung Anfrage Stornierung | GPKE Teil 4 | Empf. → Sender | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-stornierung` |
| 55024 | Ablehnung Anfrage Stornierung | GPKE Teil 4 | Empf. → Sender | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-stornierung` |
| 55035 | Antwort auf GDA verb. MaLo | GPKE Teil 4 | NB → LF | — | ✅ | — | ✅ | ✅ | — |
| 55036 | Existierende Zuordnung | GPKE Teil 2 | NB → LFN | — | ✅ | — | ✅ | ✅ | — |
| 55037 | Beendigung der Zuordnung | GPKE Teil 2 | NB → LFA | — | ✅ | — | ✅ | ✅ | — |
| 55038 | Aufhebung einer zuk. Zuordnung | GPKE Teil 2 | NB → LFZ | — | ✅ | — | ✅ | ✅ | — |
| 55039 | Kündigung MSB | WiM Strom Teil 1 | MSBN → MSBA | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-device-change` |
| 55040 | Bestätigung Kündigung MSB | WiM Strom Teil 1 | MSBA → MSBN | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-device-change` |
| 55041 | Ablehnung Kündigung MSB | WiM Strom Teil 1 | MSBA → MSBN | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-device-change` |
| 55042 | Anmeldung MSB | WiM Strom Teil 1 | MSBN → NB | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-device-change` |
| 55043 | Bestätigung Anmeldung MSB | WiM Strom Teil 1 | NB → MSBN | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-device-change` |
| 55044 | Ablehnung Anmeldung MSB | WiM Strom Teil 1 | NB → MSBN | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-device-change` |
| 55051 | Ende MSB | WiM Strom Teil 1 | MSBA → NB | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-device-change` |
| 55052 | Bestätigung Ende MSB | WiM Strom Teil 1 | NB → MSBA | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-device-change` |
| 55053 | Ablehnung Ende MSB | WiM Strom Teil 1 | NB → MSBA | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-device-change` |
| 55060 | Antwort auf GDA | GPKE Teil 4 | NB → MSB | — | ✅ | — | ✅ | ✅ | — |
| 55062 | Aktivierung von ZP | MaBiS / AWH Modell 2 ladev.scharf. bila. Energie.zuord.möglichkeit | NB → NB · NB → BIKO · NB → LF · ÜNB → LF · ÜNB → BIKO · BIKO → NB · BIKO → BKV · BIKO → ÜNB · ÜNB → NB · ÜNB → BKV · NB → ÜNB | — | ✅ | — | ✅ | ✅ | — |
| 55063 | Deaktivierung von ZP | MaBiS / AWH Modell 2 ladev.scharf. bila. Energie.zuord.möglichkeit | NB → NB · NB → BIKO · NB → LF · ÜNB → LF · ÜNB → BIKO · BIKO → NB · BIKO → BKV · BIKO → ÜNB · ÜNB → NB · ÜNB → BKV · NB → ÜNB | — | ✅ | — | ✅ | ✅ | — |
| 55064 | Antwort | MaBiS | NB → NB · BIKO → NB · BIKO → ÜNB | — | ✅ | — | ✅ | ✅ | — |
| 55065 | Lieferantenclearingliste | MaBiS | NB → LF · ÜNB → LF | — | ✅ | — | ✅ | ✅ | — |
| 55066 | Korrekturliste zu Lieferantenclearingliste | MaBiS | LF → NB · LF → ÜNB | — | ✅ | — | ✅ | ✅ | — |
| 55067 | Bilanzkreiszuordnungsliste | MaBiS | NB → BKV · ÜNB → BKV | — | ✅ | — | ✅ | ✅ | — |
| 55069 | Clearingliste DZR | MaBiS | BIKO → NB · BIKO → ÜNB | — | ✅ | — | ✅ | ✅ | — |
| 55070 | Clearingliste BAS | MaBiS | BIKO → BKV | — | ✅ | — | ✅ | ✅ | — |
| 55071 | Aktivierung der Zuordnungsermächtigung | MaBiS | BKV → NB | — | ✅ | — | ✅ | ✅ | — |
| 55072 | Deaktivierung der Zuordnungsermächtigung | MaBiS | BKV → NB | — | ✅ | — | ✅ | ✅ | — |
| 55073 | Übermittlung der Profildefinitionen | MaBiS | NB → MSB · NB → LF | — | ✅ | — | ✅ | ✅ | — |
| 55074 | Stammdaten auf eine ORDERS | HKN-R (NB↔UBA) | NB → HKN-R | — | ✅ | — | ✅ | ✅ | — |
| 55075 | Stammdaten aufgrund einer Änderung | HKN-R (NB↔UBA) | NB → HKN-R | — | ✅ | — | ✅ | ✅ | — |
| 55076 | Antwort auf Stammdatenänderung | HKN-R (NB↔UBA) | HKN-R → NB | — | ✅ | — | ✅ | ✅ | — |
| 55077 | Anmeldung erz. MaLo | GPKE Teil 2 | LFN → NB | — | ✅ | — | ✅ | ✅ | — |
| 55078 | Bestätigung Anmeldung erz. MaLo | GPKE Teil 2 | NB → LFN | 55077 | ✅ | — | ✅ | ✅ | — |
| 55080 | Ablehnung Anmeldung erz. MaLo | GPKE Teil 2 | NB → LFN | 55077 | ✅ | — | ✅ | ✅ | — |
| 55095 | Antwort auf GDA erz. MaLo | GPKE Teil 4 | NB → LF | — | ✅ | — | ✅ | ✅ | — |
| 55109 | Änderung Daten der MaLo | GPKE Teil 4 | LF → NB | — | ✅ | — | ✅ | ✅ | — |
| 55110 | Änderung Daten der MaLo | GPKE Teil 4 | LF → MSB | — | ✅ | — | ✅ | ✅ | — |
| 55126 | Abr.-Daten BK-Abr. verb. MaLo | GPKE Teil 2 / AWH NBW | NB → LF · NBA → NBN | — | ✅ | — | ✅ | ✅ | — |
| 55136 | Rückmeldung/Anfrage Daten der MaLo | GPKE Teil 4 | MSB → LF | — | ✅ | — | ✅ | ✅ | — |
| 55137 | Rückmeldung/Anfrage Daten der MaLo | GPKE Teil 4 | NB → LF | 55109 | ✅ | — | ✅ | ✅ | — |
| 55156 | Rückmeldung/Anfrage Abr.-Daten BK-Abr. verb. MaLo | GPKE Teil 2 | LF → NB | 55126 | ✅ | — | ✅ | ✅ | — |
| 55168 | Verpflichtungsanfrage / Aufforderung | WiM Strom Teil 1 | NB → gMSB | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-device-change` |
| 55169 | Bestätigung Verpflichtungsanfrage | WiM Strom Teil 1 | gMSB → NB | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-device-change` |
| 55170 | Ablehnung Verpflichtungsanfrage | WiM Strom Teil 1 | gMSB → NB | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-device-change` |
| 55173 | Änderung der Lokationsbündelstruktur | GPKE Teil 4 | NB → MSB | — | ✅ | — | ✅ | ✅ | — |
| 55175 | Änderung der Lokationsbündelstruktur | GPKE Teil 4 | NB → LF | — | ✅ | — | ✅ | ✅ | — |
| 55177 | Rückmeldung/Anfrage Lokationsbündelstruktur | GPKE Teil 4 | MSB → NB | 55173 | ✅ | — | ✅ | ✅ | — |
| 55180 | Rückmeldung/Anfrage Lokationsbündelstruktur | GPKE Teil 4 | LF → NB | 55175 | ✅ | — | ✅ | ✅ | — |
| 55194 | Antowrt auf GDA (Strom an Gas) | GPKE Teil 4 | NB → MSB | — | ✅ | — | ✅ | ✅ | — |
| 55195 | Bilanzierungsgebietsclearingliste | MaBiS | ÜNB → NB | — | ✅ | — | ✅ | ✅ | — |
| 55196 | Antwort auf Bilanzierungsgebietsclearingliste | MaBiS | NB → ÜNB | — | ✅ | — | ✅ | ✅ | — |
| 55197 | Aktivierung ZP tägliche AAÜZ | MaBiS | ANB → ÜNB | — | ✅ | — | ✅ | ✅ | — |
| 55198 | Deaktivierung tägliche AAÜZ | MaBiS | ANB → ÜNB | — | ✅ | — | ✅ | ✅ | — |
| 55199 | Aktivierung ZP LF-AASZR | MaBiS | ANB → LF | — | ✅ | — | ✅ | ✅ | — |
| 55200 | Deaktivierung ZP LF-AASZR | MaBiS | ANB → LF | — | ✅ | — | ✅ | ✅ | — |
| 55201 | LF-AACL | MaBiS | NB → LF | — | ✅ | — | ✅ | ✅ | — |
| 55202 | Korrekturliste LF-AACL | MaBiS | LF → NB | — | ✅ | — | ✅ | ✅ | — |
| 55203 | Aktivierung ZP monatliche AAÜZ | MaBiS | ANB → BIKO | — | ✅ | — | ✅ | ✅ | — |
| 55204 | Antwort auf Aktivierung ZP | MaBiS | BIKO → ANB | — | ✅ | — | ✅ | ✅ | — |
| 55205 | Weiterleitung Aktivierung ZP | MaBiS | BIKO → BKV | — | ✅ | — | ✅ | ✅ | — |
| 55206 | Deaktivierung ZP monatliche AAÜZ | MaBiS | ANB → BIKO | — | ✅ | — | ✅ | ✅ | — |
| 55207 | Antwort auf Deaktivierung ZP | MaBiS | BIKO → ANB | — | ✅ | — | ✅ | ✅ | — |
| 55208 | Weiterleitung Deaktivierung ZP | MaBiS | BIKO → BKV | — | ✅ | — | ✅ | ✅ | — |
| 55209 | Aktivierung ZP monatliche AAÜZ | MaBiS | ANB → BIKO | — | ✅ | — | ✅ | ✅ | — |
| 55210 | Antwort auf Aktiveirung ZP | MaBiS | BIKO → ANB | — | ✅ | — | ✅ | ✅ | — |
| 55211 | Weiterleitung Aktivierung ZP | MaBiS | BIKO → BKV | — | ✅ | — | ✅ | ✅ | — |
| 55212 | Deaktivierung ZP monatliche AAÜZ | MaBiS | ANB → BIKO | — | ✅ | — | ✅ | ✅ | — |
| 55213 | Antwort auf Deaktivierung ZP | MaBiS | BIKO → ANB | — | ✅ | — | ✅ | ✅ | — |
| 55214 | Weiterleitung Deaktivierung ZP | MaBiS | BIKO → BKV | — | ✅ | — | ✅ | ✅ | — |
| 55218 | Abr.-Daten NNA | GPKE Teil 2 / AWH NBW | NB → LF · NBA → NBN | — | ✅ | — | ✅ | ✅ | — |
| 55220 | Rückmeldung/Anfrage Abr.-Daten NNA | GPKE Teil 2 | LF → NB | 55218 | ✅ | — | ✅ | ✅ | — |
| 55223 | DZÜ-Liste | MaBiS | ÜNB → NB | — | ✅ | — | ✅ | ✅ | — |
| 55224 | Antwort auf DZÜ-Liste | MaBiS | NB → ÜNB | — | ✅ | — | ✅ | ✅ | — |
| 55225 | Änderung Blindabr.-Daten der NeLo | GPKE Teil 4 / AWH NBW | NB → LF · NBA → NBN | — | ✅ | — | ✅ | ✅ | — |
| 55227 | Rückmeldung/Anfrage Blindabr.-Daten der NeLo | GPKE Teil 4 | LF → NB | 55225 | ✅ | — | ✅ | ✅ | — |
| 55230 | Änderung Blindabr.-Daten der NeLo | GPKE Teil 4 | LF → NB | — | ✅ | — | ✅ | ✅ | — |
| 55232 | Rückmeldung/Anfrage Blindabr.-Daten der NeLo | GPKE Teil 4 | NB → LF | 55230 | ✅ | — | ✅ | ✅ | — |
| 55235 | Zuordnung ZP der NGZ zur NZR | AWH MaBiS-Ergänzung | NB → NB · NB → ÜNB | — | ✅ | — | ✅ | ✅ | — |
| 55236 | Beendigung Zuordnung ZP der NGZ zur NZR | AWH MaBiS-Ergänzung | NB → NB · NB → ÜNB | — | ✅ | — | ✅ | ✅ | — |
| 55237 | Antwort | AWH MaBiS-Ergänzung | NB → NB | — | ✅ | — | ✅ | ✅ | — |
| 55238 | Anmeldung in Modell 2 | AWH Modell 2 ladev.scharf. bila. Energie.zuord.möglichkeit | NB → VNB | — | ✅ | — | ✅ | ✅ | — |
| 55239 | Antwort auf Anmeldung | AWH Modell 2 ladev.scharf. bila. Energie.zuord.möglichkeit | VNB → NB | — | ✅ | — | ✅ | ✅ | — |
| 55240 | Beendigung der Zuordnung zur Marktlokation | AWH Modell 2 ladev.scharf. bila. Energie.zuord.möglichkeit | VNB → LF | — | ✅ | — | ✅ | ✅ | — |
| 55241 | Antwort auf Beendigung | AWH Modell 2 ladev.scharf. bila. Energie.zuord.möglichkeit | LF → VNB | — | ✅ | — | ✅ | ✅ | — |
| 55242 | Abmeldung aus dem Modell 2 | AWH Modell 2 ladev.scharf. bila. Energie.zuord.möglichkeit | NB → VNB | — | ✅ | — | ✅ | ✅ | — |
| 55243 | Antwort auf Abmeldung | AWH Modell 2 ladev.scharf. bila. Energie.zuord.möglichkeit | VNB → NB | — | ✅ | — | ✅ | ✅ | — |
| 55553 | Daten auf individuelle Bestellung | GPKE Teil 4 | MSB → NB · MSB → LF · MSB → MSB | — | ✅ | — | ✅ | ✅ | — |
| 55555 | Anfrage Daten der individuellen Bestellung | GPKE Teil 4 | NB → MSB · LF → MSB · MSB → MSB | 55553 | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-anfrage-bestellung` |
| 55557 | Änderung MSB-Abr.-Daten der MaLo | GPKE Teil 4 | MSB → NB | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-supplier-change` |
| 55559 | Rückmeldung/Anfrage MSB-Abr.-Daten der MaLo | GPKE Teil 4 | NB → MSB | 55557 | ✅ | — | ✅ | ✅ | — |
| 55600 | Anmeldung neuer verb. MaLo | GPKE Teil 2 | LF → NB | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-neuanlage` |
| 55601 | Anmeldung neuer erz. MaLo | GPKE Teil 2 | LF → NB | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-neuanlage` |
| 55602 | Bestätigung Anmeldung neuer verb. MaLo | GPKE Teil 2 | NB → LF | 55600 | ✅ | — | ✅ | ✅ | — |
| 55603 | Bestätigung Anmeldung neuer erz. MaLo | GPKE Teil 2 | NB → LF | 55601 | ✅ | — | ✅ | ✅ | — |
| 55604 | Ablehnung Anmeldung neuer verb. MaLo | GPKE Teil 2 | NB → LF | 55600 | ✅ | — | ✅ | ✅ | — |
| 55605 | Ablehnung Anmeldung neuer erz. MaLo | GPKE Teil 2 | NB → LF | 55601 | ✅ | — | ✅ | ✅ | — |
| 55607 | Ankündigung Zuordnung / Zuordnung des LF zur MaLo/ Tranche | GPKE Teil 2 | NB → LFN (Notiz "LF des Unternehmens Netzbetreiber") · NB → LFN | — | ✅ | — | ✅ | ✅ | — |
| 55608 | Bestätigung Zuordnung des LF zur MaLo/ Tranche | GPKE Teil 2 | LFN (Notiz "LF des Unternehmens Netzbetreiber") → NB · LFN → NB | — | ✅ | — | ✅ | ✅ | — |
| 55609 | Ablehnung Zuordnung des LF zur MaLo/ Tranche | GPKE Teil 2 | LFN (Notiz "LF des Unternehmens Netzbetreiber") → NB · LFN → NB | — | ✅ | — | ✅ | ✅ | — |
| 55611 | Beendigung der Zuordnung | GPKE Teil 2 | NB → MSB · NB → MSBZ | — | ✅ | — | ✅ | ✅ | — |
| 55613 | Abr.-Daten BK-Abr. verb. MaLo | GPKE Teil 2 | NB → ÜNB | — | ✅ | — | ✅ | ✅ | — |
| 55614 | Rückmeldung/Anfrage Abr.-Daten BK-Abr. verb. MaLo | GPKE Teil 2 | ÜNB → NB | 55613 | ✅ | — | ✅ | ✅ | — |
| 55615 | Änderung Daten der NeLo | GPKE Teil 4 / AWH NBW | NB → LF · NBA → NBN | — | ✅ | — | ✅ | ✅ | — |
| 55616 | Änderung Daten der MaLo | GPKE Teil 4 / AWH NBW | NB → LF · NBA → NBN | — | ✅ | — | ✅ | ✅ | — |
| 55617 | Änderung Daten der TR | GPKE Teil 4 / AWH NBW | NB → LF · NBA → NBN | — | ✅ | — | ✅ | ✅ | — |
| 55618 | Änderung Daten der SR | GPKE Teil 4 / AWH NBW | NB → LF · NBA → NBN | — | ✅ | — | ✅ | ✅ | — |
| 55619 | Änderung Daten der Tranche | GPKE Teil 4 / AWH NBW | NB → LF · NBA → NBN | — | ✅ | — | ✅ | ✅ | — |
| 55620 | Änderung Daten der MeLo | GPKE Teil 4 / AWH NBW | NB → LF · NBA → NBN | — | ✅ | — | ✅ | ✅ | — |
| 55621 | Rückmeldung/Anfrage Daten zur NeLo | GPKE Teil 4 | LF → NB | 55615 | ✅ | — | ✅ | ✅ | — |
| 55622 | Rückmeldung/Anfrage Daten der MaLo | GPKE Teil 4 | LF → NB | 55616 | ✅ | — | ✅ | ✅ | — |
| 55623 | Rückmeldung/Anfrage Daten der TR | GPKE Teil 4 | LF → NB | 55617 | ✅ | — | ✅ | ✅ | — |
| 55624 | Rückmeldung/Anfrage Daten der SR | GPKE Teil 4 | LF → NB | 55618 | ✅ | — | ✅ | ✅ | — |
| 55625 | Rückmeldung/Anfrage Daten der Tranche | GPKE Teil 4 | LF → NB | 55619 | ✅ | — | ✅ | ✅ | — |
| 55626 | Rückmeldung/Anfrage Daten der MeLo | GPKE Teil 4 | LF → NB | 55620 | ✅ | — | ✅ | ✅ | — |
| 55627 | Änderung Daten der NeLo | GPKE Teil 4 | NB → MSB | — | ✅ | — | ✅ | ✅ | — |
| 55628 | Änderung Daten der MaLo | GPKE Teil 4 | NB → MSB | — | ✅ | — | ✅ | ✅ | — |
| 55629 | Änderung Daten der TR | GPKE Teil 4 | NB → MSB | — | ✅ | — | ✅ | ✅ | — |
| 55630 | Änderung Daten der SR | GPKE Teil 4 | NB → MSB | — | ✅ | — | ✅ | ✅ | — |
| 55632 | Änderung Daten der MeLo | GPKE Teil 4 | NB → MSB | — | ✅ | — | ✅ | ✅ | — |
| 55633 | Rückmeldung/Anfrage Daten zur NeLo | GPKE Teil 4 | MSB → NB | 55627 | ✅ | — | ✅ | ✅ | — |
| 55634 | Rückmeldung/Anfrage Daten der MaLo | GPKE Teil 4 | MSB → NB | 55628 | ✅ | — | ✅ | ✅ | — |
| 55635 | Rückmeldung/Anfrage Daten der TR | GPKE Teil 4 | MSB → NB | 55629 | ✅ | — | ✅ | ✅ | — |
| 55636 | Rückmeldung/Anfrage Daten der SR | GPKE Teil 4 | MSB → NB | 55630 | ✅ | — | ✅ | ✅ | — |
| 55638 | Rückmeldung/Anfrage Daten der MeLo | GPKE Teil 4 | MSB → NB | 55632 | ✅ | — | ✅ | ✅ | — |
| 55639 | Änderung Daten der NeLo | GPKE Teil 4 | MSB → NB | — | ✅ | — | ✅ | ✅ | — |
| 55640 | Änderung Daten der MaLo | GPKE Teil 4 | MSB → NB | — | ✅ | — | ✅ | ✅ | — |
| 55641 | Änderung Daten der SR | GPKE Teil 4 | MSB → NB | — | ✅ | — | ✅ | ✅ | — |
| 55642 | Änderung Daten der Tranche | GPKE Teil 4 | MSB → NB | — | ✅ | — | ✅ | ✅ | — |
| 55643 | Änderung Daten der MeLo | GPKE Teil 4 | MSB → NB | — | ✅ | — | ✅ | ✅ | — |
| 55644 | Rückmeldung/Anfrage Daten der NeLo | GPKE Teil 4 | NB → MSB | 55639 | ✅ | — | ✅ | ✅ | — |
| 55645 | Rückmeldung/Anfrage Daten der MaLo | GPKE Teil 4 | NB → MSB | 55640 | ✅ | — | ✅ | ✅ | — |
| 55646 | Rückmeldung/Anfrage Daten der SR | GPKE Teil 4 | NB → MSB | 55641 | ✅ | — | ✅ | ✅ | — |
| 55647 | Rückmeldung/Anfrage Daten der Tranche | GPKE Teil 4 | NB → MSB | 55642 | ✅ | — | ✅ | ✅ | — |
| 55648 | Rückmeldung/Anfrage Daten der MeLo | GPKE Teil 4 | NB → MSB | 55643 | ✅ | — | ✅ | ✅ | — |
| 55649 | Änderung Daten der NeLo | GPKE Teil 4 | MSB → LF | — | ✅ | — | ✅ | ✅ | — |
| 55650 | Änderung Daten der MaLo | GPKE Teil 4 | MSB → LF | — | ✅ | — | ✅ | ✅ | — |
| 55651 | Änderung Daten der SR | GPKE Teil 4 | MSB → LF | — | ✅ | — | ✅ | ✅ | — |
| 55652 | Änderung Daten der Tranche | GPKE Teil 4 | MSB → LF | — | ✅ | — | ✅ | ✅ | — |
| 55653 | Änderung Daten der MeLo | GPKE Teil 4 | MSB → LF | — | ✅ | — | ✅ | ✅ | — |
| 55654 | Rückmeldung/Anfrage Daten der NeLo | GPKE Teil 4 | LF → MSB | 55649 | ✅ | — | ✅ | ✅ | — |
| 55655 | Rückmeldung/Anfrage Daten der MaLo | GPKE Teil 4 | LF → MSB | 55650 | ✅ | — | ✅ | ✅ | — |
| 55656 | Rückmeldung/Anfrage Daten der SR | GPKE Teil 4 | LF → MSB | 55651 | ✅ | — | ✅ | ✅ | — |
| 55657 | Rückmeldung/Anfrage Daten der Tranche | GPKE Teil 4 | LF → MSB | 55652 | ✅ | — | ✅ | ✅ | — |
| 55658 | Rückmeldung/Anfrage Daten der MeLo | GPKE Teil 4 | LF → MSB | 55653 | ✅ | — | ✅ | ✅ | — |
| 55659 | Änderung Daten der NeLo | GPKE Teil 4 | MSB → MSB | — | ✅ | — | ✅ | ✅ | — |
| 55660 | Änderung Daten der MaLo | GPKE Teil 4 | MSB → MSB | — | ✅ | — | ✅ | ✅ | — |
| 55661 | Änderung Daten der SR | GPKE Teil 4 | MSB → MSB | — | ✅ | — | ✅ | ✅ | — |
| 55662 | Änderung Daten der Tranche | GPKE Teil 4 | MSB → MSB | — | ✅ | — | ✅ | ✅ | — |
| 55663 | Änderung Daten der MeLo | GPKE Teil 4 | MSB → MSB | — | ✅ | — | ✅ | ✅ | — |
| 55664 | Rückmeldung/Anfrage Daten der NeLo | GPKE Teil 4 | MSB → MSB | 55659 | ✅ | — | ✅ | ✅ | — |
| 55665 | Rückmeldung/Anfrage Daten der MaLo | GPKE Teil 4 | MSB → MSB | 55660 | ✅ | — | ✅ | ✅ | — |
| 55666 | Rückmeldung/Anfrage Daten der SR | GPKE Teil 4 | MSB → MSB | 55661 | ✅ | — | ✅ | ✅ | — |
| 55667 | Rückmeldung/Anfrage Daten der Tranche | GPKE Teil 4 | MSB → MSB | 55662 | ✅ | — | ✅ | ✅ | — |
| 55669 | Rückmeldung/Anfrage Daten der MeLo | GPKE Teil 4 | MSB → MSB | 55663 | ✅ | — | ✅ | ✅ | — |
| 55670 | Stammdaten BK-Treue | GPKE Teil 4 | NB → ÜNB | — | ✅ | — | ✅ | ✅ | — |
| 55671 | Rückmeldung auf Stammdaten BK-Treue | GPKE Teil 4 | ÜNB → NB | 55670 | ✅ | — | ✅ | ✅ | — |
| 55672 | Abr.-Daten BK-Abr. erz. Malo | GPKE Teil 2 / AWH NBW | NB → LF · NBA → NBN | — | ✅ | — | ✅ | ✅ | — |
| 55673 | Rückmeldung/Anfrage Abr.-Daten BK-Abr. erz. Malo | GPKE Teil 2 | LF → NB | 55672 | ✅ | — | ✅ | ✅ | — |
| 55674 | Abr.-Daten BK-Abr. erz. Malo | GPKE Teil 2 | NB → ÜNB | — | ✅ | — | ✅ | ✅ | — |
| 55675 | Rückmeldung/Anfrage Abr.-Daten BK-Abr. erz. Malo | GPKE Teil 2 | ÜNB → NB | 55674 | ✅ | — | ✅ | ✅ | — |
| 55684 | Änderung Daten der MaLo | GPKE Teil 4 | MSB → ÜNB | — | ✅ | — | ✅ | ✅ | — |
| 55685 | Rückmeldung/Anfrage Daten der MaLo | GPKE Teil 4 | ÜNB → MSB | 55684 | ✅ | — | ✅ | ✅ | — |
| 55686 | Änderung Daten der Tranche | GPKE Teil 4 | MSB → ÜNB | — | ✅ | — | ✅ | ✅ | — |
| 55687 | Rückmeldung/Anfrage Daten der Tranche | GPKE Teil 4 | ÜNB → MSB | 55686 | ✅ | — | ✅ | ✅ | — |
| 55688 | Änderung Daten der MaLo | GPKE Teil 4 | NB → ÜNB | — | ✅ | — | ✅ | ✅ | — |
| 55689 | Rückmeldung/Anfrage Daten der MaLo | GPKE Teil 4 | ÜNB → NB | — | ✅ | — | ✅ | ✅ | — |
| 55690 | Lokationsbündelstruktur und DB | AWH NBW | NBA → NBN | — | — | — | ✅ | ✅ | — |
| 55691 | Änderung Paket-ID der MaLo | GPKE Teil 4 / AWH NBW | NB → LF · NB → MSB · NB → ÜNB · NBA → NBN | — | ✅ | — | ✅ | ✅ | — |
| 55692 | Rückmeldung/Anfrage Paket-ID der MaLo | GPKE Teil 4 | LF → NB · MSB → NB · ÜNB → NB | 55691 | ✅ | — | ✅ | ✅ | — |
| 55693 | Änderung Daten der TR | GPKE Teil 4 | LF → NB | — | — | ✅ | ⚠️ | ✅ | — |
| 55694 | Rückmeldung/ Anfrage Daten der TR | GPKE Teil 4 | NB → LF | 55693 | — | ✅ | ⚠️ | ✅ | — |

## UTILMD AHB Gas

| PID | Beschreibung | Prozess | Von → An | Reaktion | ⚡ | 🔥 | 3.3 | 4.0 | Crate / Workflow |
|-----|--------------|---------|----------|----------|---|---|-----|-----|------------------|
| 44001 | Anmeldung NN | GeLi Gas 2.0 | LFN → NB | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-supplier-change` |
| 44002 | Bestätigung Anmeldung | GeLi Gas 2.0 | NB → LFN | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-supplier-change` |
| 44003 | Ablehnung Anmeldung | GeLi Gas 2.0 | NB → LFN | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-supplier-change` |
| 44004 | Abmeldung NN | GeLi Gas 2.0 | LF → NB | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-supplier-change` |
| 44005 | Bestätigung Abmeldung | GeLi Gas 2.0 | NB → LF | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-supplier-change` |
| 44006 | Ablehnung Abmeldung | GeLi Gas 2.0 | NB → LF | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-supplier-change` |
| 44007 | Abmeldung NN vom NB | GeLi Gas 2.0 | NB → LF | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-supplier-change` |
| 44008 | Bestätigung Abmeldung vom NB | GeLi Gas 2.0 | LF → NB | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-supplier-change` |
| 44009 | Ablehnung Abmeldung vom NB | GeLi Gas 2.0 | LF → NB | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-supplier-change` |
| 44010 | Abmeldungsanfrage des NB | GeLi Gas 2.0 | NB → LFA | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-supplier-change` |
| 44011 | Bestätigung Abmeldungsanfrage | GeLi Gas 2.0 | LFA → NB | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-supplier-change` |
| 44012 | Ablehnung Abmeldungsanfrage | GeLi Gas 2.0 | LFA → NB | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-supplier-change` |
| 44013 | Anmeldung EoG | GeLi Gas 2.0 | NB → LF | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-supplier-change` |
| 44014 | Bestätigung EoG Anmeldung | GeLi Gas 2.0 | LF → NB | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-supplier-change` |
| 44015 | Ablehnung EoG Anmeldung | GeLi Gas 2.0 | LF → NB | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-supplier-change` |
| 44016 | Kündigung beim alten Lieferanten | GeLi Gas 2.0 | LFN → LFA | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-supplier-change` |
| 44017 | Bestätigung Kündigung | GeLi Gas 2.0 | LFA → LFN | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-supplier-change` |
| 44018 | Ablehnung Kündigung | GeLi Gas 2.0 | LFA → LFN | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-supplier-change` |
| 44019 | Bestandsliste zugeordnete Marktlokationen | GeLi Gas 2.0 | NB → LF | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-supplier-change` |
| 44020 | Änderungsmeldung zur Bestandsliste | GeLi Gas 2.0 | LF → NB | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-supplier-change` |
| 44021 | Antwort auf Änderungsmeldung zur Bestandsliste | GeLi Gas 2.0 | NB → LF | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-supplier-change` |
| 44022 | Anfrage nach Stornierung | WiM Gas / GeLi Gas 2.0 | Sender → Empf. · orig. → orig. | — | — | ✅ | ✅ | ✅ | `mako-wim-gas` `wim-gas-stornierung` |
| 44023 | Bestätigung Anfrage Stornierung | WiM Gas / GeLi Gas 2.0 | Empf. → Sender | — | — | ✅ | ✅ | ✅ | `mako-wim-gas` `wim-gas-stornierung` |
| 44024 | Ablehnung Anfrage Stornierung | WiM Gas / GeLi Gas 2.0 | Empf. → Sender | — | — | ✅ | ✅ | ✅ | `mako-wim-gas` `wim-gas-stornierung` |
| 44035 | Antwort auf die Geschäftsdatenanfrage | GeLi Gas 2.0 | NB → LF | 17101 | — | ✅ | ✅ | ✅ | — |
| 44036 | Informationsmeldung über existierende Zuordnung | GeLi Gas 2.0 | NB → LFN | — | — | ✅ | ✅ | ✅ | — |
| 44037 | Informationsmeldung zur Beendigung der Zuordnung | GeLi Gas 2.0 | NB → LFA | — | — | ✅ | ✅ | ✅ | — |
| 44038 | Informationsmeldung zur Aufhebung einer zuk. Zuordnung | GeLi Gas 2.0 | NB → LF | — | — | ✅ | ✅ | ✅ | — |
| 44039 | Kündigung MSB | WiM Gas | MSBN → MSBA | — | — | ✅ | ✅ | ✅ | `mako-wim-gas` `wim-gas-kuendigung` |
| 44040 | Bestätigung Kündigung MSB | WiM Gas | MSBA → MSBN | — | — | ✅ | ✅ | ✅ | `mako-wim-gas` `wim-gas-kuendigung` |
| 44041 | Ablehnung Kündigung MSB | WiM Gas | MSBA → MSBN | — | — | ✅ | ✅ | ✅ | `mako-wim-gas` `wim-gas-kuendigung` |
| 44042 | Anmeldung MSB | WiM Gas | MSBN → NB | — | — | ✅ | ✅ | ✅ | `mako-wim-gas` `wim-gas-anmeldung` |
| 44043 | Bestätigung Anmeldung MSB | WiM Gas | NB → MSBN | — | — | ✅ | ✅ | ✅ | `mako-wim-gas` `wim-gas-anmeldung` |
| 44044 | Ablehnung Anmeldung MSB | WiM Gas | NB → MSBN | — | — | ✅ | ✅ | ✅ | `mako-wim-gas` `wim-gas-anmeldung` |
| 44051 | Ende MSB | WiM Gas | MSBA → NB | — | — | ✅ | ✅ | ✅ | `mako-wim-gas` `wim-gas-anmeldung` |
| 44052 | Bestätigung Ende MSB | WiM Gas | NB → MSBA | — | — | ✅ | ✅ | ✅ | `mako-wim-gas` `wim-gas-anmeldung` |
| 44053 | Ablehnung Ende MSB | WiM Gas | NB → MSBA | — | — | ✅ | ✅ | ✅ | `mako-wim-gas` `wim-gas-anmeldung` |
| 44060 | Antwort auf die Geschäftsdatenanfrage | GeLi Gas 2.0 | NB → MSB | — | — | ✅ | ✅ | ✅ | — |
| 44101 | Stammdaten zur Messlokation | NBW Leitfaden | NBN → MSB | — | — | ✅ | ✅ | ✅ | — |
| 44102 | Aktualisierte Stammdaten zur Messlokation | NBW Leitfaden | NBN → MSB | — | — | ✅ | ✅ | ✅ | — |
| 44103 | Stammdaten zur verbrauchenden Marktlokation | NBW Leitfaden | NBN → LF | — | — | ✅ | ✅ | ✅ | — |
| 44104 | Aktualisierte Stammdaten zur verbrauchenden Marktlokation | NBW Leitfaden | NBN → LF | — | — | ✅ | ✅ | ✅ | — |
| 44105 | Ablehnung auf Stammdaten zur verbrauchenden Marktlokation | NBW Leitfaden | LF → NBN | — | — | ✅ | ✅ | ✅ | — |
| 44109 | Nicht bila.rel Änderung vom LF | GeLi Gas 2.0 | LF → NB | — | — | ✅ | ✅ | ✅ | — |
| 44111 | Antwort auf Änderung vom LF | GeLi Gas 2.0 | NB → LF | 44109 | — | ✅ | ✅ | ✅ | — |
| 44112 | Nicht bila.rel. Änderung vom NB | Marktraumumstellung / WiM Gas / GeLi Gas 2.0 | NB → LF | — | — | ✅ | ✅ | ✅ | — |
| 44113 | Nicht bila.rel. Änderung vom NB | Marktraumumstellung / GeLi Gas 2.0 | NB → MSB | — | — | ✅ | ✅ | ✅ | — |
| 44115 | Antwort auf Änderung vom NB | Marktraumumstellung / GeLi Gas 2.0 | MSB → NB · LF → NB | 44112, 44113 | — | ✅ | ✅ | ✅ | — |
| 44116 | Änderung vom MSB mit Abhängigkeiten | GeLi Gas 2.0 | MSB → NB | — | — | ✅ | ✅ | ✅ | — |
| 44117 | Änderung vom MSB mit Abhängigkeiten | GeLi Gas 2.0 | NB → LF | — | — | ✅ | ✅ | ✅ | — |
| 44119 | Antwort auf Änderung vom MSB | GeLi Gas 2.0 | NB → MSB · LF → NB | 44116, 44117 | — | ✅ | ✅ | ✅ | — |
| 44120 | Bila.rel. Änderung vom LF | Marktraumumstellung / GeLi Gas 2.0 | LF → NB | — | — | ✅ | ✅ | ✅ | — |
| 44121 | Antwort auf Änderung vom LF | Marktraumumstellung / GeLi Gas 2.0 | NB → LF | 44120 | — | ✅ | ✅ | ✅ | — |
| 44123 | Bila.rel. Änderung vom NB mit Abhängigkeiten | GeLi Gas 2.0 | NB → LF | — | — | ✅ | ✅ | ✅ | — |
| 44124 | Antwort auf Änderung vom NB | GeLi Gas 2.0 | LF → NB | 44123 | — | ✅ | ✅ | ✅ | — |
| 44137 | Nicht bila. rel. Anfrage an LF | GeLi Gas 2.0 | NB → LF | — | — | ✅ | ✅ | ✅ | — |
| 44138 | Antwort auf Anfrage | GeLi Gas 2.0 | LF → NB | 44137 | — | ✅ | ✅ | ✅ | — |
| 44139 | Nicht bila.rel. Anfrage an NB | Marktraumumstellung / GeLi Gas 2.0 | LF → NB | — | — | ✅ | ✅ | ✅ | — |
| 44140 | Nicht bila.rel. Anfrage an NB | Marktraumumstellung / GeLi Gas 2.0 | MSB → NB | — | — | ✅ | ✅ | ✅ | — |
| 44142 | Antwort auf Anfrage | Marktraumumstellung / GeLi Gas 2.0 | NB → LF · NB → MSB | 44139, 44140 | — | ✅ | ✅ | ✅ | — |
| 44143 | Anfrage an MSB mit Abhängigkeiten | GeLi Gas 2.0 | LF → NB | — | — | ✅ | ✅ | ✅ | — |
| 44145 | Antwort auf Anfrage | GeLi Gas 2.0 | NB → LF | 44143 | — | ✅ | ✅ | ✅ | — |
| 44146 | Ablehnung der Anfrage | GeLi Gas 2.0 | NB → LF | 44143 | — | ✅ | ✅ | ✅ | — |
| 44147 | Anfrage an MSB mit Abhängigkeiten | GeLi Gas 2.0 | NB → MSB | — | — | ✅ | ✅ | ✅ | — |
| 44148 | Anfrage an MSB mit Abhängigkeiten | GeLi Gas 2.0 | NB → MSB | — | — | ✅ | ✅ | ✅ | — |
| 44149 | Antwort auf Anfrage | GeLi Gas 2.0 | MSB → NB | 44147, 44148 | — | ✅ | ✅ | ✅ | — |
| 44150 | Bila. rel. Anfrage an LF | GeLi Gas 2.0 | NB → LF | — | — | ✅ | ✅ | ✅ | — |
| 44151 | Antwort auf Anfrage | GeLi Gas 2.0 | LF → NB | 44150 | — | ✅ | ✅ | ✅ | — |
| 44152 | Ablehnung der Anfrage | GeLi Gas 2.0 | LF → NB | 44150 | — | ✅ | ✅ | ✅ | — |
| 44156 | Bila.rel. Anfrage an NB mit Abhängigkeiten | GeLi Gas 2.0 | LF → NB | — | — | ✅ | ✅ | ✅ | — |
| 44157 | Antwort auf Anfrage | GeLi Gas 2.0 | NB → LF | 44156 | — | ✅ | ✅ | ✅ | — |
| 44159 | Änderung vom MSB ohne Abhängigkeiten | GeLi Gas 2.0 | MSB → NB | — | — | ✅ | ✅ | ✅ | — |
| 44160 | Änderung vom MSB ohne Abhängigkeiten | GeLi Gas 2.0 | NB → LF | — | — | ✅ | ✅ | ✅ | — |
| 44161 | Antwort auf Änderung | GeLi Gas 2.0 | NB → MSB · LF → NB | 44159, 44160 | — | ✅ | ✅ | ✅ | — |
| 44162 | Anfrage an MSB ohne Abhängigkeiten | GeLi Gas 2.0 | LF → NB | — | — | ✅ | ✅ | ✅ | — |
| 44163 | Antwort auf Anfrage | GeLi Gas 2.0 | NB → LF | 44162 | — | ✅ | ✅ | ✅ | — |
| 44164 | Ablehnung Anfrage | GeLi Gas 2.0 | NB → LF | 44162 | — | ✅ | ✅ | ✅ | — |
| 44165 | Nicht bila. rel Anfrage an MSB ohne Abhängigkeiten | GeLi Gas 2.0 | NB → MSB | — | — | ✅ | ✅ | ✅ | — |
| 44166 | Nicht bila. rel Anfrage an MSB ohne Abhängigkeiten | GeLi Gas 2.0 | NB → MSB | — | — | ✅ | ✅ | ✅ | — |
| 44167 | Antwort auf Anfrage | GeLi Gas 2.0 | MSB → NB | 44165, 44166 | — | ✅ | ✅ | ✅ | — |
| 44168 | Verpflichtungsanfrage / Aufforderung | WiM Gas | NB → gMSB | — | — | ✅ | ✅ | ✅ | `mako-wim-gas` `wim-gas-verpflichtungsanfrage` |
| 44169 | Bestätigung Verpflichtungsanfrage | WiM Gas | gMSB → NB | — | — | ✅ | ✅ | ✅ | `mako-wim-gas` `wim-gas-verpflichtungsanfrage` |
| 44170 | Ablehnung Verpflichtungsanfrage | WiM Gas | gMSB → NB | — | — | ✅ | ✅ | ⚠️ | `mako-wim-gas` `wim-gas-verpflichtungsanfrage` |
| 44175 | Änderung der Marktlokationsstruktur | GeLi Gas 2.0 | NB → LF | — | — | ✅ | ✅ | ✅ | — |
| 44176 | Antwort auf Änderung der Marktlokationsstruktur | GeLi Gas 2.0 | LF → NB | 44175 | — | ✅ | ✅ | ✅ | — |
| 44180 | Anfrage der Marktlokationsstruktur | GeLi Gas 2.0 | LF → NB | — | — | ✅ | ✅ | ✅ | — |
| 44181 | Antwort auf Anfrage der Marktlokationsstruktur | GeLi Gas 2.0 | NB → LF | 44108 | — | ✅ | ✅ | ✅ | — |
| 44182 | Ablehnung der Anfrage der Marktlokationsstruktur | GeLi Gas 2.0 | NB → LF | 44180 | — | ✅ | ✅ | ✅ | — |
| 44183 | Ende MSB von NB | AWH WiM Gas 2.0 | NB → MSB | — | — | — | ⚠️ | ✅ | — |

## ORDERS AHB

| PID | Beschreibung | Prozess | Von → An | Reaktion | ⚡ | 🔥 | 3.3 | 4.0 | Crate / Workflow |
|-----|--------------|---------|----------|----------|---|---|-----|-----|------------------|
| 17001 | Bestellung Geräteübernahmeangebot | WiM Gas / WiM Strom Teil 1 | MSBN → MSBA | — | ✅ | ✅ | ✅ | ✅ | `mako-wim` `wim-geraeteubernahme` |
| 17002 | Weiterverpflichtung | WiM Gas / WiM Strom Teil 1 | NB → MSB · NB → MSBA | — | ✅ | ✅ | ✅ | ✅ | `mako-wim` `wim-geraeteubernahme` |
| 17004 | Anforderung von Werten | WiM Strom Teil 2 / GeLi Gas 2.0 | NB → MSB · MSB → MSB · LF → MSB | — | ✅ | ✅ | ✅ | ✅ | — |
| 17005 | Bestellung Angebot Rechnungsabwicklung Messstellenbetrieb | WiM Strom Teil 1 | LF → MSB | — | ✅ | — | ✅ | ✅ | — |
| 17006 | Beendigung Rechnungsabwicklung MSB über LF | WiM Strom Teil 1 | MSB → LF · LF → MSB | — | ✅ | — | ✅ | ✅ | — |
| 17007 | Bestellung und Abbestellung von Werten ESA | WiM Strom Teil 2 Kap. 4 | ESA → MSB | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-wertebestellung` |
| 17009 | Ankündigung Gerätewechselabsicht | WiM Gas / WiM Strom Teil 1 | MSBN → MSBA | — | ✅ | ✅ | ✅ | ✅ | `mako-wim` `wim-geraeteubernahme` |
| 17011 | Beauftragung zur Änderung der Technik (Messlokationsänderung Strom) | WiM Strom Teil 1 | NB → MSB · LF → MSB | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-technik-aenderung` |
| 17101 | Anfrage Stammdaten Marktlokation (Gas) | GeLi Gas 2.0 | LF → NB | — | — | ✅ | ✅ | ✅ | — |
| 17102 | Anfrage von Werten | GPKE Teil 4 / GeLi Gas 2.0 | LF → MSB · LF → NB | — | ✅ | ✅ | ✅ | ✅ | — |
| 17103 | Anfrage Brennwert / Zustandszahl | GeLi Gas 2.0 | LF → NB | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-datenabruf` |
| 17104 | Anfrage vom MSB Gas | GPKE Teil 4 | MSB → NB | — | ✅ | — | ✅ | ✅ | `mako-geli-gas` `geli-gas-datenabruf` |
| 17110 | Anforderung der Allokationsliste | MMM Strom/Gas | LF → NB | — | — | ✅ | ✅ | ✅ | `mako-gabi-gas` `gabi-gas-mmma` |
| 17113 | Reklamation von Werten | WiM Gas / WiM Strom Teil 2 | LF → NB · NB → MSB · MSB → MSB · LF → MSB · ÜNB → MSB | — | ✅ | ✅ | ✅ | ✅ | — |
| 17114 | Anforderung bilanzierte Menge | MMM Strom/Gas | NB → ÜNB | — | ✅ | — | ✅ | ⚠️ | — |
| 17115 | Sperrauftrag | AWH Sperrprozesse Gas / GPKE Teil 2 | LF → NB | — | ✅ | ✅ | ✅ | ✅ | `mako-gpke` `gpke-sperrung` (Strom inbound NB-role) · `mako-gpke` `gpke-sperrung-lf` (Strom outbound LF-role) · `mako-geli-gas` `geli-gas-sperrung-lf` (Gas outbound LF-role) · `mako-geli-gas` `geli-gas-sperrung-nb` (Gas inbound GNB-role) |
| 17116 | Anfrage Sperrung | AWH Sperrprozesse Gas / GPKE Teil 2 | NB → MSB | — | ✅ | ✅ | ✅ | ✅ | `mako-gpke` `gpke-sperrung` · `mako-geli-gas` `geli-gas-sperrung-nb` (Gas GNB→gMSB role) |
| 17117 | Entsperrauftrag | AWH Sperrprozesse Gas / GPKE Teil 2 | LF → NB | — | ✅ | ✅ | ✅ | ✅ | `mako-gpke` `gpke-sperrung` (Strom inbound NB-role) · `mako-gpke` `gpke-sperrung-lf` (Strom outbound LF-role) · `mako-geli-gas` `geli-gas-sperrung-lf` (Gas outbound LF-role) · `mako-geli-gas` `geli-gas-sperrung-nb` (Gas inbound GNB-role) |
| 17118 | Bestellung einer Konfigurationsänderung | GPKE Teil 3 | MSB → MSB | — | ✅ | — | ✅ | ✅ | — |
| 17120 | Bestellung Änderung Prognosegrundlage | GPKE Teil 3 | LF → NB | — | ✅ | — | ✅ | ✅ | — |
| 17121 | Bestellung Änderung | GPKE Teil 3 | NB → MSB | — | ✅ | — | ✅ | ✅ | — |
| 17122 | Reklamation einer Definition | GPKE Teil 3 | LF → NB · MSB → NB · NB → LF · MSB → LF | — | ✅ | — | ✅ | ✅ | — |
| 17123 | Bestellung Änderung Zählzeitdefinition | GPKE Teil 3 | LF → NB · LF → MSB | — | ✅ | — | ✅ | ✅ | — |
| 17126 | Anfrage Stammdaten Messlokation (Gas) | GeLi Gas 2.0 | MSB → NB | — | — | ✅ | ✅ | ✅ | — |
| 17128 | Reklamation einer Konfiguration | GPKE Teil 3 | NB → MSB · LF → MSB · MSB → MSB | — | ✅ | — | ✅ | ✅ | — |
| 17129 | Bestellung Beendigung einer Konfiguration | GPKE Teil 3 | NB → MSB · MSB → MSB · LF → MSB | — | ✅ | — | ✅ | ✅ | — |
| 17130 | Bestellung einer Konfiguration | GPKE Teil 3 | NB → MSB · MSB → MSB · LF → MSB | — | ✅ | — | ✅ | ✅ | — |
| 17131 | Bestellung Angebot einer Konfiguration | GPKE Teil 3 | NB → MSB · LF → MSB | — | ✅ | — | ✅ | ✅ | — |
| 17132 | Anfrage Stammdaten (Strom) | GPKE Teil 4 | LF → NB · MSB → NB | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-stammdaten` |
| 17133 | Bestellung Änderung Abrechnungsdaten | GPKE Teil 2 | LF → NB | — | ✅ | — | ✅ | ✅ | — |
| 17134 | Einrichtung Konfiguration Zuordnung LF von NB | GPKE Teil 3 | NB → MSB | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-konfiguration` |
| 17135 | Einrichtung Konfiguration Zuordnung LF von MSB | GPKE Teil 3 | MSB → MSB | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-konfiguration` |
| 17201 | Anforder. normierter Profile und Profilscharen | MaBiS | LF → NB | — | ✅ | — | ✅ | ✅ | — |
| 17202 | Anforder. Lieferantenclearingliste | MaBiS | LF → NB · LF → ÜNB | — | ✅ | — | ✅ | ✅ | — |
| 17203 | Anforder. Bilanzkreiszuordnungsliste | MaBiS | BKV → NB · BKV → ÜNB | — | ✅ | — | ✅ | ✅ | — |
| 17204 | Anforder. Clearingliste BAS | MaBiS | BKV → BIKO | — | ✅ | — | ✅ | ✅ | — |
| 17205 | Anforder. Clearingliste DZR | MaBiS | NB → BIKO | — | ✅ | — | ✅ | ✅ | — |
| 17206 | Anforderung Bilanzierungsgebietsclearingliste | MaBiS | NB → ÜNB | — | ✅ | — | ✅ | ✅ | — |
| 17207 | Ab-/Bestellung BK-SZR auf Aggregationsebene RZ | MaBiS | BKV → ÜNB | — | ✅ | — | ✅ | ✅ | — |
| 17208 | Anforderung Clearingliste ÜNB-DZR | MaBiS | ÜNB → BIKO | — | ✅ | — | ✅ | ✅ | — |
| 17209 | Anforderung Ausfallarbeit | Redispatch 2.0 | aNB → ANB | — | ✅ | — | ✅ | ✅ | `mako-redispatch` `redispatch-aktivierung` |
| 17210 | Anforderung Lieferantenausfallarbeitsclearingliste | MaBiS | LF → ANB | — | ✅ | — | ✅ | ✅ | `mako-redispatch` `redispatch-aktivierung` |
| 17211 | Reklamation Profile bzw. Profilscharen | MABIS | LF → NB | — | ✅ | — | ✅ | ✅ | `mako-redispatch` `redispatch-aktivierung` |
| 17301 | Anforderung von Stammdaten bzw. Messwerten | HKN-R (NB↔UBA) | HKN-R → NB | — | ✅ | — | ✅ | ✅ | — |

## ORDRSP AHB

| PID | Beschreibung | Prozess | Von → An | Reaktion | ⚡ | 🔥 | 3.3 | 4.0 | Crate / Workflow |
|-----|--------------|---------|----------|----------|---|---|-----|-----|------------------|
| 19001 | Bestellbestätigung | WiM Gas / WiM Strom Teil 1 | MSBA → MSBN | — | ✅ | ✅ | ✅ | ✅ | `mako-gpke` `gpke-konfiguration` ⁽ᴺᴮ⁾ · `mako-wim` `wim-geraeteubernahme` |
| 19002 | Ablehnung der Bestellung | WiM Gas / WiM Strom Teil 1 | MSBA → MSBN | — | ✅ | ✅ | ✅ | ✅ | `mako-gpke` `gpke-konfiguration` ⁽ᴺᴮ⁾ · `mako-wim` `wim-geraeteubernahme` |
| 19003 | Fortführungsbestätigung | WiM Gas / WiM Strom Teil 1 | MSB → NB · MSBA → NB | — | ✅ | ✅ | ✅ | ✅ | — |
| 19004 | Ablehnung Fortführung | WiM Gas / WiM Strom Teil 1 | MSB → NB · MSBA → NB | — | ✅ | ✅ | ✅ | ✅ | — |
| 19005 | Bestätigung Auftrag Änderung Technik | WiM Gas / WiM Strom Teil 1 / AWH Änd. Technik | MSB → LF · MSB → NB | — | ✅ | ✅ | ✅ | ✅ | — |
| 19006 | Ablehnung Auftrag Änderung Technik | WiM Gas / WiM Strom Teil 1 / AWH Änd. Technik | MSB → LF · MSB → NB | — | ✅ | ✅ | ✅ | ✅ | — |
| 19007 | Ablehnung Anforderung Werte | WiM Strom Teil 2 / GeLi Gas 2.0 | MSB → NB · MSB → MSB · MSB → LF | — | ✅ | ✅ | ✅ | ✅ | — |
| 19009 | Bestätigung Beendigung Rechnungsabwicklung MSB | WiM Strom Teil 1 | LF → MSB · MSB → LF | — | ✅ | — | ✅ | ✅ | — |
| 19010 | Ablehnung Beendigung Rechnungsabwicklung MSB | WiM Strom Teil 1 | LF → MSB · MSB → LF | — | ✅ | — | ✅ | ✅ | — |
| 19011 | Bestätigung der Ab-/Bestellung von Werten für ESA | WiM Strom Teil 2 Kap. 4 | MSB → ESA | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-wertebestellung` |
| 19012 | Ablehnung der Ab-/Bestellung von Werten für ESA | WiM Strom Teil 2 Kap. 4 | MSB → ESA | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-wertebestellung` |
| 19013 | Bestätigung der Stornierung einer Bestellung | WiM Strom Teil 2 | MSB → ESA | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-stornierung` |
| 19014 | Ablehnung der Stornierung einer Bestellung | WiM Strom Teil 2 | MSB → ESA | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-stornierung` |
| 19015 | Bestätigung Gerätewechselabsicht | WiM Gas / WiM Strom Teil 1 | MSBA → MSBN | — | ✅ | ✅ | ✅ | ✅ | `mako-wim` `wim-geraeteubernahme` |
| 19016 | Ablehnung Gerätewechselabsicht | WiM Gas / WiM Strom Teil 1 | MSBA → MSBN | — | ✅ | ✅ | ✅ | ✅ | `mako-wim` `wim-geraeteubernahme` |
| 19101 | Ablehnung der Anfrage  Stammdaten | GPKE Teil 4 / GeLi Gas 2.0 | NB → MSB · NB → LF | 17101 | ✅ | ✅ | ✅ | ✅ | — |
| 19102 | Ablehnung der Anfrage Werte | GPKE Teil 4 / GeLi Gas 2.0 | MSB → LF · NB → LF | 17102 | ✅ | ✅ | ✅ | ✅ | — |
| 19103 | Ablehnung der Anfrage Brennwert / Zustandszahl | GeLi Gas 2.0 | NB → LF | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-datenabruf` |
| 19104 | Ablehnung der Anfrage vom MSB Gas | GPKE Teil 4 | NB → MSB | — | ✅ | — | ✅ | ✅ | `mako-geli-gas` `geli-gas-datenabruf` |
| 19110 | Ablehnung der Anforderung Allokationsliste | MMM Strom/Gas | NB → LF | — | — | ✅ | ✅ | ✅ | `mako-gabi-gas` `gabi-gas-mmma` |
| 19114 | Ablehnung Reklamation | WiM Gas / WiM Strom Teil 2 | NB → LF · MSB → NB · MSB → MSB · MSB → LF · MSB → ÜNB | — | ✅ | ✅ | ✅ | ✅ | — |
| 19115 | Ablehnung der Anforderung bilanzierte Menge | MMM Strom/Gas | ÜNB → NB | — | ✅ | — | ✅ | ⚠️ | — |
| 19116 | Bestätigung Sperr-/Entsperrauftrag | AWH Sperrprozesse Gas / GPKE Teil 2 | NB → LF | — | ✅ | ✅ | ✅ | ✅ | `mako-gpke` `gpke-sperrung-lf` · `mako-geli-gas` `geli-gas-sperrung-lf` |
| 19117 | Ablehnung Sperr-/Entsperrauftrag | AWH Sperrprozesse Gas / GPKE Teil 2 | NB → LF | — | ✅ | ✅ | ✅ | ✅ | `mako-gpke` `gpke-sperrung-lf` · `mako-geli-gas` `geli-gas-sperrung-lf` |
| 19118 | Bestätigung Anfrage Sperrung | AWH Sperrprozesse Gas / GPKE Teil 2 | MSB → NB | — | ✅ | ✅ | ✅ | ✅ | `mako-gpke` `gpke-sperrung` · `mako-geli-gas` `geli-gas-sperrung-nb` |
| 19119 | Ablehnung Anfrage Sperrung | AWH Sperrprozesse Gas / GPKE Teil 2 | MSB → NB | — | ✅ | ✅ | ✅ | ✅ | `mako-gpke` `gpke-sperrung` · `mako-geli-gas` `geli-gas-sperrung-nb` |
| 19120 | Mitteilung zur Änderung | GPKE Teil 3 | MSB → NB | 17121 | ✅ | — | ✅ | ✅ | — |
| 19121 | Mitteilung zur Änderung Prognosegrundlage | GPKE Teil 3 | NB → LF | 17120 | ✅ | — | ✅ | ✅ | — |
| 19123 | Ablehnung Reklamation einer Definition | GPKE Teil 3 | NB → LF · NB → MSB · LF → NB · LF → MSB | — | ✅ | — | ✅ | ✅ | — |
| 19124 | Mitteilung zur Änderung Zählzeitdefinition | GPKE Teil 3 | NB → LF · MSB → LF | 17123 | ✅ | — | ✅ | ✅ | — |
| 19127 | Mitteilung zur Konfigurationsänderung | GPKE Teil 3 | MSB → MSB | 17118 | ✅ | — | ✅ | ✅ | — |
| 19128 | Bestätigung Stornierung Sperr-/Entsperrauftrag | AWH Sperrprozesse Gas / GPKE Teil 2 | NB → LF | — | ✅ | ✅ | ✅ | ✅ | `mako-gpke` `gpke-sperrung-lf` · `mako-geli-gas` `geli-gas-sperrung-lf` |
| 19129 | Ablehnung Stornierung Sperr-/Entsperrauftrag | AWH Sperrprozesse Gas / GPKE Teil 2 | NB → LF | — | ✅ | ✅ | ✅ | ✅ | `mako-gpke` `gpke-sperrung-lf` · `mako-geli-gas` `geli-gas-sperrung-lf` |
| 19130 | Bearbeitungsstand Reklamation Konfiguration | GPKE Teil 3 | MSB → NB · MSB → LF · MSB → MSB | — | ✅ | — | ✅ | ✅ | — |
| 19131 | Mitteilung zur Beendigung Konfiguration | GPKE Teil 3 | MSB → NB · MSB → MSB · MSB → LF | 17129 | ✅ | — | ✅ | ✅ | — |
| 19132 | Mitteilung zur Bestellung Konfiguration | GPKE Teil 3 | MSB → NB · MSB → MSB · MSB → LF | 17130 | ✅ | — | ✅ | ✅ | — |
| 19133 | Bearbeitungsstand Bestellung Änderung Abrechnungsdaten | GPKE Teil 2 | NB → LF | 17133 | ✅ | — | ✅ | ✅ | — |
| 19204 | Ablehnung Ab-/Bestellung der Aggregationsebene | MaBiS | ÜNB → BKV | — | ✅ | — | ✅ | ✅ | `mako-redispatch` `redispatch-aktivierung` |
| 19301 | Abl. der Anforderung | HKN-R (NB↔UBA) | NB → HKN-R | — | ✅ | — | ✅ | ✅ | `mako-redispatch` `redispatch-aktivierung` |
| 19302 | Best. der Anforderung zum Beenden des Abos zur Stammdaten bzw. Messwertübermittlung | HKN-R (NB↔UBA) | NB → HKN-R | — | ✅ | — | ✅ | ✅ | `mako-redispatch` `redispatch-aktivierung` |

## ORDCHG AHB

| PID | Beschreibung | Prozess | Von → An | Reaktion | ⚡ | 🔥 | 3.3 | 4.0 | Crate / Workflow |
|-----|--------------|---------|----------|----------|---|---|-----|-----|------------------|
| 39000 | Stornierung Sperr-/Entsperrauftrag | AWH Sperrprozesse Gas / GPKE Teil 2 | LF → NB | — | ✅ | ✅ | ✅ | ✅ | `mako-gpke` `gpke-sperrung` (inbound NB-role) · `mako-gpke` `gpke-sperrung-lf` (outbound LF-role) |
| 39001 | Weiterleitung der Stornierung | AWH Sperrprozesse Gas / GPKE Teil 2 | NB → MSB | — | ✅ | ✅ | ✅ | ✅ | — |
| 39002 | Stornierung der Bestellung | WiM Strom Teil 2 | ESA → MSB | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-stornierung` |

## IFTSTA AHB

| PID | Beschreibung | Prozess | Von → An | Reaktion | ⚡ | 🔥 | 3.3 | 4.0 | Crate / Workflow |
|-----|--------------|---------|----------|----------|---|---|-----|-----|------------------|
| 21000 | Statusmeldung | MaBiS | LF → NB · LF → ÜNB · LF → ANB | — | ✅ | — | ✅ | ✅ | `mako-mabis` `mabis-billing` |
| 21001 | Statusmeldung | MaBiS | NB → NB | — | ✅ | — | ✅ | ✅ | `mako-mabis` `mabis-billing` |
| 21002 | Abweisung | MaBiS | BIKO → NB · BIKO → ÜNB · BIKO → ANB | — | ✅ | — | ✅ | ✅ | `mako-mabis` `mabis-billing` |
| 21003 | Statusmeldung | MaBiS | BIKO → ÜNB · BIKO → NB · BIKO → ANB | — | ✅ | — | ✅ | ✅ | `mako-mabis` `mabis-billing` |
| 21004 | Statusmeldung | MaBiS | BIKO → NB · BIKO → BKV | — | ✅ | — | ✅ | ✅ | `mako-mabis` `mabis-billing` |
| 21005 | Statusmeldung | MaBiS | NB → BIKO · BKV → BIKO | — | ✅ | — | ✅ | ✅ | `mako-mabis` `mabis-billing` |
| 21007 | Statusmeldung | WiM Gas / WiM Strom Teil 1 | NB → MSBA · NB → LF | — | ✅ | ✅ | ✅ | ✅ | `mako-wim` `wim-device-change` |
| 21009 | Statusmeldung | WiM Gas / WiM Strom Teil 1 | MSBN → NB | — | ✅ | ✅ | ✅ | ✅ | — |
| 21010 | Statusmeldung | WiM Gas / WiM Strom Teil 1 | MSBN → NB · gMSB → NB · MSBN → MSBA | — | ✅ | ✅ | ✅ | ✅ | — |
| 21011 | Statusmeldung | WiM Gas / WiM Strom Teil 1 | NB → MSBN · NB → MSBA · NB → LF | — | ✅ | ✅ | ✅ | ✅ | — |
| 21012 | Statusmeldung | WiM Gas / WiM Strom Teil 1 | NB → MSBN | — | ✅ | ✅ | ✅ | ✅ | — |
| 21013 | Statusmeldung | WiM Gas / WiM Strom Teil 1 | NB → MSBA · NB → MSBN · NB → LF | — | ✅ | ✅ | ✅ | ✅ | — |
| 21015 | Informationsmeldung | WiM Gas | NB → MSBA | — | — | ✅ | ✅ | ⚠️ | — |
| 21018 | Statusmeldung | WiM Gas / WiM Strom Teil 1 | NB → MSBA | — | ✅ | ✅ | ✅ | ✅ | — |
| 21024 | Statusmeldung | WiM Gas | MSB → LF | — | — | ✅ | ✅ | ⚠️ | — |
| 21025 | Statusmeldung | WiM Gas / WiM Strom Teil 1 / AWH Änd. Technik | MSB → LF · gMSB → LF | — | ✅ | ✅ | ✅ | ✅ | — |
| 21026 | Statusmeldung | WiM Gas | MSB → NB | — | — | ✅ | ✅ | ⚠️ | — |
| 21027 | Statusmeldung | WiM Gas / WiM Strom Teil 1 / AWH Änd. Technik | MSB → NB · wMSB → gMSB · gMSB → NB | — | ✅ | ✅ | ✅ | ✅ | — |
| 21028 | Informationsmeldung | GeLi Gas 2.0 | MSB → NB | — | — | ✅ | ✅ | ✅ | — |
| 21029 | Vorabinformation | WiM Strom Teil 1 | gMSB → LF · gMSB → wMSB · gMSB → NB | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-device-change` |
| 21030 | iMS-Ersteinbauzust. | WiM Strom Teil 1 | wMSB → gMSB | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-device-change` |
| 21031 | Bestandss. / Eigenausbau iMS | WiM Strom Teil 1 | wMSB → gMSB | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-device-change` |
| 21032 | Antwort auf das Angebot | WiM Strom Teil 1 | LF → MSB | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-device-change` |
| 21033 | Ablehnung der Anfrage | GPKE Teil 3 / WiM Strom Teil 1 / WiM Strom Teil 2 / AWH Änd. Technik | MSB → NB · MSB → LF · MSB → ESA | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-supplier-change` |
| 21035 | Rückmeld. a. Liefers. | GPKE Teil 2 | LF → NB | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-supplier-change` |
| 21036 | Statusmeldung | WiM Strom Teil 1 | MSBN → MSBA | — | ✅ | — | ✅ | ✅ | — |
| 21037 | Ansicht NB | Redispatch 2.0 | NB → BTR | — | ✅ | — | ✅ | ✅ | `mako-redispatch` `redispatch-aktivierung` |
| 21038 | Ansicht BTR | Redispatch 2.0 | BTR → NB | — | ✅ | — | ✅ | ✅ | `mako-redispatch` `redispatch-aktivierung` |
| 21039 | Auftragsstatus (Sperren) | AWH Sperrprozesse Gas / GPKE Teil 2 | NB → LF · NB → MSB · NB → ÜNB | — | ✅ | ✅ | ✅ | ✅ | `mako-gpke` `gpke-sperrung-lf` |
| 21040 | Info Entsperrauftrag | AWH Sperrprozesse Gas / GPKE Teil 2 | NB → MSB | — | ✅ | ✅ | ✅ | ✅ | — |
| 21042 | Bestellung (WiM) | WiM Strom Teil 2 | MSB → ESA | — | ✅ | — | ✅ | ✅ | — |
| 21043 | Bestellungsantwort / -mitteilung | GPKE Teil 3 | NB → LF · MSB → MSB · MSB → NB · MSB → LF | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-konfiguration` |
| 21044 | Bestellungsbeendigung | GPKE Teil 3 | MSB → NB · MSB → LF | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-konfiguration` |
| 21045 | EnFG Informationen | GPKE Teil 4 | LF → NB | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-supplier-change` |
| 21047 | Bearbeitungsstandsmeldung | GPKE Teil 2 / GPKE Teil 4 | NB → LF · NB → ÜNB · NB → MSB · LF → NB · LF → MSB · MSB → NB · MSB → LF · MSB → MSB · MSB → ÜNB | 55156, 55220, 55673 | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-supplier-change` |

## MSCONS AHB

| PID | Beschreibung | Prozess | Von → An | Reaktion | ⚡ | 🔥 | 3.3 | 4.0 | Crate / Workflow |
|-----|--------------|---------|----------|----------|---|---|-----|-----|------------------|
| 13002 | Zählerstand (Gas) | WiM Gas / NBW Leitfaden / GeLi Gas 2.0 | MSBN → NB · MSBA → NB · NBA → NBN · LF → NB · MSB → NB · NB → MSB · NB → LF | 17102 | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-mscons` |
| 13003 | Summenzeitreihe | MaBiS / AWH Modell 2 ladev.scharf. bila. Energie.zuord.möglichkeit | NB → NB · NB → BIKO · NB → LF · ÜNB → LF · ÜNB → BIKO · BIKO → NB · BIKO → BKV · BIKO → ÜNB · ÜNB → NB · ÜNB → BKV · NB → ÜNB | — | ✅ | — | ✅ | ✅ | `mako-mabis` `mabis-billing` |
| 13005 | EEG-Überf.-ZR | EEG-Überf.-ZR | BIKO → BKV · BIKO → NB | — | ✅ | — | ✅ | ✅ | — |
| 13006 | Messwert Storno | WiM Gas / GPKE Teil 2 / WiM Strom Teil 2 / GeLi Gas 2.0 | MSBA → NB · MSBN → NB · NB → LF · MSB → MSB · MSB → NB · MSB → LF · MSB → ÜNB · LF → MSB · NB → MSB | — | ✅ | ✅ | ✅ | ✅ | — |
| 13007 | Gasbeschaffenheit | KoV BK-Mgmt Gas / WiM Gas / GeLi Gas 2.0 | NB → LF · NB → NB · MSBN → NB · MSBA → NB · MSB → NB | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-mscons` |
| 13008 | Lastgang (Gas) | KoV BK-Mgmt Gas / WiM Gas / Marktkommunikation mit der Sicherheitsplattform Gas / GeLi Gas 2.0 | NB → NB · MSBN → NB · MSBA → NB · NB → LF · NB → MSB · NB → MGV · MSB → NB | 17102 | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-mscons` |
| 13009 | Energiemenge (Gas) | WiM Gas / GeLi Gas 2.0 | MSBN → NB · MSBA → NB · MSB → NB · NB → MSB · NB → LF | 17102 | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-mscons` |
| 13010 | normiertes Profil | MaBiS | NB → MSB · NB → LF | — | ✅ | — | ✅ | ✅ | `mako-mabis` `mabis-billing` |
| 13011 | Profilschar | MaBiS | NB → MSB · NB → LF | — | ✅ | — | ✅ | ✅ | `mako-mabis` `mabis-billing` |
| 13012 | TEP vergh. Werte Referenzmessung | MaBiS | NB → MSB · NB → LF | — | ✅ | — | ✅ | ✅ | `mako-mabis` `mabis-billing` |
| 13013 | Marktlokationsscharfe Allokationsliste Gas (MMMA) | MMM Strom/Gas | NB → LF | — | — | ✅ | ✅ | ✅ | `mako-gabi-gas` `gabi-gas-mmma` |
| 13014 | Marktlokationsscharfe bilanzierte Menge Strom/Gas (MMMA) | MMM Strom/Gas | ÜNB → NB · NB → LF | — | ✅ | ✅ | ✅ | ✅ | `mako-gpke` `gpke-allokationsliste` |
| 13015 | Arbeit Leistungsmax. Kalenderjahr vor Lieferbeginn | GPKE Teil 2 | NB → LF | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-messwerte` |
| 13016 | Energiemenge u. Leistungsmax. (Strom) | GPKE Teil 2 / GPKE Teil 4 / WiM Strom Teil 2 | NB → LF · MSB → LF · MSB → NB | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-messwerte` |
| 13017 | Zählerstand (Strom) | HKN-R (NB↔UBA) / GPKE Teil 4 / WiM Strom Teil 1 / WiM Strom Teil 2 | NB → HKN-R · MSB → LF · MSBN → MSBA · MSBA → MSBN · MSB → MSB · MSB → NB · LF → MSB · NB → MSB | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-messwerte` |
| 13018 | Lastgang Messlokation, Netzkoppelpunkt, Netzlokation | MaBiS / BK-Treue / MaBiS / GPKE Teil 4 / WiM Strom Teil 2 | NB → NB · NB → ÜNB · MSB → LF · MSB → MSB · MSB → NB | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-messwerte` |
| 13019 | Energiemenge (Strom) | HKN-R (NB↔UBA) / GPKE Teil 2 / GPKE Teil 4 / WiM Strom Teil 2 | NB → HKN-R · NB → LF · MSB → LF · MSB → NB | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-messwerte` |
| 13020 | Ausfallarbeitsüberführungszeitreihe | MaBiS | ANB → ÜNB · ANB → BIKO · BIKO → BKV · ÜNB → BIKO · BIKO → ANB | — | ✅ | — | ✅ | ✅ | `mako-redispatch` `redispatch-aktivierung` |
| 13021 | Übermittlung von meteorologischen Daten | Redispatch 2.0 | BTR → ANB · ANB → aNB | — | ✅ | — | ✅ | ✅ | `mako-redispatch` `redispatch-aktivierung` |
| 13022 | Redispatch 2.0 Einzelzeitreihe Ausfallarbeit | Redispatch 2.0 / MaBiS | NB → BTR · BTR → NB · aNB → ANB · ANB → LF · ANB → aNB | — | ✅ | — | ✅ | ✅ | `mako-redispatch` `redispatch-aktivierung` |
| 13023 | Redispatch 2.0 Ausfallarbeitssummenzeitreihe | MaBiS | ANB → LF | — | ✅ | — | ✅ | ✅ | `mako-redispatch` `redispatch-aktivierung` |
| 13025 | Lastgang Marktlokation, Tranche | HKN-R (NB↔UBA) / GPKE Teil 4 / WiM Strom Teil 2 | NB → HKN-R · MSB → LF · MSB → NB · MSB → ÜNB | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-messwerte` |
| 13026 | EEG-Überf.-ZR Aufgrund Ausfallarbeit | EEG-Überf.-ZR | BIKO → BKV · BIKO → NB | — | ✅ | — | ✅ | ✅ | `mako-redispatch` `redispatch-aktivierung` |
| 13027 | Werte nach Typ 2 | WiM Strom Teil 2 | MSB → NB · MSB → LF · MSB → ESA | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-messwerte` |
| 13028 | Grundlage POG-Ermittlung | GPKE Teil 4 | NB → MSB | — | ✅ | — | ✅ | ✅ | — |

## INVOIC AHB

| PID | Beschreibung | Prozess | Von → An | Reaktion | ⚡ | 🔥 | 3.3 | 4.0 | Crate / Workflow |
|-----|--------------|---------|----------|----------|---|---|-----|-----|------------------|
| 31001 | Abschlagsrechnung | GPKE Teil 2 / GeLi Gas 2.0 | NB → LF | — | ✅ | ✅ | ✅ | ✅ | `mako-gpke` `gpke-abrechnung` |
| 31002 | NN-Rechnung | GPKE Teil 2 / GeLi Gas 2.0 | NB → LF | — | ✅ | ✅ | ✅ | ✅ | `mako-gpke` `gpke-abrechnung` |
| 31003 | WiM-Rechnung | WiM Gas / WiM Strom Teil 1 | MSBA → NB · MSBA → MSBN · MSBA → MSBN/gMSB | — | ✅ | ✅ | ✅ | ✅ | `mako-wim-gas` `wim-gas-invoic` |
| 31004 | Stornorechnung | WiM Gas / Kapazitätsabrechnung / MMM Strom/Gas / AWH Sperrprozesse Gas / GPKE Teil 2 / GPKE Teil 3 / WiM Strom Teil 1 / WiM Strom Teil 2 / AWH Änd. Technik / GeLi Gas 2.0 | MSBA → NB · MSBA → MSBN · NB → TK/KN · NB → LF · NB → MGV · MSB → NB · MSB → LF · MSBA → MSBN/gMSB · MSB → ESA | — | ✅ | ✅ | ✅ | ✅ | `mako-wim-gas` `wim-gas-invoic` |
| 31005 | MMM-Rechnung | MMM Strom/Gas | NB → LF | — | ✅ | ✅ | ✅ | ✅ | `mako-gpke` `gpke-abrechnung` |
| 31006 | MMM-selbst ausgest. Rechnung | MMM Strom/Gas | NB → LF | — | ✅ | ✅ | ✅ | ✅ | `mako-gpke` `gpke-abrechnung` |
| 31007 | Aggreg. MMM-Rechnung | MMM Strom/Gas | NB → MGV | — | — | ✅ | ✅ | ✅ | `mako-gabi-gas` `gabi-gas-invoic` |
| 31008 | Aggreg. MMM-selbst ausgest. Rechnung | MMM Strom/Gas | NB → MGV | — | — | ✅ | ✅ | ✅ | `mako-gabi-gas` `gabi-gas-invoic` |
| 31009 | MSB-Rechnung | GPKE Teil 3 / WiM Strom Teil 1 / WiM Strom Teil 2 / AWH Änd. Technik | MSB → NB · MSB → LF · MSB → ESA | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-rechnung` |
| 31010 | Kapazitätsrechnung | Kapazitätsabrechnung | NB → KN | — | — | ✅ | ✅ | ✅ | `mako-gabi-gas` `gabi-gas-invoic` |
| 31011 | Rechnung sonstige Leistung | AWH Sperrprozesse Gas / GPKE Teil 2 | NB → LF | — | ✅ | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-sperrprozesse-invoic` |

## REMADV AHB

| PID | Beschreibung | Prozess | Von → An | Reaktion | ⚡ | 🔥 | 3.3 | 4.0 | Crate / Workflow |
|-----|--------------|---------|----------|----------|---|---|-----|-----|------------------|
| 33001 | Bestätigung | WiM Gas / Kapazitätsabrechnung / MMM Strom/Gas / AWH Sperrprozesse Gas / GPKE Teil 2 / GPKE Teil 3 / WiM Strom Teil 1 / WiM Strom Teil 2 / AWH Änd. Technik / GeLi Gas 2.0 | NB → MSBA · MSBN → MSBA · KN → NB · LF → NB · MGV → NB · NB → MSB · LF → MSB · MSBN/gMSB → MSBA · ESA → MSB | — | ✅ | ✅ | ✅ | ✅ | `mako-gpke` `gpke-abrechnung` · `mako-wim` `wim-rechnung` · `mako-wim-gas` `wim-gas-invoic` · `mako-gabi-gas` `gabi-gas-invoic` |
| 33002 | Abweisung | WiM Gas / Kapazitätsabrechnung / MMM Strom/Gas / AWH Sperrprozesse Gas / GPKE Teil 2 / GPKE Teil 3 / WiM Strom Teil 1 / WiM Strom Teil 2 / AWH Änd. Technik / GeLi Gas 2.0 | NB → MSBA · MSBN → MSBA · KN → NB · LF → NB · MGV → NB · NB → MSB · LF → MSB · MSBN/gMSB → MSBA · ESA → MSB | — | ✅ | ✅ | ✅ | ✅ | `mako-gpke` `gpke-abrechnung` · `mako-wim` `wim-rechnung` · `mako-wim-gas` `wim-gas-invoic` |
| 33003 | Strom Abweisung Kopf und Summe | GPKE Teil 2 / GPKE Teil 3 / WiM Strom Teil 1 / WiM Strom Teil 2 / AWH Änd. Technik | LF → NB · NB → MSB · LF → MSB · MSBN/gMSB → MSBA · ESA → MSB | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-abrechnung` |
| 33004 | Strom Abweisung Position | GPKE Teil 2 / GPKE Teil 3 / WiM Strom Teil 1 / WiM Strom Teil 2 / AWH Änd. Technik | LF → NB · NB → MSB · LF → MSB · MSBN/gMSB → MSBA · ESA → MSB | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-abrechnung` |

## PARTIN AHB

| PID | Beschreibung | Prozess | Von → An | Reaktion | ⚡ | 🔥 | 3.3 | 4.0 | Crate / Workflow |
|-----|--------------|---------|----------|----------|---|---|-----|-----|------------------|
| 37000 | Kommunikationsdaten des LF Strom | GPKE Teil 4 | LF → LF · LF → NB · LF → MSB · LF → ÜNB | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-partin` |
| 37001 | Kommunikationsdaten des NB Strom | GPKE Teil 4 | NB → LF · NB → MSB · NB → NB · NB → BKV · NB → BIKO · NB → ÜNB | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-partin` |
| 37002 | Kommunikationsdaten des MSB Strom | GPKE Teil 4 | MSB → NB · MSB → LF · MSB → ÜNB · MSB → MSB · MSB → ESA | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-partin` |
| 37003 | Kommunikationsdaten des BKV Strom | GPKE Teil 4 | BKV → NB · BKV → BIKO · BKV → ÜNB | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-partin` |
| 37004 | Kommunikationsdaten des BIKO Strom | GPKE Teil 4 | BIKO → NB · BIKO → BKV · BIKO → ÜNB | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-partin` |
| 37005 | Kommunikationsdaten des ÜNB Strom | GPKE Teil 4 | ÜNB → NB · ÜNB → LF · ÜNB → BKV · ÜNB → BIKO · ÜNB → MSB | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-partin` |
| 37006 | Kommunikationsdaten des ESA Strom | GPKE Teil 4 | ESA → MSB | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-partin` |
| 37008 | Kommunikationsdaten des LF Gas | GeLi Gas 2.0 | LF → NB · LF → MSB · LF → LF | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-partin` |
| 37009 | Kommunikationsdaten des NB Gas | GeLi Gas 2.0 | NB → MSB · NB → LF · NB → NB · NB → MGV | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-partin` |
| 37010 | Kommunikationsdaten des MSB Gas | GeLi Gas 2.0 | MSB → NB · MSB → MSB · MSB → LF | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-partin` |
| 37011 | Kommunikationsdaten des MGV Gas | GeLi Gas 2.0 | MGV → NB | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-partin` |
| 37012 | Spartenübergreifende Kommunikationsdaten des NB Gas | GeLi Gas 2.0 | NB → MSB | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-partin` |
| 37013 | Spartenübergreifende Kommunikationsdaten des MSB Gas | GeLi Gas 2.0 | MSB → MSB | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-partin` |
| 37014 | Spartenübergreifende Kommunikationsdaten des MSB Strom | GeLi Gas 2.0 | MSB → MSB · MSB → NB | — | — | ✅ | ✅ | ✅ | `mako-geli-gas` `geli-gas-partin` |

## REQOTE AHB

| PID | Beschreibung | Prozess | Von → An | Reaktion | ⚡ | 🔥 | 3.3 | 4.0 | Crate / Workflow |
|-----|--------------|---------|----------|----------|---|---|-----|-----|------------------|
| 35001 | Anfrage Geräteübernahmeangebot | WiM Gas / WiM Strom Teil 1 | MSBN → MSBA | — | ✅ | ✅ | ✅ | ✅ | `mako-wim` `wim-preisanfrage` (Strom 5WT) · Gas-only deployment: `mako-wim-gas` `wim-gas-preisanfrage` (10WT) |
| 35002 | Anfrage Rechnungsabwicklung MSB über LF | WiM Strom Teil 1 | LF → MSB | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-preisanfrage` |
| 35003 | Anfrage von Werten | WiM Strom Teil 2 | ESA → MSB | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-preisanfrage` |
| 35004 | Anfrage einer Konfiguration | GPKE Teil 3 | NB → MSB · LF → MSB | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-preisanfrage` |
| 35005 | Anfrage Angebot Änderung Technik | AWH Änd. Technik | NB → MSB · LF → MSB | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-preisanfrage` |

## QUOTES AHB

| PID | Beschreibung | Prozess | Von → An | Reaktion | ⚡ | 🔥 | 3.3 | 4.0 | Crate / Workflow |
|-----|--------------|---------|----------|----------|---|---|-----|-----|------------------|
| 15001 | Angebot Geräteübernahme | WiM Gas / WiM Strom Teil 1 | MSBA → MSBN | — | ✅ | ✅ | ✅ | ✅ | `mako-wim` `wim-preisanfrage` (Strom 5WT) · Gas-only deployment: `mako-wim-gas` `wim-gas-preisanfrage` (10WT) |
| 15002 | Angebot Abrechnung Messstellenbetrieb MSB | WiM Strom Teil 1 | MSB → LF | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-preisanfrage` |
| 15003 | Angebot zur Anfrage von Werten | WiM Strom Teil 2 | MSB → ESA | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-preisanfrage` |
| 15004 | Angebot  einer Konfiguration | GPKE Teil 3 | MSB → NB · MSB → LF | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-preisanfrage` |
| 15005 | Angebot Änderung Technik | AWH Änd. Technik | MSB → NB · MSB → LF | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-preisanfrage` |

## PRICAT AHB

| PID | Beschreibung | Prozess | Von → An | Reaktion | ⚡ | 🔥 | 3.3 | 4.0 | Crate / Workflow |
|-----|--------------|---------|----------|----------|---|---|-----|-----|------------------|
| 27001 | Übermittlung Ausgleichsenergiepreis | MaBiS | BIKO → BKV | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-preisliste` |
| 27002 | Preisblätter MSB-Leistungen | GPKE Teil 3 / WiM Strom Teil 1 / AWH Änd. Technik | MSB → NB · MSB → LF | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-preisliste` |
| 27003 | Preisblätter NB-Leistungen | AWH Sperrprozesse Gas / GPKE Teil 2 | NB → LF | — | ✅ | ✅ | ✅ | ✅ | `mako-wim` `wim-preisliste` (Strom) · Gas-only deployment: `mako-geli-gas` `geli-gas-preisliste` (10WT) |

## INSRPT AHB

| PID | Beschreibung | Prozess | Von → An | Reaktion | ⚡ | 🔥 | 3.3 | 4.0 | Crate / Workflow |
|-----|--------------|---------|----------|----------|---|---|-----|-----|------------------|
| 23001 | Störungsmeldung | WiM Gas / WiM Strom Teil 2 | LF → MSB · NB → MSB · Melder → MSB | — | ✅ | ✅ | ✅ | ✅ | `mako-wim` `wim-insrpt` (Strom 5WT · combined) · `mako-wim-gas` `wim-gas-insrpt` (Gas-only 10WT) |
| 23003 | Ablehnung | WiM Gas / WiM Strom Teil 2 | MSB → LF · MSB → NB · MSB → Melder | — | ✅ | ✅ | ✅ | ✅ | `mako-wim` `wim-insrpt` (Strom 5WT · combined) · `mako-wim-gas` `wim-gas-insrpt` (Gas-only 10WT) |
| 23004 | Bestätigung | WiM Gas / WiM Strom Teil 2 | MSB → LF · MSB → NB · MSB → Melder | — | ✅ | ✅ | ✅ | ✅ | `mako-wim` `wim-insrpt` (Strom 5WT · combined) · `mako-wim-gas` `wim-gas-insrpt` (Gas-only 10WT) |
| 23005 | Ablehnung Gas-Variante | WiM Gas | MSB → NB · MSB → MSB | — | — | ✅ | ✅ | ✅ | `mako-wim-gas` `wim-gas-insrpt` (Gas-only + combined) |
| 23008 | Ergebnisbericht | WiM Gas / WiM Strom Teil 2 | MSB → LF · MSB → NB · MSB → Melder | — | ✅ | ✅ | ✅ | ✅ | `mako-wim` `wim-insrpt` (Strom 5WT · combined) · `mako-wim-gas` `wim-gas-insrpt` (Gas-only 10WT) |
| 23009 | Ergebnisbericht Gas-Variante | WiM Gas | MSB → NB · MSB → MSB | — | — | ✅ | ✅ | ✅ | `mako-wim-gas` `wim-gas-insrpt` (Gas-only + combined) |
| 23011 | Informationsmeldung | WiM Strom Teil 2 | MSB → NB · MSB → LF · MSB → ÜNB | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-insrpt` |
| 23012 | Informationsmeldung | WiM Strom Teil 2 | MSB → NB · MSB → LF · MSB → ÜNB | — | ✅ | — | ✅ | ✅ | `mako-wim` `wim-insrpt` |

## UTILTS AHB

| PID | Beschreibung | Prozess | Von → An | Reaktion | ⚡ | 🔥 | 3.3 | 4.0 | Crate / Workflow |
|-----|--------------|---------|----------|----------|---|---|-----|-----|------------------|
| 25001 | Berechnungsformel | WiM Strom Teil 2 / AWH NBW | NB → MSB · NB → LF · NBA → NBN | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-utilts` |
| 25004 | Übermittlung Übersicht Zählzeitdefinitionen | GPKE Teil 3 | NB → LF · NB → MSB · LF → MSB | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-utilts` |
| 25005 | Übermittlung einer ausgerollten Zählzeitdefinition | GPKE Teil 3 | NB → LF · NB → MSB · LF → MSB | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-utilts` |
| 25006 | Übermittlung Übersicht Schaltzeitdefinitionen | GPKE Teil 3 | NB → LF · NB → MSB · LF → NB · LF → MSB | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-utilts` |
| 25007 | Übermittlung Übersicht Leistungskurvendefinitionen | GPKE Teil 3 | NB → LF · NB → MSB · LF → NB · LF → MSB | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-utilts` |
| 25008 | Übermittlung einer ausgerollten Schaltzeitdefinition | GPKE Teil 3 | NB → LF · NB → MSB · LF → NB · LF → MSB | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-utilts` |
| 25009 | Übermittlung einer ausgerollten Leistungskurvendefinition | GPKE Teil 3 | NB → LF · NB → MSB · LF → NB · LF → MSB | — | ✅ | — | ✅ | ✅ | `mako-gpke` `gpke-utilts` |
| 25010 | Antwort auf Berechnungsformel | WiM Strom Teil 2 | MSB → NB | — | ✅ | — | ✅ | ✅ | — |

## COMDIS AHB

| PID | Beschreibung | Prozess | Von → An | Reaktion | ⚡ | 🔥 | 3.3 | 4.0 | Crate / Workflow |
|-----|--------------|---------|----------|----------|---|---|-----|-----|------------------|
| 29001 | Ablehnung REMADV | AWH Sperrprozesse Gas / GPKE Teil 2 / GPKE Teil 3 / WiM Strom Teil 1 / WiM Strom Teil 2 / AWH Änd. Technik / GeLi Gas 2.0 | NB → LF · MSB → NB · MSB → LF · MSB → ESA | — | ✅ | ✅ | ✅ | ✅ | `mako-gpke` `gpke-abrechnung` · `mako-wim` `wim-rechnung` · `mako-wim-gas` `wim-gas-invoic` · `mako-gabi-gas` `gabi-gas-invoic` |
| 29002 | Ablehnung IFTSTA | GPKE Teil 2 | NB → LF | — | ✅ | — | ✅ | ✅ | — |

## SSQNOT AHB

| PID | Beschreibung | Prozess | Von → An | Reaktion | ⚡ | 🔥 | 3.3 | 4.0 | Crate / Workflow |
|-----|--------------|---------|----------|----------|---|---|-----|-----|------------------|
| 70095 | Mehr-/Mindermengenmeldung SLP | MMM Strom/Gas | NB → MGV | — | — | ✅ | ✅ | ✅ | — |
| 70096 | Mehr-/Mindermengenmeldung RLP | MMM Strom/Gas | NB → MGV | — | — | ✅ | ✅ | ✅ | — |

---

*Source: BDEW PID 3.3 (FV2025-10-01, Fehlerkorrektur 27.03.2026) and PID 4.0 (FV2026-10-01).*

---

## Redispatch 2.0 — XML document types (not EDIFACT PIDs)

Redispatch 2.0 uses CIM/IEC 62325-based **XML** documents, not EDIFACT. These
document types have no Prüfidentifikator and are therefore not listed in the
BDEW PID overview. They are handled by the `redispatch-xml` crate.

| Document type | XSD version | Crate |
|---|---|---|
| `ActivationDocument` | 1.1f | `redispatch-xml` |
| `PlannedResourceScheduleDocument` | 1.0f | `redispatch-xml` |
| `AcknowledgementDocument` | 1.0f | `redispatch-xml` |
| `Stammdaten` | 1.4b | `redispatch-xml` |
| `StatusRequest_MarketDocument` | 1.1 | `redispatch-xml` |
| `Unavailability_MarketDocument` | 1.1b | `redispatch-xml` |
| `Kaskade` | 1.0 | `redispatch-xml` |
| `NetworkConstraintDocument` | 1.1b | `redispatch-xml` |
| `Kostenblatt` | 1.0d | `redispatch-xml` |

IFTSTA status messages that accompany Redispatch 2.0 workflows are EDIFACT and
use PIDs 21035–21047 (see the IFTSTA AHB section above).

See [`crates/redispatch-xml`](../crates/redispatch-xml/README.md) for schema
documentation and the parse/serialize/validate API.

