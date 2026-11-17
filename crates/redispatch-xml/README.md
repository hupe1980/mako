# redispatch-xml

[![Crates.io](https://img.shields.io/crates/v/redispatch-xml?label=redispatch-xml&color=f59e0b&logo=rust)](https://crates.io/crates/redispatch-xml)

The XML format layer of Redispatch 2.0 (§§ 13, 13a, 14 EnWG): parse, serialize
and validate the nine CIM/IEC 62325-based BDEW document types, structurally and
semantically. It ships no XSD and runs no XSD validation — the BDEW schemas are
not redistributable. The process engine is
[`mako-redispatch`](https://crates.io/crates/mako-redispatch); the EDIFACT
IFTSTA leg is `edi-energy`'s.

## Example

```rust,no_run
use redispatch_xml::{Document, detect, parse_and_validate, serialize};

let xml = std::fs::read("activation.xml")?;

println!("{:?}", detect(&xml)?);               // the DocumentType, without deserialising
let doc: Document = parse_and_validate(&xml)?;   // namespace, structure and semantics
println!("{} from {}", doc.mrid(), doc.sender_id());

let bytes = serialize(&doc)?;                    // back to XML
```

## What it covers

| Document type | XSD version |
|---|---|
| `ActivationDocument` (ACO/ACR/AAR) | 1.1f (AWT 1.1g from 01.04.2027) |
| `AcknowledgementDocument` | 1.0g |
| `Kaskade` | 1.0 |
| `PlannedResourceScheduleDocument` | 1.0f |
| `Stammdaten` | 1.4b |
| `StatusRequest_MarketDocument` | 1.1 |
| `Unavailability_MarketDocument` | 1.1b |
| `NetworkConstraintDocument` | 1.1b |
| `Kostenblatt` | 1.0d |

- `parse`, `parse_as::<T>`, `parse_and_validate`, `detect` — read a document, with or without validation.
- `serialize`, `serialize_as` — write it back.
- `validate` → `ValidationResult`; `into_errors()` gives `Ok(warnings)` or every error.
- `Document::mrid()`, `sender_id()`, `receiver_id()` — the keys a process router correlates on.

The structural checks assert the cardinalities and required elements the XSDs
express; the semantic checks assert what an XSD cannot. `tests/it/xsd_coverage.rs`
asserts that every element the published XSDs declare appears in the model —
`serde` drops unknown elements silently — and names each deliberate omission in
`NOT_MODELLED` with a reason; it skips, visibly, when no local XSD copy is
present.

## Sources

- BDEW XML-Datenformate Redispatch 2.0 — XSDs and Formatbeschreibungen at
  [bdew-mako.de](https://www.bdew-mako.de/market_communication/documents) (topicGroupId 25)
- BNetzA BK6-23-241 (BilAReM)

Reference: [Redispatch](https://hupe1980.github.io/mako/docs/reference/redispatch/) ·
API: <https://docs.rs/redispatch-xml>

License: MIT OR Apache-2.0
