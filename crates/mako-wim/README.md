# mako-wim

[![Crates.io](https://img.shields.io/crates/v/mako-wim?label=mako-wim&color=f59e0b&logo=rust)](https://crates.io/crates/mako-wim)

WiM workflows — Wechselprozesse im Messwesen — for **both Sparten**: MSB-Wechsel,
Geräteübernahme, Stammdaten, Preisanfrage, ESA Wertebestellung, Störungsmeldung
(INSRPT) and the MSB-Rechnung. Governed by **BK6-24-174 Anlagen 2a/2b** (WiM
Strom Teil 1 and 2) and the **AWH WiM Gas 2.0**. `WimModule` registers 14
workflows; there is no separate Gas module.

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

## One engine, two Sparten

Only the UTILMD PID namespace splits (55xxx / 44xxx); the Sparte comes from the
recipient MP-ID and decides:

| | Strom | Gas |
|---|---|---|
| Antwort-Codeliste | `S_00xx` | `G_00xx` |
| Zuordnungszeitpunkt | 00:00 Uhr | **06:00 Uhr** (Gastag) |
| APERAK | positiv **und** negativ, 45 min für UTILMD/ORDERS | **nur negativ**; nächster WT 12:00 (Folgeprozess) / 3 WT (Initialprozess) |
| Störungs-Fristen | nach Messtechnik | flach |

Antwortfristen are per PID and the same in both Sparten — 55039·44039 → 3 WT,
55042·44042 → 5 WT, 55051·44051 → 7 WT, 55168·44168 → 1 WT (WiM Teil 1
Kap. 2.2.2 Nr. 2 / 2.3.2 Nr. 2 / 2.4.2 Nr. 2 / 2.4.2 Nr. 4) — never a flat window and never the APERAK
clock. All of them come from `mako_fristen`.

## Workflows

| Rust module        | Contents                                                                  |
|--------------------|---------------------------------------------------------------------------|
| `geraetewechsel`   | MSB-Wechsel: UTILMD 55039/55042/55051/55168, the Gas twins 44039/44042/44051/44168 and 44183, IFTSTA 21007/21009–21013/21015/21018/21036. Inbound and ERP-initiated directions; `antwort_frist_werktage()` per PID |
| `geraeteubernahme` | ORDERS 17001 → ORDRSP 19001/19002 and 17009 → 19015/19016 (Kap. 3.1/3.2); a 17009 date inside the `mako_fristen::vorlauf` Mindestvorlaufzeit is refused with `E17` |
| `weiterverpflichtung` | ORDERS 17002 → ORDRSP 19003/19004 — the NB keeping the abgebender MSB on the Messlokation while the gMSB prepares to take over (Kap. 2.4.2 Nr. 5/6, `E_0203`) |
| `technik_aenderung` | REQOTE 35005 → QUOTES 15005, ORDERS 17011/17118 → ORDRSP 19005/19006, IFTSTA 21025/21027 — Messlokationsänderung, 10 WT Antwort |
| `ersteinbau` | IFTSTA 21029 → 21030/21031 — Ersteinbau eines iMS in eine bestehende Messlokation, **3 WT** Antwort aus `E_0233` (Kap. 3.5, Strom only) |
| `stammdaten`       | PIDs 17102–17133, 17132 — Stammdaten Anforderung / Übermittlung           |
| `wertebestellung`  | MSB side of the ESA Wertebestellung (WiM Teil 2 Kap. 4): REQOTE 35003 → QUOTES 15003, ORDERS 17007/17008, ORDCHG 39002, ORDRSP 19011–19014, IFTSTA 21042; answers carry an `E_0254`/`E_0256`/`E_0257` code |
| `invoic`           | INVOIC 31009 (MSB-Rechnung, Strom), 31003 (Dienstleistungen, beide Sparten), 31004 (Stornorechnung); inbound REMADV 33001–33004 and COMDIS 29001. The state machine is `mako-invoic`'s |
| `preisanfrage`     | PIDs 35001/35002/35004/35005 (REQOTE), 15001/15002/15004/15005 (QUOTES) — Preisanfrage            |
| `preisliste`       | PIDs 27001–27003 — Preisliste PRICAT                                      |
| `esa_wertebestellung` | The ESA side of the same relationship (`esa-wertebestellung`), with disjoint commands so one deployment may hold both roles |
| `insrpt`           | INSRPT 23001 → 23003/23004 (Entscheidungen), 23005/23009/23011/23012 (Informationsmeldungen), 23008 (Ergebnisbericht) — Störungsmeldung, both sides |
| `rechnungsabwicklung` | ORDERS 17005/17006 → ORDRSP 19009/19010 — Rechnungsabwicklung des MSB über den LF, **8 WT** (`E_0205`–`E_0209`) |
| `steuerungsauftrag`| PIDs 11021–11023 — iMS Steuerungsauftrag (API-Webdienste REST channel)    |

API documentation: <https://docs.rs/mako-wim>. The PID-to-workflow map for the
whole platform is the [PID reference](https://hupe1980.github.io/mako/docs/regulatory/pid-reference/).
