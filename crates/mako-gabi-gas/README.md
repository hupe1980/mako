# mako-gabi-gas

[![Crates.io](https://img.shields.io/crates/v/mako-gabi-gas?label=mako-gabi-gas&color=f59e0b&logo=rust)](https://crates.io/crates/mako-gabi-gas)

The GaBi Gas processes of German gas balancing — allocation, nomination,
Mehr-/Mindermengen, the Allokationsliste Gas and the balancing invoices between
Netzbetreiber, Kapazitätsnutzer and Marktgebietsverantwortlicher — as pure
`mako-engine` workflows, plus the gas-day domain model (`GasDay`,
`GasQuantity`, `AllocationVersion`). `GaBiGasModule` registers five workflows.
The DVGW formats are parsed by `dvgw-edi` at the transport boundary, not here.

## Example

```rust
use mako_engine::builder::EngineBuilder;
use mako_engine::event_store::InMemoryEventStore;
use mako_engine::marktrolle::DeploymentRoles;
use mako_gabi_gas::GaBiGasModule;

// EngineBuilder::new() and InMemoryEventStore need mako-engine's `testing` feature.
let ctx = EngineBuilder::new()
    .with_event_store(InMemoryEventStore::new())
    .with_deployment_roles(DeploymentRoles::nb())
    .register(Box::new(GaBiGasModule))
    .build();

assert!(ctx.registered_workflows().contains(&mako_gabi_gas::allocation::WORKFLOW_NAME));
```

## What it covers

| Workflow | PIDs | Source |
|---|---|---|
| `gabi-gas-invoic` | INVOIC 31010 Kapazitätsrechnung (NB → KN), 31007/31008 Aggreg. MMM-Rechnung (NB → MGV) | BK7-24-01-008 |
| `gabi-gas-allocation` | ALOCAT 70001–70023, receive-and-record | DVGW ALOCAT 5.11a |
| `gabi-gas-nomination` | NOMINT 70030–70034, NOMRES 70035–70039; both ends | DVGW NOMINT 4.6 FK / NOMRES 4.7 FK |
| `gabi-gas-mmma` | ORDERS 17110 Anforderung Allokationsliste in; a Start Abo refused on ORDRSP 19110 out of `E_0803` / `G_0001`; MSCONS 13013 the list | MMM AWH V2.1 Kap. 6.2–6.4 |
| `gabi-gas-mehr-mindermengen` | SSQNOT 70095 SLP / 70096 RLM (NB → MGV); both ends | DVGW SSQNOT 5.7 |

INVOIC 31011 (Rechnung sonstige Leistung, Sperrprozesse Gas) belongs to
`mako-geli-gas`.

## Gas-day deadlines

The gas day runs 06:00–06:00 German time. `GasDay` computes:

| Deadline | Time | Source | Method |
|---|---|---|---|
| Daily ALOCAT | D+1 12:00 | § 46 Ziffer 1/3 KoV XV | `taegliche_alocat_deadline_utc()` |
| Final allocation, SLP | D-1 12:00 | § 47 Ziffer 1 KoV XV | `finale_allokation_deadline_utc(AllokationsSerie::Slp, …)` |
| Final allocation, RLM, Entryso/Exitso | end of the 14th Werktag after the Liefermonat | § 47 Ziffer 1 KoV XV | `finale_allokation_deadline_utc(AllokationsSerie::Rlm, …)` |
| NOMINT | D-1 13:00 | operating convention, no KoV Frist | `nomination_deadline_utc()` |
| NOMRES | D-1 15:00 | operating convention, no KoV Frist | `nomres_deadline_utc()` |

The untertägige Meldungen of § 46 Ziffer 2 KoV XV are a separate obligation
and are not modelled.

## Sources

- BNetzA BK7-24-01-008 — GaBi Gas 2.1
- Kooperationsvereinbarung Gas XV
- BDEW/VKU/bne/GEODE AWH Mehr-/Mindermengen Strom und Gas V2.1
- DVGW ALOCAT 5.11a, NOMINT 4.6 FK, NOMRES 4.7 FK, SSQNOT 5.7

Reference: [PID reference](https://hupe1980.github.io/mako/docs/regulatory/pid-reference/) ·
[DVGW formats](https://hupe1980.github.io/mako/docs/reference/dvgw/) ·
API: <https://docs.rs/mako-gabi-gas>

License: MIT OR Apache-2.0
