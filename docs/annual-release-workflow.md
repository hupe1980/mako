---
layout: default
title: Annual Release Workflow
nav_order: 12
parent: Release & Compliance
description: >
  Step-by-step engineering playbook for incorporating a new BDEW annual
  release: extract-pdf, import-xml-ahb, codegen, validate-pruefids, add-release,
  check-release-coverage.
---

# Annual Release Workflow

This document is the step-by-step engineering playbook for incorporating a new
BDEW annual release into the `mako` workspace.  Follow the steps in order for every
October 1 rollover cycle.

---

## Prerequisites

- `cargo xtask` is the primary tool for every step.  Build it once before starting:
  ```
  cargo build -p xtask
  ```
- You need the new BDEW PDF specification files from
  **[bdew-mako.de](https://www.bdew-mako.de/market_communication/documents)**
  (German).  Download the MIG and AHB PDFs for each message type that changed.

---

## Step 1 — Extract draft profiles from PDFs

Run the PDF extractor for each **changed** message type:

```bash
cargo xtask extract-pdf --message-type <TYPE> --release <fvYYYYMMDD> \
    --mig-pdf path/to/UTILMD_MIG_S2.3.pdf \
    --ahb-pdf path/to/UTILMD_AHB_S2.3.pdf
```

This creates **draft** JSON files in `profiles/<type>/<fvYYYYMMDD>/`:
```
profiles/utilmd/fv20271001/mig.draft.json
profiles/utilmd/fv20271001/ahb.draft.json
profiles/utilmd/fv20271001/codelists.json   ← if codelists changed
```

### ⚠ Mandatory extraction quality check

Before continuing, verify that the extraction produced reasonable output:

```bash
python3 -c "
import json
for f in ['mig.draft.json', 'ahb.draft.json']:
    with open(f'profiles/utilmd/fv20271001/{f}') as fp:
        d = json.load(fp)
    segs = len(d.get('segments', []))
    pids = len(d.get('pruefidentifikatoren', []))
    print(f'{f}: {segs} segments, {pids} PIDs')
"
```

**Expected minimum counts** (adjust per message type):

| Message type | Min MIG segments | Min AHB PIDs |
|---|---|---|
| UTILMD | 40 | 15 |
| MSCONS | 25 | 5 |
| APERAK | 15 | 2 |
| CONTRL | 8 | – |
| INVOIC | 30 | 8 |
| REMADV | 20 | 5 |

If counts are below threshold, the PDF layout changed.  Edit the extractor
heuristics in `xtask/src/extract_pdf.rs` and re-run.  Do **not** promote a
partial draft to production.

---

## Step 2 — Manual review and editing

Open the draft files alongside the BDEW PDF specification and review each entry:

1. **`mig.draft.json`** — Verify segment order, cardinality (`max_occurrences`),
   and group membership against the MIG table in the PDF.
2. **`ahb.draft.json`** — Check each Prüfidentifikator's `segment_rules`.
   Pay special attention to changed `requirement` codes (`M`/`S`/`C`/`N`/`O`/`X`)
   and conditional rule operators (`I`/`V`/`E`/`X`/`U`/`O`/`G`/`Z`).
3. **`codelists.json`** — Verify code additions/removals against the AHB annex.

The extractor embeds `"_WARNING"` fields in draft output.  Remove all `_WARNING`
fields before promoting.

**Typical review time:**
- Minor update (codelists only): 30 minutes
- Full MIG/AHB update: 2–3 hours per message type

---

## Step 3 — Promote drafts to production

Rename draft files to production names:

```bash
mv profiles/utilmd/fv20271001/mig.draft.json profiles/utilmd/fv20271001/mig.json
mv profiles/utilmd/fv20271001/ahb.draft.json profiles/utilmd/fv20271001/ahb.json
```

Set the `valid_from` date in `mig.json`.  If the previous release now has a
known expiry date, set `valid_until` on **that** file as well:

```json
// profiles/utilmd/fv20261001/mig.json — add or confirm:
"valid_until": "2027-09-30"

// profiles/utilmd/fv20271001/mig.json:
"valid_from": "2027-10-01"
```

> **Rule:** every profile that is superseded by a new one **must** have a
> `valid_until` date.  Open-ended profiles (`valid_until` absent) are treated
> as permanently valid by the registry.

---

## Step 4 — Validate profiles

```bash
cargo xtask validate-profiles --message-type <TYPE>
```

Fix every reported violation before continuing.  Common errors:
- Code values referenced in AHB qualifier rules that do not exist in `codelists.json`
- `element_index` values that exceed the segment's element count
- PID codes outside the valid range 10000–99999
- `_WARNING` fields still present (marks incomplete extraction)

---

## Step 5 — Regenerate code

```bash
cargo xtask codegen
```

This rewrites all `src/generated/*.rs` files and `src/generated/mod.rs`.

Verify the file count increased as expected:
```bash
ls crates/edi-energy/src/generated/*.rs | wc -l
```

---

## Step 6 — Verify CI drift gate

```bash
cargo xtask codegen --check
```

This regenerates in memory and compares against committed files.  Must exit 0.
If it exits 1, you have uncommitted changes or a codegen inconsistency — check
`git diff crates/edi-energy/src/generated/`.

---

## Step 7 — Compile and test

```bash
RUSTFLAGS='-D warnings -D deprecated' cargo check --all-targets --all-features
cargo test --all-features
```

Both must succeed with zero errors.

---

## Step 8 — Run release-diff for the PR audit trail

```bash
cargo xtask release-diff \
    --message-type UTILMD \
    --from fv20261001 \
    --to fv20271001
```

Review the output to confirm only the expected segment rules changed.

---

## Step 9 — Check PID fixture coverage

```bash
cargo xtask validate-pruefids
```

For the updated message type, add at least one `.edi` fixture per **new or
changed** PID under `crates/edi-energy/tests/fixtures/`.  Use the BDEW test
message examples as a starting point.  Run again to confirm the MISSING count
decreased for the updated types.

---

## Step 10 — Archive expired profiles

After adding the new release profiles, mark any that are now more than 90 days
past their `valid_until` as archived so they are excluded from the default build:

```bash
cargo xtask codegen --prune-expired
```

This sets `"archived": true` in the `mig.json` of each expired profile and
regenerates `mod.rs` with archive-gated `#[cfg]` attributes.  Archived profiles
continue to compile — but only when the `{type}-archive` or `archive` Cargo
feature is enabled — so historical validation tooling still works.

The `archived` flag is an explicit JSON marker, not computed from the current
date.  This keeps `cargo xtask codegen --check` deterministic in CI.

> **Default grace period:** 90 days after `valid_until`.  Override with
> `--grace-days N` if your deployment needs a different retention window.

Commit the updated `mig.json` files and regenerated `mod.rs` together.

---

## Step 11 — PR checklist

Before merging:

- [ ] All `_WARNING` fields removed from profile JSON files
- [ ] `valid_until` set on previous release profile
- [ ] `valid_from` set on new profile
- [ ] `cargo xtask codegen --prune-expired` run; expired profiles archived
- [ ] `cargo xtask validate-profiles` exits 0
- [ ] `cargo xtask codegen --check` exits 0
- [ ] `cargo xtask validate-release-codes` exits 0 — every profile `release` field matches a UNH 0057 value in a fixture (F-033)
- [ ] `cargo test --all-features` exits 0
- [ ] At least one `.edi` fixture added for newly introduced PIDs
- [ ] If any workflow state schema changed: bespoke `StateMigration` impl added
  in the domain crate and dispatch table in `services/makod/src/migration_api.rs`
  updated with the new concrete migration type (replacing the `identity!` entry).
- [ ] If any `#[ignore = "... until FVYYYYMMDD"]` tests exist past their date,
  un-ignore them.
- [ ] PIDs marked ⚠️ in the new PID overview (absent from next FV) removed from
  their owning `*_PIDS` arrays and any generated FV profiles updated.

---

## Step 12 — Deploy new binary with both FVs active (zero-downtime rollout)

Deploy the new binary so that **both** format versions are registered in the
adapter registry simultaneously. The new binary can accept both old-FV and new-FV
inbound messages; in-flight processes continue under their originating FV.

```bash
# Kubernetes example — rolling restart with the new image:
kubectl set image deployment/makod makod=registry.example/makod:FV2026-10-01
kubectl rollout status deployment/makod
```

Do **not** remove the old FV from the adapter config yet.

---

## Step 13 — Run in-flight process migration (online, no downtime)

While the daemon is running, call the migration endpoint to advance all
in-flight processes from the old FV snapshot to the new FV.

> **Why online?**  `makod` holds an exclusive lock on its data directory via
> SlateDB. A separate `makod migrate` binary cannot open the same path while
> the daemon is live. The HTTP endpoint runs migration in-process using the
> daemon's own open store handles, avoiding the lock entirely.

```bash
# Replace FV dates as appropriate for the current release cycle.
curl -s -X POST \
     -H "Authorization: Bearer ${TOKEN}" \
     -H "Content-Type: application/json" \
     -d '{"from":"FV2025-10-01","to":"FV2026-10-01"}' \
     http://makod-admin:8080/admin/migrations | jq .
```

Expected response (success):

```json
{
  "from": "FV2025-10-01",
  "to": "FV2026-10-01",
  "migrated": 47,
  "skipped": 18234,
  "errors": [],
  "runners_executed": 34
}
```

**Assert `errors == []` before proceeding.** Non-empty `errors` means some
process streams could not be migrated (deserialization failure, missing state
data, etc.) — investigate each failure before retiring the old FV.

If a workflow's state schema changed between FVs, add a bespoke `StateMigration`
implementation in the domain crate (see
[`mako_engine::migration::StateMigration`](../crates/mako-engine/src/migration.rs))
and update `services/makod/src/migration_api.rs` to use it.

---

## Step 14 — Retire old FV from adapter registry and redeploy

After a successful migration (Step 13, `errors == []`), remove the old FV from
the adapter config and do a final rolling restart:

```bash
# Remove old FV profile registration from makod.toml or deployment config,
# then redeploy:
kubectl set image deployment/makod makod=registry.example/makod:FV2026-10-01-final
kubectl rollout status deployment/makod
```

New inbound messages now exclusively use the new FV. Old-FV streams have been
migrated; new events for those processes are written under the new `workflow_id`.

---

## Appendix A — Message type feature flags

| Message type | Feature flag | Default? |
|---|---|---|
| UTILMD | `utilmd` | ✓ |
| MSCONS | `mscons` | ✓ |
| APERAK | `aperak` | ✓ |
| CONTRL | `contrl` | ✓ |
| INVOIC | `invoic` | – |
| REMADV | `remadv` | – |
| ORDERS | `orders` | – |
| IFTSTA | `iftsta` | – |
| INSRPT | `insrpt` | – |
| REQOTE | `reqote` | – |
| PARTIN | `partin` | – |
| ORDCHG | `ordchg` | – |
| ORDRSP | `ordrsp` | – |
| QUOTES | `quotes` | – |
| COMDIS | `comdis` | – |
| PRICAT | `pricat` | – |
| UTILTS | `utilts` | – |

## Appendix B — BDEW Übergangsfrist (7-day grace period)

During the 7 days following each October 1 cutover, both the outgoing and
incoming release formats are normatively acceptable.  The library handles this
automatically via `TransitionState` — no code changes are needed during the
grace period.  See `docs/release-lifecycle.md` for details.

---

## Appendix E — FV2026 profile effective dates and pairing rules

Different message types take effect on **different dates** within the 2026
release cycle.  This is the BDEW-mandated staggered rollout schedule.  When an
engine processes a message after a partial cutover (e.g., INVOIC effective from
2026-04-01 but UTILMD not until 2026-10-01), it must apply the correct profile
for each message type independently.

`FormatVersion` is chosen per-message-type at parse time based on the
`UNH DE 0057` wire release code.  The engine never infers a profile from the
calendar date; the sender declares which profile version to use in the wire
format itself.  **Profile pairing is therefore a routing concern, not a clock
concern** — concurrent processes can run under different FVs without conflict
(see `WorkflowVersionPolicy::ForwardCompatible`).

### FV2026 profile inventory

| Message type | FV2025 baseline | FV2026 release | Effective from |
|---|---|---|---|
| `aperak` | `fv20251001` | `fv20261001` | 2026-10-01 |
| `comdis` | `fv20251001` | `fv20261001` | 2026-10-01 |
| `contrl` | `fv20251001` | `fv20260101` | 2026-01-01 |
| `iftsta` | `fv20251001` | `fv20261001` | 2026-10-01 |
| `insrpt` | `fv20211001` | `fv20260101` | 2026-01-01 |
| `invoic` | `fv20251001` | `fv20260401` | 2026-04-01 |
| `mscons` | `fv20251001` | `fv20261001` | 2026-10-01 |
| `ordchg` | `fv20241001` | `fv20260401` | 2026-04-01 |
| `orders` | `fv20251001` | `fv20260401` | 2026-04-01 |
| `ordrsp` | `fv20251001` | `fv20260401` | 2026-04-01 |
| `partin` | `fv20251001` | `fv20260401` | 2026-04-01 |
| `pricat` | `fv20250401` | `fv20260401` | 2026-04-01 |
| `quotes` | `fv20250401` | `fv20260401` | 2026-04-01 |
| `remadv` | `fv20251001` | `fv20260401` | 2026-04-01 |
| `reqote` | `fv20250401` | `fv20260401` | 2026-04-01 |
| `utilmd` (Strom) | `fv20251001` | `fv20261001` | 2026-10-01 |
| `utilmd` (Gas) | `fv20251001_gas` | `fv20261001_gas` | 2026-10-01 |
| `utilts` | `fv20241001` | `fv20260401` | 2026-04-01 |

### Transition windows

There are two distinct cutover dates in the 2026 cycle:

- **2026-01-01** — `contrl`, `insrpt` (limited scope; verify with BDEW release notes)
- **2026-04-01** — billing and operational message types (INVOIC, ORDERS, ORDRSP, ORDCHG, PARTIN, REMADV, PRICAT, REQOTE, QUOTES, UTILTS)
- **2026-10-01** — core supply-chain message types (UTILMD, MSCONS, APERAK, COMDIS, IFTSTA)

During any transition window, the engine serves **both** FVs concurrently.
Old processes continue to run under the FV they were spawned with
(`WorkflowVersionPolicy::ForwardCompatible`).  New inbound messages select
the profile using the wire release code in `UNH DE 0057`.

### xtask coverage check

After adding a new FV profile, verify that the `check-release-coverage` gate
covers the new effective date:

```bash
cargo xtask check-release-coverage --date 2026-04-01
cargo xtask check-release-coverage --date 2026-10-01
```

Both commands must exit 0 for the workspace to be considered FV2026-ready.



## Appendix C — Release naming conventions

| Profile directory | Wire release code | Rule |
|---|---|---|
| `fv20271001` | e.g. `S2.3` | UTILMD Strom |
| `fv20271001_gas` | e.g. `G1.3` | UTILMD Gas |
| `fv20271001` | e.g. `2.5b` | MSCONS |

The wire release code comes from UNH segment, data element 0057 (association-
assigned code).  It must match the `release` field in `mig.json` exactly.

## Appendix D — Archive features

Profiles marked `"archived": true` in `mig.json` are excluded from the default
build.  They can still be compiled for historical validation by enabling the
matching Cargo feature:

| Scenario | Feature to enable |
|---|---|
| Validate old MSCONS messages | `mscons-archive` |
| Validate old CONTRL messages | `contrl-archive` |
| All archived profiles at once | `archive` |

The `archive` meta-feature activates all per-type archive features:

```toml
[dependencies]
edi-energy = { version = "0.8", features = ["archive"] }
```

Archive features always imply their base type feature (`mscons-archive` implies
`mscons`), so you never need to list both.

See `docs/schema-versioning.md` for the full policy on how the `archived` flag
is set and what the codegen guarantees are.
