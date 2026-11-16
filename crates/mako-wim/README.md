# mako-wim

[![Crates.io](https://img.shields.io/crates/v/mako-wim?label=mako-wim&color=f59e0b&logo=rust)](https://crates.io/crates/mako-wim)

The WiM processes — Wechselprozesse im Messwesen — for Strom and Gas as pure
`mako-engine` workflows: MSB-Wechsel, Geräteübernahme, Messlokationsänderung,
Ersteinbau iMS, Preisanfrage and Preisliste, ESA Wertebestellung,
Störungsmeldung (INSRPT) and the MSB-Rechnung. `WimModule` registers 14
workflows and covers both Sparten; there is no separate Gas module. The crate
does no I/O and parses no EDIFACT; the host (`makod`) builds each command at the
transport boundary.

## Example

```rust
use mako_engine::builder::EngineBuilder;
use mako_engine::event_store::InMemoryEventStore;
use mako_engine::marktrolle::DeploymentRoles;
use mako_wim::WimModule;

// EngineBuilder::new() and InMemoryEventStore need mako-engine's `testing` feature.
let ctx = EngineBuilder::new()
    .with_event_store(InMemoryEventStore::new())
    .with_deployment_roles(DeploymentRoles::msb())
    .register(Box::new(WimModule))
    .build();

assert!(ctx.registered_workflows().contains(&mako_wim::geraetewechsel::WORKFLOW_NAME));
```

## What it covers

| Module | Workflow | PIDs |
|---|---|---|
| `geraetewechsel` | `wim-device-change` | UTILMD 55039/55042/55051/55168, Gas 44039/44042/44051/44168 and 44183; IFTSTA 21007/21009–21013/21015/21018/21036 |
| `geraeteubernahme`, `geraeteubernahme_msbn` | `wim-geraeteubernahme`, `wim-geraeteubernahme-msbn` | ORDERS 17001 → ORDRSP 19001/19002, 17009 → 19015/19016; MSBA and MSBN side |
| `weiterverpflichtung` | `wim-weiterverpflichtung` | ORDERS 17002 → ORDRSP 19003/19004 (`E_0203`) |
| `technik_aenderung` | `wim-technik-aenderung` | ORDERS 17011 → ORDRSP 19005/19006, both sides; 10 WT |
| `ersteinbau` | `wim-ersteinbau` | IFTSTA 21029 → 21030/21031 (`E_0233`), Strom |
| `preisanfrage`, `preisliste` | `wim-preisanfrage`, `wim-preisliste` | REQOTE 35001/35002/35004/35005 → QUOTES 15001/15002/15004/15005; PRICAT 27001–27003 |
| `wertebestellung`, `esa_wertebestellung` | `wim-wertebestellung`, `esa-wertebestellung` | REQOTE 35003 → QUOTES 15003, ORDERS 17007/17008, ORDCHG 39002, ORDRSP 19011–19014, IFTSTA 21042; MSB and ESA side |
| `rechnungsabwicklung` | `wim-rechnungsabwicklung` | ORDERS 17005/17006 → ORDRSP 19009/19010; 8 WT |
| `insrpt` | `wim-insrpt` | INSRPT 23001 → 23003/23004, 23005/23009/23011/23012, 23008; both sides |
| `invoic` | `wim-invoic` | INVOIC 31009 (MSB-Rechnung), 31003, 31004 (Stornorechnung); REMADV, COMDIS 29001 |
| `steuerungsauftrag` | `wim-steuerungsauftrag` | Steuerbefehl over the API-Webdienste Strom `controlMeasuresV1` channel |

AWH WiM Gas 2.0 restates WiM Strom Teil 1 use-case for use-case, so one
workflow is parameterised by `Sparte`: only the UTILMD PID namespace, the
Antwort-Codeliste (`S_00xx` against `G_00xx`), the APERAK regime and the
Zuordnungszeitpunkt (06:00 Uhr Gastag against 00:00) differ.

Three clocks apply, all from `mako_fristen`:

| Clock | Window | Source |
|---|---|---|
| APERAK | 45 Minuten for Strom UTILMD/ORDERS | APERAK AHB 1.1 Kap. 2.4.1 |
| Antwortfrist, per PID | 55039 → 3 WT, 55042 → 5 WT, 55051 → 7 WT, 55168 → 1 WT (`antwort_frist_werktage`); the Gas twins alike | WiM Teil 1 Kap. 2.2.2 Nr. 2, 2.3.2 Nr. 2, 2.4.2 Nr. 2, 2.4.2 Nr. 4 |
| Vorlauffrist | anchored on the date the message carries (`mako_fristen::vorlauf`) | — |

The Antwortfrist is never a flat window and never the APERAK clock.

## Sources

- BNetzA BK6-24-174 Anlagen 2a/2b — WiM Strom Teil 1 und 2
- BDEW AWH WiM Gas 2.0
- BDEW AWH Prozesse zur Änderung der Technik an Lokationen V1.1
- APERAK AHB 1.1, ORDERS/ORDRSP AHB 1.1b, REQOTE AHB 1.1

Reference: [PID reference](https://hupe1980.github.io/mako/docs/regulatory/pid-reference/) ·
[Processes](https://hupe1980.github.io/mako/docs/reference/processes/) ·
API: <https://docs.rs/mako-wim>

License: MIT OR Apache-2.0
