# Getting Started with edi-energy-rs

> **⚠️ Experimental** — Pre-1.0. APIs may change between releases.

This guide walks you from zero to a working EDI@Energy EDIFACT pipeline in Rust.

---

## Prerequisites

- Rust **1.85** or newer (`rustup update stable`)
- Basic familiarity with EDIFACT or the BDEW MaKo specification

---

## Installation

Add the dependency to your `Cargo.toml`:

```toml
[dependencies]
edi-energy = "0.1"
```

By default this enables the four most common message types: **UTILMD**, **MSCONS**, **APERAK**, **CONTRL**.

To enable additional types, list them under `features`:

```toml
[dependencies]
edi-energy = { version = "0.1", features = ["invoic", "remadv", "orders"] }
```

To enable everything:

```toml
[dependencies]
edi-energy = { version = "0.1", features = ["invoic", "remadv", "orders", "iftsta", "insrpt", "reqote", "partin", "ordchg", "ordrsp", "quotes", "comdis", "pricat", "utilts"] }
```

Or in Cargo.toml shorthand: `--all-features` at the CLI level.

---

## Feature Flags

| Flag | Default | Description |
|---|---|---|
| `utilmd` | ✅ | UTILMD Strom and Gas — grid connection processes |
| `mscons` | ✅ | MSCONS — metered services consumption reports |
| `aperak` | ✅ | APERAK — application error acknowledgements |
| `contrl` | ✅ | CONTRL — interchange syntax acknowledgements |
| `invoic` | | Invoices |
| `remadv` | | Remittance advice |
| `orders` | | Purchase orders |
| `iftsta` | | Multimodal status reports |
| `insrpt` | | Inspection reports |
| `reqote` | | Requests for quotation |
| `partin` | | Party information |
| `ordchg` | | Purchase order changes |
| `ordrsp` | | Purchase order responses |
| `quotes` | | Quotations |
| `comdis` | | Commercial dispute (Handelsunstimmigkeit) |
| `pricat` | | Price/sales catalogue |
| `utilts` | | Technical master data |
| `utilmd-archive` | | Archived UTILMD profiles (expired release windows) |
| `mscons-archive` | | Archived MSCONS profiles |
| `contrl-archive` | | Archived CONTRL profiles |
| `insrpt-archive` | | Archived INSRPT profiles |
| *(other `-archive` flags)* | | One per message type — see `Cargo.toml` |
| `archive` | | All archived profiles (meta-feature) |
| `serde` | | `Serialize` on `EdiEnergyReport` and validation issue types |
| `diagnostics` | | `miette::Diagnostic` on reports (rich error output) |
| `tracing` | | Structured spans via the `tracing` crate |

---

## Your First Parse

```rust
use edi_energy::{parse, EdiEnergyMessage};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = std::fs::read("my_message.edi")?;
    let msg = parse(&input)?;

    if let Some(mt) = msg.try_message_type() {
        println!("Type    : {}", mt.as_str());
    }
    println!("Release : {}", msg.detect_release()?.as_str());
    println!("PID     : {}", msg.detect_pruefidentifikator()?.as_u32());

    let report = msg.validate()?;
    println!("Valid   : {}", report.is_valid());

    // Turn the report into a Result — propagates as Error::Validation
    report.into_error_result()?;

    Ok(())
}
```

---

## Running the Built-in Examples

The crate ships six runnable examples that cover the full API surface:

```bash
# Parse a UTILMD and inspect typed fields
cargo run --example 01_parse_utilmd

# Parse a MSCONS metering report
cargo run --example 02_parse_mscons

# Build messages from scratch
cargo run --example 03_build_messages

# Route a multi-message interchange
cargo run --example 04_interchange_dispatch

# Full validation report API walkthrough
cargo run --example 05_validate

# Streaming interchange reader
cargo run --example 06_parse_reader
```

---

## Key Concepts

### Pruefidentifikator (PID)

Every EDI@Energy message has a 5-digit **Pruefidentifikator** (e.g. `55001 = Lieferbeginn Strom`) stored in the BGM segment. The library uses it to select the correct set of AHB validation rules.

### Release

A **release** is a BDEW format-version string such as `"S2.1"` (UTILMD Strom) or `"2.4c"` (MSCONS). Releases are registered in the global `ReleaseRegistry` and are used to look up the right MIG and AHB profiles.

### Profile

A **profile** bundles a MIG (structural rules), AHB (PID-specific rules), and codelists for a specific message type and annual release. Profiles live in `crates/edi-energy/profiles/` as JSON and are compiled into the binary.

### Validation Layers

| Layer | What it checks |
|---|---|
| 1 — Schema | Segment presence and mandatory data elements |
| 2 — Code lists | Data element values against allowed code lists |
| 3 — MIG | Message structure rules (segment order, group cardinality) |
| 4 — AHB | Pruefidentifikator-specific mandatory/conditional rules |
| 5 — Semantic | Cross-field business logic (date coherence, reference completeness) |

---

## Next Steps

- [Parsing Guide](./parsing.md) — single message, interchange, streaming reader
- [Validation Guide](./validation.md) — interpreting reports, severity levels
- [Builder Guide](./builders.md) — constructing messages programmatically
- [Platform Guide](./platform.md) — multi-tenant use and test isolation
- [Release Lifecycle](./release-lifecycle.md) — annual BDEW profile updates
- [API-Webdienste Strom](./api-webdienste.md) — REST/JSON channel for iMS processes (`energy-api` crate)
