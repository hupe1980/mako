# edi-energy

**EDIFACT parse · validate · build — stateless German energy market library**

`edi-energy` is the low-level EDIFACT processing layer for the German energy
market (EDI@Energy / BDEW MaKo). It is a **purely stateless library**: no async,
no I/O, no runtime dependencies. All parsing, validation, and message building
happen in-process without allocating threads or network connections.

This crate is the foundation for [`makod`] and the `mako-*` domain crates, but
it is also useful standalone for:
- AS4 gateway pre-processing
- Regulatory compliance checking pipelines
- ERP import/export converters
- Testing harnesses for MaKo messages

## Message Types

| Feature flag | Message type | BDEW abbreviation | Profiles |
|---|---|---|---|
| `utilmd` | Utility Master Data | UTILMD | Strom S2.1/S2.2 · Gas G1.1/G1.2 |
| `mscons` | Metered Services Consumption | MSCONS | 2.4c · 2.5 |
| `aperak` | Application Error and Acknowledgement | APERAK | 2.1i · 2.2 |
| `contrl` | Interchange Control Response | CONTRL | 2.0b |
| `invoic` | Invoice | INVOIC | 2.8e |
| `remadv` | Remittance Advice | REMADV | 2.9f |
| `orders` | Purchase Order | ORDERS | 1.4b · 1.4c |
| `ordrsp` | Purchase Order Response | ORDRSP | 1.4b · 1.4c |
| `ordchg` | Purchase Order Change | ORDCHG | 1.1 |
| `iftsta` | International Multimodal Status Report | IFTSTA | 2.0g · 2.1 |
| `insrpt` | Inspection Report | INSRPT | 1.1a |
| `reqote` | Request for Quotation | REQOTE | 1.3c |
| `quotes` | Quotation | QUOTES | 1.3b · 1.3c |
| `pricat` | Price/Sales Catalogue | PRICAT | 2.0e · 2.1 |
| `comdis` | Commercial Dispute | COMDIS | 1.0g |
| `partin` | Party Information | PARTIN | 1.0f · 1.1 |
| `utilts` | Utility Time Series | UTILTS | 1.1e |

The default feature set enables `utilmd`, `mscons`, `aperak`, and `contrl` —
the four message types every MaKo deployment needs. Enable the other flags
explicitly (or `--all-features`) for the full set of 17.

## Quick Start

Add to `Cargo.toml`:

```bash
cargo add edi-energy
```

### Parse and validate

```rust,no_run
use edi_energy::{parse, EdiEnergyReport};

let bytes = std::fs::read("message.edi")?;
let msg = parse(&bytes)?;

// Detect what arrived
let pid = msg.detect_pruefidentifikator().map(|p| p.as_u32());
let fv  = msg.detect_release().map(|r| r.to_string());
println!("PID {pid:?}  FV {fv:?}");

// Run AHB + MIG rule enforcement
let report: EdiEnergyReport = msg.validate()?;
if !report.is_valid() {
    for err in report.errors() {
        eprintln!("[{}] {}", err.rule_id.as_deref().unwrap_or("-"), err.message);
    }
} else {
    println!("valid ✓");
}
```

### Parse as hard error

```rust,no_run
use edi_energy::parse;

let msg = parse(&bytes)?;
msg.validate()?.into_error_result()?;  // returns Err if any validation error
```

### Interchange stream (multiple messages)

```rust,no_run
use edi_energy::{Platform, parse_interchange};
use std::io::BufReader;

let reader = BufReader::new(std::fs::File::open("bulk.edi")?);
for result in parse_interchange(reader) {
    let msg = result?;
    println!("{}", msg.try_message_type().map_or("?", |t| t.as_str()));
}
```

### Build a UTILMD message

```rust,no_run
use edi_energy::builders::UtilmdBuilder;
use edi_energy::utilmd_codes::{Produktpaket, Transaktionsgrund, dtm, transaktionsgrund};
use edi_energy::{Pruefidentifikator, Release};

let bytes = UtilmdBuilder::new(Release::new("S2.2"))
    .pruefidentifikator(Pruefidentifikator::new(55001)?)
    .sender("4012345000023")
    .receiver("9900357000004")
    .document_date("20261001")
    // `IDE+24` DE 7402 is a **Vorgangsnummer**, never a Lokations-ID.
    .transaction("VORGANG-0001")
    // `SG4 DTM+92` „Datum Vertragsbeginn" — the Anmeldung's process date.
    .date(dtm::BEGINN_ZUM, "20261101")
    .transaktionsgrund(Transaktionsgrund::verbrauchende_malo(transaktionsgrund::WECHSEL))
    // `SG8 SEQ+Z79` Produktpaket — Muss on an Anmeldung: without a Bilanzkreis
    // the NB cannot assign the Marktlokation (UTILMD AHB Strom 2.2 Kap. 5.3).
    .produktpaket(Produktpaket::bilanzkreis("11XBK-STD-----9"))
    // The Lokations-ID lives in `SG5 LOC+Z16`.
    .marktlokation("51238696799")
    .done()
    .serialize()?;

println!("{}", String::from_utf8_lossy(&bytes));
```

See the [builder guide][builders] for the full builder API and all message types.

## The Produktpaket an Anmeldung must carry

UTILMD AHB Strom 2.2 Kap. 5.3 makes `SG8 SEQ+Z79` „Bestandteil eines
Produktpakets" and `SG8 SEQ+ZH0` „Priorisierung erforderliches Produktpaket"
Muss on **55001, 55077, 55600, 55601, 55014 and 55608**, and the Codeliste der
Konfigurationen 1.4 Kap. 6.1.1 makes one product unconditional inside it:

> `9991000002082` **Bilanzkreis** — „Dieses Produkt ist je Produktpaket-ID in
> der UTILMD zwingend anzugeben."

[`Produktpaket::bilanzkreis`] builds that package; the emitted shape is

```text
SEQ+Z79+1
PIA+5+9991000002082:Z11
CCI+Z66
CAV+ZV4:::11XBK-STD-----9
SEQ+ZH0+1
CCI+Z65+++Z01
```

The Bilanzkreis is *not* an `FTX+ACB` remark: on a 55608 the AHB admits that
segment under Bedingung [48] („Wenn … STS+E01++A99 vorhanden"), i.e. on the
Ablehnung only.

**GeLi Gas has no Produktpaket.** UTILMD AHB Gas 1.2 marks `SG10 CCI+Z19` with
the Bilanzkreis in DE 7037 Muss on a 44001 — one segment, no `SEQ`. Build it
with `.merkmal(produkt::CCI_BILANZKREIS_GAS, "…")`; the two shapes carry the
same fact and neither is sendable on the other Sparte.

**Reading it back.** `UtilmdTransaction::sequences` nests each `CAV` under the
`CCI` above it — the only thing that says which Merkmal a value belongs to — and
`UtilmdTransaction::bilanzkreis()` accepts either Sparte's shape. `SG12 NAD`
lands in `UtilmdTransaction::parties` with the full five-component `C080` name.

[`Produktpaket::bilanzkreis`]: https://docs.rs/edi-energy/latest/edi_energy/utilmd_codes/struct.Produktpaket.html

## Interchange party identity (§2.13)

`parse` and `parse_interchange` reject a message whose `NAD` parties disagree
with the interchange envelope:

```
interchange party mismatch: UNB DE0004 is "9900555000005" but NAD+MS is
"9900111000002" (message 0) — BDEW Allgemeine Festlegungen §2.13 requires them
to be identical
```

> "Die im UNB- und NAD-Segment für den Absender / Empfänger verwendeten MP-ID
> sind identisch."
> — Allgemeine Festlegungen V6.1d §2.13

This is an authorisation boundary, not a formatting rule. AS4 authenticates the
**envelope** sender, while consuming services read `NAD+MS` for consent gates,
partner lookup and role resolution. Tolerating a mismatch would let an
authenticated partner attribute a message to a different market participant.

A party absent from either side is not a mismatch — some profiles omit one, and
whether that is legal is an AHB question. `AnyMessage::nad_sender()` /
`nad_receiver()` expose the message-level parties uniformly; CONTRL has none,
being an interchange-level acknowledgement.

## Segment definitions and element positions

A MIG lists only the elements *that profile uses*, but an element's **position**
inside a segment is fixed by the UN/EDIFACT directory — it is what a
counterparty writes on the wire. Deriving positions from the order of the MIG's
list therefore shifts every element that follows an omitted one, and the shift
stays invisible until a real message arrives.

Two tables describe those positions, because parsing and validation reach them
by different routes:

| Source | Authored | Used by |
|---|---|---|
| `src/messages/layouts.rs` | by hand from the MIG PDFs | the `EdifactDeserialize` derive, to resolve `#[edifact(element = "4440")]` when **reading** a typed segment |
| `src/generated/*.rs` | `cargo xtask codegen` from `profiles/` | segment arity **validation** of an inbound message |

Segments are addressed by UN/EDIFACT data element identifier
(`#[edifact(element = "4440")]`), never by ordinal, so a mistyped code is a
compile error rather than a silent read of the wrong slot.
`xtask::codegen::CANONICAL_ELEMENT_POSITIONS` pins the segments whose MIG
listing omits an unused element (`FTX`, `CCI`, `IMD`, `STS`) to their directory
positions; the rest are dense and need no entry.

Three tests hold the sources together, and all three must pass before a layout
change ships:

- `tests/element_positions.rs` — no element appears at two different positions
  across the generated message families, the known layouts are exact, **and the
  hand-authored layouts agree with the generated tables**.
- `tests/segment_layout_guard.rs` — every data element the accessors address
  sits where the BDEW MIG puts it.

When a MIG import turns out to be missing an element the AHB requires, fix the
profile JSON against the MIG PDF and regenerate; do not work around it in a
builder.

## Active Format Versions

| Format version | Strom | Gas | Valid period |
|---|---|---|---|
| `FV2024-10-01` | ✓ S1.2 (LFW24 predecessor) | ✓ G0.x | 2024-10-01 – 2025-09-30 |
| `FV2025-10-01` | ✓ S2.1 — **current production** | ✓ G1.1 | 2025-10-01 – 2026-09-30 |
| `FV2026-10-01` | ✓ S2.2 — next release | ✓ G1.2 | from 2026-10-01 |

Both `FV2025-10-01` and `FV2026-10-01` are simultaneously active in production
deployments during the transition window (±7 days around each annual cutover).
The profile registry resolves the correct rules from the UNH association code
automatically — no per-message format selection is needed.

## Features

| Feature | Default | Description |
|---|---|---|
| `utilmd`, `mscons`, `aperak`, `contrl` | ✓ on | The default message-type set |
| All other message-type flags above | off | Enable the corresponding profile and parser |
| `serde` | off | `serde::{Serialize, Deserialize}` on public types |
| `diagnostics` | off | Rich validation error messages with segment context |
| `tracing` | off | Emit `tracing` events during parse (performance overhead) |
| `archive` | off | Include expired profile versions (`FV2024-10-01` and earlier) |

To enable a minimal build for a single message type:

```bash
cargo add edi-energy --no-default-features --features utilmd,serde
```

## Multi-Tenant and Test Isolation

The module-level `parse()` and `parse_interchange()` functions use a global profile
registry. For test isolation or multi-tenant gateways use `Platform` directly:

```rust,no_run
use edi_energy::Platform;

let platform = Platform::with_all_profiles();
let msg = platform.parse(&bytes)?;
let report = msg.validate()?;
```

Platforms are cheap to clone (profile data is `Arc`-shared). See the [platform
guide][platform] for custom profile subsets, DoS limits, and hot-reload patterns.

## Built-In Examples

Run with `cargo run --example <name> --all-features`:

| Example | What it shows |
|---|---|
| `01_parse_utilmd` | Parse a UTILMD Strom message, inspect segments |
| `02_parse_mscons` | Parse a MSCONS Summenzeitreihe (MABIS path) |
| `03_build_messages` | Build UTILMD + APERAK using the type-state builders |
| `04_interchange_dispatch` | Stream-parse a bulk interchange with PID-based dispatch |
| `05_validate` | Run AHB validation and render error diagnostics |
| `06_parse_reader` | Low-allocation streaming parse from a `BufRead` |

## Documentation

Full documentation: <https://hupe1980.github.io/mako/>

| Topic | Link |
|---|---|
| Getting started (full engine) | [Guide][getting-started] |
| Parsing guide | [Parsing][parsing] |
| Validation guide | [Validation][validation] |
| Builder guide | [Builders][builders] |
| Platform (multi-tenant / test isolation) | [Platform][platform] |
| Profile format versions | [Release lifecycle][release-lifecycle] |
| PID reference | [PID reference][pid-reference] |

## Regulatory Standards

- EDI@Energy **UTILMD** Strom AHB S2.2 / Gas AHB G1.2 — BDEW, FV2026-10-01
- EDI@Energy **MSCONS** AHB 3.2 — BDEW, FV2026-10-01
- EDI@Energy **APERAK** AHB 2.2 — BDEW, FV2026-10-01
- EDI@Energy **CONTRL** AHB 1.0 + ausserordentliche Veröffentlichung 2025-12-11
- All current MIG/AHB releases as of `FV2026-10-01`
- BNetzA rulings BK6-24-174, BK6-22-024, BK7-24-01-009 (process scope)

[`makod`]: ../../services/makod
[getting-started]: https://hupe1980.github.io/mako/docs/guide/getting-started/
[parsing]: https://hupe1980.github.io/mako/docs/reference/parsing/
[validation]: https://hupe1980.github.io/mako/docs/reference/validation/
[builders]: https://hupe1980.github.io/mako/docs/reference/builders/
[platform]: https://hupe1980.github.io/mako/docs/reference/platform/
[release-lifecycle]: https://hupe1980.github.io/mako/docs/compliance/release-lifecycle/
[pid-reference]: https://hupe1980.github.io/mako/docs/regulatory/pid-reference/
