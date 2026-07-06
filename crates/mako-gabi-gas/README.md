# mako-gabi-gas

**GaBi Gas — Gasbilanzierung Gas (Gas Balancing)**

Process engine workflows for the German gas balancing framework under
GaBi Gas 2.0 (BNetzA BK7-14-020). Governs allocation, nomination, and
billing between balance responsible parties (BKV), network operators
(FNB/VNB), and market area managers (MGV).

## Implemented processes

| Workflow | PIDs / Message types | Governing document | Status |
|---|---|---|---|
| `gabi-gas-invoic` | INVOIC 31010 (Kapazitätsrechnung, NB/VNB → BKV) + 31007/31008 (Aggreg. MMM-Rechnung, NB → MGV) | BK7-14-020 | ✅ |
| `gabi-gas-allocation` | ALOCAT (synthetic PIDs 90001–90003) | BK7-14-020 / DVGW ALOCAT 5.11a | ✅ |
| `gabi-gas-nomination` | NOMINT (90011/90012) + NOMRES (90021/90022) | BK7-14-020 / DVGW NOMINT 4.6 FK / NOMRES 4.7 FK | ✅ |
| `gabi-gas-mmma` | MSCONS 13013 + ORDERS 17110 + ORDRSP 19110 (Allokationsliste Gas, MMMA) | BK7-14-020 | ✅ |
| `gabi-gas-schedl` | SCHEDL (synthetic PIDs) | DVGW SCHEDL G685/G2000 | ✅ |
| `gabi-gas-imbnot` | IMBNOT (synthetic PIDs) | DVGW IMBNOT 5.7a | ✅ |
| `gabi-gas-tranot` | TRANOT (synthetic PIDs) | DVGW TRANOT 5.8b | ✅ |
| `gabi-gas-delivery-order` | DELORD + DELRES (synthetic PIDs) | DVGW DELORD 4.5 FK / DELRES 4.6 FK | ✅ |

## Domain background

**GaBi Gas** (*Gasbilanzierung Gas*) is the BNetzA framework for gas network
balancing, established under the Gasnetzzugangsverordnung (GasNZV). It defines
how gas quantities are allocated, nominated, and settled across the German gas
transport and balancing market. The current version is **GaBi Gas 2.0**
(BNetzA BK7-14-020), which introduced the two-market-area model and mandatory
DVGW-format electronic exchange for all balancing processes.

## Key boundary: GaBi Gas vs. GeLi Gas

| Aspect | GeLi Gas (`mako-geli-gas`) | GaBi Gas (`mako-gabi-gas`) |
|---|---|---|
| Governing document | BK7-24-01-009 | BK7-14-020 |
| Scope | Supplier switching (Lieferantenwechsel Gas) + AWH billing | Gas balancing (Bilanzierung) |
| Parties | LFN ↔ GNB | BKV ↔ FNB/VNB ↔ MGV |
| Primary formats | UTILMD G (PIDs 44xxx), INVOIC 31011 | ALOCAT, NOMINT, NOMRES, INVOIC 31007/31008/31010, MSCONS 13013 |
| INVOIC billing | ✅ PID 31011 (NB → LF, AWH Sperrprozesse) | ✅ PID 31010 (NB → BKV, Kapazität) |

GaBi Gas capacity billing (PID 31010) is in this crate; AWH Sperrprozesse billing (PID 31011) is in `mako-geli-gas`.

## Two-crate architecture

| Crate | Responsibility |
|---|---|
| `dvgw-edi` | EDIFACT parsing — ALOCAT, NOMINT, NOMRES, SCHEDL, IMBNOT, TRANOT, DELORD, DELRES |
| `mako-gabi-gas` | Process engine — all eight workflow state machines, PID routing, deadline handling |

## INVOIC billing workflows

`GaBiGasInvoicWorkflow` handles all three INVOIC PIDs via a single state machine:

| PID   | Process name                                          | Direction   |
|-------|-------------------------------------------------------|-------------|
| 31010 | Kapazitätsrechnung (NB/VNB → BKV/KN)                 | NB → BKV    |
| 31007 | Aggreg. MMM-Rechnung Gas (NB → MGV)                   | NB → MGV    |
| 31008 | MMM-Rechnung Gas selbst ausgestellt (MGV → NB)        | MGV → NB    |

> PIDs 31007/31008 are Gas-only (GaBi Gas, BK7-14-020, NB → MGV).
> PID 31010 is capacity billing between NB/VNB and BKV.
> PID 31011 (AWH Sperrprozesse Gas, NB → LF) belongs to `mako-geli-gas` — it is
> billed by GNB for actions during the Sperrprozess, not by GaBi.

```text
New ──ReceiveInvoic──► InvoicReceived ──[valid]──► ValidationPassed
                                     ╰──[invalid]──► Rejected
ValidationPassed ──SettleInvoice──► Settled
                 ╰─DisputeInvoice──► Disputed
Any active state ──TimeoutExpired──► Rejected
```

After `ValidationPassed`, register a deadline with label
`"gabi-gas-invoic-settlement-deadline"` to enforce the contractual response window.

## Allokationsliste Gas MMMA (`gabi-gas-mmma`)

The MMMA (Marktgebiets-Mehr-/Mindermengenabrechnungs-Allokation) process handles
the allocation list exchange between NB and MGV in the gas balancing framework.

```text
NB ──(ORDERS 17110 Anfrage)──► MGV
                                 │ [accepted]
                                 ├──(MSCONS 13013 Allokationsliste)──► NB
                                 │ [rejected]
                                 └──(ORDRSP 19110 Ablehnung)──► NB
```

| PID   | Message | Process name                              | Direction  |
|-------|---------|-------------------------------------------|------------|
| 17110 | ORDERS  | Anfrage Allokationsliste Gas              | NB → MGV   |
| 19110 | ORDRSP  | Ablehnung Anfrage Allokationsliste Gas    | MGV → NB   |
| 13013 | MSCONS  | Allokationsliste Gas (MMMA)               | MGV → NB   |

> PID 17110 here is Gas (GaBi, BK7-14-020). The same PID also exists in `mako-gpke`
> for the Strom Allokationsliste (different commodity — never cross-register).

## DVGW transport workflows

DVGW message types are parsed by `dvgw-edi` and routed via synthetic PIDs
(90001–90062) through `mako-engine`. Each workflow corresponds to one DVGW
message exchange:

| Workflow | Synthetic PIDs | DVGW message(s) | Description |
|---|---|---|---|
| `gabi-gas-allocation` | 90001–90003 | ALOCAT 5.11a | Gas quantity allocation per exit zone / entry point / measurement point |
| `gabi-gas-nomination` | 90011/90012 (NOMINT) · 90021/90022 (NOMRES) | NOMINT 4.6 FK · NOMRES 4.7 FK | BKV → FNB/MGV nomination + FNB confirmation/rejection |
| `gabi-gas-schedl` | synthetic | SCHEDL G685/G2000 | Transport schedule for a gas day (FNB → BKV) |
| `gabi-gas-imbnot` | synthetic | IMBNOT 5.7a | Intraday imbalance notification (MGV/FNB → BKV) |
| `gabi-gas-tranot` | synthetic | TRANOT 5.8b | Transport notification — capacity restriction or event (FNB/VNB → BKV/GH/MGV) |
| `gabi-gas-delivery-order` | synthetic | DELORD 4.5 FK · DELRES 4.6 FK | Delivery nomination (BKV → FNB) + FNB confirmation/rejection |

Synthetic PID assignment follows `dvgw_edi::AnyDvgwMessage::detect_pid(role_qualifier)`.
PIDs in the 90001–90062 range are unique to this crate and never overlap with
BDEW EDI@Energy PIDs.

## Market roles

| Role | Abbrev. | Description |
|---|---|---|
| Fernleitungsnetzbetreiber | FNB | Gas transmission system operator |
| Verteilnetzbetreiber | VNB | Gas distribution system operator |
| Bilanzkreisverantwortlicher | BKV | Balance responsible party |
| Marktgebietsverantwortlicher | MGV | Market area manager |
| Kapazitätsnutzer | KN | Capacity user — books entry/exit points; counterparty in PID 31010 |

## Regulatory references

| Document | Scope |
|---|---|
| **GasNZV** | Statutory basis for gas network access and balancing |
| **BNetzA BK7-14-020** | GaBi Gas 2.0 — current ruling |
| **DVGW G 685** | Technical standard for gas metering and allocation |

DVGW AHBs and MIGs: <https://www.dvgw-sc.de/leistungen/it-dienstleistungen/datenaustausch-gas>
