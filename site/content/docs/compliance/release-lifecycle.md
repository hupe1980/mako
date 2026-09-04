+++
title = "Release Lifecycle"
description = "BDEW format version lifecycle: active, upcoming, and archived states. How mako-engine handles concurrent FV coexistence with WorkflowVersionPolicy::ForwardCompatible."
weight = 11
+++
# Annual BDEW Release Lifecycle

EDI@Energy specifications are updated on a recurring cycle. This document describes how new BDEW releases are incorporated into `edi-energy`, how they are rolled out across the platform, and what the `xtask` automation covers.

---

## BDEW Release Cycle

Cutovers are **staggered per message type**, not synchronised on one annual date. A given format version carries its own `valid_from`, and different message types move on different dates in the same year.

| Event | Timing |
|---|---|
| BDEW publishes a new specification | **six months** before its Anwendungszeitpunkt — published 01.04., applies 01.10.; published 01.10., applies 01.04. (Allgemeine Festlegungen 6.1d §2.5.1/§2.5.2) |
| The specification becomes **valid** | its own `valid_from` — **January 1**, **April 1** or **October 1** (e.g. `fv20260101`, `fv20260401`, `fv20261001`) |
| The predecessor **expires** | the day before its successor's `valid_from` |
| Transition window (both valid) | **none** — EDIFACT changes at a single Anwendungszeitpunkt (Allgemeine Festlegungen 6.1 §2.5) |

`edi-energy` enforces this via `valid_from` / `valid_until` metadata in each profile JSON: a release is acceptable from its `valid_from` up to and including its `valid_until`, and the leading edge is hard. `ReleaseRegistry::with_receive_tolerance_days(n)` extends the *trailing* edge for an operator who chooses to accept a late-arriving message in the superseded format — a local receiving policy, defaulting to zero.

The 15-Werktage Übergangszeitraum in Allgemeine Festlegungen §8.5 is the **XML** rule: it begins at the Anwendungszeitpunkt, counts Werktage, and selects the version by the Erfüllungsdatum stated in the message. It does not apply to the EDIFACT formats.

Because the cutover dates differ per message type, a running instance normally holds several format versions valid at once — see [Annual Release Workflow](@/docs/compliance/annual-release-workflow.md) for the step-by-step rollout and its appendices.

---

## Profile Directory Structure

```
crates/edi-energy/profiles/
├── sources.json           # every profile: release, dates, AHB version, its MIG and AHB PDF
├── pid-overview.json      # the published Prüfidentifikator inventory (check-pid-coverage)
└── utilmd/
    ├── fv20251001/        # Strom S2.1, valid Oct 2025 → Sep 2026
    │   ├── mig.json       # Nachrichtenstruktur + Segmentlayouts, by Nr
    │   └── ahb.json       # one Prüfschablone per Anwendungsfall, by Nr
    ├── fv20260401_gas/    # Gas G1.1, valid Apr 2026 → Sep 2026
    ├── fv20261001/        # Strom S2.2, from Oct 2026
    └── fv20261001_gas/    # Gas G1.2, from Oct 2026
```

Every profile subdirectory follows the naming convention `fv<YYYYMMDD>`, where the date is the **Anwendungszeitpunkt** — never the Publikationsdatum printed on the document. The files are generated from the PDFs and never edited by hand; see [Profile Files](@/docs/compliance/schema-versioning.md).

---

## Adding a Release

```bash
# 1. Mirror the new documents (regulatories/ is gitignored)
cargo xtask sync-regulatories --download

# 2. Name the profile: release, dates, AHB version, the two PDFs
$EDITOR crates/edi-energy/profiles/sources.json

# 3. Generate it from the PDFs, then read the delta against the predecessor
cargo xtask import-profiles --profile utilmd/fv20271001
cargo xtask profile-diff utilmd fv20261001 fv20271001

# 4. The generated profile validates its own skeletons — every column's minimal
#    message against its own Prüfschablone — and the corpus still holds
cargo test -p edi-energy --all-features

# 5. The committed profiles are consistent, and the inventory is covered
cargo xtask validate-profiles
cargo xtask check-pid-coverage
cargo xtask check-release-coverage --date 2027-10-01
```

`import-profiles` fails on a table it cannot read rather than guessing: a row whose statuses land between columns, a segment the AHB names that the MIG structure lacks, a column without `UNH`, a Bedingung a status expression cites but whose text the reader did not find. `BDEW_DEBUG=1` traces every row the AHB reader assigns and `cargo xtask pdf-grid <pdf>` dumps the character grid it reads, which is how a new layout quirk is found. A Prüfidentifikator that leaves between two releases is an import regression unless `validate-profiles`' `RETIRED_PIDS` records BDEW's retirement.

The predecessor gets its `valid_until` (the day before the successor's `valid_from`) in `sources.json`; `validate-profiles` refuses a gap, an overlap and a chain whose newest profile is not open-ended. Nothing is archived: a profile stays compiled for as long as it is in `sources.json`, and is deleted when no deployment can still receive its format.

---

## CI Gates

| Gate | What it holds |
|---|---|
| `tests/skeletons.rs` | every Anwendungsfall's skeleton validates against its own Prüfschablone |
| `tests/validation_snapshot.rs` | the verdict of every fixture, rule id by rule id |
| `validate-profiles` | `sources.json` ↔ files, dates and continuity, Prüfidentifikatoren, AHB rows ↔ MIG structure, every cited Bedingung has its text |
| `import-profiles --check` | a committed profile against its PDF — SKIPs where the mirror is absent |
| `check-pid-coverage` | the shipped columns against BDEW's Anwendungsübersicht |
| `check-release-coverage` | a profile covers the reference date for every message type |

`cargo xtask profile-diff <type> <from-fv> <to-fv>` is the reader's half of the same
set: per Prüfidentifikator, which appeared or were withdrawn, which places changed
their status, which codes gained or lost an operand, and which Bedingungen and
Pakete were rewritten. It reads the committed profiles alone, so it runs in the
release PR without the document mirror.

---

## Publishing a Crate Release

When all profile and code changes are merged and `just ci` is green:

1. **Bump the workspace version** with `cargo xtask bump-version <X.Y.Z>`.
2. **Create and push a tag**: `git tag vX.Y.Z && git push origin vX.Y.Z`.
3. The `release.yml` GitHub Actions workflow runs automatically:
   - Runs pre-flight `fmt` / `clippy` / `test` and `validate-profiles`.
   - Publishes the workspace's library crates to [crates.io](https://crates.io)
     via `cargo publish`, in dependency order.
   - Builds and pushes multi-arch Docker images (`linux/amd64`, `linux/arm64`)
     for each service daemon to `ghcr.io/hupe1980/mako-<service>` (e.g.
     `ghcr.io/hupe1980/mako-makod`) with tags `X.Y.Z`, `X.Y`, and `latest`.
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

Each matrix entry passes its `target` to `maturin build` explicitly rather than
relying on the runner's host architecture. The Intel macOS wheel is
**cross-compiled from the Apple Silicon runner**: because the extension is abi3
and `pyo3/extension-module` leaves the Python symbols undefined, nothing links
against libpython, so no Intel runner or Intel interpreter is required.

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

To read the delta between two annual releases — for the release PR's summary
and for reviewing what the Festlegung changed:

```bash
cargo xtask profile-diff utilmd fv20251001 fv20261001
cargo xtask profile-diff utilmd fv20251001 fv20261001 --pid 55001
```

It lists which Prüfidentifikatoren appeared or were withdrawn and, per
Prüfidentifikator, the places whose status changed, the codes that gained or
lost an operand, and the Bedingungen and Pakete that were rewritten. Places are
named by where they sit and what the MIG calls them, because the MIG renumbers
its segments between Nachrichtentypversionen.

---

## How a profile is applied

There is no code generation. `mig.json` and `ahb.json` are embedded at build
time and read as data:

- `Profile::validate` resolves each segment of a message to its place in the
  Nachrichtenstruktur, applies the MIG's own checks, then the selected column's
  Prüfschablone, evaluating each Bedingung against the message.
- `Profile::skeleton` runs the validator's findings to a fixpoint to produce the
  minimal conformant message of a column, and `Profile::complete` does the same
  seeded with a caller's message.
- A profile is loaded once per process through `LazyLock`, so repeated
  validation does not re-parse the JSON.

Adding a message type or a Formatversion is therefore an import and a
`sources.json` entry — never a generated-source review.

A profile stays compiled in for as long as `sources.json` names it, and is
deleted when no deployment can still receive its format. Nothing is archived
behind a feature flag: a Formatversion is either shipped or gone.

---

## Transition Window Handling

**The EDIFACT cut-over has no Übergangsfrist.** A message is validated against
the profile in force on its date; the day the successor applies, the predecessor
stops being acceptable. The 15-Werktage Übergangszeitraum of Allgemeine
Festlegungen § 8.5 is the **XML** rule and does not transfer — it starts at the
Anwendungszeitpunkt, counts Werktage rather than calendar days, and selects by
the Erfüllungsdatum stated in the message rather than by when it was sent.
`DEFAULT_RECEIVE_TOLERANCE_DAYS` is therefore `0`.

An operator who chooses to accept a late-arriving message in the superseded
format sets its own trailing-edge tolerance. It is an inbound policy, not a
licence to send late:

```rust
use edi_energy::Platform;

let platform = Platform::with_all_profiles().with_receive_tolerance_days(3);
```

`ParseConfig::with_reference_date()` reproduces the exact profile selection for
any historical date:

```rust
use edi_energy::{parse_with_config, ParseConfig};
use time::macros::date;

// Select profiles as they stood on 3 October 2026
let config = ParseConfig::new().with_reference_date(date!(2026-10-03));
let msg = parse_with_config(bytes, config)?;
```

---

## See Also

- [Platform Guide](@/docs/reference/platform.md)
- [Validation Guide](@/docs/reference/validation.md)
- [Getting Started](@/docs/guide/getting-started.md)
