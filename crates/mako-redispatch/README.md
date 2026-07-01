# mako-redispatch

Event-sourced process engine for **Redispatch 2.0** congestion-management
workflows under §§ 13, 13a, 14 EnWG. Part of the `mako` workspace.

## Regulatory scope

Redispatch 2.0 is mandatory for all German grid operators (ÜNB and VNB) and
their connected asset operators (ANB) under BNetzA rulings BK6-20-059,
BK6-20-060, and BK6-20-061, effective 2021-10-01. Suppliers (LF) and metering
operators (MSB) are out of scope.

**Market roles in scope:** ANB (Anlagenbetreiber), VNB (Verteilnetzbetreiber),
ÜNB (Übertragungsnetzbetreiber), BKV (Bilanzkreisverantwortlicher).

## Three-crate architecture

| Crate | Responsibility |
|---|---|
| `edi-energy` | IFTSTA status messages (EDIFACT, PIDs 21037/21038) |
| `redispatch-xml` | XML/XSD format parsing (ActivationDocument, Stammdaten, …) |
| `mako-redispatch` | Process engine — workflows, routing, deadlines |

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

## Regulatory deadlines

| Obligation | Deadline | Clock |
|---|---|---|
| `AcknowledgementDocument` | 6 wall-clock hours | **UTC** |
| `StatusRequest` response | 24 wall-clock hours | **UTC** |
| Stammdaten forward (VNB→ÜNB) | 1 Werktag | German local time |
| Activation (ACO) response | **5 minutes** | **UTC** |
| Kostenblatt submission | 15th of following month | German local time |

> **5-minute hard real-time constraint:** The `makod` Redispatch deadline
> scheduler must poll at ≤ 30-second intervals. Configure a dedicated
> `DeadlineScheduler` instance for Redispatch workflows — the standard
> Werktage-based scheduler used for GPKE/WiM is not sufficient.

## IFTSTA PIDs

Redispatch 2.0 IFTSTA messages (confirmed from IFTSTA AHB 2.1 + PID 4.0):

| PID | Perspective | Description |
|-----|-------------|-------------|
| 21037 | NB (VNB) | Kommunikationsprozesse Redispatch — Ansicht NB |
| 21038 | BTR | Kommunikationsprozesse Redispatch — Ansicht BTR |

These PIDs route to the `redispatch-aktivierung` workflow via `PidRouter` and
are registered by `RedispatchModule` in `makod`.

## Routing

Unlike GPKE/WiM/GeLi Gas (EDIFACT `RFF+Z13` Prüfidentifikatoren), Redispatch
2.0 XML documents are routed by `RedispatchRouter` based on XML document type,
not EDIFACT PID. The `makod` inbound dispatcher detects `application/xml`
content and calls `redispatch_xml::detect(bytes)` before routing.

## Regulatory basis

| Document | Topic |
|---|---|
| BK6-20-059 | AcknowledgementDocument (6h), StatusRequest (24h) |
| BK6-20-060 | Stammdaten (1 Werktag), Activation (5 min) |
| BK6-20-061 | Kostenblatt (15th of following month) |

---

## Engine module

`RedispatchModule` implements `EngineModule` and is registered in `makod` when
`DeploymentRoles` contains at least one of `Marktrolle::Nb`, `Marktrolle::Unb`,
or `Marktrolle::Anb`:

```rust,ignore
if roles.contains_any(&[Marktrolle::Nb, Marktrolle::Unb, Marktrolle::Anb]) {
    builder.register(Box::new(RedispatchModule));
}
```

`RedispatchModule::configure()` wires all 8 workflows into a `RedispatchRouter`
and registers IFTSTA PIDs 21037 / 21038 into the `PidRouter`.

### AcknowledgementDocument routing

`AcknowledgementDocument` is **not** registered in the type-based router.
Inbound ACKs carry a `ReceivingDocumentIdentification` field identifying the
workflow instance they belong to. The `makod` dispatcher resolves that
correlation key against the `ProcessRegistry` and delivers the ACK directly to
the originating workflow.

### Deadline scheduler note

The 5-minute Activation (ACO) deadline requires the `DeadlineScheduler` to poll
at ≤ 30-second intervals. Use a dedicated scheduler instance for Redispatch
workflows — the standard Werktage-based GPKE/WiM scheduler is insufficient.

---

## Related crates

| Crate | Role |
|---|---|
| `redispatch-xml` | XML format layer — parse · serialize · validate (required by this crate) |
| `mako-redispatch` ← **this crate** | Event-sourced process engine — 8 workflows, `RedispatchRouter`, `RedispatchModule` |
| `edi-energy` | IFTSTA status messages (EDIFACT, PIDs 21037/21038) |
| `mako-engine` | Event-sourced workflow runtime (`Workflow`, `Process`, `EventStore`) |
