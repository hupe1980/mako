# redispatch-xml

XML/XSD format parsing, serialization, and validation for **Redispatch 2.0**,
the German electricity-grid congestion-management protocol (§§ 13, 13a, 14 EnWG,
mandatory since 1 October 2021).

All nine BDEW Redispatch 2.0 document types are fully implemented: parse, serialize,
structural validation, and semantic validation. The crate targets MSRV 1.89 and is
`#![deny(unsafe_code)]`.

---

## Regulatory basis

| Document | Authority | Binding since |
|---|---|---|
| NABEG 2019, § 13 ff. EnWG | Bundestag | 2021-10-01 |
| BNetzA BK6-20-059 (Abrechnungsbilanzkreis) | BNetzA | 2021-10-01 |
| BNetzA BK6-20-060 (Netzbetreiber-Koordination) | BNetzA | 2021-10-01 |
| BNetzA BK6-20-061 (Informationsbereitstellung) | BNetzA | 2021-10-01 |
| BDEW XML-Datenformate Redispatch 2.0 | BDEW | Annual update |

All German grid operators (TSO/DSO) must implement Redispatch 2.0. Absence of a
conformant implementation is a regulatory violation under § 14 EnWG.

---

## Document types

All documents are CIM/IEC 62325-based XML, **not** EDIFACT. IFTSTA status
messages (EDIFACT) are handled by the `edi-energy` crate.

| Document type | XSD version | Valid from | Status |
|---|---|---|---|
| `ActivationDocument` | 1.1f | 2025-10-01 | ✅ Implemented |
| `PlannedResourceScheduleDocument` | 1.0f | 2025-10-01 | ✅ Implemented |
| `AcknowledgementDocument` | 1.0f | 2025-10-01 | ✅ Implemented |
| `Stammdaten` (master data) | 1.4b | 2025-10-01 | ✅ Implemented |
| `StatusRequest_MarketDocument` | 1.1 | 2025-10-01 | ✅ Implemented |
| `Unavailability_MarketDocument` | 1.1b | 2025-10-01 | ✅ Implemented |
| `Kaskade` | 1.0 | 2025-10-01 | ✅ Implemented |
| `NetworkConstraintDocument` | 1.1b | 2025-10-01 | ✅ Implemented |
| `Kostenblatt` | 1.0d | 2025-10-01 | ✅ Implemented |

XSD schemas and application guidelines are published by BDEW at
[bdew-mako.de](https://www.bdew-mako.de/market_communication/documents)
(topicGroupId 25 — XML-Datenformate Redispatch 2.0).

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
| `Document::sender_id(&self)` | Sender GLN / EIC (13 digits) |
| `Document::receiver_id(&self)` | Receiver GLN / EIC (13 digits) |
| `ValidationResult::into_errors()` | Consume result — `Ok(warnings)` or `Err(errors)` with the full list |

---

## Type system highlights

- **`DocumentId`** / **`MarketParticipantId`** — validated newtypes with `Display`,
  `AsRef<str>`, `TryFrom<&str>`, `TryFrom<String>`, custom serde.
- **`TimeInterval`** — parses/serializes `"yyyy-mm-ddThh:mmZ/yyyy-mm-ddThh:mmZ"`,
  validates UTC and start-before-end. Implements `Display`.
- **`Decimal3`** — non-negative `f64` serialized as `"NNN.NNN"` (3 dp). Implements `Display`.
- **`AttrV<T>`** — ENTSO-E attr-v pattern wrapper with `From<T>`, `Display`, `Deref`.
- All public fallible constructors are annotated `#[must_use]`.
- Enums open for extension: `Direction`, `MeasureUnit`, `CodingScheme`, `ControlZone`
  are all `#[non_exhaustive]`.

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
| `redispatch-xml` ← **this crate** | XML format layer (parse / serialize / validate) |
| `mako-redispatch` | Event-sourced process engine — 8 workflows, `RedispatchRouter`, `RedispatchModule` |
| `edi-energy` | IFTSTA status messages (EDIFACT, PIDs 21037/21038) |
| `mako-engine` | Event-sourced workflow runtime (`Workflow`, `Process`, `EventStore`) |
