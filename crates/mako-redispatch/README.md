# mako-redispatch

[![Crates.io](https://img.shields.io/crates/v/mako-redispatch?label=mako-redispatch&color=f59e0b&logo=rust)](https://crates.io/crates/mako-redispatch)

The Redispatch 2.0 workflows for German grid congestion management (§§ 13, 13a,
14 EnWG) as a `mako-engine` module, plus the BilAReM Ausfallarbeit arithmetic
(`bilarem`, `ausfallarbeit`). Governed by BNetzA **BK6-23-241** (Beschluss
07.05.2026). The crate is format-agnostic: workflows see
`RedispatchDocumentKind`, and the XML itself is
[`redispatch-xml`](https://crates.io/crates/redispatch-xml)'s.

## Example

```rust
use mako_engine::builder::EngineBuilder;
use mako_engine::event_store::InMemoryEventStore;
use mako_engine::marktrolle::DeploymentRoles;
use mako_redispatch::RedispatchModule;

// EngineBuilder::new() and InMemoryEventStore need mako-engine's `testing` feature.
let ctx = EngineBuilder::new()
    .with_event_store(InMemoryEventStore::new())
    .with_deployment_roles(DeploymentRoles::nb())
    .register(Box::new(RedispatchModule))
    .build();

assert!(ctx.registered_workflows().contains(&"redispatch-aktivierung"));
```

## What it covers

| Workflow | Document type | Direction |
|---|---|---|
| `redispatch-aktivierung` | `ActivationDocument` (ACO/ACR/AAR) | ÜNB → VNB → ANB |
| `redispatch-stammdaten` | `Stammdaten` | ANB → VNB → ÜNB |
| `redispatch-planungsdaten` | `PlannedResourceScheduleDocument` | ÜNB → VNB → ANB |
| `redispatch-verfuegbarkeit` | `UnavailabilityMarketDocument` | ANB → VNB |
| `redispatch-netzengpass` | `NetworkConstraintDocument` | ÜNB ↔ VNB |
| `redispatch-kaskade` | `Kaskade` (§ 13 Abs. 2 EnWG) | ÜNB → VNB → ANB |
| `redispatch-statusanfrage` | `StatusRequest_MarketDocument` | bidirectional |
| `redispatch-kostenblatt` | `Kostenblatt` | VNB → ÜNB |

`EivModule` is the other side of the same documents — the
Einsatzverantwortliche (`eiv`), registered for a deployment that plays it:

| Workflow | Document | Direction |
|---|---|---|
| `redispatch-eiv-planungsdaten` | `PlannedResourceScheduleDocument` `A14` / `Z11` | EIV → DP |
| `redispatch-eiv-nichtbeanspruchbarkeit` | `Unavailability_MarketDocument` `A76` / `A80` | EIV → DP |
| `redispatch-eiv-stammdaten` | `Stammdaten` `Z02` | EIV → DP |
| `redispatch-eiv-kostenblatt` | `Kostenblatt` `Z05` | EIV → DP |
| `redispatch-eiv-abruf` | `ActivationDocument` `A96` in, `A41` out after a person's release | DP → EIV → DP |

Each sending ends `Bestaetigt` on an `A01` acknowledgement and in a Klärfall on
an `A02` or on none (`AcknowledgementDocument` FB 1.0g S. 11). An activation is
never answered without a person's release. Which side an inbound document
reaches is decided by its `ReceiverRole` (`eiv::an_eiv_adressiert`), and the
two modules register disjoint workflows.

XML documents route by document type through `RedispatchRouter`; an
`AcknowledgementDocument` is correlated by `ReceivingDocumentIdentification`.
Five EDIFACT PIDs route to `redispatch-aktivierung`: MSCONS **13021**
(meteorologische Daten) and **13022** (Einzelzeitreihe Ausfallarbeit), ORDERS
**17209** (Anforderung der Ausfallarbeit), IFTSTA **21037**/**21038** (Ansicht
NB / BTR). 13020 and 13023 are MaBiS Summenzeitreihen owned by `mako-mabis`;
the trees `E_0901`/`E_0902` live in `mako_pruefung::mabis::ausfallarbeit`.

`fristen` holds the sourced windows:

| Obligation | Value | Source |
|---|---|---|
| `AcknowledgementDocument` | 3 minutes, unverzüglich | AcknowledgementDocument FB 1.0g |
| Vorab-Information, Prognosemodell | 30 minutes before validity | BilAReM Kap. 6.3.1 |
| Ausfallarbeit final or Dissens | end of the 3rd following month | BilAReM Kap. 6.4.3 |
| Wetterdaten of the Anlagenbetreiber | 4th Werktag of the following month | BilAReM Kap. 3.2.1 |
| Stammdaten `gueltig_ab` | ≥ 5 or ≥ 10 Werktage ahead, ≤ 2 years | Stammdaten AWT 1.4b Fn. 27/31/32/33 |
| Überführung ins Planwertmodell | ≥ 6 months' notice, only on 01.01./04./07./10. | BilAReM Kap. 2.3.2 |
| Zuordnung einer neuen SR | 5 WT before IBN if informed ≥ 10 WT ahead, else 5 WT after the information is complete | BilAReM Kap. 2.3.2 |

The activation response, the Kostenblatt submission and the Stammdaten forward
have no published Frist under BK6-23-241 (Tenorziffer 7 leaves the
Prozessbeschreibungen to the ÜNB); `fristen::Betreiberfristen` carries them as
operator configuration.

## Features

- `testing` — enables `mako-engine/testing` (in-memory stores).
- `tracing` — enables `mako-engine/tracing`.

## Sources

- BNetzA BK6-23-241 (Beschluss 07.05.2026), Anlage BilAReM
- EDI@Energy AcknowledgementDocument Formatbeschreibung 1.0g; Stammdaten AWT 1.4b
- IFTSTA AHB 2.1, PID-Übersicht 4.0

Reference: [Redispatch](https://hupe1980.github.io/mako/docs/reference/redispatch/) ·
API: <https://docs.rs/mako-redispatch>

License: MIT OR Apache-2.0
