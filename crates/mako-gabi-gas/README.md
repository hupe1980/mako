# mako-gabi-gas

**GaBi Gas — Gasbilanzierung Gas (Gas Balancing)**

> **This crate is a name reservation. Implementation is pending until
> `dvgw-edi` (the DVGW format layer) is complete.**

Process engine workflows for the German gas balancing framework. Governs
Allokation, Nominierung, and Mehr-/Mindermengenabrechnung Gas between balance
responsible parties (BKV), network operators (FNB/VNB), and market area
managers (MGV).

## Domain background

**GaBi Gas** (*Gasbilanzierung Gas*) is the BNetzA framework for gas network
balancing, established under the Gasnetzzugangsverordnung (GasNZV). It defines
how gas quantities are allocated, nominated, and settled across the German gas
market. The current version is **GaBi Gas 2.0** (BNetzA BK7-14-020).

## Key boundary: GaBi Gas vs. GeLi Gas

| Aspect | GeLi Gas (`mako-geli-gas`) | GaBi Gas (`mako-gabi-gas`) |
|---|---|---|
| Governing body | BNetzA BK7, GeLi Gas 3.0 (BK7-24-01-009) | BNetzA BK7, GaBi Gas 2.0 (BK7-14-020) |
| Scope | Supplier switching (Lieferantenwechsel) | Gas balancing (Bilanzierung) |
| Parties | LFN ↔ GNB | BKV ↔ FNB/VNB ↔ MGV |
| EDIFACT | UTILMD G (PIDs 44xxx) | ALLOCAT, NOMINT, NOMRES, INVOIC |
| INVOIC billing? | ❌ No | ✅ Yes — PIDs 31010–31011 |

> **Gas MMM billing belongs here, not in GeLi Gas.**
> PIDs 31010 (Kapazitätsrechnung) and 31011 (Rechnung sonstige Leistung)
> are GaBi Gas INVOIC processes governed under BK7 Bilanzierung, not GeLi Gas.

## Two-crate architecture

| Crate | Layer | Status |
|---|---|---|
| `dvgw-edi` | EDIFACT parsing/validation (ALLOCAT, NOMINT, NOMRES) | ⏳ Placeholder |
| `mako-gabi-gas` | Process engine — Workflow impls, PID routing, deadline handling | ⏳ **This crate** |

## Process families (planned)

| Process | Primary message | PIDs (planned) | Status |
|---|---|---|---|
| Allokation (gas quantity allocation) | ALLOCAT | — | ⏳ Planned |
| Nominierung (gas nominations) | NOMINT / NOMRES | — | ⏳ Planned |
| Kapazitätsrechnung | INVOIC | 31010 | ⏳ Planned |
| Rechnung sonstige Leistung | INVOIC | 31011 | ⏳ Planned |
| Tagesbilanz / Monatsbilanz | ALLOCAT | — | ⏳ Planned |

## Market roles

| Role | Abbrev. | Description |
|---|---|---|
| Fernleitungsnetzbetreiber | FNB | Gas transmission system operator |
| Verteilnetzbetreiber | VNB | Gas distribution system operator |
| Bilanzkreisverantwortlicher | BKV | Balance responsible party |
| Marktgebietsverantwortlicher | MGV | Market area manager |

## APERAK Frist

There is no standard APERAK Frist for GaBi Gas processes — these use
bilateral confirmation flows (NOMRES for nominations, REMADV for billing).

## Regulatory references

- **GasNZV** — Gasnetzzugangsverordnung, statutory basis for gas network access
- **BNetzA BK7-14-020** — GaBi Gas 2.0 ruling (current)
- **DVGW G 685** — technical rules for gas metering and allocation
- **BNetzA BK7** — governing regulatory chamber for gas
- DVGW AHBs and MIGs: <https://www.dvgw.de> / <https://www.bdew-mako.de>
