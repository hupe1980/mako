# redispatch-xml

[![Crates.io](https://img.shields.io/crates/v/redispatch-xml?label=redispatch-xml&color=f59e0b&logo=rust)](https://crates.io/crates/redispatch-xml)

The XML format layer of Redispatch 2.0 (§§ 13, 13a, 14 EnWG): parse,
serialize and validate the nine CIM/IEC 62325-based BDEW document types,
structurally and semantically. The process engine is
[`mako-redispatch`](../mako-redispatch/); the EDIFACT IFTSTA leg is
`edi-energy`'s. Governing determination: BNetzA **BK6-23-241** (BilAReM).

## Example

```rust,no_run
use redispatch_xml::{Document, detect, parse_and_validate, serialize};

let xml = std::fs::read("activation.xml")?;

println!("{:?}", detect(&xml)?);               // the DocumentType, without deserialising
let doc: Document = parse_and_validate(&xml)?;   // namespace, structure and semantics
println!("{} from {}", doc.mrid(), doc.sender_id());

let bytes = serialize(&doc)?;                    // back to XML
```

## Document types

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

Versions and Anwendungszeitpunkte are BDEW's catalogue metadata; a
`Fehlerkorrektur` supersedes its base revision without a version bump. The XSDs
are published at [bdew-mako.de](https://www.bdew-mako.de/market_communication/documents)
(topicGroupId 25) and are not redistributed here.

## API

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

`tests/it/xsd_coverage.rs` asserts that every element the published XSDs declare
appears in the model — `serde` ignores unknown elements, so a missing field would
drop inbound data silently — and lists deliberate omissions in `NOT_MODELLED`
with a reason. It skips, visibly, when no local XSD copy is present.

API documentation: <https://docs.rs/redispatch-xml>; reference:
[Redispatch](https://hupe1980.github.io/mako/docs/reference/redispatch/).
