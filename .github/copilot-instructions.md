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
crates/mako-gpke/         GPKE — UTILMD Strom (55001–55018, 55555, 56001–56004) + INVOIC (31001–31008) + ORDERS/ORDRSP Konfiguration (17134/17135, 19001/19002)
crates/mako-wim/          WiM Strom — Gerätewechsel (11001–11003) + ORDERS Geräteübernahme + Stammdaten
crates/mako-geli-gas/     GeLi Gas 3.0 — UTILMD G (44001–44006, 44017–44018, 44555)
crates/mako-mabis/        MABIS — PID 13003 (Bilanzkreisabrechnung Strom, BKV↔ÜNB)
crates/mako-wim-gas/      WiM Gas — UTILMD G (44022–44053) [placeholder]
crates/mako-gabi-gas/     GaBi Gas — INVOIC (31010–31011) [placeholder]
crates/dvgw-edi/          DVGW EDIFACT formats — ALLOCAT, NOMINT, NOMRES [placeholder]
crates/mako-nbw/          Netzbetreiberwechsel — PARTIN bulk DSO handover [placeholder]
crates/energy-api/        BDEW API-Webdienste Strom REST/WebSocket client+server
crates/mako-as4/          AS4 transport [placeholder]
crates/mako-redispatch/   Redispatch 2.0 [placeholder]
crates/redispatch-xml/    Redispatch 2.0 XML/XSD format parsing
services/makod/           Production daemon — assembles all modules
xtask/                    Build/codegen/validation tasks
docs/                     Architecture docs
```

---

## Build and Test

```bash
# Full CI gate — run before every commit:
just ci

# Individual gates:
cargo check --all-targets --all-features
cargo test --all-features
cargo test -p mako-engine --all-features
cargo test --test <name> --all-features
cargo build -p makod --release --features slatedb
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all
cargo deny check

# xtask tasks:
cargo xtask bump-version X.Y.Z       # bump [workspace.package].version
cargo xtask codegen                   # regenerate profile Rust code from YAML
cargo xtask validate-profiles         # validate all profiles against EDIFACT specs
cargo xtask validate-pruefids         # validate Prüfidentifikatoren (AHB check)
cargo xtask audit-ahb                 # audit Application Handbooks
cargo xtask check-release-coverage    # verify format-version coverage
cargo xtask generate-fixtures         # regenerate EDIFACT test fixtures
cargo xtask extract-pdf               # extract tables from BDEW PDFs (docs/pdfs/)
cargo xtask import-codelists          # import BDEW code lists
cargo xtask import-xml-ahb            # import AHB rules from BDEW XML
cargo xtask release-diff              # diff between format versions
```

**`just ci` is the minimum gate before any commit.** It runs check + test + clippy
+ fmt-check + deny + codegen-check + validate-profiles-strict + validate-pruefids-strict.

**MSRV: 1.88** — do not use language features or stdlib APIs introduced after 1.88.

---

## Toolchain and Edition

- Rust edition: **2024** (all crates)
- Toolchain: **1.88** (pinned in `rust-toolchain.toml` — do not change to `stable`)
- Components: `rustfmt`, `clippy`

---

## Active Format Versions

| Format version | Valid period | Status |
|---|---|---|
| `FV2025-10-01` | 2025-10-01 through 2026-09-30 | **Current production** |
| `FV2026-10-01` | from 2026-10-01 | **Next release — profiles must exist** |

Both coexist in the same engine instance simultaneously. A process started under
`FV2025-10-01` continues under those rules until it completes, even after the
`FV2026-10-01` cutover.

---

## Code Conventions

### Error handling
- All public APIs return `Result<_, EngineError>` or `Result<_, WorkflowError>`.
- Use `thiserror` for error type definitions. Do not use `anyhow` inside library crates.
- `anyhow` is acceptable in `xtask` and `makod` (binary crates).
- Every `Result`-returning function must be annotated `#[must_use]`.

### Async
- All async code targets **Tokio** (version 1).
- Use async-fn-in-trait (AFIT) — stabilised at Rust 1.75, available on MSRV 1.88.
- Do not use `tokio::runtime::Handle::try_current()` as a runtime-detection backdoor.

### Types
- All IDs are UUID v4 newtypes defined via `define_id!` in `mako-engine/src/ids.rs`.
  Never accept or return plain `String` or `Uuid` where a typed ID belongs.
- Timestamps use `time::OffsetDateTime` — **not** `chrono::DateTime<Utc>`.
- EDIFACT payloads and event payloads use `serde_json::Value` — **not** `Vec<u8>` or `Bytes`.

### Workflow determinism
- `Workflow::handle` and `Workflow::apply` must be **pure functions**: no I/O,
  no clock access, no global state mutation.
- All parsing, validation, and external calls happen before the command is
  constructed, at the transport boundary.

### Feature flags
- `slatedb` — opt in at the binary level only; never enable in library crate defaults.
- `testing` — enables `InMemoryXxx`/`NoopXxx` stores; must never appear in production builds.
- `tracing` — optional instrumentation; off by default.

### Versioning
- Use **BDEW format versions** (`FV<YYYY>-<MM>-<DD>`) as version keys, not SemVer.
- Always use `FormatVersion::parse(...)` for user-supplied or deserialized strings.
- `FormatVersion::new(...)` is unchecked — only for known-valid compile-time literals.

---

## Domain Rules — Do Not Get Wrong

### PID ownership — authoritative table

| PID range | Crate | Source |
|---|---|---|
| 55001–55018, 55555 | `mako-gpke` | BK6-24-174 |
| 56001–56004 | `mako-gpke` (ex-MPES, absorbed per BK6-22-024, eff. 2025-06-06) | BK6-22-024 |
| 11001–11003 | `mako-wim` | BK6-24-174 |
| 13003 | `mako-mabis` | BK6-24-174 |
| 44001–44006, 44017–44018, 44555 | `mako-geli-gas` | BK7-24-01-009 |
| 44022–44053 | `mako-wim-gas` | BK7-24-01-009 |
| 31001–31002, 31004–31008 | `mako-gpke` | BK6-24-174 |
| 31003, 31009 | `mako-wim` (WiM-Rechnung / MSB-Rechnung) | BK6-24-174 |
| 31010–31011 | `mako-gabi-gas` (GaBi Gas MMM INVOIC) | BK7 |
| 17134–17135, 19001–19002 | `mako-gpke` (Konfiguration, BK6-22-024 Teil 4) | BK6-22-024 |

**PIDs that do NOT exist — never register:**
- 44007–44016: not defined in any GeLi Gas AHB
- 56005–56010: former MPES PIDs, not in any current AHB
- 13001: not defined in any MSCONS AHB
- 11004–11099: reserved but not in current WiM AHB

### MPES is dissolved
PIDs 56001–56004 were transferred from MPES into **GPKE** per BK6-22-024 (LFW24),
effective **2025-06-06**. There is no `mako-mpes` crate.

### GeLi Gas 3.0
Governed by **BK7-24-01-009** (Beschluss 12.09.2025). Supersedes BK7-19-001 and BK7-06-067.
Scope: UTILMD G (PIDs 44001–44006, 44017–44018, 44555) **only**.
No INVOIC billing in GeLi Gas — gas MMM billing (31010–31011) belongs to `mako-gabi-gas`.

### MABIS vs Messwesen
Only PID **13003** is MABIS (Bilanzkreisabrechnung Strom, BKV↔ÜNB).
PIDs 13002–13028 (excluding 13003) are Messwesen PIDs — do not register them under MABIS.

### APERAK Fristen — never mix these up

| Process | Deadline | Function | Source |
|---|---|---|---|
| GPKE | **24 wall-clock hours** | `fristen::add_hours(t, 24)` | BK6-22-024 §5 |
| WiM | **5 Werktage** | `fristen::add_werktage(d, 5, BdewMaKo)` | BK6-24-174 |
| GeLi Gas | **10 Werktage** | `fristen::add_werktage(d, 10, BdewMaKo)` | BK7-24-01-009 |

**Saturday = Werktag.** Sunday and public holidays do not count.
All deadline arithmetic uses **German local time (CET/CEST)**, not UTC.
An off-by-one-hour error at DST transitions is a regulatory deadline violation.

### Format-version coexistence
`WorkflowVersionPolicy::ForwardCompatible` is the correct default for **all** MaKo
workflows. Do not default to `Pinned`.

### Dual-write atomicity
Events and outbox entries must be written in a single `WriteBatch` via
`AtomicAppend::append_with_outbox`. Never write events first and outbox second —
a crash between the two produces a lost APERAK with no recovery path.

---

## Licenses

Only these SPDX identifiers are allowed (enforced by `cargo deny`):
MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2-Clause, BSD-3-Clause,
ISC, Unicode-3.0, Zlib, CDLA-Permissive-2.0, MIT-0.

---

## Key Documentation

| Topic | File |
|---|---|
| Process engine guide | [docs/engine.md](../docs/engine.md) |
| `makod` operator guide | [docs/makod.md](../docs/makod.md) |
| ERP integration (Command API, webhooks) | [docs/erp-integration.md](../docs/erp-integration.md) |
| Parsing guide | [docs/parsing.md](../docs/parsing.md) |
| Validation guide | [docs/validation.md](../docs/validation.md) |
| Builder patterns | [docs/builders.md](../docs/builders.md) |
| Annual release workflow | [docs/annual-release-workflow.md](../docs/annual-release-workflow.md) |
| Schema versioning | [docs/schema-versioning.md](../docs/schema-versioning.md) |
| API-Webdienste Strom | [docs/api-webdienste.md](../docs/api-webdienste.md) |
| Release lifecycle | [docs/release-lifecycle.md](../docs/release-lifecycle.md) |
| BNetzA regulatory reference | [docs/bnetza.md](../docs/bnetza.md) |
| PID reference | [docs/pid-reference.md](../docs/pid-reference.md) |
