+++
title = "Release Lifecycle"
description = "The BDEW format-version lifecycle — active, upcoming and archived — and how mako-engine runs two Formatversionen side by side without splitting a process."
weight = 11
+++
How a BDEW release enters `edi-energy`, which gates hold it, and what a mako release ships.

---

## BDEW Release Cycle

Cutovers are **staggered per message type**: each format version carries its own `valid_from`.

| Event | Timing |
|---|---|
| BDEW publishes a new specification | on the Änderungsmanagement schedule, **six months** before its Anwendungszeitpunkt (Allgemeine Festlegungen 6.1d §2.5.1/§2.5.2); a deviating date names the Mitteilung that set it (`publikation_quelle`) |
| The specification becomes **valid** | its own `valid_from` — **January 1**, **April 1** or **October 1** (e.g. `fv20260101`, `fv20260401`, `fv20261001`) |
| The predecessor **expires** | the day before its successor's `valid_from` |
| Transition window (both valid) | **none** — EDIFACT changes at a single Anwendungszeitpunkt (Allgemeine Festlegungen 6.1 §2.5) |

Each profile's `valid_from` / `valid_until` bound when it is acceptable, and
the leading edge is hard; see [Transition Window Handling](#transition-window-handling)
for the trailing edge. A running instance normally holds several format versions
at once — the rollout steps are in the
[Annual Release Workflow](@/docs/compliance/annual-release-workflow.md).

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
# 1. Mirror the new BDEW documents into the local document mirror
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
| `tests/it/skeletons.rs` | every Anwendungsfall's skeleton validates against its own Prüfschablone |
| `tests/it/validation_snapshot.rs` | the verdict of every fixture, rule id by rule id |
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
   - Runs pre-flight `fmt` / `clippy` / `test` and checks the tag equals the
     workspace version and the publish order resolves; beside it, the
     `guards` job runs every `just ci` recipe that is one `cargo xtask` call
     (`validate-profiles` among them). Nothing is published unless both pass.
   - Runs `just semver` (`cargo-semver-checks` against the previous `v*` tag,
     over every crate the workflow publishes): a public-API break the new
     version number does not admit stops the release before anything is
     published. It runs only here, not in `just ci`.
   - Publishes the 24 published library crates (every crate under `crates/`
     but `mako-service`, which is `publish = false`) to [crates.io](https://crates.io)
     with `cargo publish`, in dependency order, by Trusted Publishing.
   - Builds and pushes multi-arch Docker images (`linux/amd64`, `linux/arm64`)
     for each service daemon to `ghcr.io/hupe1980/mako-<service>` (e.g.
     `ghcr.io/hupe1980/mako-makod`) with tags `X.Y.Z`, `X.Y`, and `latest`,
     and attests each image and each binary inside it.
   - Builds and publishes the [`makotest`](@/docs/reference/makotest.md) Python
     package to [PyPI](https://pypi.org/project/makotest/).
   - Creates the GitHub Release `vX.Y.Z` carrying the evidence pack, the
     CycloneDX SBOMs and the VEX document.

### What a release carries

A release answers two questions for an operator, an auditor or an incident
responder: what is in it, and who built it. Each answer is an artefact a third
party checks without trusting this repository.

| Artefact | Where | How to check it |
|---|---|---|
| **Dependency manifest in every binary** | each service binary, `.dep-v0` section (`cargo auditable`) | `rust-audit-info <binary>` or `cargo audit bin <binary>` on a binary copied out of the image |
| **Image provenance and SBOM** (BuildKit, `provenance: mode=max`, `sbom: true`) | the image index on ghcr.io | `docker buildx imagetools inspect <image>` lists the attestation manifests |
| **CycloneDX SBOM per service binary** | GitHub Release assets `<service>_bin.cdx.json` | any CycloneDX reader |
| **Build-provenance and SBOM attestations** for each image and each binary | GitHub's attestation store and, for images, the registry | `gh attestation verify oci://ghcr.io/hupe1980/mako-makod:X.Y.Z --repo hupe1980/mako`; add `--predicate-type https://cyclonedx.org/bom` for the SBOM |
| **VEX** for every advisory `deny.toml` ignores | GitHub Release asset `vex.json` (OpenVEX) | any OpenVEX consumer, e.g. a scanner reading the SBOM |
| **Evidence pack** | GitHub Release asset `evidence-vX.Y.Z.json`, itself attested | `gh attestation verify evidence-vX.Y.Z.json --repo hupe1980/mako` |

**The manifest survives `strip`.** `cargo auditable` writes the dependency list
into a `.dep-v0` section, and the Dockerfile's GNU `strip` keeps it. Each
builder stage reads the manifest back from every stripped binary with
`rust-audit-info` and fails the image build when one is absent; the release
reads it once more from the binaries it extracts from the pushed images.

**The SBOMs.** `cargo cyclonedx` describes each service binary from the
lockfile's resolved graph, without build-time dependencies, and `processd`
with the `integrated` feature the image builds it with. Regulation (EU)
2024/2847 Annex I Part II point (1) asks a manufacturer to draw up „a software
bill of materials in a commonly used and machine-readable format covering at
the very least the top-level dependencies"; Annex VII point 2(b) places it in
the technical documentation, and Annex VII point 8 provides it to a market
surveillance authority on reasoned request. Whether those obligations bind mako
depends on its role under the Regulation, which this page does not determine.

**VEX.** `release/vex.json` carries one OpenVEX statement per RustSec ID in
`deny.toml`'s `[advisories] ignore` list — status, justification and impact
statement — so a scanner does not re-raise an advisory the project has
already assessed. `cargo xtask check-vex` holds the two lists equal in both
directions, refuses a subcomponent version that neither `Cargo.lock` nor
`fuzz/Cargo.lock` holds and, where a local RustSec database is present, refuses
a statement naming a different crate than the advisory does. `fuzz/` lies
outside the workspace, so `just deny` (and the CI `deny` job) runs the
advisory check over its lock separately, under the same `deny.toml`. The Regulation does not name
VEX; it is the machine-readable form chosen here for documenting how a known
vulnerability is addressed (Annex I Part II point (2)).

**Who built it.** Every attestation is a keyless Sigstore bundle signed with
the release job's OIDC identity: repository, workflow file and commit are in
the certificate, and no signing key exists to leak. GitHub's documentation
(*Artifact attestations*) states that artifact attestations alone provide SLSA
v1.0 Build Level 2.

**Publishing without a stored secret.** crates.io trusts repository
`hupe1980/mako`, workflow `release.yml` and environment `crates-io` for each
crate. `rust-lang/crates-io-auth-action` exchanges the job's OIDC token for a
crates.io token valid for 30 minutes, revoked when the job ends; because 24
verified publishes take longer than that, each crate mints its own immediately
before it publishes. The workflow reads no repository secret to publish. Trusted
Publishing takes over an existing crate only: a new crate is published once by
hand, then given a trusted publisher on crates.io, and the release refuses a
crate crates.io does not know yet, by name, before uploading anything. PyPI
works the same way (below).

**One compiler.** `rust-toolchain.toml` names the toolchain. Every workflow job
and the Docker `chef` stage assert that the active `rustc` is that channel and
fail otherwise, so an override that would compile a release with a different
compiler cannot pass silently.

**Pinned supply chain.** Every third-party action is pinned to a full commit
SHA with its release tag in a trailing comment, every Dockerfile base image to
an `@sha256:` digest, and every workflow grants `contents: read` by default with
each job declaring what more it needs. Python packages a workflow installs come
from hash-pinned requirements files under `.github/requirements/`
(`pip install --require-hashes`), the job's own built wheel or sdist is installed
`--no-deps`, and the site tooling installs with `npm ci` against the tracked
`site/package-lock.json`. `cargo xtask check-action-pins` refuses a
tag-pinned `uses:`, a digest-less `FROM`, a wider workflow default, an
unpinned `pip install` and an `npm install`; a pin moves only in a reviewed
change. OpenSSF Scorecard scores the repository weekly
(`scorecard.yml`), publishes the result and fails below a recorded floor.

#### The evidence pack

`cargo xtask evidence-pack` assembles `evidence-vX.Y.Z.json` in the last release
job from what the earlier jobs produced:

| Section | Content |
|---|---|
| `fidelity` | `DAMAGE_BUDGET` and `SILENT_PERMIT_BUDGET` from `extraction_fidelity.rs`, and the values the test measured on the release commit |
| `outbound_sweep` | the outbound conformance sweep's per-PID result |
| `guards` | every recipe of `just ci`: an exit status for each recipe that is one `cargo xtask` call (run by the `guards` job) and for `fmt-check`, `clippy` and `check` (run by `preflight`); `not measured` for the `_ci` recipes the release does not run |
| `mutation_score` | the mutation score |
| `release_lead_time` | the lead time from BDEW publication to a shipped profile |
| `sbom` | the SHA-256 of each CycloneDX document |
| `attestations` | per service: the image index digest, the binary digests and the URL of each attestation |

`outbound_sweep`, `mutation_score` and `release_lead_time` have no producer in
the release workflow, so every pack reads `not measured` for them.

**A section without its producer reads `not measured`, never a zero**, with a
`reason` and no figure. `cargo xtask evidence-pack --check` refuses
a pack with a section absent, a status other than `measured` or `not measured`,
a `measured` section without its figures, a fidelity figure over budget, or a
guard that exited non-zero — and the GitHub Release is not created from a
refused pack. The verdicts themselves are taken before anything is published:
`publish`, `makotest-publish` and `docker-bake` need the `guards` job, which
fails on any non-zero guard, so a refused guard stops the release before a
crate, a wheel or an image leaves it.

### The `makotest` Python package

`makotest` inherits `workspace.package.version` (its `pyproject.toml` declares
`dynamic = ["version"]`), so the same tag releases the crates, the images, and
the wheel at one version — they cannot drift.

Wheels are **abi3-py311**: one wheel per platform serves every Python ≥ 3.11, so
the release matrix is over target platforms (linux x86_64/aarch64, macOS
x86_64/aarch64, Windows x86_64) rather than interpreter versions. Linux wheels
build inside a `manylinux` container so they install on distros older than the
runner.

Each matrix entry passes its `target` to `maturin build`; the Intel macOS
wheel is cross-compiled from the Apple Silicon runner, since an abi3 extension
does not link libpython.

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

`cargo xtask profile-diff utilmd fv20251001 fv20261001 [--pid 55001]` reads
the delta between two releases (see [CI Gates](#ci-gates)); places are named by
position and MIG name, because the MIG renumbers its segments between
Nachrichtentypversionen.

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

---

## Transition Window Handling

**The EDIFACT cut-over has no Übergangsfrist** (Allgemeine Festlegungen 6.1
§2.5): the day the successor applies, the predecessor stops being acceptable.
The 15-Werktage Übergangszeitraum of § 8.5 is the **XML** rule — it counts
Werktage from the Anwendungszeitpunkt and selects by the Erfüllungsdatum in the
message. `DEFAULT_RECEIVE_TOLERANCE_DAYS` is therefore `0`; an operator who
accepts late-arriving messages in the superseded format sets an inbound
trailing-edge tolerance:

```rust
use edi_energy::Platform;

let platform = Platform::with_all_profiles().with_receive_tolerance_days(3);
```

`ParseConfig::with_reference_date()` reproduces the exact profile selection for
any historical date:

```rust
use edi_energy::{ParseConfig, Parser};
use time::macros::date;

// Select profiles as they stood on 3 October 2026
let config = ParseConfig::default().with_reference_date(date!(2026-10-03));
let msg = Parser::with_config(config).parse(bytes)?;
```

---

## See Also

- [Platform Guide](@/docs/reference/platform.md)
- [Validation Guide](@/docs/reference/validation.md)
- [Getting Started](@/docs/guide/getting-started.md)
