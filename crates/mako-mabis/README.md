# mako-mabis

[![Crates.io](https://img.shields.io/crates/v/mako-mabis?label=mako-mabis&color=f59e0b&logo=rust)](https://crates.io/crates/mako-mabis)

MaBiS workflows for the German electricity balance-group settlement:
Summenzeitreihen with their versions, Prüfmitteilungen and Datenstatus, the
Clearing- and Korrekturlisten, the MaBiS-Zählpunkt lifecycle and the list
requests. Governed by **BK6-24-174 Anlage 3**; Strom only. `MabisModule`
registers six workflows; `summenzeitreihe` is the aggregation builder
`mabis-syncd` files with.

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

## Workflows

| Module | Workflow | Contents |
|---|---|---|
| `bilanzkreisabrechnung` | `mabis-billing` | one Summenzeitreihe over one Bilanzierungsmonat: versions, Prüfmitteilungen, Datenstatus |
| `profile` | `mabis-profile` | normierte Profile und Profilscharen + the LF's Reklamation |
| `clearingliste` | `mabis-clearingliste` | the four record-only UTILMD lists |
| `listenabgleich` | `mabis-listenabgleich` | the four lists that owe a Korrekturliste |
| `zp_lifecycle` | `mabis-zp-lifecycle` | MaBiS-ZP Aktivierung/Deaktivierung per series, incl. the NZR-EMob Zuordnung des ZP der NGZ zur NZR (55235–55237, AHB Kap. 13.16) |
| `anforderung` | `mabis-anforderung` | ORDERS list requests + the one Ablehnung |
| `fristen` | — | Kap. 3.10 Tabelle 2, executable |
| `zeitreihen` | — | Kap. 2 Tabelle 1 + the `CAV` codelist |
| `summenzeitreihe` | — | the aggregation builder (`mabis-syncd` uses it) |

| Workflow | PIDs |
|---|---|
| `mabis-billing` | MSCONS 13003, 13020, 13023 · IFTSTA 21000–21005 |
| `mabis-profile` | MSCONS 13010–13012 · ORDERS 17211 |
| `mabis-clearingliste` | UTILMD 55067, 55069, 55070, 55073 |
| `mabis-listenabgleich` | 55065+55066, 55195+55196, 55201+55202, 55223+55224 |
| `mabis-zp-lifecycle` | 55062–55064, 55071/55072, 55197–55200, 55203–55214, 55235–55237 |
| `mabis-anforderung` | ORDERS 17201–17208, 17210 · ORDRSP 19204 |

## What MaBiS does differently

- **No Prüfmitteilung deadline.** Kap. 9.8.2 Nr. 1 carries Frist „–" and the
  receiver *kann* answer; what bounds a Prüfmitteilung is the clearing window of
  Kap. 3.10 Tabelle 2. The real 1-Werktag Fristen are the BIKO's (Kap. 9.8.2
  Nr. 3, Kap. 9.9.2 Nr. 1).
- **A settlement is a stream of versions** (Kap. 3.8.2), each identified by its
  Erstellungszeitpunkt — `SzrVersion` holds the 17 characters verbatim.
- **55062 / 55063 / 55064 are generic codes** shared by eleven Summenzeitreihen;
  `ZpSerie::from_wire` reads the discriminator (`SG10 CCI+++ZB4` / `CAV`) and
  refuses a message that carries none. An Antwortcode is `(DE 9013, DE 1131)`.

## Fristenkalender

`fristen::Bilanzierungsmonat` is Kap. 3.10 Tabelle 2, anchored on the end of the
Bilanzierungsmonat and measured at arrival at the BIKO:

| Zeitreihe | BKA Erstaufschlag | BKA Clearing | KBKA |
|---|---|---|---|
| BG-SZR (Kategorie B) · NZR | 1.–10. WT | 11.–30. WT | 31. WT – Ende 7. Monat |
| BK-SZR (Kategorie A und B) | 1.–12. WT | 13.–30. WT | 31. WT – Ende 7. Monat |
| DZÜ | — | 31.–34. WT | 1.–8. WT des 8. Monats |

| Abrechnungsstichtag | BKA | KBKA |
|---|---|---|
| Vorläufige Bilanzierung | 18. WT, Datenstand 15. WT | 8. WT des 5. Monats, Datenstand Ende 4. Monat |
| Abrechnungsrelevante Bilanzierung | 42. WT, Datenstand 30. WT | Ende 8. Monat, Datenstand Ende 7. Monat |

A version inside the Erstaufschlag window is assigned „Abrechnungsdaten"
automatically; after it, „Prüfdaten" until a positive Prüfmitteilung
(Kap. 3.8.3).

API documentation: <https://docs.rs/mako-mabis>. The PID-to-workflow map for the
whole platform is the [PID reference](https://hupe1980.github.io/mako/docs/regulatory/pid-reference/).
