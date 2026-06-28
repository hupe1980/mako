# Copilot Instructions — mako

## Project Overview

Rust workspace implementing an end-to-end pipeline for German energy market
communication (MaKo / BDEW EDI@Energy). Two distinct concerns:

- **`edi-energy`** — EDIFACT parsing, validation, and schema layer (stateless library)
- **`mako-engine`** — event-sourced process runtime for long-running MaKo workflows

---

## Workspace Structure

```
crates/edi-energy/        EDIFACT parse/validate/schema — stateless library
crates/mako-engine/       Event-sourced runtime (EventStore, Workflow, Process, …)
crates/mako-gpke/         GPKE domain workflows — UTILMD supplier-switch/Sperrung (55001–55002, 55017, 55555, 56001–56004) + INVOIC billing (31001–31008) + ORDERS/ORDRSP Konfiguration (17134/17135, 19001/19002)
crates/mako-wim/          WiM domain workflows (PIDs 11001–11099)
crates/mako-geli-gas/     GeLi Gas domain workflows (PIDs 44001–44006, 44017–44018, 44555)
crates/mako-mabis/        MABIS domain workflows (PID 13003 — Bilanzkreisabrechnung Strom)
crates/mako-gabi-gas/     GaBi Gas domain workflows — Allokation, Nominierung, MMM Gas (placeholder)
crates/dvgw-edi/          DVGW EDIFACT formats — ALLOCAT, NOMINT, NOMRES (placeholder)
crates/mako-nbw/          Netzbetreiberwechsel — PARTIN bulk DSO handover (placeholder)
crates/energy-api/        BDEW API-Webdienste Strom REST/WebSocket client+server
crates/mako-as4/          AS4 transport (placeholder)
crates/mako-redispatch/   Redispatch 2.0 (placeholder)
crates/redispatch-xml/    Redispatch 2.0 XML/XSD format parsing
services/makod/           Production daemon — assembles all modules
xtask/                    Build/codegen/validation tasks
docs/                     Architecture docs (builders.md, parsing.md, bnetza.md, …)
```

---

## Build and Test

```bash
# Check all targets (the minimum gate before any commit):
cargo check --all-targets --all-features

# Run all tests:
cargo test --all-features

# Run tests for a single crate:
cargo test -p mako-engine --all-features

# Run a specific integration test:
cargo test --test <name> --all-features

# Build the production binary:
cargo build -p makod --release --features slatedb

# Lint:
cargo clippy --all-targets --all-features -- -D warnings

# Format:
cargo fmt --all

# Dependency audit (license + security):
cargo deny check

# xtask dev tasks:
cargo xtask bump-version X.Y.Z # bump [workspace.package].version in root Cargo.toml
cargo xtask codegen            # regenerate profile Rust code from YAML schemas
cargo xtask validate-profiles  # validate all profiles against EDIFACT specs
cargo xtask validate-pruefids  # validate Prüfidentifikatoren (AHB check)
cargo xtask audit-ahb          # audit Application Handbooks
cargo xtask check-release-coverage  # verify format-version coverage
cargo xtask generate-fixtures  # regenerate EDIFACT test fixtures
cargo xtask extract-pdf        # extract tables from BDEW PDFs (in docs/pdfs/)
cargo xtask import-codelists   # import BDEW code lists
cargo xtask import-xml-ahb     # import AHB rules from official BDEW XML (requires BDEW subscription)
cargo xtask release-diff       # diff between format versions
```

**MSRV: 1.88** — do not use language features or stdlib APIs introduced after 1.88.

---

## Toolchain and Edition

- Rust edition: **2024** (all crates)
- Toolchain: **stable** (see `rust-toolchain.toml`)
- Components: `rustfmt`, `clippy`

---

## Code Conventions

### Error handling
- All public APIs return `Result<_, EngineError>` or `Result<_, WorkflowError>`.
- Use `thiserror` for error type definitions. Do not use `anyhow` inside library crates.
- `anyhow` is acceptable in `xtask` and `makod` (binary crates).

### Async
- All async code targets **Tokio** (version 1).
- Use async-fn-in-trait (AFIT) — available on MSRV 1.85.
- Do not use `tokio::runtime::Handle::try_current()` as a runtime-detection backdoor.

### Types
- All IDs are UUID v4 newtypes defined via `define_id!` in `mako-engine/src/ids.rs`.
  Never accept or return plain `String` or `Uuid` where a typed ID belongs.
- Timestamps use `time::OffsetDateTime` — **not** `chrono::DateTime<Utc>`.
- EDIFACT payloads and event payloads use `serde_json::Value` — **not** `Vec<u8>` or `Bytes`.

### Workflow determinism
- `Workflow::handle` and `Workflow::apply` must be **pure functions**: no I/O,
  no clock access, no global state mutation.
- All parsing, validation, and external calls happen before the command is constructed,
  at the transport boundary.

### Feature flags
- `slatedb` — opt in at the binary level only; never enable in library crate `[features]` defaults.
- `testing` — enables `InMemoryXxx`/`NoopXxx` stores; must never appear in production builds.
- `tracing` — optional instrumentation; off by default.

### Versioning
- Use **BDEW format versions** (`FV<YYYY>-<MM>-<DD>`) as version keys, not SemVer.
- Always use `FormatVersion::parse(...)` for user-supplied or deserialized strings;
  `FormatVersion::new(...)` is unchecked and only for known-valid compile-time literals.

---

## Domain Rules — Do Not Get Wrong

### MPES is dissolved
PIDs 56001–56004 (Einspeisestelle) were transferred from MPES into **GPKE** per BNetzA BK6-22-024 (LFW24),
effective **2025-06-06**. Former PIDs 56005–56010 do not exist in any current AHB.
There is no `mako-mpes` crate. All ex-MPES processes live in `mako-gpke`.

### GeLi Gas 3.0
The gas supplier-switch process is governed by **BK7-24-01-009** ("GeLi Gas 3.0"),
Beschluss 12.09.2025, abgeschlossen 24.09.2025. This supersedes BK7-19-001 and BK7-06-067.
Scope: UTILMD G (PIDs 44001–44018, 44555) **only** — no INVOIC billing in GeLi Gas.
Gas MMM billing (INVOIC PIDs 31010–31011) belongs to the GaBi Gas domain (`mako-gabi-gas`).

### MABIS vs. Messwesen PIDs
Only PID **13003** is MABIS (Bilanzkreisabrechnung Strom, MSCONS Summenzeitreihen und
Ausfallarbeitssummen, BKV↔ÜNB). PID 13001 does not exist in any MSCONS AHB version.
PIDs 13002–13028 (excluding 13003) are **Messwerten-PIDs** (MSCONS meter data exchange)
and do **not** belong to MABIS. Never register any PID other than 13003 under a
`"mabis-billing"` workflow.

### APERAK Fristen — never mix these up
| Process family | Deadline unit | Calculation |
|---|---|---|
| GPKE | **24 wall-clock hours** (BK6-22-024) | `fristen::add_hours(t, 24)` |
| WiM | **5 Werktage** | `fristen::add_werktage(d, 5, BdewMaKo)` |
| GeLi Gas | **10 Werktage** | `fristen::add_werktage(d, 10, BdewMaKo)` |

**Saturday counts as a Werktag.** Sunday and public holidays do not. This is a common mistake.

### Format-version coexistence
A process started under `FV2025-10-01` continues executing under those rules until it
completes, even after the `FV2026-10-01` cutover. Both coexist in the same engine instance
simultaneously. `WorkflowVersionPolicy::ForwardCompatible` is the correct default for all
MaKo workflows — **do not default to `Pinned`**.

### Dual-write atomicity
Events and outbox entries must be written in a single `WriteBatch` via
`SlateDbStore::append_with_outbox`. Never write events first and outbox second — a crash
between the two produces a lost APERAK.

---

## Licenses

Only these licenses are allowed (enforced by `cargo deny`):
MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2-Clause, BSD-3-Clause,
ISC, Unicode-3.0, Zlib, CDLA-Permissive-2.0, MIT-0.

---

## Key Documentation

| Topic | File |
|---|---|
| Process engine guide | [docs/engine.md](../docs/engine.md) |
| `makod` operator guide | [docs/makod.md](../docs/makod.md) |
| Parsing guide | [docs/parsing.md](../docs/parsing.md) |
| Validation guide | [docs/validation.md](../docs/validation.md) |
| Builder patterns | [docs/builders.md](../docs/builders.md) |
| Annual release workflow | [docs/annual-release-workflow.md](../docs/annual-release-workflow.md) |
| Schema versioning | [docs/schema-versioning.md](../docs/schema-versioning.md) |
| API-Webdienste Strom | [docs/api-webdienste.md](../docs/api-webdienste.md) |
| Release lifecycle | [docs/release-lifecycle.md](../docs/release-lifecycle.md) |
| BNetzA regulatory reference | [docs/bnetza.md](../docs/bnetza.md) |
