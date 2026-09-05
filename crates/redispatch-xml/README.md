# redispatch-xml

XML/XSD format parsing, serialization, and validation for **Redispatch 2.0**,
the German electricity-grid congestion-management protocol (§§ 13, 13a, 14 EnWG,
mandatory since 1 October 2021).

All nine BDEW Redispatch 2.0 document types are fully implemented: parse, serialize,
structural validation, and semantic validation. The crate targets MSRV 1.94 and is
`#![deny(unsafe_code)]`.

---

## Regulatory basis

| Document | Authority | Status |
|---|---|---|
| NABEG 2019, § 13 ff. EnWG | Bundestag | in force since 2021-10-01 |
| **BNetzA BK6-23-241** — Anlage „BilAReM" | BNetzA | Beschluss 07.05.2026; ÜNB settle under it since 01.07.2026 |
| BNetzA BK6-20-059 | BNetzA | TZ 1 repealed with the end of 30.06.2026; TZ 2 survives until the new EDI@Energy documents apply |
| BNetzA BK6-20-060 (Netzbetreiber-Koordination) | BNetzA | **repealed** by BK6-23-241 TZ 4 |
| BNetzA BK6-20-061 (Informationsbereitstellung) | BNetzA | **repealed** by BK6-23-241 TZ 3 |
| BDEW XML-Datenformate Redispatch 2.0 | BDEW | the XSDs this crate models |

All German grid operators (TSO/DSO) must implement Redispatch 2.0. Absence of a
conformant implementation is a regulatory violation under § 14 EnWG.

---

## Document types

All documents are CIM/IEC 62325-based XML, **not** EDIFACT. IFTSTA status
messages (EDIFACT) are handled by the `edi-energy` crate.

| Document type | XSD version | Valid from |
|---|---|---|
| `ActivationDocument` | 1.1f | 2026-04-01 |
| `AcknowledgementDocument` | 1.0g | 2026-04-01 |
| `Kaskade` | 1.0 | 2026-04-01 |
| `PlannedResourceScheduleDocument` | 1.0f | 2025-10-01 |
| `Stammdaten` (master data) | 1.4b | 2025-10-01 |
| `StatusRequest_MarketDocument` | 1.1 | 2025-10-01 |
| `Unavailability_MarketDocument` | 1.1b | 2025-10-01 |
| `NetworkConstraintDocument` | 1.1b | 2025-10-01 |
| `Kostenblatt` | 1.0d | 2025-10-01 |

Versions and Anwendungszeitpunkte are BDEW's own catalogue metadata; a
`Fehlerkorrektur` supersedes its base revision without bumping the version, and
the conformance test below always reads the newest revision on disk.

XSD schemas and application guidelines are published by BDEW at
[bdew-mako.de](https://www.bdew-mako.de/market_communication/documents)
(topicGroupId 25 — XML-Datenformate Redispatch 2.0).

---

## XSD conformance is checked, not assumed

`tests/xsd_coverage.rs` reads the published XSDs out of the local document mirror
and asserts that **every element BDEW declares appears in the model**, scoped per
document. Anything deliberately left out is listed in `NOT_MODELLED` with a
reason; an unexplained entry fails the test.

The check exists because the failure mode is invisible. `serde` ignores unknown
XML elements, so a field simply absent from a struct means an inbound document
carrying it is accepted and the value silently dropped — indistinguishable from
a document that genuinely omitted it. It caught, among others:

| Defect | Consequence |
|---|---|
| `sender_MarketParticipant` modelled as a **nested container** in `Kaskade`, `StatusRequest` and `Unavailability` | The XSD declares the flat dotted `sender_MarketParticipant.mRID`. Every document mako emitted failed XSD validation at the counterparty; every inbound one lost its sender. Same for `receiver_…`, `biddingZone_Domain.mRID`, `quantity_Measure_Unit.name`, `unavailability_Time_Period.timeInterval` |
| `MeasureUnit` where the XSD says `MeasurementUnit` | Kostenblatt, PlannedResourceScheduleDocument. Only `ActivationDocument` uses the shorter spelling, and only in its `ActivationTimeSeries` |
| `Bilanzkreis_Ausgleichsfahrplan_anfNB` and the per-Quote `Bilanzkreis_Ausgleichsfahrplan` absent | The **Redispatch-Bilanzkreis**, which `BilAReM` Kap. 2.3.2 names as one of the three things a Planwertmodell-Zuordnung must carry — where the bilanzielle Ausgleich is booked |
| `ScheduleTimeSeries` absent from `ActivationDocument` | The korrespondierende Fahrpläne. `BilAReM` Kap. 2.1.2: „Der bilanzielle Ausgleich erfolgt durch die Anmeldung korrespondierender Fahrpläne" — the half of the process that moves the energy on paper |
| `Abrechnungsmodell` absent from `Enthaltene_TR` | The Spitz / vereinfachte Spitz / Pauschal election. Without it the Ausfallarbeit of a Maßnahme cannot be computed at all |
| `Available_Period` / `Point` absent from `Unavailability` `TimeSeries` | The availability curve itself. The model reduced an unavailability to a date range, dropping the per-interval capacity that bounds `P_bean` in `BilAReM` Kap. 3.2.2.1 |
| One `TechnischeParameter` type shared across three different XSD complexTypes | The whole TR nameplate (Nettonennleistung, Nabenhöhe, storage capacities …) was dropped |

The XSDs are third-party publications and are **not redistributed with this
crate**, so the test skips — visibly, with a message — when no local copy is
present. They are published at
[bdew-mako.de](https://www.bdew-mako.de/market_communication/documents)
(topicGroupId 25), and the determination behind them is BNetzA BK6-23-241.

---

## Quick start

```rust
use redispatch_xml::{parse, parse_and_validate, serialize, Document};

// Auto-detect document type and parse
let doc = parse(xml_bytes)?;

// Parse + structural/semantic validation in one step
let doc = parse_and_validate(xml_bytes)?;

// Serialize a Document back to XML bytes
let bytes = serialize(&doc)?;

// Serialize a specific type (when document type is known at compile time)
use redispatch_xml::{serialize_as, documents::activation::ActivationDocument};
let bytes = serialize_as(&activation_doc, /* add_xml_decl: */ true)?;

// Parse a specific type directly
use redispatch_xml::parse_as;
let doc: ActivationDocument = parse_as(xml_bytes)?;
```

---

## API overview

| Function | Description |
|---|---|
| `parse(xml)` | Detect type, deserialize, validate namespace |
| `parse_as::<T>(xml)` | Deserialize into a known type `T` |
| `parse_and_validate(xml)` | Parse + structural + semantic validation |
| `detect(xml)` | Return `DocumentType` without deserializing |
| `serialize(doc)` | Serialize `Document` enum to XML bytes |
| `serialize_as(doc, decl)` | Serialize any `Serialize` type to XML bytes |
| `validate(doc)` | Run structural + semantic validation, return `ValidationResult` |
| `Document::mrid(&self)` | Primary document identifier — correlation key for process routing |
| `Document::sender_id(&self)` | Sender identifier, as it appears in the document |
| `Document::receiver_id(&self)` | Receiver identifier, as it appears in the document |
| `ValidationResult::into_errors()` | Consume result — `Ok(warnings)` or `Err(errors)` with the full list |

---

## Type system highlights

- **`DocumentId`** / **`MarketParticipantId`** — validated newtypes with `Display`,
  `AsRef<str>`, `TryFrom<&str>`, `TryFrom<String>`, custom serde.
- **`TimeInterval`** — parses/serializes `"yyyy-mm-ddThh:mmZ/yyyy-mm-ddThh:mmZ"`,
  validates UTC and start-before-end. Implements `Display`.
- **`Decimal3`** — non-negative `f64` serialized as `"NNN.NNN"` (3 dp). Implements `Display`.
- **`AttrV<T>`** — ENTSO-E attr-v wrapper (`<Element v="…"/>`) with `new`,
  `value()`, `From<T>` and `Display`; `AttrVWithScheme<T, S>` adds the
  `codingScheme` attribute.
- All public fallible constructors are annotated `#[must_use]`.
- Enums open for extension: `Direction`, `MeasureUnit`, `MarketRoleType` and
  `ControlZone` are `#[non_exhaustive]`. `CodingScheme` is **not** — its three
  values (`A10` GS1, `NDE` BDEW, `A01` EIC) are the closed set the XSDs admit.

---

## Market roles

| Abbrev. | Role |
|---|---|
| ÜNB | Übertragungsnetzbetreiber (TSO) |
| VNB | Verteilnetzbetreiber (DSO) |
| ANB | Anlagenbetreiber (generation asset operator) |
| DV | Direktvermarkter |
| BKV | Bilanzkreisverantwortlicher |

---

## Related crates

| Crate | Role |
|---|---|
| [`redispatch-xml`](https://docs.rs/redispatch-xml) ← **this crate** | The XML format layer — parse · serialize · validate against the published XSDs |
| [`mako-redispatch`](https://docs.rs/mako-redispatch) | Event-sourced process engine — 8 workflows, `RedispatchRouter`, `RedispatchModule` |
| [`edi-energy`](https://docs.rs/edi-energy) | IFTSTA status messages (EDIFACT, PIDs 21037/21038) |
| [`mako-engine`](https://docs.rs/mako-engine) | Event-sourced workflow runtime — `Workflow`, `Process`, `EventStore`, deadlines |
| [`makod`](https://hupe1980.github.io/mako/docs/services/makod/) | Production daemon — routes both the XML and the EDIFACT leg |

Part of **mako**, an open-source Rust platform for German energy market
communication (Marktkommunikation). Full documentation: <https://hupe1980.github.io/mako/>
