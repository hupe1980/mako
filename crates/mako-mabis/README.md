# mako-mabis

[![Crates.io](https://img.shields.io/crates/v/mako-mabis?label=mako-mabis&color=f59e0b&logo=rust)](https://crates.io/crates/mako-mabis)

The MaBiS processes of German electricity balance-group settlement —
Summenzeitreihen with their versions, Prüfmitteilungen and Datenstatus, the
Clearing- and Korrekturlisten, the MaBiS-Zählpunkt lifecycle with the
Zuordnungsermächtigung, and the list requests — as pure `mako-engine`
workflows. Strom only. `MabisModule` registers six workflows; `summenzeitreihe`
is the aggregation model `netzbilanzd` files with, and `fristen` is the
Fristenkalender of BK6-24-174 Anlage 3 Kap. 3.10 Tabelle 2.

## Example

```rust
use mako_engine::builder::EngineBuilder;
use mako_engine::event_store::InMemoryEventStore;
use mako_engine::marktrolle::DeploymentRoles;
use mako_mabis::MabisModule;

// EngineBuilder::new() and InMemoryEventStore need mako-engine's `testing` feature.
let ctx = EngineBuilder::new()
    .with_event_store(InMemoryEventStore::new())
    .with_deployment_roles(DeploymentRoles::nb())
    .register(Box::new(MabisModule))
    .build();

assert!(ctx.registered_workflows().contains(&mako_mabis::bilanzkreisabrechnung::WORKFLOW_NAME));
```

## What it covers

| Module | Workflow | PIDs |
|---|---|---|
| `bilanzkreisabrechnung` | `mabis-billing` | MSCONS 13003, 13020, 13023; IFTSTA 21000–21005 |
| `profile` | `mabis-profile` | MSCONS 13010–13012; ORDERS 17211 Reklamation |
| `clearingliste` | `mabis-clearingliste` | UTILMD 55067, 55069, 55070, 55073 — record-only lists |
| `listenabgleich` | `mabis-listenabgleich` | 55065+55066, 55195+55196, 55201+55202, 55223+55224 — list and Korrekturliste |
| `zp_lifecycle` | `mabis-zp-lifecycle` | 55062–55064, 55197–55200, 55203–55214, 55235–55237; Zuordnungsermächtigung 55071/55072 |
| `anforderung` | `mabis-anforderung` | ORDERS 17201–17208, 17210; ORDRSP 19204 |

A NB, ÜNB or BIKO deployment registers the whole family. A Lieferant registers
`LF_PIDS` and a Bilanzkreisverantwortlicher `BKV_PIDS` — the rows of the
Anwendungsübersicht der Prüfidentifikatoren 4.0 with that role on either side —
so a MaBiS message addressed to another role is refused as an unknown PID.

The NB records a BKV's Zuordnungsermächtigung (55071 Aktivierung, 55072
Deaktivierung) with `ZpLifecycleCommand::ReceiveZuordnungsermaechtigung`: it
„erfolgt je ZRT, BG, BK und LF ab einem bestimmten Zeitpunkt" (Kap. 10.2.1,
S. 119) and is the Prüfgrundlage of `E_0622` Prüfschritt 60.

## How MaBiS differs

- **No Prüfmitteilung Frist.** Kap. 9.8.2 Nr. 1 carries Frist „–"; what bounds
  a Prüfmitteilung is the clearing window of Tabelle 2. The two 1-Werktag
  Fristen are the BIKO's (Kap. 9.8.2 Nr. 3, Kap. 9.9.2 Nr. 1).
- **A settlement is a stream of versions**, each identified by its
  Erstellungszeitpunkt (`SzrVersion`). A version arriving inside the
  Erstaufschlag window is assigned „Abrechnungsdaten", after it „Prüfdaten"
  (Kap. 3.8.3); the command carries `im_erstaufschlag`, resolved from the
  receipt instant.
- **55062 / 55063 / 55064 are generic codes** shared by many Summenzeitreihen;
  `ZpSerie::from_wire` resolves the series from the `CAV` code and the
  Verantwortlicher together, and returns `None` when the pair names none.

`fristen::Bilanzierungsmonat` anchors Tabelle 2 on the end of the
Bilanzierungsmonat, measured at arrival at the BIKO:

| Zeitreihe | BKA Erstaufschlag | BKA Clearing | KBKA |
|---|---|---|---|
| BG-SZR (Kategorie B), NZR | 1.–10. WT | 11.–30. WT | 31. WT – Ende 7. Monat |
| BK-SZR (Kategorie A und B) | 1.–12. WT | 13.–30. WT | 31. WT – Ende 7. Monat |
| DZÜ | — | 31.–34. WT | 1.–8. WT des 8. Monats |

`mabis-billing` closes the BKA window on `CLEARING_ENDE_LABEL` and the KBKA
window on `KBKA_CLEARING_ENDE_LABEL`.

## Sources

- BNetzA BK6-24-174 Anlage 3 — MaBiS
- UTILMD AHB Strom 2.2, MSCONS AHB
- Anwendungsübersicht der Prüfidentifikatoren 4.0

Reference: [PID reference](https://hupe1980.github.io/mako/docs/regulatory/pid-reference/) ·
[netzbilanzd](https://hupe1980.github.io/mako/docs/services/netzbilanzd/) ·
API: <https://docs.rs/mako-mabis>

License: MIT OR Apache-2.0
