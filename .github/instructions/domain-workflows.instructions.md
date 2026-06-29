---
description: "Use when working in domain workflow crates: mako-gpke, mako-wim, mako-geli-gas, mako-mabis, mako-wim-gas, mako-gabi-gas. Covers PID ownership, governing rulings, APERAK deadlines, and process-family-specific rules."
applyTo: "crates/mako-gpke/**, crates/mako-wim/**, crates/mako-geli-gas/**, crates/mako-mabis/**, crates/mako-wim-gas/**, crates/mako-gabi-gas/**"
---

# Domain Workflow Crates Instructions

## PID Ownership — Authoritative

| PID range | Crate | Governing ruling |
|---|---|---|
| 55001–55018, 55555 | `mako-gpke` | BK6-24-174 |
| 56001–56004 | `mako-gpke` (ex-MPES, absorbed 2025-06-06) | BK6-22-024 |
| 17134–17135, 19001–19002 | `mako-gpke` (Konfiguration Teil 4) | BK6-22-024 |
| 31001–31002, 31004–31008 | `mako-gpke` (INVOIC) | BK6-24-174 |
| 11001–11003 | `mako-wim` | BK6-24-174 |
| 31003, 31009 | `mako-wim` (WiM-Rechnung / MSB-Rechnung) | BK6-24-174 |
| 13003 | `mako-mabis` | BK6-24-174 |
| 44001–44006, 44017–44018, 44555 | `mako-geli-gas` | BK7-24-01-009 |
| 44022–44053 | `mako-wim-gas` | BK7-24-01-009 |
| 31010–31011 | `mako-gabi-gas` | BK7 |

**PIDs that do NOT exist — never register:**
44007–44016, 56005–56010, 13001, 11004–11099.

## APERAK Fristen — never mix these up

| Crate | Deadline | Implementation |
|---|---|---|
| `mako-gpke` | **24 wall-clock hours** | `fristen::add_hours(t, 24)` |
| `mako-wim` | **5 Werktage** | `fristen::add_werktage(d, 5, BdewMaKo)` |
| `mako-geli-gas` | **10 Werktage** | `fristen::add_werktage(d, 10, BdewMaKo)` |

Saturday = Werktag. Sundays and German public holidays do not count. All deadline arithmetic in German local time (CET/CEST).

## crates/mako-gpke

- Governed by **BK6-24-174** (Teil 1–3, eff. 2025-06-06) and **BK6-22-024** (Teil 4 Konfiguration + ex-MPES PIDs).
- Source modules: `wechselprozesse`, `lf_anmeldung`, `lf_abmeldung`, `neuanlage`, `abrechnung`, `sperrung`, `konfiguration`, `post_acceptance`.
- MPES is **dissolved** — PIDs 56001–56004 live in this crate, not in any `mako-mpes` crate.
- The `ForwardCompatible` version policy is mandatory for all GPKE workflows.

## crates/mako-wim

- Governed by **BK6-24-174** (Wechselprozesse im Messwesen Strom, eff. 2025-06-06).
- APERAK deadline: **5 Werktage** — do not accidentally apply the GPKE 24h rule here.
- Includes WiM-Rechnung (PID 31003) and MSB-Rechnung (PID 31009) INVOIC workflows.

## crates/mako-geli-gas

- Governed by **BK7-24-01-009** (GeLi Gas 3.0, Beschluss 12.09.2025). Supersedes BK7-19-001 and BK7-06-067.
- Scope: UTILMD G only (PIDs 44001–44006, 44017–44018, 44555).
- **No INVOIC billing** belongs here — gas MMM billing (31010–31011) is in `mako-gabi-gas`.
- APERAK deadline: **10 Werktage**.

## crates/mako-mabis

- Governed by **BK6-24-174**.
- Only PID **13003** (Bilanzkreisabrechnung Strom, BKV↔ÜNB). No other PIDs.
- PIDs 13002–13028 (excl. 13003) are Messwesen PIDs — they do not belong here.

## crates/mako-wim-gas / crates/mako-gabi-gas

Placeholder crates. Do not implement business logic until the governing ruling is final and AHB profiles exist.

## Cross-crate Rules

- Never register a PID in more than one crate.
- Never import workflow types from a sibling domain crate — use `mako-engine` traits and message types only.
- Each crate depends on `mako-engine` and `edi-energy`; domain crates must not depend on each other.
