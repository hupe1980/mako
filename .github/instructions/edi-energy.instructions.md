---
description: "Use when working in crates/edi-energy: parsing EDIFACT, validating messages, implementing or extending profiles/AHB rules, running codegen, or writing tests against the edi-energy API."
applyTo: "crates/edi-energy/**"
---

# edi-energy Crate Instructions

## Parsing API

Use the top-level entry points — do **not** construct segment trees manually:

```rust
// Preferred: stateless one-shot
let msg = edi_energy::parse(input)?;

// With custom config (e.g. reference date for release detection)
let msg = edi_energy::Parser::new()
    .with_config(ParseConfig::default().with_reference_date(date))
    .parse(input)?;

// Interchange (multi-message file)
for msg in edi_energy::parse_interchange(reader) { … }

// Lightweight header-only parse (no segment tree)
let hdr = edi_energy::parse_envelope_only(input)?;
```

## Profile and Registry System

- `ReleaseRegistry::global()` is the process-global singleton, populated by `xtask codegen`.
- Each `(MessageType, Release)` pair maps to a `Profile` with MIG + AHB rule packs.
- `PidSource` controls where the Prüfidentifikator is extracted from (`BgmDe1004` default; `RffZ13` for COMDIS/PRICAT).
- Never hard-code message-type → PID extraction logic; always consult the registry.

## Validation

```rust
let report = msg.validate()?;               // validates against bundled profile
let report = msg.validate_against(&profile)?; // validates against custom profile

// A valid report still carries warnings — always inspect severity:
if report.is_valid() { … }
for finding in report.findings() { println!("{:?}", finding.severity()); }
```

Classic UTILMD releases (5.5.x) can be **parsed** but **not validated** — `validate()` returns `Error::ProfileNotFound`. This is intentional; do not add 5.5.x profiles to the bundled registry.

## Profile YAML and Codegen

- Profile source of truth: `crates/edi-energy/profiles/<message_type>/<fvYYYYMMDD>/{mig.json, ahb.json, codelists.json}` (schemas in `profiles/schemas/*.schema.json`)
- Generated Rust: `crates/edi-energy/src/generated/` — **never edit by hand**
- Regenerate after any YAML change: `cargo xtask codegen`
- Validate profiles: `cargo xtask validate-profiles`
- Validate Prüfidentifikatoren: `cargo xtask validate-pruefids`

## Crate Lint Configuration

The crate enforces `#![deny(unsafe_code)]` and `#![warn(missing_docs, clippy::pedantic)]`. Several pedantic lints are `#[allow]`-listed for generated code — do not suppress them in hand-written modules. All `#[allow]` items in `lib.rs` are explicitly justified by comments; follow the same pattern if a new allow is genuinely required.

## Testing

- Integration tests live in `crates/edi-energy/tests/`
- Fixtures live in `crates/edi-energy/tests/fixtures/` — regenerate with `cargo xtask generate-fixtures`
- Never hard-code raw EDIFACT bytes inline in tests; use fixture files or the builder API (`crates/edi-energy/src/builders/`)
- Run with: `cargo test -p edi-energy --all-features`
