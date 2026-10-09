# mako-gabi-gas

[![Crates.io](https://img.shields.io/crates/v/mako-gabi-gas?label=mako-gabi-gas&color=f59e0b&logo=rust)](https://crates.io/crates/mako-gabi-gas)

GaBi Gas workflows for German gas balancing — allocation, nomination,
Mehr-/Mindermengen and the balancing invoices between Netzbetreiber,
Kapazitätsnutzer and Marktgebietsverantwortlicher — plus the gas-day domain model
(`GasDay`, `GasQuantity`, `AllocationVersion`). Governed by **GaBi Gas 2.1,
BK7-24-01-008**; the DVGW formats are parsed by `dvgw-edi`. `GaBiGasModule`
registers five workflows.

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

## Workflows

| Workflow | PIDs / Message types | Governing document |
|---|---|---|
| `gabi-gas-invoic` | INVOIC 31010 (Kapazitätsrechnung, NB → KN) + 31007/31008 (Aggreg. MMM-Rechnung, NB → MGV) | BK7-24-01-008 |
| `gabi-gas-allocation` | ALOCAT (PIDs 70001–70023) | BK7-24-01-008 / DVGW ALOCAT 5.11a |
| `gabi-gas-nomination` | NOMINT (70030–70034) + NOMRES (70035–70039); both ends | BK7-24-01-008 / DVGW NOMINT 4.6 FK / NOMRES 4.7 FK |
| `gabi-gas-mmma` | MSCONS 13013 (Allokationsliste Gas, MMMA) — a routing-only driver: the MSCONS resumes the `gpke-allokationsliste` process, which also routes ORDERS 17110 / ORDRSP 19110 | BK7-24-01-008 |
| `gabi-gas-mehr-mindermengen` | SSQNOT (70095 SLP / 70096 RLM, NB → MGV); both ends | BK7-24-01-008 / DVGW SSQNOT 5.7 |

INVOIC 31011 (AWH Sperrprozesse Gas) belongs to `mako-geli-gas`. The DVGW formats
`dvgw-edi` does not parse (SCHEDL, IMBNOT, TRANOT, DELORD/DELRES, CHACAP, NUEVOR,
SLPASP, TSIMSG) have no workflow.

## Gas-day deadlines

The gas day runs 06:00–06:00 German time (23 or 25 hours across a DST change).
`GasDay` computes:

| Deadline | Time | Source | Method |
|---|---|---|---|
| Daily ALOCAT | D+1 12:00 | § 46 Ziff. 1/3 KoV XV | `taegliche_alocat_deadline_utc()` |
| Untertägige Meldungen | 15:00 and 18:00 on D | § 46 Ziff. 2 KoV XV | *(not modelled — a second obligation, not this one)* |
| Final allocation, **SLP** | D-1 12:00 | § 47 Ziff. 1 KoV XV | `finale_allokation_deadline_utc(Slp, …)` |
| Final allocation, **RLM** and Entry-/Exitso | end of the 14th Werktag after the Liefermonat | § 47 Ziff. 1 KoV XV | `finale_allokation_deadline_utc(Rlm, …)` |
| NOMINT submission | D-1 13:00 CET | *operating convention* | `nomination_deadline_utc()` |
| NOMRES response window | D-1 15:00 CET | *operating convention* | `nomres_deadline_utc()` |

API documentation: <https://docs.rs/mako-gabi-gas>. The PID-to-workflow map for
the whole platform is the [PID reference](https://hupe1980.github.io/mako/docs/regulatory/pid-reference/).
