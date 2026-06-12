# Annual Release Workflow

This document is the step-by-step engineering playbook for incorporating a new
BDEW annual release into `edi-energy-rs`.  Follow the steps in order for every
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
    --to fv20271001 \
    --output-file REVIEW.md
```

Commit `REVIEW.md` as part of the PR so reviewers can see exactly what changed
without reading the full generated diff.

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

## Step 10 — Update CHANGELOG.md

Add an entry to the top-level `CHANGELOG.md`:

```markdown
## Unreleased

### Added
- UTILMD Strom S2.3 profile (`fv20271001`, effective 2027-10-01)
- MSCONS 3.x profile (`fv20271001`, effective 2027-10-01)

### Changed
- UTILMD Strom S2.2 (`fv20261001`) `valid_until` set to `2027-09-30`
```

---

## Step 11 — Archive expired profiles

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

## Step 12 — PR checklist

Before merging:

- [ ] All `_WARNING` fields removed from profile JSON files
- [ ] `valid_until` set on previous release profile
- [ ] `valid_from` set on new profile
- [ ] `cargo xtask codegen --prune-expired` run; expired profiles archived
- [ ] `cargo xtask validate-profiles` exits 0
- [ ] `cargo xtask codegen --check` exits 0
- [ ] `cargo test --all-features` exits 0
- [ ] `REVIEW.md` committed with release-diff output
- [ ] CHANGELOG.md updated
- [ ] At least one `.edi` fixture added for newly introduced PIDs

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
edi-energy = { version = "0.1", features = ["archive"] }
```

Archive features always imply their base type feature (`mscons-archive` implies
`mscons`), so you never need to list both.

See `docs/schema-versioning.md` for the full policy on how the `archived` flag
is set and what the codegen guarantees are.
