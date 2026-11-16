# mako-geli-gas

[![Crates.io](https://img.shields.io/crates/v/mako-geli-gas?label=mako-geli-gas&color=f59e0b&logo=rust)](https://crates.io/crates/mako-geli-gas)

The GeLi Gas processes of the German gas market — Lieferantenwechsel,
Stornierung, Stammdatenänderung, Geschäftsdatenanfrage, Messdaten, PARTIN Gas
and the Gas Sperrprozesse with their INVOIC 31011 — as pure `mako-engine`
workflows routed by Prüfidentifikator, for the GNB, LF and gMSB roles.
`GeliGasModule` registers 14 workflows. The crate does no I/O and parses no
EDIFACT; the host (`makod`) builds each command at the transport boundary.

## Example

```rust
use mako_engine::builder::EngineBuilder;
use mako_engine::event_store::InMemoryEventStore;
use mako_engine::marktrolle::DeploymentRoles;
use mako_geli_gas::GeliGasModule;

// EngineBuilder::new() and InMemoryEventStore need mako-engine's `testing` feature.
let ctx = EngineBuilder::new()
    .with_event_store(InMemoryEventStore::new())
    .with_deployment_roles(DeploymentRoles::nb())
    .register(Box::new(GeliGasModule))
    .build();

assert!(ctx.registered_workflows().contains(&mako_geli_gas::lieferbeginn::WORKFLOW_NAME));
```

## What it covers

| Process | Workflow (`geli-gas-…`) | PIDs |
|---|---|---|
| Lieferantenwechsel, both sides of every Anfrage | `supplier-change` | UTILMD 44001–44021, incl. the GNB's Abmeldungsanfrage 44010 → 44011/44012 |
| Lieferant side | `lf-anmeldung` | sends 44001/44004/44016, receives 44002/44003 · 44005/44006 · 44017/44018 |
| Informationsmeldungen um den Lieferbeginn | `zuordnungsmeldung` | 44036–44038, one-way |
| Stornierung | `stornierung` (GNB), `stornierung-lf` (LF) | 44022 → 44023/44024 |
| Stammdatenänderung | `stammdatenaenderung` | 44109–44182; Zustimmung/Ablehnung from the Antwort column's Codeliste (`ANTWORT_CODELISTEN`) |
| Geschäftsdatenanfrage | `geschaeftsdatenanfrage` | ORDERS 17101/17126 → UTILMD 44035/44060 or ORDRSP 19101 |
| Datenabruf | `datenabruf` | ORDERS 17103/17104, ORDRSP 19103/19104 |
| Messdaten | `mscons` | MSCONS 13002/13007/13008/13009 |
| Sperrung / Entsperrung | `sperrung-nb`, `sperrung-lf`, `sperrung-msb` | ORDERS 17115–17117, ORDCHG 39000/39001, ORDRSP 19116–19119/19128/19129 |
| Rechnung sonstige Leistung | `sperrprozesse-invoic` | INVOIC 31011, GNB → LF |
| Kommunikationsdaten | `partin` | PARTIN 37008–37014 |

Three clocks apply, and none of them is ten Werktage:

| Clock | Window | Source |
|---|---|---|
| CONTRL | 6 Stunden | CONTRL AHB 1.0 Kap. 1.2 |
| APERAK | nächster Werktag 12 Uhr (Folgeprozess), 3 Werktage (Initialprozess) — Verarbeitbarkeitsfehlermeldung only | APERAK AHB 1.1 Kap. 2.3.1 |
| Business answer | 4 / 3 / 2 Werktage per Prüfidentifikator | GeLi Gas 3.0 Kap. 3.1–3.3 |

The ten Werktage are the supplier's Vorlauffrist („mindestens 10 Werktage vor
Aufnahme der Belieferung", GeLi Gas 3.0 Kap. 3.2.3), not the GNB's answer
window. GeLi Gas 3.0 holds no Sperrprozess: those follow the BDEW AWH
„Unterbrechung / Wiederherstellung der Anschlussnutzung" and are hosted here.

## Sources

- BNetzA BK7-24-01-009 — GeLi Gas 3.0 (Beschluss 12.09.2025, Tenor ab 01.01.2026)
- BDEW/VKU/GEODE/FNB Gas AWH GeLi Gas V1.2 (gültig ab 01.04.2026)
- UTILMD AHB Gas 1.1 / 1.2, APERAK AHB 1.1, CONTRL AHB 1.0

Reference: [PID reference](https://hupe1980.github.io/mako/docs/regulatory/pid-reference/) ·
[Processes](https://hupe1980.github.io/mako/docs/reference/processes/) ·
API: <https://docs.rs/mako-geli-gas>

License: MIT OR Apache-2.0
