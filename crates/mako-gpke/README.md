# mako-gpke

[![Crates.io](https://img.shields.io/crates/v/mako-gpke?label=mako-gpke&color=f59e0b&logo=rust)](https://crates.io/crates/mako-gpke)

The GPKE processes of the German electricity market — supplier switching and
Zuordnung, Stammdaten, Messwerte, Konfiguration, Sperrung and grid-use billing
— as pure `mako-engine` workflows routed by Prüfidentifikator, for the NB, LF
and MSB roles. `GpkeModule` registers 27 workflows. The crate does no I/O and
parses no EDIFACT: the host extracts a message's facts into a command at the
transport boundary and runs it through `Process::run` — `makod` is that host.

## Example

```rust
use mako_engine::builder::EngineBuilder;
use mako_engine::event_store::InMemoryEventStore;
use mako_engine::marktrolle::DeploymentRoles;
use mako_gpke::GpkeModule;

// EngineBuilder::new() and InMemoryEventStore need mako-engine's `testing`
// feature; a production host wires real stores with EngineBuilder::with_stores.
let ctx = EngineBuilder::new()
    .with_event_store(InMemoryEventStore::new())
    .with_deployment_roles(DeploymentRoles::nb())
    .register(Box::new(GpkeModule))
    .build();

assert!(ctx.registered_workflows().contains(&mako_gpke::wechselprozesse::WORKFLOW_NAME));
```

## What it covers

| Process | Workflows (`gpke-…`) | PIDs |
|---|---|---|
| Anmeldung / Abmeldung (NB) | `supplier-change` | UTILMD 55001/55004/55077 → 55002/55003 · 55005/55006 · 55078/55080; IFTSTA Vollzugsmeldungen |
| Lieferant side | `lf-anmeldung` | sends 55001/55004/55016/55077/55600/55601, receives their answers |
| Kündigung (LFN → LFA) | `kuendigung` | 55016 → 55017/55018 |
| Stornierung | `stornierung` | 55022 → 55023/55024 |
| Lieferende von NB an LF | `nb-lieferende`, `lf-abmeldung` | 55007 → 55008/55009 |
| Beendigung der Zuordnung | `beendigung-zuordnung` | 55010 → 55011/55012 |
| Ersatz-/Grundversorgung (§§ 36, 38 EnWG) | `eog` | 55013 → 55014/55015 |
| Zuordnungsmeldungen | `zuordnungsmeldung`, `ankuendigung-zuordnung-lf` | 55036–55038, 55611; 55607 → 55608/55609 |
| Neuanlage | `neuanlage` | 55600/55601 → 55602–55605 |
| Stammdatenänderung (GPKE Teil 4) | `stammdatenaenderung` | 55615–55694, 55109/55110; Rückmeldung A01/A02 |
| Messwerte | `messwerte` | MSCONS 13005/13006/13015–13019/13025/13027 |
| Datenabruf | `datenabruf`, `datenabruf-msb`, `geschaeftsdatenanfrage`, `anfrage-bestellung` | ORDERS 17004/17102/17113, 17132, UTILMD 55555 |
| Allokationsliste | `allokationsliste` | ORDERS 17110/17114, ORDRSP 19110/19115, MSCONS 13014 |
| Konfiguration (GPKE Teil 3) | `konfiguration`, `konfiguration-aenderung` | ORDERS 17134/17135, 17118/17120–17123/17128–17131/17133; ORDRSP 19120/19121/19123/19124/19127/19130–19133 |
| Sperrung | `sperrung`, `sperrung-lf`, `sperrung-msb` | ORDERS 17115–17117, ORDCHG 39000/39001, ORDRSP 19116–19119/19128/19129 |
| Netznutzungsabrechnung | `abrechnung`, `abrechnungsdaten` | INVOIC 31001/31002/31005/31006; 55156/55220/55614/55673/55675 → IFTSTA 21047 |
| Kommunikationsdaten, Netzzustand | `partin`, `utilts` | PARTIN 37000–37006, UTILTS 25001/25004–25010 |

An Anmeldung's `ReceiveUtilmd` carries the `bilanzkreis` it registers into
(UTILMD AHB Strom 2.2 Kap. 5.3), so `processd` can hold it against the
Zuordnungsermächtigungen on file.

Two clocks apply, both from `mako_fristen`. The APERAK is due 45 Minuten after
a UTILMD or ORDERS arrives, Sunday 12 Uhr for a Saturday arrival (APERAK AHB
1.1 Kap. 2.4.1). The business answer is a clock time on the 1. WT nach dem ÜT —
11:00 / 06:00 / 05:00 / 09:00 Uhr for 55001/55077, 55004, 55007 and 55010
(`mako_fristen::antwort`).

## Sources

- BNetzA BK6-24-174 — GPKE Teil 1–3
- BNetzA BK6-22-024 Anlage 1d — GPKE Teil 4
- UTILMD AHB Strom 2.2, ORDERS/ORDRSP AHB 1.1b, APERAK AHB 1.1

Reference: [PID reference](https://hupe1980.github.io/mako/docs/regulatory/pid-reference/) ·
[Processes](https://hupe1980.github.io/mako/docs/reference/processes/) ·
API: <https://docs.rs/mako-gpke>

License: MIT OR Apache-2.0
