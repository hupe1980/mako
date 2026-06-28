# redispatch-xml

**Status: ⏳ Placeholder — implementation in progress.**

XML/XSD format parsing and validation for **Redispatch 2.0**, the German
electricity-grid congestion-management protocol (§§ 13, 13a, 14 EnWG,
mandatory since 1 October 2021).

---

## Regulatory basis

| Document | Authority | Binding since |
|---|---|---|
| NABEG 2019, § 13 ff. EnWG | Bundestag | 2021-10-01 |
| BNetzA BK6-20-059 (Abrechnungsbilanzkreis) | BNetzA | 2021-10-01 |
| BNetzA BK6-20-060 (Netzbetreiber-Koordination) | BNetzA | 2021-10-01 |
| BNetzA BK6-20-061 (Informationsbereitstellung) | BNetzA | 2021-10-01 |
| BDEW XML-Datenformate Redispatch 2.0 | BDEW | Annual update |

All German grid operators (TSO/DSO) must implement Redispatch 2.0. Absence of
a conformant implementation is a regulatory violation under § 14 EnWG.

---

## Document types in scope

All documents are CIM/IEC 62325-based XML, **not** EDIFACT. IFTSTA status
messages (EDIFACT) are handled by the `edi-energy` crate.

| Document type | XSD version | Valid from |
|---|---|---|
| `ActivationDocument` | 1.1d | 2025-10-01 |
| `PlannedResourceScheduleDocument` | 1.0f | 2025-10-01 |
| `AcknowledgementDocument` | 1.0f | 2025-10-01 |
| `Stammdaten` (master data) | 1.4b | 2025-10-01 |
| `StatusRequest_MarketDocument` | 1.1 | 2025-10-01 |
| `Unavailability_MarketDocument` | 1.1b | 2025-10-01 |
| `Beschaffungsanforderung_energetischerAusgleich` | — | 2025-10-01 |
| `Beschaffungsvorbehalt` | — | 2025-10-01 |
| `Kostenblatt` | — | 2025-10-01 |

XSD schemas and application guidelines are published by BDEW at
[bdew-mako.de](https://www.bdew-mako.de/market_communication/documents)
(topicGroupId 25 — XML-Datenformate Redispatch 2.0).

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

## Planned implementation

This crate will expose:

```rust
// Parse and validate an XML document against the BDEW XSD schema
redispatch_xml::parse(xml_bytes, DocumentType::ActivationDocument)?

// Serialize a domain object to XML
redispatch_xml::serialize(&activation_doc)?
```

It will be a dependency of `mako-redispatch` (the process engine), which in
turn is mounted by `makod` when Redispatch 2.0 process handling is enabled.

---

## Related crates

| Crate | Role |
|---|---|
| `redispatch-xml` ← **this crate** | XML format layer |
| `mako-redispatch` | Workflow / process engine |
| `edi-energy` | IFTSTA status messages (EDIFACT) |
| `mako-engine` | Event-sourced runtime |
