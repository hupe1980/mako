---
description: "Use when working in domain workflow crates: mako-gpke, mako-wim, mako-geli-gas, mako-mabis, mako-wim-gas, mako-gabi-gas. Covers PID ownership, governing rulings, APERAK deadlines, and process-family-specific rules."
applyTo: "crates/mako-gpke/**, crates/mako-wim/**, crates/mako-geli-gas/**, crates/mako-mabis/**, crates/mako-wim-gas/**, crates/mako-gabi-gas/**"
---

# Domain Workflow Crates Instructions

## PID Ownership — Authoritative

| PID range | Crate | Governing ruling |
|---|---|---|
| 55001–55018, 55555 | `mako-gpke` | BK6-24-174 |
| 17115–17117 (Sperrung Strom, ORDERS) | `mako-gpke` `gpke-sperrung` | BK6-22-024 |
| 17134–17135 (Konfiguration, ORDERS) | `mako-gpke` `gpke-konfiguration` | BK6-22-024 |
| 19001–19002 (ORDRSP) | `mako-gpke` `gpke-konfiguration` ⁿBNB-role⁾ · `mako-wim` `wim-geraeteubernahme` | BK6-24-174 |
| 31001–31002, 31005–31008 | `mako-gpke` `gpke-abrechnung` (INVOIC) | BK6-24-174 |
| 37000–37006 | `mako-gpke` `gpke-partin` (PARTIN Strom) | PARTIN AHB 1.0f |
| 11001–11003 | `mako-wim` | BK6-24-174 |
| 31009 | `mako-wim` `wim-rechnung` (MSB-Rechnung) | BK6-24-174 |
| 23001, 23003, 23004, 23008 | `mako-wim` `wim-insrpt` (Strom 5WT / combined) · `mako-wim-gas` `wim-gas-insrpt` (Gas-only 10WT) | BK6-24-174 / BK7-24-01-009 |
| 23005, 23009 | `mako-wim-gas` `wim-gas-insrpt` (Gas-only INSRPT, always 10WT) | BK7-24-01-009 |
| 13003 | `mako-mabis` | BK6-24-174 |
| 44001–44021 | `mako-geli-gas` (UTILMD G Lieferantenwechsel Gas) | BK7-24-01-009 |
| 17115–17117 (Sperrung Gas, ORDERS) | `mako-geli-gas` `geli-gas-sperrung-lf` | BK7-24-01-009 |
| 37008–37014 | `mako-geli-gas` `geli-gas-partin` (PARTIN Gas) | PARTIN AHB 1.0f |
| 31011 | `mako-geli-gas` `geli-gas-sperrprozesse-invoic` (Rechnung sonstige Leistung, AWH Sperrprozesse Gas, NB → LF) | BK7-24-01-009 |
| 31003 | `mako-wim-gas` `wim-gas-invoic` (WiM-Rechnung Gas) | BK7-24-01-009 |
| 31004 | `mako-wim-gas` `wim-gas-invoic` (Stornorechnung WiM Gas) | BK7-24-01-009 |
| 44022–44024 | `mako-wim-gas` `wim-gas-stornierung` (multi-domain: WiM Gas / GeLi Gas 2.0) | BK7-24-01-009 |
| 44039–44041 | `mako-wim-gas` `wim-gas-kuendigung` (Kündigung MSB Gas) | BK7-24-01-009 |
| 44042–44053 | `mako-wim-gas` `wim-gas-anmeldung` (Anmeldung / Ende MSB Gas) | BK7-24-01-009 |
| 44168–44170 | `mako-wim-gas` `wim-gas-verpflichtungsanfrage` (Verpflichtungsanfrage) | BK7-24-01-009 |
| 31010 | `mako-gabi-gas` `gabi-gas-invoic` (Kapazitätsrechnung, FNB/VNB → BKV) | BK7-14-020 |

**PIDs that do NOT exist — never register:**
44555, 56001–56010, 13001, 11004–11099.

**PIDs 44022–44024 — ownership note:**
These are multi-domain (WiM Gas / GeLi Gas 2.0 per BDEW PID 3.3/4.0). **Currently routed to `mako-wim-gas` `wim-gas-stornierung`.** GeLi Gas Stornierung role routing (LFN/LFA context) is a TODO in `mako-geli-gas`.

## APERAK Fristen — never mix these up

| Crate | Deadline | Implementation |
|---|---|---|
| `mako-gpke` | **24 wall-clock hours** | `fristen::add_hours(t, 24)` |
| `mako-wim` | **5 Werktage** | `fristen::add_werktage(d, 5, BdewMaKo)` |
| `mako-geli-gas` | **10 Werktage** | `fristen::add_werktage(d, 10, BdewMaKo)` |
| `mako-wim-gas` | **10 Werktage** | `fristen::add_werktage(d, 10, BdewMaKo)` |

Saturday = Werktag. Sundays and German public holidays do not count. All deadline arithmetic in German local time (CET/CEST).

## crates/mako-gpke

- Governed by **BK6-24-174** (Teil 1–3, eff. 2025-06-06) and **BK6-22-024** (Teil 4 Konfiguration).
- Source modules: `wechselprozesse`, `lf_anmeldung`, `lf_abmeldung`, `neuanlage`, `abrechnung`, `sperrung`, `konfiguration`, `post_acceptance`.
- The `ForwardCompatible` version policy is mandatory for all GPKE workflows.

## crates/mako-wim

- Governed by **BK6-24-174** (Wechselprozesse im Messwesen Strom, eff. 2025-06-06).
- APERAK deadline: **5 Werktage** — do not accidentally apply the GPKE 24h rule here.
- Includes MSB-Rechnung (PID 31009) INVOIC workflow (`wim-rechnung`). PID 31003 (WiM-Rechnung Gas) is in `mako-wim-gas`, not here.

## crates/mako-geli-gas

- Governed by **BK7-24-01-009** (GeLi Gas 3.0, Beschluss 12.09.2025). Supersedes BK7-19-001 and BK7-06-067.
- Scope: UTILMD G (PIDs 44001–44021) + ORDERS Sperrung Gas (17115–17117) + PARTIN Gas (37008–37014) + **INVOIC 31011** (Rechnung sonstige Leistung, AWH Sperrprozesse Gas, NB → LF).
- PID 31011 is billed by GNB/VNB to LFN/LFA for performing AWH (Sperrung/Entsperrung). Direction is NB → LF — NOT NB → BKV. This is GeLi Gas (BK7-24-01-009), not GaBi Gas (BK7-14-020).
- **APERAK deadline: 10 Werktage.**

## crates/mako-mabis

- Governed by **BK6-24-174**.
- Only PID **13003** (Bilanzkreisabrechnung Strom, BKV↔ÜNB). No other PIDs.
- PIDs 13002–13028 (excl. 13003) are Messwesen PIDs — they do not belong here.

## crates/mako-wim-gas

- Governed by **BK7-24-01-009** (same umbrella as GeLi Gas).
- PID ownership: **44022–44024** (Stornierung Gas, multi-domain), **44039–44041** (Kündigung MSB Gas), **44042–44053** (Anmeldung / Ende MSB Gas), **44168–44170** (Verpflichtungsanfrage), **31003** (WiM-Rechnung Gas), **31004** (Stornorechnung WiM Gas).
- APERAK deadline: **10 Werktage** (same as GeLi Gas).
- All three workflow modules (`kuendigung`, `anmeldung`, `verpflichtungsanfrage`) are implemented with full state machines.
- AHB profiles for these PIDs are not yet imported; the adapter layer applies `pid_has_ahb_rules()` guards.

## crates/mako-gabi-gas

- Governed by **BK7-14-020** (GaBi Gas 2.0, Bundesnetzagentur).
- Scope: INVOIC **31010** only (Kapazitätsrechnung, FNB/VNB → BKV).
- GaBi Gas = gas balancing. Key roles: FNB, VNB, BKV, MGV. The BKV pays the FNB/VNB for transmission capacity.
- PID 31011 (Rechnung sonstige Leistung, AWH Sperrprozesse Gas) belongs to `mako-geli-gas`. Direction NB → LF (not NB → BKV) confirms this is NOT a GaBi Gas process.

## Cross-crate Rules

- Never register a PID in more than one crate.
- Never import workflow types from a sibling domain crate — use `mako-engine` traits and message types only.
- Each crate depends on `mako-engine` and `edi-energy`; domain crates must not depend on each other.
