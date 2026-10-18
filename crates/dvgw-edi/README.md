# dvgw-edi

[![Crates.io](https://img.shields.io/crates/v/dvgw-edi?label=dvgw-edi&color=f59e0b&logo=rust)](https://crates.io/crates/dvgw-edi)

DVGW EDIFACT for the German gas transport and balancing market — ALOCAT,
NOMINT, NOMRES and SSQNOT under GaBi Gas 2.1 (BK7-24-01-008): parse, validate
against the Segmentlayout, and write. The DVGW counterpart to `edi-energy`;
CONTRL and APERAK for DVGW interchanges stay there.

**A DVGW message does not name itself in `UNH`.** `UNH` carries the D.07A
carrier (`ORDERS`/`ORDRSP`); `BGM` C002 DE 1001 says which message it is, and
`dvgw_edi::sniff` reads exactly that to tell a DVGW interchange from a BDEW one.
`DTM+137` is when the message was written; the gas day is `DTM+Z01` (format
`719`).

## Examples

```rust,no_run
use dvgw_edi::{DvgwMessageType, DvgwPlatform};

let raw = std::fs::read("alocat.edi")?;
let platform = DvgwPlatform::default();
for result in platform.parse_interchange(&raw) {
    let msg = result?;
    println!("{} ({})", msg.message_type, msg.document.description());

    // The gas day, decoded through the DTM's own format code.
    if let Some(period) = msg.validity_period {
        println!("  Gastag {period}");
    }

    // A LOC group carries a time series — Edig@s SG37 repeats up to 199 times —
    // so a profile transmits many quantities, each with its own period.
    for qty in msg.quantities() {
        println!("  {:?} {:?} {:?}", qty.value, qty.unit, qty.period);
    }

    if msg.message_type == DvgwMessageType::Nomint {
        // RFF+AGO — the nomination this one corrects, for a re-nomination.
        // A NOMRES has no such reference: it is paired on the business key.
        println!("  korrigiert {:?}", msg.original_nomination_ref());
    }

    let report = DvgwPlatform::validate_message(&msg);
    for issue in report.errors() {
        eprintln!("  {issue}");
    }
}
```

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
    .build()?;
```

`build()` refuses a message missing a `Muss` field; the `UNB`/`UNZ` envelope
belongs to the AS4 layer.

## Formats

| Message | Carrier | Document codes (`BGM` DE 1001) | Prüfidentifikatoren |
|---|---|---|---|
| **ALOCAT** — Allokationsnachricht | `ORDRSP` | `X1G X2G X3G X4G X5G X6G X7G XBG` | 70001–70023 |
| **NOMINT** — Nominierung | `ORDERS` | `01G 55G Y1G Y6G Y7G` | 70030–70034 |
| **NOMRES** — Nominierungsantwort | `ORDRSP` | `07G 08G 19G 20G Y2G` | 70035–70039 |
| **SSQNOT** — Mehr-/Mindermengenmeldung | `ORDRSP` | `BAG` | 70095 (SLP), 70096 (RLM) |

## Key types

| Item | Purpose |
|---|---|
| `DvgwPlatform` | `parse_interchange`, `validate_message` → `DvgwIssue`s with a stable rule id |
| `DvgwMessage` | `quantities()`, `energy_by_qualifier()` (a `KW1`/`KW2` QTY is a rate, integrated over its own period), `correlation_key()` / `process_key()` (the published Zuordnungstupel) |
| `MessageBuilder`, `Position` | write a conformant message, including a re-nomination (`original_nomination`) and a SSQNOT |
| `ssqnot::MehrMindermengenmeldung` | a SSQNOT as one record |
| `sniff` | DVGW or BDEW, from the head of the interchange |

Quantities are `Decimal`. Sources: DVGW-Nachrichtenbeschreibungen ALOCAT
5.11a, NOMINT 4.6, NOMRES 4.7, SSQNOT 5.7. API documentation:
<https://docs.rs/dvgw-edi>; reference: [DVGW](https://hupe1980.github.io/mako/docs/reference/dvgw/).
