---
description: "Use when working in xtask: the profile importer, validation gates, release tooling, or xtask CLI commands."
applyTo: "xtask/**"
---

# xtask Instructions

## General

- `xtask` is a binary crate — `anyhow` is acceptable here (no `thiserror` required).
- MSRV still applies: 1.94. Do not use features introduced after 1.94.
- Each task is a separate source file under `xtask/src/`. Add new tasks there and wire them into `main.rs` and `HELP`.

## Profile pipeline

The profiles under `crates/edi-energy/profiles/<type>/<fvYYYYMMDD>/{mig.json, ahb.json}` are **generated from the BDEW PDFs**, never edited by hand.

```bash
cargo xtask sync-regulatories --download   # mirror the BDEW documents into regulatories/bdew-mako/
cargo xtask import-profiles                # every profile in profiles/sources.json, from its MIG + AHB PDF
cargo xtask import-profiles --profile utilmd/fv20261001
cargo xtask import-profiles --check        # a committed profile drifted from its PDF (SKIPs without the mirror)
cargo xtask pdf-grid <pdf>                 # the character grid the importer reads, for debugging a table
cargo xtask validate-profiles              # sources ↔ directories, dates and continuity, Prüfidentifikatoren, AHB rows ↔ MIG
cargo xtask check-pid-coverage             # the shipped columns against the published Prüfidentifikator inventory
```

`xtask/src/bdew/` holds the readers: `mod.rs` renders `pdftotext -bbox-layout` onto a character grid, `mig.rs` reads the Nachrichtenstruktur and the Segmentlayouts, `ahb.rs` reads one Prüfschablone per AHB column. `BDEW_DEBUG=1` traces every row the AHB reader assigns. A new format version is one entry in `profiles/sources.json` (release, dates, AHB version, the two PDF file names) followed by `import-profiles`.

Every parser change must keep `cargo test -p edi-energy --all-features --test skeletons` at 100 %: the skeleton of every Anwendungsfall validates against its own Prüfschablone, which is the witness that extraction and validator agree.

## Release tooling

```bash
cargo xtask bump-version X.Y.Z       # bumps [workspace.package].version across all Cargo.toml
cargo xtask check-release-coverage   # every message type has a profile covering the reference date
```

`check-release-coverage` verifies that for the reference date (`--date YYYY-MM-DD`, defaulting to today) every message type is covered by exactly one profile span, reporting any gaps.
