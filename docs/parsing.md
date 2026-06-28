---
layout: default
title: Parsing
nav_order: 10
parent: Reference
description: >
  All entry points for reading EDI@Energy EDIFACT data: parse, parse_interchange,
  Platform::parse, ParseConfig DoS limits, and error variants.
---

# Parsing Guide

This guide covers all available entry points for reading EDIFACT data.

---

## Entry Points Overview

| Function | Use case |
|---|---|
| `parse(bytes)` | Single message from an in-memory byte slice |
| `parse_with_config(bytes, config)` | Single message with custom DoS limits |
| `parse_interchange(reader)` | Lazy iterator over a multi-message interchange |
| `Platform::parse(bytes)` | Single message via an explicit platform instance |
| `Platform::parse_interchange(reader)` | Interchange via explicit platform |

---

## `parse` — Single In-memory Message

The simplest entry point. Expects the full EDIFACT message (from `UNH` to `UNT`, or `UNB` to `UNZ`) as a byte slice.

```rust
use edi_energy::{parse, EdiEnergyMessage};

let bytes: Vec<u8> = std::fs::read("message.edi")?;
let msg = parse(&bytes)?;

if let Some(mt) = msg.try_message_type() {
    println!("type: {}", mt.as_str());
}
println!("pid:  {}", msg.detect_pruefidentifikator()?.as_u32());
```

### Error variants

| Error | Meaning |
|---|---|
| `Error::Parse(e)` | EDIFACT syntax error |
| `Error::EmptyInput` | No segments found |
| `Error::MissingRelease` | UNH S009 association code absent |
| `Error::MissingPruefidentifikator` | BGM DE 1004 absent |
| `Error::InvalidPruefidentifikator` | BGM value outside 10000–99999 |
| `Error::InputTooLarge` | Byte count exceeds `ParseConfig::max_input_bytes` |

---

## `parse_with_config` — Custom DoS Limits

The default `ParseConfig` is generous but bounded.
Override limits for resource-constrained environments:

```rust
use edi_energy::{parse_with_config, ParseConfig};

let config = ParseConfig::new()
    .with_max_input_bytes(1_048_576)   // 1 MB hard cap
    .with_max_segments(2_000)          // 2 000 segments max
    .with_max_segment_bytes(32_768);   // 32 KB per segment

let msg = parse_with_config(bytes, config)?;
```

### Default limits

| Limit | Default |
|---|---|
| `max_input_bytes` | 10 MB |
| `max_segments` | 10 000 |
| `max_segment_bytes` (`DEFAULT_MAX_SEGMENT_BYTES`) | 64 KB |
| `max_messages_per_interchange` | 1 000 |

### Validation date override

For reproducible tests or backdate processing:

```rust
use edi_energy::{parse_with_config, ParseConfig};
use time::Date;

let config = ParseConfig::new()
    .with_reference_date(Date::from_calendar_date(2025, time::Month::January, 1)?);

let msg = parse_with_config(bytes, config)?;
// validate() will use 2025-01-01 as "today" for release transition checks
```

---

## `parse_interchange` — Multi-message Interchange

A single UNB…UNZ envelope may contain multiple UNH…UNT messages of any type. `parse_interchange` returns a lazy iterator; messages are parsed and dispatched one at a time.

```rust
use std::io::{BufReader, File};
use edi_energy::{parse_interchange, EdiEnergyMessage};

let file = File::open("bulk.edi")?;
let reader = BufReader::new(file);

for result in parse_interchange(reader) {
    let msg = result?;
    match msg.try_message_type() {
        Some(t) => println!("  {t}: PID {:?}", msg.detect_pruefidentifikator().ok()),
        None    => println!("  (unknown type)"),
    }
}
```

### Buffered iterator (`InterchangeFullBufferedIter`)

If you need to collect all messages before processing (e.g. for transactional commit semantics), use the buffered variant:

```rust
use std::io::Cursor;
use edi_energy::{parse_interchange_buffered};

let reader = Cursor::new(bytes);
let messages: Vec<_> = parse_interchange_buffered(reader)?.collect::<Result<_, _>>()?;
```

---

## `AnyMessage` — Pattern Matching All Types

Every parse function returns `AnyMessage`, an enum over all supported message types.

```rust
use edi_energy::{parse, AnyMessage, EdiEnergyMessage};

let msg = parse(bytes)?;

match &msg {
    AnyMessage::Utilmd(m)  => handle_utilmd(m),
    AnyMessage::Mscons(m)  => handle_mscons(m),
    AnyMessage::Aperak(m)  => handle_aperak(m),
    AnyMessage::Contrl(m)  => handle_contrl(m),
    AnyMessage::Invoic(m)  => handle_invoic(m),   // requires `invoic` feature
    AnyMessage::Unknown { message_type_code, raw_segments } => {
        eprintln!("Unrecognised message type: {message_type_code}");
    }
    _ => {}
}
```

> `AnyMessage` is `#[non_exhaustive]` — always include a wildcard arm for future message types.

---

## Typed Field Access

Each message variant exposes strongly typed accessors derived from the EDIFACT segments:

### UTILMD

```rust
if let AnyMessage::Utilmd(m) = &msg {
    // BGM
    if let Some(bgm) = m.bgm() {
        println!("doc code: {}", bgm.document_code);
    }

    // DTM — all date/time entries
    for dtm in m.dtm() {
        if dtm.is_document_date() {
            println!("document date: {}", dtm.value_str().unwrap_or("-"));
        }
    }

    // Parties (NAD segments)
    if let Some(sender)   = m.sender()   { println!("sender: {}", sender.party_id.as_deref().unwrap_or("-")); }
    if let Some(receiver) = m.receiver() { println!("recv:   {}", receiver.party_id.as_deref().unwrap_or("-")); }

    // Header references (SG1)
    for r in m.references() {
        println!("ref {} = {}", r.rff.qualifier, r.rff.reference.as_deref().unwrap_or("-"));
    }

    // Transactions / metering points (SG4)
    for tx in m.transactions() {
        println!("transaction IDE: {}", tx.ide.object_id.as_deref().unwrap_or("-"));
    }
}
```

### MSCONS

```rust
if let AnyMessage::Mscons(m) = &msg {
    for group in m.meter_reading_groups() {
        println!("loc: {}", group.location.as_deref().unwrap_or("-"));
        for reading in &group.readings {
            println!("  qty: {}", reading.quantity.as_deref().unwrap_or("-"));
        }
    }
}
```

---

## Security Notes

- **Input bounds**: All parse functions enforce byte-count, segment-count, and per-segment byte limits before any field parsing begins. Maliciously large inputs are rejected immediately.
- **Release-code sanitization**: Untrusted release codes from `UNH` are sanitized before being included in any log output (max 16 ASCII alphanum + `.`).
- **Fuzz tested**: The `fuzz_parse_validate` target has accumulated 1 100+ corpus entries with zero panics or crashes.

---

## See Also

- [Validation Guide](./validation.md)
- [Platform Guide](./platform.md) — explicit registries, multi-tenant isolation
- [Getting Started](./getting-started.md)
