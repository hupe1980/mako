# mako-gpke

[![Crates.io](https://img.shields.io/crates/v/mako-gpke?label=mako-gpke&color=f59e0b&logo=rust)](https://crates.io/crates/mako-gpke)

GPKE workflows for the German electricity market — supplier switching and
Zuordnung, Stammdaten, Sperrung, Messwerte, Konfiguration and grid-use billing —
as pure `mako-engine` workflows routed by Prüfidentifikator. Governed by
**BK6-24-174** (GPKE Teil 1–3, gültig seit 06.06.2025) and **BK6-22-024**
(GPKE Teil 4). `GpkeModule` registers 23 workflows.

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

A host then spawns or resumes a process per inbound message and runs the
command through `Process::run`; `makod` is that host.

## Workflows

| Rust module                 | Workflow name                    | Contents                                                            |
|-----------------------------|----------------------------------|---------------------------------------------------------------------|
| `wechselprozesse`           | `gpke-supplier-change`           | PIDs 55001/55004/55077 inbound (`UTILMD_ANFRAGE_PIDS`, NB role), answered 55002/55003 · 55005/55006 · 55078/55080 + IFTSTA 21024–21028/21033/21035/21045/21047 |
| `stornierung`               | `gpke-stornierung`               | PIDs 55022–55024 (UTILMD Stornierung Zuordnungsprozess) + 55023/55024 derived |
| `kuendigung`                | `gpke-kuendigung`                | PIDs 55016 → 55017/55018 (Kündigung, LFN → LFA — no NB in between), answered out of `E_0614` |
| `zuordnungsmeldung`         | `gpke-zuordnungsmeldung`         | PIDs 55036/55037/55038 (Meldepflichten um den Lieferbeginn) + 55611 (Lieferende von NB an MSB/MSBZ); one-way, no Antwortnachricht |
| `beendigung_zuordnung`      | `gpke-beendigung-zuordnung`      | PID 55010 (NB Abmeldeanfrage) + 55011/55012 derived responses |
| `eog`                       | `gpke-eog`                       | PID 55013 (Anmeldung/Zuordnung EOG §36/§38 EnWG) + 55014/55015 derived |
| `lf_anmeldung`              | `gpke-lf-anmeldung`              | PIDs 55001/55004/55016/55077 (LF outbound) + 55002-55003/55005-55006/55017-55018/55078/55080 (LF-role receive NB ANTWORT) |
| `lf_abmeldung`              | `gpke-lf-abmeldung`              | PID 55007 (Lieferende von NB an LF) + 55008/55009 derived     |
| `stammdatenaenderung`       | `gpke-stammdatenaenderung`       | GPKE Teil 4 Stammdatenänderung 55615–55694, 55109/55110 — inbound MaLo change → apply to marktd + Rückmeldung A01/A02 (quality feedback, tacit acceptance after 2 WT) |
| `neuanlage`                 | `gpke-neuanlage`                 | PIDs 55600/55601 (Neuanlage MaLo, LF → NB) + 55602–55605 derived   |
| `messwerte`                 | `gpke-messwerte`                 | MSCONS PIDs 13005/13006/13015–13019/13025/13027 (Messwerte NB/MSB → LF) |
| `datenabruf`                | `gpke-datenabruf`                | ORDERS 17004/17102/17113 (Anfrage) + ORDRSP 19101/19102/19114 (Ablehnung) |
| `allokationsliste`          | `gpke-allokationsliste`          | ORDERS 17110/17114 + ORDRSP 19110/19115 + MSCONS 13014 (Allokationsliste Strom) |
| `anfrage_bestellung`        | `gpke-anfrage-bestellung`        | PID 55555 (Anfrage Daten der individuellen Bestellung, GPKE Teil 4)  |
| `abrechnung`                | `gpke-abrechnung`                | PIDs 31001/31002/31005/31006 (INVOIC Netznutzungsabrechnung)        |
| `abrechnungsdaten`          | `gpke-abrechnungsdaten`          | PIDs 55156/55220/55673 (Rückmeldung/Bestellung Abrechnungsdaten, LF → NB) → IFTSTA 21047 Bearbeitungsstand, `E_0595` |
| `konfiguration`             | `gpke-konfiguration`             | PIDs 17134/17135 (ORDERS outbound) + 19001/19002 (ORDRSP inbound) — GPKE Teil 4 |
| `konfiguration_aenderung`   | `gpke-konfiguration-aenderung`   | ORDERS/ORDRSP for configuration changes (NB role)                   |
| `sperrung`                  | `gpke-sperrung`                  | NB role: ORDERS 17115–17117 + ORDCHG 39000/39001 + ORDRSP 19118/19119 (the MSB's answer to a 17116) |
| `sperrung_lf`               | `gpke-sperrung-lf`               | LF-side Sperrung: ORDERS 17115/17117 + ORDCHG 39000 outbound, ORDRSP 19116/19117 · 19128/19129 + IFTSTA 21039 inbound |
| `ankuendigung_zuordnung_lf` | `gpke-ankuendigung-zuordnung-lf` | PIDs 55607–55609 (UTILMD Ankündigung Zuordnung LF)                 |
| `partin`                    | `gpke-partin`                    | PIDs 37000–37006 (PARTIN Strom Kommunikationsdaten)                |
| `utilts`                    | `gpke-utilts`                    | UTILTS PIDs 25001/25004–25010 (Netzzustandsdaten NB → LF)          |

## Fristen

Two clocks apply, both from `mako_fristen`. The APERAK is due 45 Minuten after
a UTILMD or ORDERS arrives, Sunday 12:00 for a Saturday arrival (APERAK AHB 1.1
§ 2.4.1). The business answer is a wall-clock instant per PID —
11:00 / 06:00 / 05:00 / 09:00 Uhr des 1. WT nach dem ÜT for the GPKE Teil 2
core processes (`mako_fristen::antwort`). There is no 24-hour message window.

API documentation: <https://docs.rs/mako-gpke>. The PID-to-workflow map for the
whole platform is the [PID reference](https://hupe1980.github.io/mako/docs/regulatory/pid-reference/).
