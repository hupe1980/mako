# dvgw-edi

[![Crates.io](https://img.shields.io/crates/v/dvgw-edi?label=dvgw-edi&color=f59e0b&logo=rust)](https://crates.io/crates/dvgw-edi)

DVGW EDIFACT for the German gas transport and balancing market — ALOCAT,
NOMINT, NOMRES and SSQNOT under GaBi Gas 2.1 (BK7-24-01-008): parse, validate
and write. A DVGW message does not name itself in `UNH`, which carries the D.07A
carrier (`ORDERS`/`ORDRSP`); `BGM` DE 1001 says which message it is, and
`sniff` reads exactly that to tell a DVGW interchange from a BDEW one. CONTRL
and APERAK, and the `UNB`/`UNZ` envelope on the way out, are not here.

## Example

```rust,no_run
use dvgw_edi::{DvgwMessageType, DvgwPlatform};

let raw = std::fs::read("alocat.edi")?;
for result in DvgwPlatform::default().parse_interchange(&raw) {
    let msg = result?;
    println!("{} ({})", msg.message_type, msg.document.description());
    if let Some(gastag) = msg.validity_period {   // DTM+Z01, format 719
        println!("  Gastag {gastag}");
    }
    for qty in msg.quantities() {
        println!("  {:?} {:?} {:?}", qty.value, qty.unit, qty.period);
    }
    if msg.message_type == DvgwMessageType::Nomint {
        println!("  korrigiert {:?}", msg.original_nomination_ref()); // RFF+AGO
    }
    for issue in DvgwPlatform::validate_message(&msg).errors() {
        eprintln!("  {issue}");
    }
}
```

Writing a nomination:

```rust
use dvgw_edi::{DvgwDocument, DvgwPeriod, MessageBuilder, Position};
use time::macros::datetime;

let gas_day = DvgwPeriod {
    start: datetime!(2026-03-01 05:00 UTC),
    end:   datetime!(2026-03-02 05:00 UTC),
};

let wire = MessageBuilder::new(DvgwDocument::NominierungTransportkunde)
    .document_number("NOMINT00052")
    .version("DVGW17")
    .pruefidentifikator(70030)
    .message_datetime(datetime!(2026-02-28 20:56 UTC))
    .validity_period(gas_day)
    .sender("9870009700005")
    .receiver("9870009700006")
    .position(
        Position::new()
            .location("Z19", Some("ABCD1234"))
            .quantity("Z03", "6782", gas_day)
            .party("ZEU", "BK-CODE-1")
            .party("ZES", "BK-CODE-2"),
    )
    .build()?; // refuses a message missing a Muss field
```

## What it covers

| Message | Carrier | `BGM` DE 1001 | Prüfidentifikatoren |
|---|---|---|---|
| ALOCAT — Allokationsnachricht | `ORDRSP` | `X1G`–`X7G`, `XBG` | 70001–70023 |
| NOMINT — Nominierung | `ORDERS` | `01G 55G Y1G Y6G Y7G` | 70030–70034 |
| NOMRES — Nominierungsantwort | `ORDRSP` | `07G 08G 19G 20G Y2G` | 70035–70039 |
| SSQNOT — Mehr-/Mindermengenmeldung | `ORDRSP` | `BAG` | 70095 (SLP), 70096 (RLM) |

- `DvgwPlatform` — `parse_interchange`, `validate_message` → `DvgwIssue`s with a stable rule id.
- `DvgwMessage` — `quantities()`, `energy_by_qualifier()` (a `KW1`/`KW2` rate integrated over its period), `correlation_key()` / `process_key()`.
- `MessageBuilder`, `Position` — a conformant message, including a re-nomination (`original_nomination`).
- `ssqnot::MehrMindermengenmeldung` — a SSQNOT as one record.

Quantities are `Decimal`.

## Features

- `serde` — `Serialize` / `Deserialize` on the public value types.
- `tracing` — structured tracing spans.

## Sources

- DVGW-Nachrichtenbeschreibungen ALOCAT 5.11a, NOMINT 4.6, NOMRES 4.7, SSQNOT 5.7
- BNetzA BK7-24-01-008 (GaBi Gas 2.1)

Reference: [DVGW](https://hupe1980.github.io/mako/docs/reference/dvgw/) ·
API: <https://docs.rs/dvgw-edi>

License: MIT OR Apache-2.0
