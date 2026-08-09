+++
title = "Parsing"
description = "All entry points for reading EDI@Energy EDIFACT data: parse, parse_interchange, Platform::parse, ParseConfig DoS limits, and error variants."
weight = 10
+++
# Parsing Guide

This guide covers all available entry points for reading EDIFACT data.

---

## Entry Points Overview

| Function | Use case |
|---|---|
| `parse(bytes)` | Single message from an in-memory byte slice |
| `Parser::with_config(config).parse(bytes)` | Single message with custom DoS limits |
| `parse_interchange(reader)` | Lazy iterator over a multi-message interchange |
| `Parser::with_config(config).parse_interchange_buffered(reader)` | Buffered interchange with eager UNB header |
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

## `Parser::with_config` — Custom DoS Limits

The default `ParseConfig` is generous but bounded.
Build a `Parser` with a custom config to override limits for
resource-constrained environments:

The fields are public; construct a config with a struct literal, filling the
rest from `ParseConfig::default()`:

```rust
use edi_energy::{Parser, ParseConfig};

let config = ParseConfig {
    max_input_bytes: Some(1_048_576),   // 1 MB hard cap
    max_segments: Some(2_000),          // 2 000 segments max
    max_segment_bytes: 32_768,          // 32 KB per segment
    ..ParseConfig::default()
};

let msg = Parser::with_config(config).parse(bytes)?;
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
use edi_energy::{Parser, ParseConfig};
use time::Date;

let config = ParseConfig::default()
    .with_reference_date(Date::from_calendar_date(2025, time::Month::January, 1)?);

let msg = Parser::with_config(config).parse(bytes)?;
// validate() will use 2025-01-01 as "today" for release transition checks
```

---

## `parse_interchange` — Multi-message Interchange

A single UNB…UNZ envelope may contain multiple UNH…UNT messages of any type. `parse_interchange` returns a lazy iterator; messages are parsed and dispatched one at a time.

```rust
use std::fs::File;
use std::io::BufReader;
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

### Buffered iterator (`Parser::parse_interchange_buffered`)

When you need the UNB interchange header up front (e.g. to route by sender/recipient
before touching the payload), use the buffered variant on `Parser`. It returns the
`InterchangeHeader` eagerly plus an `InterchangeIter` that yields one
`MessageEnvelope` at a time:

```rust
use std::io::Cursor;
use edi_energy::Parser;

let (header, iter) = Parser::new().parse_interchange_buffered(Cursor::new(bytes))?;
println!("interchange from {}", header.sender_id);

let envelopes: Vec<_> = iter.collect::<Result<_, _>>()?;
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
    AnyMessage::Unknown { message_type_code, .. } => {
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

### Data elements are addressed by code, not by position

Segment accessors name the BDEW data element they read —
`#[edifact(element = "9013")]`, not `element = 2` — and the derive resolves it
against a `SegmentDefinition` **at compile time**. A code that does not exist in
the segment, or that appears at more than one position, is a build error rather
than a silent read of the neighbouring field.

The layouts come from the BDEW MIGs rather than the UN/EDIFACT directory,
because for EDI@Energy the two differ: a MIG may mark a composite *nicht
benutzt* (which keeps its slot but empties it), and it may restrict which
components a composite carries. `edi_energy::messages::layouts` holds them, and
a guard checks every addressed code against the MIG layouts so an edit to one
has to move the other.

### `STS` is polymorphic in its Statuskategorie

`STS` carries its value in one of two composites, and **which one depends on
DE 9015 in element 0**. The BDEW MIGs mark the other *nicht benutzt* in each
case, so reading the wrong one yields `None` for every conformant message.

| Statuskategorie (element 0) | Value in | Element | Example |
|---|---|---|---|
| `7` Transaktionsgrund (UTILMD) | `C556` / DE 9013 | 2 | `STS+7++E01'` |
| `Z33` Plausibilisierungshinweis (MSCONS) | `C556` / DE 9013 | 2 | `STS+Z33++Z84'` |
| `Z18` Bilanzkreiszuordnung (UTILMD) | `C555` / DE 4405 | 1 | `STS+Z18+Z13'` |
| `10` Messklassifizierung (MSCONS) | `C555` / DE 4405 | 1 | `STS+10+Z36'` |

`Sts` exposes both — `reason_code` (DE 9013) and `status_code` (DE 4405) — plus
`Sts::code()`, which returns whichever this instance actually populates. Prefer
`code()` unless the Statuskategorie is known.

Note the empty middle element in `STS+7++E01'`: an unused composite between two
populated ones must be written empty, never omitted, because a MIG may mark an
element unused but cannot renumber the ones after it. The same rule puts the
`CCI` Merkmal at element 2 (`CCI+15++BI1'`), behind an unused `C502`.

---

## Character repertoire

**BDEW MaKo interchanges declare `UNB+UNOC:3`, and `UNOC` is ISO 8859-1 — not
UTF-8.** In `UNOC` the `ü` of "Prüfidentifikator" is the single byte `0xFC`, so
a conformant German interchange handed to a UTF-8 parser is rejected as invalid
text. Party names, addresses and `FTX` free text carry umlauts routinely, which
makes this the common case rather than an edge one.

Every parse entry point reads the repertoire out of the interchange's own `UNB`
S001 DE 0001 and decodes accordingly — nothing to configure:

| Repertoire | Encoding | Handling |
|---|---|---|
| `UNOA`, `UNOB` | ASCII subsets | Borrowed, zero-copy |
| `UNOC` | ISO 8859-1 | Transcoded to UTF-8 |
| `UNOD`–`UNOK` | ISO 8859-2 … -9 | Transcoded to UTF-8 |
| `UNOY` | UTF-8 | Borrowed, zero-copy |
| `UNOX`, `KECA` | stateful / multi-byte | Refused rather than mis-decoded |

An ASCII payload is borrowed rather than copied, so the zero-copy path is
untouched for the interchanges this does not affect. The streaming entry points
buffer only far enough to find the `UNB` before streaming the rest through the
right decoder, so constant memory is preserved.

**Outbound**, `InterchangeBuilder` encodes into the repertoire its `UNB`
declares. A `UNOC` header over a UTF-8 body arrives as mojibake with nothing in
the file to explain why, so a character the repertoire cannot carry is refused
at build time instead.

---

## Security Notes

- **Input bounds**: All parse functions enforce byte-count, segment-count, and per-segment byte limits before any field parsing begins. Maliciously large inputs are rejected immediately.
- **Release-code sanitization**: Untrusted release codes from `UNH` are sanitized before being included in any log output (max 16 ASCII alphanum + `.`).
- **Fuzz tested**: The `fuzz_parse_validate` target has accumulated 1 373+ corpus entries with zero panics or crashes.

---

## See Also

- [Validation Guide](@/docs/reference/validation.md)
- [Platform Guide](@/docs/reference/platform.md) — explicit registries, multi-tenant isolation
- [Getting Started](@/docs/guide/getting-started.md)
