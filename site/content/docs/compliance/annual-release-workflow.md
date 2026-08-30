+++
title = "Annual Release Workflow"
description = "Step-by-step engineering playbook for incorporating a new BDEW annual release: extract-pdf, import-xml-ahb, codegen, validate-pruefids, add-release, check-release-coverage."
weight = 12
+++
# Annual Release Workflow

This document is the step-by-step engineering playbook for incorporating a new
BDEW release into the `mako` workspace.  Follow the steps in order for every
cutover.  Cutovers are staggered per message type — January 1, April 1 and
October 1 all carry format versions — so this playbook runs several times a year,
once per message type that moves.

---

## Prerequisites

- `cargo xtask` is the primary tool for every step.  Build it once before starting:
  ```
  cargo build -p xtask
  ```
- The BDEW PDFs live in `regulatories/bdew-mako/`, mirrored by
  `cargo xtask sync-regulatories` (Step 0).

---

## Step 0 — See what changed upstream

```bash
cargo xtask sync-regulatories
```

Reports every document in force that `regulatories/bdew-mako/` does not hold —
a new Formatversion appears as a block of MIG/AHB entries sharing a
`valid_from`. Add `--download` to fetch them; `--offline` checks the mirror
against its manifest without the network.

> A corrected document keeps its version number, so only the manifest's content
> hash can tell you a local copy is stale. Such a file is reported as *changed
> since it was mirrored*.

---

## Step 1 — Extract draft profiles from PDFs

Run the PDF extractor for each **changed** message type, once per source PDF.
Each run writes both `mig.draft.json` and `ahb.draft.json`; run it against the
MIG PDF to populate the MIG draft, then against the AHB PDF for the AHB draft
(a run that extracts zero entries leaves any existing draft untouched):

```bash
cargo xtask extract-pdf --file path/to/UTILMD_MIG_S2.3.pdf \
    --message-type <TYPE> --release <fvYYYYMMDD>
cargo xtask extract-pdf --file path/to/UTILMD_AHB_S2.3.pdf \
    --message-type <TYPE> --release <fvYYYYMMDD>
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
4. **`mig.json` → `dtm_formats`** — DE 2005 qualifier → the DE 2379 format codes
   the MIG admits. `validate-profiles` refuses a profile that carries `DTM` and
   declares none, because DE 2379 has no code list and would otherwise go
   unchecked.

   Read it off the MIG segment-layout tables — each `DTM` block fixes both, in
   the BDEW code column:

   ```text
    2005  …Funktion, Qualifier  M an..3  M an..3   137 Dokumentendatum
    2379  …Format, Code         C an..3  R an..3   303 CCYYMMDDHHMMZZZ
   ```

   A qualifier can admit several formats, written as **continuation rows** whose
   left half still carries the element label — read the whole block, not the
   first row:

   ```text
    2379  Datums- oder Uhrzeit- oder   C an..3  R an..3   802 Monat
          Zeitspannen-Format, Code                        803 Woche
                                                          804 Tag
   ```

   Reading only the first row makes a conformant `DTM+273:14:804` a validation
   error — the failure direction that rejects good messages.

The extractor embeds `"_WARNING"` fields in draft output.  Remove all `_WARNING`
fields before promoting.

> `extract-pdf` does not populate `dtm_formats` — its `lopdf` text extraction
> does not preserve the column layout the continuation rows depend on. Extract
> it with `pdftotext -layout` and review against the PDF.

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

Set `valid_from` and `publikationsdatum` in `mig.json`, and close the previous
release with a `valid_until`:

```json
// profiles/utilmd/fv20261001/mig.json — add or confirm:
"valid_until": "2027-09-30"

// profiles/utilmd/fv20271001/mig.json:
"publikationsdatum": "2027-04-01",   // the title page's Publikationsdatum
"valid_from":        "2027-10-01"    // the Anwendungszeitpunkt — six months later
```

> **Rule:** `valid_from` is the **Anwendungszeitpunkt**, never the date on the
> title page. A document published on 01.04. applies from 01.10. of the same
> year, one published on 01.10. from 01.04. of the next (Allgemeine Festlegungen
> 6.1d §2.5.1/§2.5.2). Name the directory after that date. Codegen refuses a
> `valid_from` that does not follow from a stated `publikationsdatum`. Omit
> `publikationsdatum` only for an ausserordentliche Veröffentlichung, whose
> Anwendungszeitpunkt the BNetzA Mitteilung names directly.

> **Rule:** every profile that is superseded by a new one **must** have a
> `valid_until` date, and it must be the day before its successor's
> `valid_from` — `validate-profiles` errors on an overlap and warns on a gap.
> Open-ended profiles (`valid_until` absent) are treated as permanently valid.

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
- [ ] `valid_from` (Anwendungszeitpunkt) and `publikationsdatum` set on new profile
- [ ] `cargo xtask codegen --prune-expired` run; expired profiles archived
- [ ] `cargo xtask validate-profiles` exits 0
- [ ] `cargo xtask codegen --check` exits 0
- [ ] `cargo xtask validate-release-codes` exits 0 — every release code a counterparty can still send matches a UNH 0057 value in a fixture. Both sides of a cutover need one: the outgoing version stays receivable until its `valid_until`.
- [ ] `cargo test --all-features` exits 0
- [ ] At least one `.edi` fixture added for newly introduced PIDs
- [ ] If any workflow state schema changed: bespoke `StateMigration` impl added
  in the domain crate and dispatch table in `services/makod/src/api/migration_api.rs`
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

### Watch for format-version substitution

`makod` derives the format version from each inbound message's release. When it
cannot — an unknown message type, an unparseable release, or a release with **no
registered profile for today's date** — it falls back to the newest known FV and
logs at `WARN`:

```
format version could not be derived from the message — validating against the
newest known release instead; the AHB rules applied are not necessarily those of
the release the message claims
  reason="no profile registered for this release on today's date"
  substituted_fv="FV2026-10-01"
```

Falling back rather than rejecting is deliberate: during a cutover a counterparty
may send a release this binary predates, and refusing the message outright is
worse than dispatching it under the closest registered version.

The substitution does **not** change which AHB rules apply — validation derives
its profile from the message's own release. What the FV selects is the
`MessageAdapter` and the `WorkflowId` name, and since adapters accept every
registered FV the substitution is usually invisible in behaviour. Two things make
it worth watching anyway: it means mako could not read the release the
counterparty stated, and the spawned process carries a `WorkflowId` naming a
release the message never claimed.

Treat a burst during the transition window as a signal that a profile is missing
from the deployed binary — not as noise.

Alert on `reason="no profile registered for this release on today's date"`
specifically; the other reasons indicate a malformed message rather than a
deployment gap.

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
  "runners_executed": 57,
  "workflows": ["esa-wertebestellung", "gabi-gas-allocation", "…"],
  "workflows_not_migrated": [
    ["gpke-messwerte", "records inbound MSCONS Messwerte and completes"],
    ["gabi-gas-mmma", "delegates delivery to gpke-allokationsliste"]
  ]
}
```

**Assert `errors == []` before proceeding.** Non-empty `errors` means some
process streams could not be migrated (deserialization failure, missing state
data, etc.) — investigate each failure before retiring the old FV.

`workflows` names every family the run covered. It is reported rather than
merely counted because a count alone reads as complete whatever the migration
happens to include, with nothing to say which families were missing. A
build-time guard requires every dispatchable workflow to have either a
migration arm or a recorded reason it needs none, and `workflows_not_migrated`
reports those reasons so the sign-off covers them too. They are pure receive-and-record families:
they record an inbound message and finish, so no process survives a cutover for
a migration to repoint.

If a workflow's state schema changed between FVs, add a bespoke `StateMigration`
implementation in the domain crate (see
[`mako_engine::migration::StateMigration`](https://github.com/hupe1980/mako/blob/main/crates/mako-engine/src/migration.rs))
and update `services/makod/src/api/migration_api.rs` to use it.

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

Inbound messages for a process are parsed and validated against the format
version active for that process; new events are written under the corresponding
`workflow_id`.

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

## Appendix B — the cutover has no Übergangsfrist

Allgemeine Festlegungen 6.1 §2.5 gives the EDIFACT formats a single
*Anwendungszeitpunkt*: before it the old format applies, from it the new one.
There is no window in which both are normatively acceptable, so a cutover is a
step, not a fade.

The 15-Werktage Übergangszeitraum in §8.5 belongs to the **XML** formats. It
starts at the Anwendungszeitpunkt, counts Werktage, and selects the version by
the *Erfüllungsdatum* stated in the message — none of which transfers to EDIFACT.

An operator who wants to keep accepting a late-arriving message in the old
format sets `ReleaseRegistry::with_receive_tolerance_days(n)`, which extends the
trailing edge only. That is a local receiving policy and defaults to zero. See
[Release Lifecycle](@/docs/compliance/release-lifecycle.md).

---

## Appendix C — Formatversion effective dates

### Publikationsdatum is not the Anwendungszeitpunkt

Allgemeine Festlegungen 6.1d §2.5.1/§2.5.2 give the cycle three instants. For the
October changeover: consultation documents 01.02., **Veröffentlichung der
konsultierten Dokumente 01.04.**, **Anwendungszeitpunkt 01.10.** The April
changeover mirrors it — published 01.10., applies 01.04. of the next year. The
six months between are the *Umsetzungsphase*, and throughout them the previous
format is the binding one.

Every EDI@Energy document prints its `Publikationsdatum` on the title page. It
goes in the profile's `publikationsdatum` field; `valid_from` and the directory
name carry the Anwendungszeitpunkt six months later. `cargo xtask codegen`
refuses a `valid_from` that does not follow §2.5 from a stated
`publikationsdatum`, and `validate-profiles` errors on two profiles claiming the
same day.

Corrections issued in between ("Konsolidierte Lesefassung mit Fehlerkorrektur")
take effect without a further BNetzA Mitteilung and do not move the
Anwendungszeitpunkt; the latest consolidated version is the one to implement.

### Which profile applies when

`profiles/<type>/<fv>/mig.json` is authoritative — `publikationsdatum`,
`valid_from`, `valid_until`. To list them:

```bash
cargo xtask validate-profiles          # prints gaps and overlaps
```

Two exceptions to the six-month rule, both by regulation rather than cycle:
`contrl`/`insrpt` run on the ausserordentliche Veröffentlichung of 11.12.2025
(applies 2026-01-01), and UTILMD Strom on BK6-22-024's LFW24 date, which
`fv20250606` carries. Those profiles state no `publikationsdatum`.

MSCONS has a gap between `fv20240401` and `fv20260401`: AHB 3.1, applying
2025-10-01, was never authored. `validate-profiles` warns about it on every run.
A gap is a coverage statement; an overlap is an error.

### Date or wire code?

`UNH DE 0057` identifies the MIG, not the Formatversion — REQOTE AHB 1.1 and 1.2
both carry wire release `1.3c`, so only the date distinguishes them.
`ReleaseRegistry::profile_on` selects the greatest `valid_from ≤ date`, and
`ProcessContext::current()` does the same for today.

Running processes continue under the FV they were spawned with
(`WorkflowVersionPolicy::ForwardCompatible`), so a partial cutover needs no
coordination.

EDIFACT has no overlap at the changeover: before the Anwendungszeitpunkt the old
format applies, from it the new one.
`ReleaseRegistry::with_receive_tolerance_days` widens only the trailing edge, as
a local inbound policy for a late-arriving message.

### xtask coverage check

After adding a new FV profile, verify that the `check-release-coverage` gate
covers the new effective date:

```bash
cargo xtask check-release-coverage --date 2026-04-01
cargo xtask check-release-coverage --date 2026-10-01
```

Both commands must exit 0 for the workspace to be considered FV2026-ready.



## Appendix D — Release naming conventions

| Profile directory | Wire release code | Rule |
|---|---|---|
| `fv20271001` | e.g. `S2.3` | UTILMD Strom |
| `fv20271001_gas` | e.g. `G1.3` | UTILMD Gas |
| `fv20271001` | e.g. `2.5b` | MSCONS |

The wire release code comes from UNH segment, data element 0057 (association-
assigned code).  It must match the `release` field in `mig.json` exactly.

## Appendix E — Archive features

Profiles marked `"archived": true` in `mig.json` are excluded from the default
build.  They can still be compiled for historical validation by enabling the
matching Cargo feature:

| Scenario | Feature to enable |
|---|---|
| Validate old MSCONS messages | `mscons-archive` |
| Validate old CONTRL messages | `contrl-archive` |
| All archived profiles at once | `archive` |

The `archive` meta-feature activates all per-type archive features:

```bash
cargo add edi-energy --features archive
```

Archive features always imply their base type feature (`mscons-archive` implies
`mscons`), so you never need to list both.

See [Schema Versioning](@/docs/compliance/schema-versioning.md) for the full policy on how the `archived` flag
is set and what the codegen guarantees are.
