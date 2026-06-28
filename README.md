# mako ⚡

> **⚠️ Experimental** — Pre-1.0. APIs may change between releases. Not yet recommended for production without thorough in-house testing.

A **Rust workspace** for end-to-end German energy market communication (**BDEW MaKo / EDI@Energy**).

Two distinct concerns live here:

- **`edi-energy`** — Stateless EDIFACT parse, validate, and build library. No async, no I/O, no runtime deps.
- **`mako-engine` + domain crates** — Event-sourced process runtime for long-running MaKo workflows with regulatory deadlines, dual-write atomicity, and AS4 inbound transport.

[![CI](https://github.com/hupe1980/mako/actions/workflows/ci.yml/badge.svg)](https://github.com/hupe1980/mako/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](./LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-1.88+-orange?logo=rust)](https://www.rust-lang.org/)
[![BDEW](https://img.shields.io/badge/BDEW-EDI%40Energy-green)](https://www.edi-energy.de/)

---

## Workspace at a Glance

| Crate / service | Purpose |
|---|---|
| `edi-energy` | Parse · validate · build all 17 EDI@Energy EDIFACT message types |
| `mako-engine` | Event-sourced runtime: `Workflow`, `Process`, `EventStore`, outbox, deadlines |
| `mako-gpke` | GPKE workflows — UTILMD supplier-switch/Sperrung (55001–55002, 55017, 55555, 56001–56004) + INVOIC billing (31001–31008) + ORDERS/ORDRSP Konfiguration (17134/17135, 19001/19002) |
| `mako-wim` | WiM workflows — PIDs 11001–11099 (Messstellenwechsel Strom) |
| `mako-geli-gas` | GeLi Gas 3.0 workflows — UTILMD G supplier-switch/Sperrung Gas (44001–44006, 44017–44018, 44555) |
| `mako-mabis` | MABIS workflows — PID 13003 (Bilanzkreisabrechnung Strom, BKV↔ÜNB) |
| `mako-gabi-gas` | GaBi Gas workflows — Allokation, Nominierung, MMM Gas INVOIC (31010–31011) — placeholder |
| `mako-nbw` | Netzbetreiberwechsel — PARTIN bulk DSO concession handover (PIDs 37000–37014) — placeholder |
| `mako-as4` | BDEW AS4 profile constants and P-Mode configuration |
| `dvgw-edi` | DVGW EDIFACT formats — ALLOCAT, NOMINT, NOMRES parsing — placeholder |
| `mako-redispatch` | Redispatch 2.0 process workflows — placeholder |
| `redispatch-xml` | Redispatch 2.0 XML/XSD format parsing |
| `energy-api` | BDEW API-Webdienste Strom — REST/WebSocket client + Axum server for iMS processes |
| `makod` | Production daemon — assembles all modules, AS4 inbound server, deadline scheduler |

---

## ✨ Features

### EDIFACT layer (`edi-energy`)

| Category | Detail |
|---|---|
| 📦 **17 message types** | UTILMD, MSCONS, APERAK, CONTRL, INVOIC, REMADV, ORDERS, IFTSTA, INSRPT, REQOTE, PARTIN, ORDCHG, ORDRSP, QUOTES, COMDIS, PRICAT, UTILTS |
| 🔍 **5-layer validation** | MIG structural rules, AHB Pruefidentifikator-specific rules, semantic cross-field rules |
| 📅 **Annual release lifecycle** | Multi-version profile registry with 7-day transition grace windows (BDEW-compliant) |
| 🔒 **Security by default** | DoS limits (max 10 MB, 10 000 segments), log-injection sanitisation, fuzz-tested with 1 100+ corpus entries |
| 🛠️ **Fluent message builders** | Type-state builder API with compile-time mandatory field enforcement |
| 🔁 **Round-trip serialisation** | Parse → validate → serialize with byte-exact EDIFACT output |
| 🧪 **Code-generated profiles** | 36 profiles across 17 types, regenerated annually via `cargo xtask codegen` |

### Process engine layer (`mako-engine` + domain crates)

| Category | Detail |
|---|---|
| ♻️ **Event-sourced processes** | Optimistic-concurrency event append with SlateDB-backed storage |
| ⚛️ **Atomic dual-write** | Events and outbox messages written in a single `WriteBatch` via `AtomicAppend` |
| ⏰ **Regulatory deadlines** | `DeadlineStore` with GPKE 24h / WiM 5-Werktage / GeLi Gas 10-Werktage Fristen |
| 📨 **AS4 inbound transport** | `makod` receives BDEW AS4 pushes via `asx-rs`, deduplicates with `SlateDbInboxStore`, routes by Pruefidentifikator |
| 🔄 **Format-version coexistence** | Processes started under `FV2025-10-01` run to completion under those rules even after `FV2026-10-01` cutover |
| 🪦 **Dead-letter sink** | Structured `DeadLetterReason` variants — `UnknownPid`, `DuplicateMessage`, `VersionMismatch`, … |

---

## 🚀 Quick Start — EDIFACT parsing

```toml
[dependencies]
edi-energy = "0.1"
```

```rust
use edi_energy::{parse, EdiEnergyMessage};

let input = std::fs::read("Netznutzung_20241015.edi")?;
let msg = parse(&input)?;
let report = msg.validate()?;
println!("Valid: {}", report.is_valid());
```

---

## 🚀 Quick Start — Process engine

```toml
[dependencies]
mako-engine = { version = "0.1", features = ["testing"] }
mako-gpke   = "0.1"
```

```rust
use mako_engine::{
    builder::EngineBuilder,
    ids::TenantId,
    version::WorkflowId,
    event_store::InMemoryEventStore,
};
use mako_gpke::lieferbeginn::SupplierChangeWorkflow;

let ctx = EngineBuilder::new()
    .with_event_store(InMemoryEventStore::new())
    .build();

// Spawn a new process for one delivery point.
let process   = ctx.spawn::<SupplierChangeWorkflow>(TenantId::new(), wf_id);
let envelopes = process.execute(initiate_cmd).await?;

// Reconstruct typed state by replaying all persisted events.
let state = process.state().await?;
```

---

## 📋 Message Type Coverage

| Message | EDIFACT Type | Latest Release | Use Case |
|---|---|---|---|
| UTILMD Strom | `UTILMD` | S2.2 (`fv20261001`) | Grid connection (supplier switch, registration) |
| UTILMD Gas | `UTILMD` | G1.2 (`fv20261001_gas`) | Gas grid connection processes |
| MSCONS | `MSCONS` | 2.5 (`fv20261001`) | Metered services consumption reports |
| APERAK | `APERAK` | 2.2 (`fv20261001`) | Application error acknowledgements |
| CONTRL | `CONTRL` | 2.0b (`fv20260101`) | Interchange control acknowledgements |
| INVOIC | `INVOIC` | 2.8e (`fv20260401`) | Invoices |
| REMADV | `REMADV` | 2.9f (`fv20260401`) | Remittance advice |
| ORDERS | `ORDERS` | 1.4b (`fv20260401`) | Purchase orders |
| IFTSTA | `IFTSTA` | 2.1 (`fv20261001`) | Multimodal status reports |
| INSRPT | `INSRPT` | 1.1a (`fv20260101`) | Inspection reports |
| REQOTE | `REQOTE` | 1.3c (`fv20260401`) | Requests for quotation |
| PARTIN | `PARTIN` | 1.1 (`fv20260401`) | Party information |
| ORDCHG | `ORDCHG` | 1.2 (`fv20260401`) | Purchase order changes |
| ORDRSP | `ORDRSP` | 1.4c (`fv20260401`) | Purchase order responses |
| QUOTES | `QUOTES` | 1.3c (`fv20260401`) | Quotations |
| COMDIS | `COMDIS` | 1.0h (`fv20261001`) | Commercial dispute (Handelsunstimmigkeit) |
| PRICAT | `PRICAT` | 2.1 (`fv20260401`) | Price/sales catalogue |
| UTILTS | `UTILTS` | 1.1e (`fv20260401`) | Technical master data |

---

## 📖 Documentation

| Document | Description |
|---|---|
| [Getting Started](./docs/getting-started.md) | Installation, first parse, first workflow |
| [Process Engine Guide](./docs/engine.md) | `mako-engine` concepts, stores, deadlines, outbox |
| [Parsing Guide](./docs/parsing.md) | Single message, interchange, streaming |
| [Validation Guide](./docs/validation.md) | Layers, reports, Pruefidentifikator |
| [Builder Guide](./docs/builders.md) | Constructing messages programmatically |
| [Platform Guide](./docs/platform.md) | Multi-tenant, test isolation, custom profiles |
| [API-Webdienste Strom](./docs/api-webdienste.md) | REST/JSON channel for iMS processes (`energy-api`) |
| [Release Lifecycle](./docs/release-lifecycle.md) | Annual BDEW profile updates, codegen pipeline |
| [Schema Versioning](./docs/schema-versioning.md) | Profile JSON schema evolution and archive lifecycle |
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

---

## 🏗️ Architecture

```
mako/
├── crates/
│   ├── edi-energy/          # EDIFACT parse · validate · build · serialize
│   │   ├── src/             # EdiEnergyMessage, Platform, builders, registry
│   │   └── profiles/        # BDEW JSON profile data (MIG + AHB + codelists)
│   │
│   ├── mako-engine/         # Event-sourced process runtime
│   │   └── src/             # Workflow, Process, EngineBuilder, all store traits
│   │                        # + SlateDB implementations, fristen, dead-letter
│   │
   ├── mako-gpke/           # GPKE domain (55001–55002, 55017, 55555, 56001–56004, INVOIC 31001–31008)
   ├── mako-wim/            # WiM domain  (11001–11099)
   ├── mako-geli-gas/       # GeLi Gas 3.0 domain (44001–44006, 44017–44018, 44555)
   ├── mako-mabis/          # MABIS domain (13003 — Bilanzkreisabrechnung Strom)
   ├── mako-gabi-gas/       # GaBi Gas domain — Allokation, Nominierung, MMM Gas (placeholder)
   ├── mako-nbw/            # Netzbetreiberwechsel — PARTIN DSO handover (placeholder)
   │
   ├── mako-as4/            # BDEW AS4 profile constants, P-Modes, security policy
   ├── dvgw-edi/            # DVGW EDIFACT formats — ALLOCAT, NOMINT, NOMRES (placeholder)
│   ├── energy-api/          # BDEW REST/WebSocket API client + Axum server (iMS)
│   ├── mako-redispatch/     # Redispatch 2.0 (placeholder)
│   └── redispatch-xml/      # Redispatch 2.0 XML/XSD parsing
│
├── services/
│   └── makod/               # Production daemon
│       └── src/             # main.rs, config.rs, as4_ingest.rs, as4_sender.rs
│                            # edifact_api.rs, commands_api.rs, webdienste.rs
│                            # adapters.rs, edifact_renderer.rs, erp_adapter.rs
│                            # partner_api.rs, deadline_dispatch.rs, health.rs, …
│                            # CLI: --data-dir, --as4-addr, --http-addr, --tenant-id
│
├── xtask/                   # Dev automation: codegen · validate · release-diff
└── fuzz/                    # cargo-fuzz targets (1 100+ corpus entries)
```

### Data flow

```
BDEW counterparty (AS4 push)
       │
       ▼
makod/as4_ingest  ──  asx-rs receive + WSS verify + dedup
       │
       ▼  raw EDIFACT bytes
Platform::parse_interchange  ──  edi-energy parse + validate
       │
       ▼  detected PID
PidRouter::route  ──  selects domain handler (GPKE / WiM / GeLi Gas / MABIS)
       │
       ▼  typed Command
Process::execute_and_enqueue  ──  replay state · Workflow::handle · AtomicAppend
       │
       ├─ EventStore (SlateDB)
       ├─ OutboxStore  ──►  delivery worker  ──►  AS4 send
       └─ DeadlineStore  ──►  scheduler  ──►  TimeoutExpired command
```

---

## ⚙️ Feature Flags — `edi-energy`

By default UTILMD, MSCONS, APERAK, and CONTRL are compiled in:

```toml
[dependencies]
edi-energy = { version = "0.1", features = ["invoic", "remadv", "orders"] }
```

| Flag | Default | Enables |
|---|---|---|
| `utilmd` | ✅ | UTILMD Strom + Gas |
| `mscons` | ✅ | MSCONS metered consumption |
| `aperak` | ✅ | APERAK error acknowledgement |
| `contrl` | ✅ | CONTRL syntax acknowledgement |
| `invoic` | | INVOIC invoice |
| `remadv` | | REMADV remittance advice |
| `orders` | | ORDERS purchase order |
| `iftsta` | | IFTSTA multimodal status |
| `insrpt` | | INSRPT inspection report |
| `reqote` | | REQOTE request for quotation |
| `partin` | | PARTIN party information |
| `ordchg` | | ORDCHG order change |
| `ordrsp` | | ORDRSP order response |
| `quotes` | | QUOTES quotation |
| `comdis` | | COMDIS commercial dispute |
| `pricat` | | PRICAT price catalogue |
| `utilts` | | UTILTS technical master data |
| `archive` | | All archived profiles (expired release windows) |
| `serde` | | `Serialize` on `EdiEnergyReport` |
| `diagnostics` | | `miette::Diagnostic` on reports |
| `tracing` | | Structured tracing spans |

## ⚙️ Feature Flags — `mako-engine` / `makod`

| Flag | Crate | Enables |
|---|---|---|
| `slatedb` | `mako-engine`, `makod` | Production `SlateDbStore` (never enable in library `[features]` defaults) |
| `testing` | `mako-engine` | `InMemoryEventStore`, `NoopDeadLetterSink`, `InMemoryInboxStore` — never in production |
| `tracing` | `mako-engine` | Structured instrumentation spans |

---

## 🔧 Development

```bash
# Check all targets — minimum gate before any commit
cargo check --all-targets --all-features

# Run all tests
cargo test --all-features

# Run tests for one crate
cargo test -p mako-engine --all-features

# Build the production daemon
cargo build -p makod --release --features slatedb

# Lint (warnings are errors)
cargo clippy --all-targets --all-features -- -D warnings

# Format
cargo fmt --all

# Dependency audit (license + security)
cargo deny check

# Validate all profile JSON against JSON Schema
cargo xtask validate-profiles

# Check that every Pruefidentifikator has a test fixture
cargo xtask validate-pruefids

# Check that today's date is covered by a current profile
cargo xtask check-release-coverage

# Regenerate all profile Rust code after editing profiles/
cargo xtask codegen

# Check no generated code has drifted
cargo xtask codegen --check

# Compute a diff between two annual releases
cargo xtask release-diff --from utilmd/fv20241001 --to utilmd/fv20261001

# Run fuzz target (requires nightly + cargo-fuzz)
cargo +nightly fuzz run fuzz_parse_validate
```

---

## 📊 Performance — `edi-energy`

Benchmarks on Apple M-series (single core, Criterion):

| Operation | Throughput |
|---|---|
| Parse minimal UTILMD | ~2 µs / message |
| Validate UTILMD S2.1 (MIG + AHB) | ~8 µs / message |
| Parse 100-message interchange | ~180 µs total |
| Build UTILMD + serialize | ~5 µs / message |

```bash
cargo bench --bench benchmarks
```

---

## 🤝 Contributing

Contributions are welcome. Open an issue before large changes.

- Run `cargo check --all-targets --all-features` and `cargo test --all-features` before submitting a PR.
- Generated files under `crates/edi-energy/src/generated/` are machine-produced — edit the profile JSON and run `cargo xtask codegen` instead.
- See [docs/release-lifecycle.md](./docs/release-lifecycle.md) for the annual BDEW profile update procedure.
- See [docs/engine.md](./docs/engine.md) for the process engine architecture and conventions.

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
- [asx-rs](https://crates.io/crates/asx-rs) — AS4/ebMS3 transport library used by `makod`
- [SlateDB](https://slatedb.io/) — Embedded LSM storage backing `mako-engine`
