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
| `remadv` | Remittance Advice | REMADV | 2.9e |
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

The default feature set enables all 17 message types. Use explicit feature
selection to reduce binary size for production tooling.

## Quick Start

Add to `Cargo.toml`:

```toml
[dependencies]
edi-energy = "0.6"
```

### Parse and validate

```rust,no_run
use edi_energy::{parse, ValidationSummary};

let bytes = std::fs::read("message.edi")?;
let msg = parse(&bytes)?;

// Detect what arrived
let pid = msg.detect_pruefidentifikator().map(|p| p.value());
let fv  = msg.detect_release().map(|r| r.to_string());
println!("PID {pid:?}  FV {fv:?}");

// Run AHB + MIG rule enforcement
let report: ValidationSummary = msg.validate()?;
if !report.is_valid() {
    for err in report.errors() {
        eprintln!("[{}] {}", err.rule_id(), err.message());
    }
} else {
    println!("valid ✓");
}
```

### Parse as hard error

```rust,no_run
use edi_energy::{parse, into_error_result};

let msg = parse(&bytes)?;
into_error_result(msg.validate()?)?;  // returns Err if any validation error
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
use edi_energy::FormatVersion;

let fv  = FormatVersion::parse("FV2025-10-01")?;
let edi = UtilmdBuilder::new(fv)
    .sender("9900357000004", "500")
    .receiver("9904320000009", "500")
    .pruefidentifikator(55001)
    .marktlokation("51238696782")
    .lieferbeginn("20251001")
    .build()?;

println!("{edi}");
```

See [`docs/builders.md`] for the full builder API and all message types.

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
| All message-type flags above | ✓ on | Enable the corresponding profile and parser |
| `serde` | ✓ on | `serde::{Serialize, Deserialize}` on public types |
| `diagnostics` | ✓ on | Rich validation error messages with segment context |
| `tracing` | off | Emit `tracing` events during parse (performance overhead) |
| `archive` | off | Include expired profile versions (`FV2024-10-01` and earlier) |

To enable a minimal build for a single message type:

```toml
[dependencies]
edi-energy = { version = "0.6", default-features = false, features = ["utilmd", "serde"] }
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

Platforms are cheap to clone (profile data is `Arc`-shared). See [`docs/platform.md`]
for custom profile subsets, DoS limits, and hot-reload patterns.

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

| Topic | Link |
|---|---|
| Getting started (full engine) | [`docs/getting-started.md`] |
| Parsing guide | [`docs/parsing.md`] |
| Validation guide | [`docs/validation.md`] |
| Builder guide | [`docs/builders.md`] |
| Platform (multi-tenant / test isolation) | [`docs/platform.md`] |
| Profile format versions | [`docs/release-lifecycle.md`] |
| PID reference | [`docs/pid-reference.md`] |

## Regulatory Standards

- EDI@Energy **UTILMD** Strom AHB S2.2 / Gas AHB G1.2 — BDEW, FV2026-10-01
- EDI@Energy **MSCONS** AHB 3.2 — BDEW, FV2026-10-01
- EDI@Energy **APERAK** AHB 2.2 — BDEW, FV2026-10-01
- EDI@Energy **CONTRL** AHB 1.0 + ausserordentliche Veröffentlichung 2025-12-11
- All current MIG/AHB releases as of `FV2026-10-01`
- BNetzA rulings BK6-24-174, BK6-22-024, BK7-24-01-009 (process scope)

[`makod`]: ../../services/makod
[`docs/builders.md`]: ../../docs/builders.md
[`docs/getting-started.md`]: ../../docs/getting-started.md
[`docs/parsing.md`]: ../../docs/parsing.md
[`docs/validation.md`]: ../../docs/validation.md
[`docs/platform.md`]: ../../docs/platform.md
[`docs/release-lifecycle.md`]: ../../docs/release-lifecycle.md
[`docs/pid-reference.md`]: ../../docs/pid-reference.md
