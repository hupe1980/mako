# mako-geli-gas

[![Crates.io](https://img.shields.io/crates/v/mako-geli-gas?label=mako-geli-gas&color=f59e0b&logo=rust)](https://crates.io/crates/mako-geli-gas)

GeLi Gas workflows for the German gas market — Lieferantenwechsel, Stornierung,
Stammdatenänderung, Messdaten, Datenabruf, PARTIN Gas and the Gas
Sperrprozesse with their INVOIC 31011 — as pure `mako-engine` workflows routed by
Prüfidentifikator. Governed by **GeLi Gas 3.0, BK7-24-01-009** (Beschluss
12.09.2025), which supersedes BK7-19-001 and BK7-06-067. `GeliGasModule`
registers 12 workflows.

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

## Differences from GPKE

| Aspect | GPKE (Strom) | GeLi Gas |
|---|---|---|
| Festlegung | BK6-24-174 (Teil 1–3), BK6-22-024 Anlage 1d (Teil 4) | **BK7-24-01-009** (GeLi Gas 3.0) |
| Antwortfrist shape | wall-clock instant on the 1. WT nach dem ÜT (11:00 / 06:00 / 05:00 / 09:00) | **Ablauf des 4. / 3. / 2. Werktags** nach Eingang |
| Zuordnungszeitpunkt | 00:00 Uhr | **06:00 Uhr** — the Gastag runs 06:00–06:00 |
| Vorlauffrist des LF | — | **10 WT** Anmeldung, **7 WT** Abmeldung, bei Lieferantenwechsel |
| Entscheidungsbäume | `E_06xx` | **`E_30xx`** |
| APERAK | Anerkennungs- *und* Verarbeitbarkeitsfehlermeldung; 45 min für UTILMD/ORDERS | **nur Verarbeitbarkeitsfehlermeldung**; nächster WT 12:00 / 3 WT |
| CONTRL | nur auf eine syntaktisch defekte APERAK | auf **jede** APERAK |
| EDIFACT profile | UTILMD Strom S2.1 / S2.2 | UTILMD Gas G1.1 / G1.2 |
| Grid operator | Netzbetreiber (NB) | Gasnetzbetreiber (GNB) |

Gas APERAKs are Verarbeitbarkeitsfehlermeldungen only (APERAK AHB 1.1 § 2.3):
next Werktag 12:00 for a Folgeprozess, 3 Werktage for an Initialprozess
(`fristen::aperak_gas_*_due_at`). The „10 Werktage" is the LFN's Vorlauffrist,
not the GNB's answer window (4 Werktage). The Gas Sperrprozesse are not part of
GeLi Gas 3.0 but of the AWH „Unterbrechung / Wiederherstellung der
Anschlussnutzung"; they are hosted here.

## Workflows

| Rust module    | Workflow name               | Contents                                                                           |
|----------------|-----------------------------|------------------------------------------------------------------------------------|
| `lieferbeginn` | `geli-gas-supplier-change`  | PIDs 44001–44021 (`UTILMD_PIDS`) Lieferantenwechsel workflow + projections, GNB role |
| `lf_anmeldung` | `geli-gas-lf-anmeldung`     | LF role: 44001/44004/44016 outbound, 44002/44003 · 44005/44006 · 44017/44018 inbound |
| `zuordnungsmeldung` | `geli-gas-zuordnungsmeldung` | PIDs 44036/44037/44038 (Informationsmeldungen um den Lieferbeginn); one-way, no Antwortnachricht |
| `stornierung`  | `geli-gas-stornierung`      | PID 44022 Nb-only (GNB receives Stornierungsanfrage inbound)                       |
| `lf_stornierung` | `geli-gas-stornierung-lf` | PIDs 44023/44024 Lf-only (LF receives GNB Stornierungsantwort inbound)             |
| `datenabruf`   | `geli-gas-datenabruf`       | PIDs 17103/17104 Gas Datenabruf (ORDERS) + ORDRSP 19103/19104                      |
| `sperrung_lf`  | `geli-gas-sperrung-lf`      | PIDs 17115/17117 Gas Sperrung LF-initiated; ORDRSP 19116/19117/19128/19129; ORDCHG 39000 |
| `sperrung_nb`  | `geli-gas-sperrung-nb`      | PIDs 17115/17116/17117 (GNB receives); ORDERS 17116 → gMSB; ORDRSP 19118/19119; ORDCHG 39000/39001 |
| `invoic`       | `geli-gas-sperrprozesse-invoic` | PID 31011 (INVOIC AWH Sperrprozesse Gas), both roles: the GNB issues (`SendInvoic`), the LFG receives (`ReceiveInvoic`). The state machine is `mako-invoic`'s |
| `stammdatenaenderung` | `geli-gas-stammdatenaenderung` | GeLi Gas Stammdatenänderung 44109–44182 — inbound MaLo change → Zustimmung (E15, apply) / Ablehnung (E13/E17); Monatserster rule for bilanzierungsrelevante changes; 10-WT Antwort-Frist |
| `mscons`       | `geli-gas-mscons`           | PIDs 13002/13007/13008/13009 (MSCONS Messdaten Gas, GNB/gMSB → LF)               |
| `partin`       | `geli-gas-partin`           | PIDs 37008–37014 Gas Kommunikationsdaten (LF, GNB, gMSB, MGV, ÜNB)               |

API documentation: <https://docs.rs/mako-geli-gas>. The PID-to-workflow map for
the whole platform is the [PID reference](https://hupe1980.github.io/mako/docs/regulatory/pid-reference/).
