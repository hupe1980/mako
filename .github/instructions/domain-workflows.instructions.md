---
description: "Use when working in domain workflow crates: mako-gpke, mako-wim, mako-geli-gas, mako-mabis, mako-gabi-gas. Covers PID ownership, governing rulings, APERAK deadlines, and process-family-specific rules."
applyTo: "crates/mako-gpke/**, crates/mako-wim/**, crates/mako-geli-gas/**, crates/mako-mabis/**, crates/mako-gabi-gas/**"
---

# Domain Workflow Crates Instructions

## PID Ownership — Authoritative

| PID range | Crate | Governing ruling |
|---|---|---|
| 55001–55018, 55555 | `mako-gpke` | BK6-24-174 |
| 17115–17117 (Sperrung Strom, ORDERS) | `mako-gpke` `gpke-sperrung` | BK6-22-024 |
| 17134–17135 (Konfiguration, ORDERS) | `mako-gpke` `gpke-konfiguration` | BK6-22-024 |
| 19001–19002 (ORDRSP) | `mako-gpke` `gpke-konfiguration` (NB-role) · `mako-wim` `wim-geraeteubernahme` | BK6-24-174 |
| 31001–31002, 31005–31006 | `mako-gpke` `gpke-abrechnung` (INVOIC) | BK6-24-174 |
| 31007–31008 | `mako-gabi-gas` `gabi-gas-invoic` (Aggreg. MMM-Rechnung Gas) | BK7-24-01-008 |
| 37000–37006 | `mako-gpke` `gpke-partin` (PARTIN Strom) | PARTIN AHB 1.0f |
| 35001/35002/35004/35005 (REQOTE), 15001/15002/15004/15005 (QUOTES) | `mako-wim` `wim-preisanfrage` | BK6-24-174 |
| 27001–27003 (PRICAT) | `mako-wim` `wim-preisliste` | BK6-24-174 |
| 17001–17011 (Geräteübernahme, ORDERS) | `mako-wim` `wim-geraeteubernahme` | BK6-24-174 |
| 17011/17118/17121 → 19003–19007 (Technik-Änderung) | `mako-wim` `wim-technik-aenderung` | BK6-24-174 |
| **ESA Wertebestellung** 35002/15003/17007/17008/39002/19011–19014 | `mako-wim` `wim-wertebestellung` (MSB) · `mako-wim` `esa-wertebestellung` (ESA) | WiM Strom Teil 2 |
| 31009 | `mako-wim` `wim-invoic` (MSB-Rechnung) | BK6-24-174 |
| 23001–23012 | `mako-wim` `wim-insrpt` — one workflow, both Sparten | BK6-22-024 Anlage 2b / AWH WiM Gas 2.0 |
| 23005, 23009 | `mako-wim` `wim-insrpt` — Gas-only Informationsmeldungen an den NB | AWH WiM Gas 2.0 |
| 13003, 13010–13012 | `mako-mabis` `mabis-billing` (Bilanzkreisabrechnung Strom) | BK6-24-174 |
| 55065, 55069, 55070 (UTILMD Clearingliste) | `mako-mabis` `mabis-clearingliste` | BK6-24-174 |
| 44001–44021 | `mako-geli-gas` (UTILMD G Lieferantenwechsel Gas) | BK7-24-01-009 |
| 17115–17117 (Sperrung Gas, ORDERS) | `mako-geli-gas` `geli-gas-sperrung-lf` | BK7-24-01-009 |
| 37008–37014 | `mako-geli-gas` `geli-gas-partin` (PARTIN Gas) | PARTIN AHB 1.0f |
| 31011 | `mako-geli-gas` `geli-gas-sperrprozesse-invoic` (Rechnung sonstige Leistung, AWH Sperrprozesse Gas, NB → LF) | BK7-24-01-009 |
| 31003 | `mako-wim` `wim-invoic` (WiM-Rechnung Gas) | AWH WiM Gas 2.0 Kap. 4.7 |
| 31004 | `mako-wim` `wim-invoic` (Stornorechnung, Sparte-neutral) | INVOIC AHB §3.1.2 |
| 44022–44024 | `mako-geli-gas` `geli-gas-stornierung` / `-lf` (multi-domain: GeLi Gas *and* WiM Gas) | BK7-24-01-009 |
| 44039–44041 | `mako-wim` `wim-device-change` (Kündigung MSB Gas, `E_2000`) | AWH WiM Gas 2.0 |
| 44042–44044, 44051–44053, 44183 | `mako-wim` `wim-device-change` (Anmeldung / Ende MSB Gas, `E_2002`/`E_2005`) | AWH WiM Gas 2.0 |
| 44168/44169 | `mako-wim` `wim-device-change` (Verpflichtungsanfrage, `E_2006`; **44170 does not exist**) | AWH WiM Gas 2.0 |
| 31010 | `mako-gabi-gas` `gabi-gas-invoic` (Kapazitätsrechnung, FNB/VNB → BKV) | BK7-24-01-008 |

**PIDs that do NOT exist — never register:**
44555, 56001–56010, 13001, 11004–11099.

**PIDs 44022–44024 — ownership note:**
These are multi-domain (WiM Gas / GeLi Gas 2.0 per BDEW PID 3.3/4.0), and one
workflow owns them: the recipient of a 44022 is the party that received the
Ursprungsnachricht, and the workflow resolves which process is meant from
`RFF+ACW`. `mako-geli-gas` `geli-gas-stornierung` takes the NB side (44022
inbound) and `geli-gas-stornierung-lf` the LF side (44023/44024 inbound).

## APERAK Fristen — never mix these up

The APERAK window is a property of the **Sparte**, not of the crate, and it is a
different clock from the business Antwortfrist. Source: APERAK AHB 1.1
§2.3.1 (Gas) and §2.4.1 (Strom).

| Sparte | Polarität | Frist | Implementation |
|---|---|---|---|
| Strom | Anerkennung `BGM+312` **und** Fehler `BGM+313` | **45 Minuten** für UTILMD/ORDERS am Werktag; Sonntag 12:00 nach einem Samstag; sonst nächster Werktag 12:00 | `fristen::aperak_strom_due_at(received)` |
| Gas | **nur** Fehler `BGM+313` — Schweigen bis zum Fristablauf ist die Anerkennung, und auf jede APERAK folgt eine CONTRL | nächster Werktag 12:00 (Folgeprozess) / **3 Werktage** (Initialprozess) | `fristen::aperak_gas_due_at(pid, received)` |

An Initialprozessschritt is decidable, not a judgement call: it is a PID whose
„Zuordnung zu einem Objekt" column in the PID-Übersicht contains `ZO-F`
(APERAK AHB 1.1 §2.1.3.6) — `fristen::GAS_INITIALPROZESS_PIDS`.

**Saturday is not a Werktag** (GPKE Teil 1 Kap. 1.7: „alle Tage …, die kein
Samstag, Sonntag oder gesetzlicher Feiertag sind"). 24.12. and 31.12. count as
holidays, and a holiday observed in any single Bundesland counts nationwide. All
deadline arithmetic runs in German local time (CET/CEST), DST-aware.

## crates/mako-gpke

- Governed by **BK6-24-174** (Teil 1–3, eff. 2025-06-06) and **BK6-22-024** (Teil 4 Konfiguration).
- Source modules: `wechselprozesse`, `lf_anmeldung`, `lf_abmeldung`, `neuanlage`, `abrechnung`, `sperrung`, `konfiguration`, `post_acceptance`.
- The `ForwardCompatible` version policy is mandatory for all GPKE workflows.

## crates/mako-wim

- Governed by **BK6-22-024 Anlagen 2a/2b** (WiM Strom Teil 1 und Teil 2) and the
  **AWH WiM Gas 2.0** (gültig ab 01.10.2026). WiM was *not* reissued under
  BK6-24-174 — cite BK6-22-024.
- **One crate, both Sparten.** The Gas UTILMD PIDs 44039/44042/44051/44168/44183
  run the same workflows as their Strom twins; the Sparte picks the
  Entscheidungsbaum (`E_2000`…`E_2006` against `E_0200`…`E_0240`), the Codeliste
  (`G_00xx` against `S_00xx`), the APERAK regime and the Zuordnungszeitpunkt
  (06:00 Uhr Gastag against 00:00).
- Antwortfristen are 3 / 5 / 7 / 1 Werktage per PID **in both Sparten** — never a
  flat window, and never the APERAK clock.
- `SG4 STS+E01` DE 1131 and `SG2 AJT` DE 1082 carry the **Codeliste**, not the
  EBD number: ask `AntwortCode::wire_codeliste()`.
- Includes the WiM-Rechnung INVOIC workflow (`wim-invoic`): 31009
  Messstellenbetrieb (Strom), 31003 Dienstleistungen (**both Sparten**) and the
  Sparte-neutral Stornorechnung 31004. The answer is due *zum* Zahlungsziel
  (`SG8 DTM+265`), except 31009 to a **NB**, which is the **4. WT davor**
  (Kap. 6.2 Nr. 2) — `mako_fristen::vorlauf::rechnung_antwort_spaetester_uet`.
- `wim-insrpt` hosts **both sides** of the Störungsbehebung. Its two windows
  branch on the Messtechnik, which no message carries; the Weiterleitung
  23011/23012 stays due after the Ergebnisbericht has closed the Use-Case.

## crates/mako-geli-gas

- Governed by **BK7-24-01-009** (GeLi Gas 3.0, Beschluss 12.09.2025). Supersedes BK7-19-001 and BK7-06-067.
- Scope: UTILMD G (PIDs 44001–44024, incl. the Stornierung shared with WiM Gas) + ORDERS Sperrung Gas (17115–17117) + PARTIN Gas (37008–37014) + **INVOIC 31011** (Rechnung sonstige Leistung, AWH Sperrprozesse Gas, NB → LF).
- PID 31011 is billed by GNB/VNB to LFN/LFA for performing AWH (Sperrung/Entsperrung). Direction is NB → LF — NOT NB → BKV. This is GeLi Gas (BK7-24-01-009), not GaBi Gas (BK7-24-01-008).
- APERAK: Gas knows only the Verarbeitbarkeitsfehlermeldung — see the table above.

## crates/mako-mabis

- Governed by **BK6-24-174**.
- MSCONS **13003 + 13010–13012** (Bilanzkreisabrechnung Strom, BKV↔ÜNB/BIKO) via
  `mabis-billing`; UTILMD Clearinglisten **55065/55069/55070** via `mabis-clearingliste`.
- The remaining 130xx Messwesen PIDs do **not** belong here.

## crates/mako-gabi-gas

- Governed by **BK7-24-01-008** (GaBi Gas 2.1, Bundesnetzagentur).
- Scope: INVOIC **31010** only (Kapazitätsrechnung, FNB/VNB → BKV).
- GaBi Gas = gas balancing. Key roles: FNB, VNB, BKV, MGV. The BKV pays the FNB/VNB for transmission capacity.
- PID 31011 (Rechnung sonstige Leistung, AWH Sperrprozesse Gas) belongs to `mako-geli-gas`. Direction NB → LF (not NB → BKV) confirms this is NOT a GaBi Gas process.

## Cross-crate Rules

- Never register a PID in more than one crate.
- Never import workflow types from a sibling domain crate — use `mako-engine` traits and message types only.
- Each crate depends on `mako-engine` and `edi-energy`; domain crates must not depend on each other.
