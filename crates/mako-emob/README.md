# mako-emob

[![Crates.io](https://img.shields.io/crates/v/mako-emob?label=mako-emob&color=f59e0b&logo=rust)](https://crates.io/crates/mako-emob)

NZR-EMob / Modell 2 — „bring your own supplier" at a charge point. A
Ladepunktbetreiber runs its own regelzonenweites Bilanzierungsgebiet and books
every charging session, quarter hour by quarter hour, into the Bilanzkreis of
the supplier the customer chose (BK6-20-160 Anlage 6, BK6-24-267). This crate
is the allocation engine and the three Modellwechsel workflows as a
`mako-engine` module — pure domain: no I/O, no wire rendering, no persistence.

## Example

```rust
use mako_engine::builder::EngineBuilder;
use mako_engine::event_store::InMemoryEventStore;
use mako_engine::marktrolle::DeploymentRoles;
use mako_emob::EmobModule;
use mako_emob::modellwechsel::EmobAnmeldungWorkflow;

// EngineBuilder::new() and InMemoryEventStore need mako-engine's `testing` feature.
let ctx = EngineBuilder::new()
    .with_event_store(InMemoryEventStore::new())
    .with_deployment_roles(DeploymentRoles::nb())
    .register(Box::new(EmobModule))
    .build();

assert!(ctx.registered_workflows().contains(&EmobAnmeldungWorkflow::WORKFLOW_NAME));
```

## What it covers

| Workflow | Request → Answer | Tree | Frist | Answered by |
|---|---|---|---|---|
| `emob-anmeldung` | 55238 → 55239 | `E_0513` → `E_0510` | Ablauf des 7. WT | NB (VNB) |
| `emob-zuordnungsende` | 55240 → 55241 | `E_0511` | Ablauf des 3. WT | LF |
| `emob-abmeldung` | 55242 → 55243 | `E_0512` | Ablauf des 3. WT | NB (VNB) |

An expired window ends in `FristAbgelaufen`: Modell 2 gives silence no default
outcome. An Antwortcode is read as `(ebd, code)` — `A01` is an Ablehnung in
`E_0510` and a Zustimmung in `E_0511`/`E_0512`. The trees live in
`mako_pruefung::emob`, the windows in `mako_fristen::antwort::EMOB`. 55235–55237
and MSCONS 13003/13018 are MaBiS and GPKE processes, registered elsewhere.

| Item | Purpose |
|---|---|
| `allocation::QuarterHourAllocation` | `NGZ = Σ Zuordnungen + Deltamenge` exactly per ¼ h, with a `ConservationProof`; over-claims are cut back in proportion |
| `allocation::Richtung` | Bezug and Einspeisung settle as separate series and never net |
| `ids::VirtualMaloId` | the LPB's per-vehicle object; refuses an eleven-digit value so it never collides with a MaLo-ID |
| `ids::TokenRef` | an opaque keyed hash, never a contract identity |
| `session::Provenance`, `SessionSplit` | where a quarter-hour energy came from, and a session split across the quarter hours it spans |

## Sources

- BK6-20-160 (21.12.2020) Anlage 6 „NZR-EMob"; Mitteilung Nr. 4 (03.05.2022)
- BDEW AWH „Zum Modell 2 …" V1.3 (01.04.2025), Kap. 2.1.2 / 2.2.2 for the Fristen
- BDEW AWH Ergänzung der Marktregeln … (MaBiS) V1.0 (27.04.2022)
- BK6-24-267 (15.05.2025)
- UTILMD AHB Strom 2.2 Kap. 11; EBD 4.3 Kap. 17
- MaBiS, BK6-24-174 Anlage 3, Kap. 3.8 / 3.10 / 5

API: <https://docs.rs/mako-emob>

License: MIT OR Apache-2.0
