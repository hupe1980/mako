# mako-redispatch

[![Crates.io](https://img.shields.io/crates/v/mako-redispatch?label=mako-redispatch&color=f59e0b&logo=rust)](https://crates.io/crates/mako-redispatch)

Redispatch 2.0 workflows for German grid congestion management under §§ 13,
13a, 14 EnWG, plus the BilAReM Ausfallarbeit arithmetic (`bilarem`,
`ausfallarbeit`). Governed by **BK6-23-241** (Beschluss 07.05.2026), which
consolidates and repeals BK6-20-059/-060/-061. The crate is format-agnostic:
workflows see `RedispatchDocumentKind`; `makod` maps the `redispatch-xml`
document types onto it.

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

## Workflows

| Workflow name | Document type | Direction |
|---|---|---|
| `redispatch-aktivierung` | `ActivationDocument` (ACO/ACR/AAR) | ÜNB → VNB → ANB |
| `redispatch-stammdaten` | `Stammdaten` | ANB → VNB → ÜNB |
| `redispatch-planungsdaten` | `PlannedResourceScheduleDocument` | ÜNB → VNB → ANB |
| `redispatch-verfuegbarkeit` | `UnavailabilityMarketDocument` | ANB → VNB |
| `redispatch-netzengpass` | `NetworkConstraintDocument` | ÜNB ↔ VNB |
| `redispatch-kaskade` | `Kaskade` (§ 13 Abs. 2 EnWG) | ÜNB → VNB → ANB |
| `redispatch-statusanfrage` | `StatusRequest_MarketDocument` | bidirectional |
| `redispatch-kostenblatt` | `Kostenblatt` | VNB → ÜNB |

XML documents route by document type through `RedispatchRouter`; an inbound
`AcknowledgementDocument` is correlated to its workflow by
`ReceivingDocumentIdentification`. The EDIFACT half — five PIDs — routes to
`redispatch-aktivierung`:

| PID | Nachricht | Inhalt | Von → An | EBD |
|----:|-----------|--------|----------|-----|
| 13021 | MSCONS | meteorologische Daten (Ex-post) | BTR → ANB · ANB → anfNB | — |
| 13022 | MSCONS | Einzelzeitreihe Ausfallarbeit | BTR ↔ NB · anfNB → ANB | — |
| 17209 | ORDERS | Anforderung der Ausfallarbeit | anfNB → ANB | — |
| 21037 | IFTSTA | Ansicht NB | NB → BTR | `E_0902` |
| 21038 | IFTSTA | Ansicht BTR | BTR → NB | — |

13020 and 13023 are MaBiS Summenzeitreihen owned by `mako-mabis`. The trees
`E_0901`/`E_0902` live in `mako_pruefung::mabis::ausfallarbeit`.

## Deadlines

`mako_redispatch::fristen` holds the sourced windows:

| Obligation | Value | Source |
|---|---|---|
| `AcknowledgementDocument` | **3 minutes**, unverzüglich | AcknowledgementDocument FB 1.0g |
| Vorab-Information, Prognosemodell | 30 minutes before validity | BilAReM Kap. 6.3.1 |
| Ausfallarbeit final or Dissens established | end of the **3rd** following month, no restart after | BilAReM Kap. 6.4.3 |
| Wetterdaten of the Anlagenbetreiber | 4th Werktag of the following month | BilAReM Kap. 3.2.1 |
| Stammdaten `gueltig_ab` | ≥ 5 or ≥ 10 Werktage ahead, ≤ 2 years | Stammdaten AWT 1.4b Fn. 27/31/32/33 |
| Überführung ins Planwertmodell | ≥ 6 months' notice, only on 01.01./04./07./10. | BilAReM Kap. 2.3.2 |
| Zuordnung einer neuen SR | ≥ 5 WT before IBN if informed ≥ 10 WT ahead, else ≤ 5 WT after | BilAReM Kap. 2.3.2 |

The activation response, the Kostenblatt submission and the Stammdaten forward
have no published Frist under BK6-23-241 (Tenorziffer 7 leaves the
Prozessbeschreibungen to the ÜNB); `fristen::Betreiberfristen` carries them as
operator configuration. Whatever is configured, the 3-minute ACK needs a
scheduler polling well inside it.

API documentation: <https://docs.rs/mako-redispatch>. Wire formats:
[`redispatch-xml`](../redispatch-xml/).
