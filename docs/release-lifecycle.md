# Annual BDEW Release Lifecycle

EDI@Energy specifications are updated annually. This document describes how new BDEW releases are incorporated into the library and what the `xtask` automation covers.

---

## BDEW Release Cycle

| Event | Date |
|---|---|
| BDEW publishes new specifications | ~ August each year |
| Specifications become **valid** | **October 1** (e.g. `fv20261001`) |
| Previous specifications **expire** | September 30 of the same year |
| Transition window (both valid) | **± 7 days** around Oct 1 |

The library enforces this via `valid_from` / `valid_until` metadata in each profile JSON and the `TRANSITION_GRACE_DAYS = 7` constant.

---

## Profile Directory Structure

```
crates/edi-energy/profiles/
└── utilmd/
    ├── fv20241001/        # Strom, valid Oct 2024 → Sep 2025
    │   ├── mig.json       # Message structure rules
    │   ├── ahb.json       # AHB Pruefidentifikator rules
    │   └── codelists.json # Code list values
    ├── fv20241001_gas/    # Gas variant, same window
    │   └── ...
    └── fv20261001/        # Strom, valid Oct 2026 → Sep 2027
        └── ...
```

Every profile subdirectory follows the naming convention `fv<YYYYMMDD>` where the date is the first day of validity.

---

## Step-by-Step: Adding a New Annual Release

### 1. Download BDEW PDFs

Download the new specification PDFs from [edi-energy.de](https://www.edi-energy.de/):

- UTILMD-Strom MIG + AHB (German: *Nachrichtenstruktur, Anwendungshandbuch*)
- UTILMD-Gas MIG + AHB
- MSCONS MIG + AHB
- etc.

Place the PDFs in `docs/pdfs/`.

### 2. Extract profile data

```bash
cargo xtask extract-pdf --input docs/pdfs/UTILMD_MIG_S3.1.pdf \
    --output crates/edi-energy/profiles/utilmd/fv20271001/mig.json

cargo xtask extract-pdf --input docs/pdfs/UTILMD_AHB_S3.1.pdf \
    --output crates/edi-energy/profiles/utilmd/fv20271001/ahb.json
```

### 3. Import updated code lists

```bash
cargo xtask import-codelists \
    --input docs/codelists/DE_Qualifier_20271001.csv \
    --profile crates/edi-energy/profiles/utilmd/fv20271001/
```

### 4. Update `valid_from` / `valid_until` in the JSON

In `mig.json`:

```json
{
  "valid_from":  "2027-10-01",
  "valid_until": "2028-09-30",
  "source_document": "UTILMD-Strom MIG S3.1, BDEW, 2027"
}
```

Update the *previous* release's `valid_until` to `"2027-09-30"` as well.

### 5. Validate the profiles

```bash
cargo xtask validate-profiles
```

This runs the JSON Schema checker against all profile files. Fix any reported errors before proceeding.

### 6. Regenerate source code

```bash
cargo xtask codegen
```

This regenerates all 37 files under `crates/edi-energy/src/generated/`. Never edit these files by hand.

### 7. Verify codegen is stable

```bash
cargo xtask codegen --check
```

Should report `All generated files are up to date.`

### 8. Run the test suite

```bash
cargo test --all-features
cargo xtask validate-profiles
cargo xtask validate-pruefids
```

### 9. Add fixtures

Add at least one `.edi` fixture file for each new PID under `crates/edi-energy/tests/fixtures/<type>/valid/`.

```bash
# Verify fixture coverage
cargo xtask validate-pruefids --message-type utilmd
```

---

## Release Diff

To see a human-readable diff between two annual releases (useful for release notes and reviewing spec changes):

```bash
cargo xtask release-diff --from utilmd/fv20241001 --to utilmd/fv20261001
```

Output shows:

- New / removed Pruefidentifikatoren
- Changed mandatory/conditional/forbidden rules
- New / removed code list entries
- `valid_from` / `valid_until` boundary changes

---

## Codegen Architecture

The code generator (`xtask/src/codegen.rs`) reads the AHB JSON profiles and emits Rust source for each message type. Key design decisions:

- **Inline closures** — each AHB rule is emitted as a Rust closure, eliminating the need for a reflection-style string-keyed rule registry.
- **Shared helpers per module** — `ahb_check_mandatory`, `ahb_check_not_used`, `ahb_check_qualifier`, etc. are emitted once per generated file with `#[allow(dead_code)]` to suppress unused-function warnings for profiles that don't exercise every helper.
- **Union pack via `merge()`** — per-PID packs are merged into a union pack at initialization time using checked `merge().expect()` so the merge invariant is explicit.
- **`LazyLock` caching** — rule packs are initialized once per process via `std::sync::LazyLock` so repeated `validate()` calls do not re-parse JSON.

---

## CI Gates

| Gate | Command | Purpose |
|---|---|---|
| Codegen drift | `cargo xtask codegen --check` | Prevents unreviewed profile changes |
| Profile JSON validity | `cargo xtask validate-profiles` | Catches schema violations |
| PID fixture coverage | `cargo xtask validate-pruefids` | Ensures every PID has a test |
| Semver check | `cargo semver-checks` | Prevents accidental API breaks |
| Changelog | Check `[Unreleased]` non-empty | Enforces documentation discipline |

### Annual maintenance

After each BDEW cycle, archive profiles that have passed their grace window:

```bash
cargo xtask codegen --prune-expired   # sets "archived": true in expired mig.json files
cargo xtask codegen --check           # confirm mod.rs is up to date
```

Archived profiles are hidden behind `{type}-archive` / `archive` Cargo features and do not
inflate compile time for standard deployments.  See `docs/schema-versioning.md` for the
full policy.

---

## Transition Window Handling

Messages dated within 7 days of a profile boundary are accepted by both the outgoing and incoming profile. This matches BDEW practice for handling messages sent just before or just after October 1.

The `ParseConfig::with_reference_date()` API lets you reproduce the exact profile selection for any historical date:

```rust
use edi_energy::{parse_with_config, ParseConfig};
use time::macros::date;

// Simulate parsing as it would behave on Oct 3, 2026
let config = ParseConfig::new().with_reference_date(date!(2026-10-03));
let msg = parse_with_config(bytes, config)?;
```

---

## See Also

- [Platform Guide](./platform.md)
- [Validation Guide](./validation.md)
- [Getting Started](./getting-started.md)
