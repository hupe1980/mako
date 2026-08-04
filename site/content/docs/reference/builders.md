+++
title = "Builders"
description = "Fluent type-state builder API for constructing valid EDI@Energy EDIFACT messages: UTILMD, MSCONS, APERAK, CONTRL, INVOIC, REMADV, ORDERS, ORDRSP."
weight = 12
+++
# Builder Guide

The `edi_energy::builders` module provides a fluent, type-state builder API for constructing valid EDI@Energy EDIFACT messages programmatically.

---

## Why Use Builders?

- **Compile-time mandatory field enforcement** — the type-state pattern prevents calling `.build()` unless all required fields have been set.
- **Correct segment ordering** — the builders emit segments in the order required by the relevant MIG profile.
- **Domain types** — use `ObjectType`, `Pruefidentifikator`, and `Release` instead of raw strings.
- **Round-trip compatible** — the output can be re-parsed and validated by the same library.

---

## Available Builders

| Builder | Message type |
|---|---|
| `UtilmdBuilder` | UTILMD (grid connection processes) |
| `MsconsBuilder` | MSCONS (metered consumption reports) |
| `AperakBuilder` | APERAK (application error acknowledgements) |
| `ContrlBuilder` | CONTRL (interchange control acknowledgements) |
| `InvoicBuilder` | INVOIC (invoices) *(requires `invoic` feature)* |
| `RemadvBuilder` | REMADV (remittance advice) *(requires `remadv` feature)* |
| `OrdersBuilder` | ORDERS (orders, e.g. Sperrung/Konfiguration; **ESA Bestellung/Abbestellung 17007/17008 — full MIG conformance** via `.reference()` (SG1 RFF+Z13), `.item_description()` (IMD), `.location()` (LOC+172), plus the mandatory `UNS`) *(requires `orders` feature)* |
| `OrdrespBuilder` | ORDRSP (order responses; **ESA-Antwort 19011–19014 — full MIG+AHB conformance**; BGM DE 1001 = `7`, `.pruefidentifikator()` sets the PID in BGM DE 1004, `.order_reference()` the SG1 RFF+ACW that echoes the answered order, `.adjustment()` → SG2 AJT, `.adjustment_reason()` → SG2 coded FTX, `.item_description()` → IMD, `.line_item()` → SG27 LIN). ORDRSP carries **no** LOC — the ESA correlates the answer by the RFF+ACW echo *(requires `ordrsp` feature)* |
| `OrdchgBuilder` | ORDCHG (order changes/cancellations; **ESA Stornierung 39002 — full MIG conformance**; `.reference()` emits the mandatory SG1 RFF — ORDCHG carries **no** LOC) *(requires `ordchg` feature)* |
| `IftstaBuilder` | IFTSTA (status reports) *(requires `iftsta` feature)* |
| `InsrptBuilder` | INSRPT (Störungsmeldung / Ablesesteuerung; **AHB-conformant** — `.doc_reference()` → SG3 `DOC`, `.pruefidentifikator()` → SG4 `RFF+Z13`, `.position()` → SG7 `LIN`, `.status()` → `STS`, `.location()` → SG8 `LOC+172`. The AHB marks `BGM`/`DOC`/`DTM`/`LIN`/`LOC`/`NAD`/`RFF`/`STS` mandatory for every PID, so a message missing any of them parses but fails validation) *(requires `insrpt` feature)* |
| `PartinBuilder` | PARTIN (party information) *(requires `partin` feature)* |
| `PricatBuilder` | PRICAT (price catalogues) *(requires `pricat` feature)* |
| `QuotesBuilder` | QUOTES (quotations / **ESA Angebot 15003 — full MIG+AHB conformance**; `.pruefidentifikator()`, `.order_reference()`, `.reference()` (SG1 RFF), `.location()`, `.bindungsfrist()` → `DTM+273`, `.reason()` → `FTX+ACB`, `.currency()` → `CUX`, `.contact()` → `CTA+COM`, `.product()` → `LIN+PIA`, `.price()` → `PRI`) *(requires `quotes` feature)* |
| `ReqoteBuilder` | REQOTE (requests for quotation / **ESA Werteanfrage 35003 — full MIG conformance**; `.reference()` (SG1 RFF+Z13), `.location()` (LOC+172), `.contact()` (CTA+COM), `.free_text()` (FTX), `.characteristic()` (CCI), `.product()` (SG27 `LIN+<n>+<Z67|Z68>` + `PIA+5`), `.line_item()` (LIN)) *(requires `reqote` feature)* |
| `ComdisBuilder` | COMDIS (commercial disputes) *(requires `comdis` feature)* |
| `UtiltsBuilder` | UTILTS (Berechnungsformeln) *(requires `utilts` feature)* |

---

## UTILMD Example

```rust
use edi_energy::{
    builders::UtilmdBuilder,
    EdiEnergyMessage, ObjectType, Pruefidentifikator,
    releases,
};

let bytes = UtilmdBuilder::new(releases::utilmd_fv20261001().clone())
    .pruefidentifikator(Pruefidentifikator::new(55001)?)
    .sender("4012345000023")
    .receiver("9900357000004")
    .message_ref("MSG-001")
    .document_code("E01")
    .document_date("20261001")
    // One SG4 transaction per metering-point / supply-point process
    .transaction(ObjectType::Messlokation, "51238696780")
        .process_date("163", "20261001")      // delivery start
        .reference("Z13", "55001")            // per-transaction PID ref
        .done()
    .build()?
    .serialize()?;

// Validate the output immediately
let msg = edi_energy::parse(&bytes)?;
msg.validate()?.into_error_result()?;
```

### Release constants

Use the constants in `edi_energy::releases` to avoid hard-coding version strings:

```rust
use edi_energy::releases;

let r_utilmd_strom = releases::utilmd_fv20261001();   // S2.2 — Strom
let r_utilmd_gas   = releases::utilmd_fv20261001_gas(); // G1.2 — Gas
let r_mscons       = releases::mscons_fv20261001();   // 2.5
let r_aperak       = releases::aperak_fv20261001();   // 2.2
let r_contrl       = releases::contrl_fv20260101();   // 2.0b
```

---

## MSCONS Example

```rust
use edi_energy::{
    builders::MsconsBuilder,
    releases,
};

let bytes = MsconsBuilder::new(releases::mscons_fv20261001().clone())
    .pruefidentifikator(edi_energy::Pruefidentifikator::new(13001)?)
    .sender("4012345000023")
    .receiver("9900357000004")
    .document_date("20261001")
    .location("DE0001234567890")
        .reading("MWH", "42.5", "163", "20261001")
        .done()
    .build()?
    .serialize()?;
```

---

## APERAK Example

```rust
use edi_energy::builders::AperakBuilder;

let bytes = AperakBuilder::new(releases::aperak_fv20261001().clone())
    .pruefidentifikator(edi_energy::Pruefidentifikator::new(29001)?)
    .sender("4012345000023")
    .receiver("9900357000004")
    .referenced_message_id("MSG-ORIG-001")
    .error_code("Z07")            // application-level rejection
    .build()?
    .serialize()?;
```

---

## ObjectType Domain Enum

Use `ObjectType` wherever the EDIFACT IDE or LOC segment identifies a supply-point object:

```rust
use edi_energy::ObjectType;

let qualifier = ObjectType::Marktlokation.qualifier_code();   // "Z18"
let qualifier = ObjectType::Messlokation.qualifier_code();    // "Z19"
let qualifier = ObjectType::Tranche.qualifier_code();         // "Z30"
let qualifier = ObjectType::Netzlokation.qualifier_code();    // "Z31"
let qualifier = ObjectType::TechnischeRessource.qualifier_code(); // "Z32"
let qualifier = ObjectType::SteuerungRessource.qualifier_code();  // "ZE7"

// Parse from raw qualifier string
let obj = ObjectType::from_qualifier_code("Z18")?;
```

---

## Pruefidentifikator

`Pruefidentifikator` wraps a u32 in the range 10000–99999:

```rust
use edi_energy::Pruefidentifikator;

let pid = Pruefidentifikator::new(55001)?;
println!("{}", pid.as_u32());  // 55001
println!("{}", pid);           // "55001"

// Common Pruefidentifikatoren
// 11001 — UTILMD Strom: Abmeldung Lieferant
// 11002 — UTILMD Strom: Abmeldung Netz
// 11003 — UTILMD Strom: Netzanschluss
// 13001 — MSCONS: Netzbetreiber an Lieferant (SLP)
// 29001 — APERAK: Annahme
// 29002 — APERAK: Ablehnung
// 55001 — UTILMD Strom: Lieferbeginn
```

---

## Type-State Enforcement

Builders use PhantomData type parameters to track which mandatory fields have been set. For example `UtilmdBuilder<NoPid, NoRelease>` cannot call `.build()` — only `UtilmdBuilder<HasPid, HasRelease>` can.

This means missing mandatory fields are a **compile error**, not a runtime panic:

```rust
// compile error: cannot call `.build()` without `.pruefidentifikator(…)`
let result = UtilmdBuilder::new(release)
    .sender("4012345000023")
    .build();  // ← won't compile
```

---

## Serialization

The built message implements `EdifactSerialize`:

```rust
let msg = builder.build()?;

// To bytes
let bytes: Vec<u8> = msg.serialize()?;

// Parse back and validate
let parsed = edi_energy::parse(&bytes)?;
parsed.validate()?.into_error_result()?;
```

### From message to interchange

`serialize()` produces a **message** (`UNH`…`UNT`). That is not the wire unit: a
market partner receives an **interchange**, which wraps one or more messages in
a `UNB` header and `UNZ` trailer. Use `InterchangeBuilder`:

```rust
use edi_energy::builders::InterchangeBuilder;

let wire = InterchangeBuilder::new("9900123456789", "9900987654321", "REF001")
    .transmission("260802", "0915")     // UNB DE0017/DE0019 — YYMMDD / HHMM
    .message(msg.serialize()?)
    .build()?;
```

The `UNZ` message count is derived from the messages actually added, so it
cannot disagree with the payload. The transmission timestamp is a parameter
rather than a clock read, keeping interchange construction deterministic for
golden-file tests.

The UNB DE 0007 party qualifier is derived by `unb_qualifier` per Allgemeine
Festlegungen 6.1d: `14` = GS1 GLN, `500` = DE BDEW (13-digit IDs starting `99`,
and 16-character EIC codes), `502` = DE DVGW (starting `98`).

`makod`'s outbound renderer and the `makotest` Python toolkit both build
interchanges through this type — the envelope is a format concern and has one
implementation.

### Separator safety

Builders never pre-join composites with `:`. Runtime data — free texts,
references, OBIS codes, party IDs — is written through
`Writer::write_composites` (the internal `emit_comp!` macro), where component
boundaries are structural and a literal `:`, `+`, `?`, or `'` inside a value
is escaped on the wire instead of being promoted to a boundary. A guard test
(`builder_writer_guard`) enforces that no `format!`-interpolated value ever
reaches the raw writer path, and a round-trip test proves a separator-hostile
free text survives builder → wire → parser unchanged.

---

## See Also

- [Parsing Guide](@/docs/reference/parsing.md)
- [Validation Guide](@/docs/reference/validation.md)
- [Getting Started](@/docs/guide/getting-started.md)
