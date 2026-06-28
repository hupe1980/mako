---
layout: default
title: Builders
nav_order: 12
parent: Reference
description: >
  Fluent type-state builder API for constructing valid EDI@Energy EDIFACT
  messages: UTILMD, MSCONS, APERAK, CONTRL, INVOIC, REMADV, ORDERS, ORDRSP.
---

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
| `OrdersBuilder` | ORDERS (purchase orders) *(requires `orders` feature)* |

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
    .transaction(ObjectType::Messlokation, "51238696781")
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

---

## See Also

- [Parsing Guide](./parsing.md)
- [Validation Guide](./validation.md)
- [Getting Started](./getting-started.md)
