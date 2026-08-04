---
description: "Use when working in xtask: adding codegen tasks, import pipelines, validation logic, release tooling, or xtask CLI commands."
applyTo: "xtask/**"
---

# xtask Instructions

## General

- `xtask` is a binary crate — `anyhow` is acceptable here (no `thiserror` required).
- MSRV still applies: 1.94. Do not use features introduced after 1.94.
- Each task is a separate source file under `xtask/src/`. Add new tasks there and wire them into `main.rs`.

## Codegen Pipeline

The codegen task reads profile JSON from `crates/edi-energy/profiles/<message_type>/<fvYYYYMMDD>/{mig.json, ahb.json, codelists.json}` and emits Rust source into `crates/edi-energy/src/generated/`. The generated files are committed to the repo.

```bash
cargo xtask codegen              # regenerate after any profile YAML change
cargo xtask validate-profiles    # validate profiles against EDIFACT specs (lenient)
cargo xtask validate-pruefids    # validate PIDs against AHB (lenient)
```

`just ci` runs `validate-profiles-strict` and `validate-pruefids-strict-ci` — these are the gates that fail CI.

## Profile YAML Schema

A profile YAML defines segments, elements, composites and AHB qualifier constraints for one `(MessageType, FormatVersion, Pruefidentifikator)` triple. Key rules:
- `format_version` must be a valid `FV<YYYY>-<MM>-<DD>` string.
- `pruefidentifikator` must match the PID ownership table (see global instructions).
- Cardinality is expressed as `M`/`O`/`R` per the AHB column definitions.
- Never invent PIDs not defined in the AHB — validate against `cargo xtask validate-pruefids`.

## Release Tooling

```bash
cargo xtask bump-version X.Y.Z       # bumps [workspace.package].version across all Cargo.toml
cargo xtask check-release-coverage   # every message type has a non-archived profile covering the reference date
cargo xtask release-diff             # diffs profiles between two format versions
```

`check-release-coverage` verifies that for the reference date (`--date YYYY-MM-DD`,
defaulting to today) every message type is covered by exactly one non-archived profile
span, reporting any coverage gaps.

## Import Tasks

```bash
cargo xtask import-codelists    # imports BDEW code lists (CSV/XLSX from regulatories/)
cargo xtask import-xml-ahb      # imports AHB rules from BDEW XML exports
cargo xtask extract-pdf         # extracts tables from BDEW PDFs
cargo xtask validate-extraction # measures a draft against the curated profile beside it
```

**A draft is never a drop-in profile.** `validate-extraction` classifies every
Prüfidentifikator as `exact`, `superset`, `subset` or `differs` against the
curated `ahb.json`. A `superset` marks more segments mandatory than the AHB
requires, so promoting it **rejects valid messages** — worse than the vacuous
validation an unimported PID has. The UTILMD draft is currently `exact` for 2 of
104 PIDs and a superset for the other 102.

These tasks write into `crates/edi-energy/profiles/` — always run `cargo xtask codegen` and `cargo xtask validate-profiles` after importing.

