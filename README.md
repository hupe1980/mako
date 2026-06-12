# mako ⚡

> **⚠️ Experimental** — This library is pre-1.0. APIs may change without notice between releases. Not yet recommended for production-critical systems without thorough in-house testing. Breaking changes are expected.

A **Rust library** for parsing, validating, and building **EDI@Energy EDIFACT** messages in compliance with the German energy market communication standard (**BDEW MaKo**).

[![CI](https://github.com/hupe1980/mako/actions/workflows/ci.yml/badge.svg)](https://github.com/hupe1980/mako/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](./LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-1.85+-orange?logo=rust)](https://www.rust-lang.org/)
[![BDEW](https://img.shields.io/badge/BDEW-EDI%40Energy-green)](https://www.edi-energy.de/)

---

## ✨ Features

| Category | Detail |
|---|---|
| 📦 **17 message types** | UTILMD, MSCONS, APERAK, CONTRL, INVOIC, REMADV, ORDERS, IFTSTA, INSRPT, REQOTE, PARTIN, ORDCHG, ORDRSP, QUOTES, COMDIS, PRICAT, UTILTS |
| 🔍 **5-layer validation** | MIG structural rules, AHB Pruefidentifikator-specific rules, semantic cross-field rules |
| 📅 **Annual release lifecycle** | Multi-version profile registry with 7-day transition grace windows (BDEW-compliant) |
| 🔒 **Security by default** | DoS limits (max 10 MB, 10 000 segments), log-injection sanitization, fuzz-tested with 1 100+ corpus entries |
| 🛠️ **Fluent message builders** | Type-state builder API with compile-time mandatory field enforcement |
| 🏗️ **Domain types** | `ObjectType`, `Pruefidentifikator`, `Release` — no raw strings in your domain code |
| 🔁 **Round-trip serialization** | Parse → validate → serialize with byte-exact EDIFACT output |
| 🧪 **Code-generated profiles** | 36 profiles across 17 types, regenerated annually via `cargo xtask codegen`; expired profiles archived behind `{type}-archive` / `archive` features via `cargo xtask codegen --prune-expired` |
| 🦺 **CI-gated codegen drift** | `cargo xtask codegen --check` prevents generated-code drift in pull requests |
| 🌐 **energy-api crate** | Axum-based server + reqwest client for BDEW API-Webdienste (MaKo REST/WebSocket) |

---

## 🚀 Quick Start

Add to your `Cargo.toml`:

```toml
[dependencies]
edi-energy = "0.1"
```

Parse and validate a UTILMD message in three lines:

```rust
use edi_energy::{parse, EdiEnergyMessage};

let input = std::fs::read("Netznutzung_20241015.edi")?;
let msg = parse(&input)?;
let report = msg.validate()?;

println!("Valid: {}", report.is_valid());
```

---

## 📋 Message Type Coverage

| Message | EDIFACT Type | Latest Release | Use Case |
|---|---|---|---|
| UTILMD Strom | `UTILMD` | S2.2 (`fv20261001`) | Grid connection processes (supplier switch, registration) |
| UTILMD Gas | `UTILMD` | G1.2 (`fv20261001_gas`) | Gas grid connection processes |
| MSCONS | `MSCONS` | 2.5 (`fv20261001`) | Metered services consumption reports |
| APERAK | `APERAK` | 2.2 (`fv20261001`) | Application error acknowledgements |
| CONTRL | `CONTRL` | 2.0b (`fv20260101`) | Interchange control acknowledgements |
| INVOIC | `INVOIC` | 2.8e (`fv20260401`) | Invoices |
| REMADV | `REMADV` | 2.9e (`fv20251001`) | Remittance advice |
| ORDERS | `ORDERS` | 1.4c (`fv20260401`) | Purchase orders |
| IFTSTA | `IFTSTA` | 2.1 (`fv20261001`) | Multimodal status reports |
| INSRPT | `INSRPT` | 1.1a (`fv20260101`) | Inspection reports |
| REQOTE | `REQOTE` | 1.3c (`fv20260401`) | Requests for quotation |
| PARTIN | `PARTIN` | 1.1 (`fv20260401`) | Party information |
| ORDCHG | `ORDCHG` | 1.2 (`fv20260401`) | Purchase order changes |
| ORDRSP | `ORDRSP` | 1.4c (`fv20260401`) | Purchase order responses |
| QUOTES | `QUOTES` | 1.3c (`fv20260401`) | Quotations |
| COMDIS | `COMDIS` | 1.0g (`fv20261001`) | Commercial dispute (Handelsunstimmigkeit) |
| PRICAT | `PRICAT` | 2.1 (`fv20260401`) | Price/sales catalogue |
| UTILTS | `UTILTS` | 1.1e (`fv20260401`) | Technical master data |

---

## 📖 Documentation

| Document | Description |
|---|---|
| [Getting Started](./docs/getting-started.md) | Installation, feature flags, first parse |
| [Parsing Guide](./docs/parsing.md) | Single message, interchange, streaming |
| [Validation Guide](./docs/validation.md) | Layers, reports, Pruefidentifikator |
| [Builder Guide](./docs/builders.md) | Constructing messages programmatically |
| [Platform Guide](./docs/platform.md) | Multi-tenant, test isolation, custom profiles |
| [Release Lifecycle](./docs/release-lifecycle.md) | Annual BDEW profile updates, codegen pipeline |
| [Schema Versioning](./docs/schema-versioning.md) | Profile JSON schema evolution policy, archive lifecycle |
| [API Reference](https://docs.rs/edi-energy) | Full rustdoc |

---

## 💡 Usage Examples

### Parse a single message

```rust
use edi_energy::{parse, AnyMessage, EdiEnergyMessage};

let msg = parse(bytes)?;

match &msg {
    AnyMessage::Utilmd(m) => {
        println!("PID: {}", m.detect_pruefidentifikator()?.as_u32());
        if let Some(bgm) = m.bgm() {
            println!("Doc code: {}", bgm.document_code);
        }
    }
    AnyMessage::Mscons(m) => {
        println!("Consumption report, {} segments", m.raw_segments().len());
    }
    AnyMessage::Unknown { message_type_code, .. } => {
        println!("Unrecognised type: {message_type_code}");
    }
    _ => {}
}
```

### Validate and inspect issues

```rust
use edi_energy::{parse, EdiEnergyMessage};

let msg = parse(bytes)?;
let report = msg.validate()?;

if !report.is_valid() {
    for issue in report.errors() {
        println!(
            "[{}] {} — {}",
            issue.rule_id.as_deref().unwrap_or("-"),
            issue.segment_tag.as_deref().unwrap_or("-"),
            issue.message,
        );
    }
}
// Propagate as Err(Error::Validation { .. }):
report.into_error_result()?;
```

### Parse a multi-message interchange

```rust
use std::io::Cursor;
use edi_energy::{parse_interchange, EdiEnergyMessage};

let reader = Cursor::new(bytes);
for msg_result in parse_interchange(reader) {
    let msg = msg_result?;
    if let Some(mt) = msg.try_message_type() {
        println!("{} — PID {:?}", mt.as_str(), msg.detect_pruefidentifikator().ok());
    }
}
```

### Build a UTILMD message

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
    .document_code("E01")
    .document_date("20261001")
    .transaction(ObjectType::Marktlokation, "51238696782")
        .process_date("163", "20261001")
        .reference("Z13", "55001")
        .done()
    .build()?
    .serialize()?;
```

### Validate with an explicit Pruefidentifikator

```rust
use edi_energy::{parse, validate_and_check_pid, Pruefidentifikator};

let msg = parse(bytes)?;
let pid = Pruefidentifikator::new(55001)?;
let report = validate_and_check_pid(&msg, pid)?;
assert!(report.is_valid());
```

---

## 🏗️ Architecture

```
mako/
├── crates/
│   ├── edi-energy/          # Core library: parse · validate · build · serialize
│   │   ├── src/
│   │   │   ├── parse.rs     # Entry points + DoS limits + release dispatch
│   │   │   ├── message.rs   # EdiEnergyMessage trait
│   │   │   ├── any_message.rs # Feature-gated enum over all message types
│   │   │   ├── builders/    # Type-state fluent builders
│   │   │   ├── messages/    # Per-type typed field access
│   │   │   ├── registry/    # Release registry + transition window logic
│   │   │   └── generated/   # Code-generated profile rule packs (37 files)
│   │   └── profiles/        # BDEW JSON profile data (MIG + AHB + codelists)
│   └── energy-api/          # BDEW REST/WebSocket API client + server
├── xtask/                   # Dev-automation: codegen · validate · release-diff
└── fuzz/                    # Cargo-fuzz targets (1 100+ corpus entries)
```

---

## ⚙️ Feature Flags

By default only UTILMD, MSCONS, APERAK, and CONTRL are compiled in.
Enable additional message types individually or use `--all-features`:

```toml
[dependencies]
edi-energy = { version = "0.1", features = ["invoic", "remadv", "orders"] }
```

| Flag | Enables |
|---|---|
| `utilmd` *(default)* | UTILMD Strom + Gas |
| `mscons` *(default)* | MSCONS metered consumption |
| `aperak` *(default)* | APERAK error acknowledgement |
| `contrl` *(default)* | CONTRL syntax acknowledgement |
| `invoic` | INVOIC invoice |
| `remadv` | REMADV remittance advice |
| `orders` | ORDERS purchase order |
| `iftsta` | IFTSTA multimodal status |
| `insrpt` | INSRPT inspection report |
| `reqote` | REQOTE request for quotation |
| `partin` | PARTIN party information |
| `ordchg` | ORDCHG order change |
| `ordrsp` | ORDRSP order response |
| `quotes` | QUOTES quotation |
| `comdis` | COMDIS commercial dispute |
| `pricat` | PRICAT price catalogue |
| `utilts` | UTILTS technical master data |
| `utilmd-archive` | Archived UTILMD profiles (expired release windows) |
| `mscons-archive` | Archived MSCONS profiles |
| `contrl-archive` | Archived CONTRL profiles |
| `insrpt-archive` | Archived INSRPT profiles |
| *(other `-archive` flags)* | One per message type — same pattern |
| `archive` | All archived profiles (meta-feature, enables all `{type}-archive`) |
| `serde` | `Serialize` on `EdiEnergyReport` and issue types |
| `diagnostics` | `miette::Diagnostic` on reports |
| `tracing` | Structured tracing spans via the `tracing` crate |

---

## 🔧 Development

```bash
# Run the full test suite
cargo test --all-features

# Validate all profile JSON against JSON Schema
cargo xtask validate-profiles

# Check that every Pruefidentifikator has a test fixture
cargo xtask validate-pruefids

# Generate synthetic .edi fixtures for uncovered PIDs
cargo xtask generate-fixtures

# Check no generated code has drifted
cargo xtask codegen --check

# Regenerate all 36 profile files after editing profiles/
cargo xtask codegen

# Archive profiles expired beyond the 90-day grace window (run annually)
cargo xtask codegen --prune-expired

# Compute a diff between two annual release profiles
cargo xtask release-diff --from utilmd/fv20241001 --to utilmd/fv20261001

# Run fuzz target (requires nightly + cargo-fuzz)
cargo +nightly fuzz run fuzz_parse_validate
```

---

## 📊 Performance

Benchmarks run on Apple M-series (single core, Criterion):

| Operation | Throughput |
|---|---|
| Parse minimal UTILMD | ~2 µs / message |
| Validate UTILMD S2.1 (MIG + AHB) | ~8 µs / message |
| Parse 100-message interchange | ~180 µs total |
| Build UTILMD + serialize | ~5 µs / message |

Run locally:

```bash
cargo bench --bench benchmarks
```

---

## 🤝 Contributing

Contributions are welcome. Please open an issue before large changes.

- Run `cargo test --all-features` and `cargo xtask validate-profiles` before submitting a PR.
- Generated files under `crates/edi-energy/src/generated/` are machine-produced — do not edit them by hand; instead edit the profile JSON and run `cargo xtask codegen`.
- See [docs/release-lifecycle.md](./docs/release-lifecycle.md) for the annual BDEW profile update procedure.

---

## 📜 License

Licensed under either of:

- [MIT License](./LICENSE-MIT)
- [Apache License, Version 2.0](./LICENSE-APACHE)

at your option.

---

## 🔗 Resources

- [edi-energy.de](https://www.edi-energy.de/) — Official BDEW specification portal
- [BDEW MaKo](https://www.bdew.de/energie/marktkommunikation/) — Market communication framework
- [edifact-rs](https://crates.io/crates/edifact-rs) — Underlying EDIFACT parser
