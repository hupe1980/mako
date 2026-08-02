+++
title = "Release Lifecycle"
description = "BDEW format version lifecycle: active, upcoming, grace-period, and archived states. How mako-engine handles concurrent FV coexistence with WorkflowVersionPolicy::ForwardCompatible."
weight = 11
+++
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
    ├── fv20241001/        # Strom, valid Oct 2024 → Sep 2025 (archived)
    │   ├── mig.json       # Message structure rules
    │   ├── ahb.json       # AHB Pruefidentifikator rules
    │   └── codelists.json # Code list values
    ├── fv20241001_gas/    # Gas variant, same window (archived)
    │   └── ...
    ├── fv20251001/        # Strom, valid Oct 2025 → Sep 2026 (⭐ current production)
    │   └── ...
    ├── fv20251001_gas/    # Gas variant, same window (⭐ current production)
    │   └── ...
    ├── fv20261001/        # Strom, valid Oct 2026 → Sep 2027 (🛠 next release)
    │   └── ...
    └── fv20261001_gas/    # Gas variant, same window (🛠 next release)
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

Place the PDFs in a local working directory.

### 2. Extract profile data

```bash
cargo xtask extract-pdf --file <working-dir>/UTILMD_MIG_S3.1.pdf \
    --message-type utilmd --release fv20271001

cargo xtask extract-pdf --file <working-dir>/UTILMD_AHB_S3.1.pdf \
    --message-type utilmd --release fv20271001
```

> **`pdftotext` is required for AHB extraction.** The AHB rule tables are column
> layouts — a row's `Muss`/`Kann` belongs to whichever Prüfidentifikator column
> it sits under — so the parser needs column-preserved text. `extract-pdf` shells
> out to poppler's `pdftotext -layout` when available and warns when it is not.
> Without it the MIG scan still works, but no Prüfidentifikatoren are found.

The AHB parser reads one requirement per PID column:

| AHB mark | Profile requirement |
|---|---|
| `Muss` | `M` |
| `Kann` | `O` |
| `Soll` | `O` — a recommendation, never promoted |

Two rules are easy to get wrong and are covered by tests:

- **Segment-group nesting does not propagate.** A `Muss` segment inside a `Kann`
  group stays `M`.
- **Optional segments are absent from the AHB table.** The AHB marks what is
  *required*; `mig.json` lists what is *available*. Complete each draft with
  every remaining MIG segment as `O`.

A conditional `Muss [n]` (e.g. "Wenn BGM+7 vorhanden") is reported as `M`; the
XML encodes those as `C` with a `conditional_rules` entry. Review those by hand.

The output directory is derived from `--message-type` and `--release`
(`crates/edi-energy/profiles/utilmd/fv20271001/`). Each run writes
`mig.draft.json` and `ahb.draft.json`. Review the drafts against the PDF, remove
the `_WARNING` fields, and rename them to `mig.json` / `ahb.json` before
continuing.

### 3. Import updated code lists

```bash
cargo xtask import-codelists \
    --file docs/codelists/DE_Qualifier_20271001.csv \
    --message-type utilmd --release fv20271001
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

> **The AHB and the MIG carry independent version numbers.** For most message
> types they differ — ORDERS ships MIG 1.4c alongside AHB 1.1b, MSCONS ships MIG
> 2.5 alongside AHB 3.2. The `release` field holds the BDEW **wire release code**,
> which tracks the MIG. Name the correct document in each file's
> `source_document`: `mig.json` cites the MIG version, `ahb.json` cites the AHB.
> Only UTILMD numbers the two alike (`S2.2`, `G1.2`).

### 5. Validate the profiles

```bash
cargo xtask validate-profiles
```

This runs the JSON Schema checker against all profile files, and verifies **PID
continuity**: a Prüfidentifikator present in one release but missing from its
successor is reported as an error, because messages carrying it would validate
against an empty AHB rule pack. When BDEW genuinely retires a PID, record it in
`RETIRED_PIDS` (in `xtask/src/validate_profiles.rs`) with the AHB version that
dropped it; a PID still published but lost during import belongs in
`KNOWN_IMPORT_GAPS` until a re-import clears it. Fix any reported errors before
proceeding.

### 6. Regenerate source code

```bash
cargo xtask codegen
```

This regenerates all files under `crates/edi-energy/src/generated/`. Never edit these files by hand.

### 7. Verify codegen is stable

```bash
cargo xtask codegen --check
```

Should report `xtask codegen --check: all generated files are up to date.`

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

## Publishing a Crate Release

When all profile and code changes are merged and `just ci` is green:

1. **Bump the workspace version** with `cargo xtask bump-version <X.Y.Z>`.
2. **Create and push a tag**: `git tag vX.Y.Z && git push origin vX.Y.Z`.
3. The `release.yml` GitHub Actions workflow runs automatically:
   - Runs pre-flight `fmt` / `clippy` / `test`, `validate-profiles`,
     `validate-pruefids`, and the `codegen --check` drift gate.
   - Publishes the workspace's library crates to [crates.io](https://crates.io)
     via `cargo publish`, in dependency order.
   - Builds and pushes multi-arch Docker images (`linux/amd64`, `linux/arm64`)
     for each service daemon to `ghcr.io/hupe1980/<service>` (e.g.
     `ghcr.io/hupe1980/makod`) with tags `X.Y.Z`, `X.Y`, and `latest`.
   - Builds and publishes the [`makotest`](@/docs/reference/makotest.md) Python
     package to [PyPI](https://pypi.org/project/makotest/).

### The `makotest` Python package

`makotest` inherits `workspace.package.version` (its `pyproject.toml` declares
`dynamic = ["version"]`), so the same tag releases the crates, the images, and
the wheel at one version — they cannot drift.

Wheels are **abi3-py311**: one wheel per platform serves every Python ≥ 3.11, so
the release matrix is over target platforms (linux x86_64/aarch64, macOS
x86_64/aarch64, Windows x86_64) rather than interpreter versions. Linux wheels
build inside a `manylinux` container so they install on distros older than the
runner.

Publication uses **PyPI Trusted Publishing** (OIDC) — no API token is stored.
PyPI is configured to trust `hupe1980/mako` with workflow `release.yml`; the
`makotest-publish` job requests `id-token: write` to mint the short-lived
credential. The upload sets `skip-existing`, so re-running a partially failed
tag is idempotent rather than a hard failure.

An **sdist** is published alongside the wheels. Because `makotest` depends on
workspace crates by `path`, maturin vendors those crates into the tarball;
CI builds the sdist and installs it into a clean virtualenv on every run, so a
tarball that cannot build standalone fails before a tag is ever cut.

The Docker images are built from the workspace `Dockerfile` (cargo-chef +
distroless) via `docker buildx bake`. See the
[makod Operator Guide](@/docs/services/makod.md#docker-deployment) for image details and
deployment patterns.

To see a human-readable diff between two annual releases (useful for release notes and reviewing spec changes):

```bash
cargo xtask release-diff --message-type utilmd --from fv20251001 --to fv20261001
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

- [Platform Guide](@/docs/reference/platform.md)
- [Validation Guide](@/docs/reference/validation.md)
- [Getting Started](@/docs/guide/getting-started.md)
