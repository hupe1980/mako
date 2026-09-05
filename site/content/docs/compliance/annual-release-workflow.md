+++
title = "Annual Release Workflow"
description = "Step-by-step engineering playbook for incorporating a new BDEW release: mirror the documents, extend sources.json, import and validate the profiles, check PID coverage, roll out and migrate."
weight = 12
+++
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
- The BDEW PDFs live in a local document mirror that `cargo xtask sync-regulatories
  --download` populates (Step 0), and it reports the directory it writes to. They are
  third-party publications and are not part of the repository.

---

## Step 0 — See what changed upstream

```bash
cargo xtask sync-regulatories
```

Reports every document in force that the mirror does not hold —
a new Formatversion appears as a block of MIG/AHB entries sharing a
`valid_from`. Add `--download` to fetch them; `--offline` checks the mirror
against its manifest without the network.

> A corrected document keeps its version number, so only the manifest's content
> hash can tell you a local copy is stale. Such a file is reported as *changed
> since it was mirrored*.

---

## Step 1 — Name the profile

Add one entry per moving message type (Strom and Gas are separate entries for
UTILMD) under the `profiles` object of
`crates/edi-energy/profiles/sources.json`:

```json
"utilmd/fv20271001": {
  "release": "S2.3",
  "track": "Strom",
  "valid_from": "2027-10-01",
  "publikationsdatum": "2027-04-01",
  "ahb_version": "2.3",
  "mig": "UTILMD_MIG_Strom_S2.3.pdf",
  "ahb": "UTILMD_AHB_Strom_2.3.pdf"
}
```

The directory name is the Anwendungszeitpunkt; the two documents are the file
names `sync-regulatories` mirrored. Give the predecessor its `valid_until`
(the day before).

---

## Step 2 — Import it from the PDFs

```bash
cargo xtask import-profiles --profile utilmd/fv20271001
```

The importer reads the MIG's Nachrichtenstruktur and every Segmentlayout, and
the AHB's tables column by column, and writes `mig.json` and `ahb.json`. It
refuses what it cannot read rather than guessing — a status that lands between
two columns, an AHB row naming a segment `Nr` the MIG has no place for, a column
without `UNH`. When it refuses:

```bash
BDEW_DEBUG=1 cargo xtask import-profiles --profile utilmd/fv20271001   # every row, its columns and cells
cargo xtask pdf-grid path/to/UTILMD_AHB_Strom_2.3.pdf                   # the grid the reader sees
```

The fix belongs in `xtask/src/bdew/`, never in the JSON.

A document defect the importer works around is printed as `warn` — an `SG27
MOA` row without any status in INVOIC AHB 1.0b gets its status from the MIG.
Read those lines; they are the places where the AHB and the profile differ.

Every Bedingung a status expression or an operand cites must have its text; a
citation the reader cannot resolve fails the import, because the evaluator
would otherwise read the place as unconditioned.

Then read the delta against the predecessor:

```bash
cargo xtask profile-diff utilmd fv20261001 fv20271001
cargo xtask profile-diff utilmd fv20261001 fv20271001 --pid 55001
```

It lists which Prüfidentifikatoren appeared or were withdrawn and, per
Prüfidentifikator, the places whose status changed, the codes that gained or
lost an operand, and the Bedingungen and Pakete that were rewritten. Places are
named by where they sit and what the MIG calls them — `SG4
Vorgangs-Identifikation / STS Transaktionsgrund` — because the MIG renumbers
its segments between Nachrichtentypversionen, and an `Nr`-keyed listing would
report every segment of every column as changed. Put it in the PR summary.

---

## Step 3 — Prove it

```bash
cargo test -p edi-energy --all-features
```

`tests/skeletons.rs` generates the minimal message of every column and
validates it against that column: an extraction gap (a lost row, a mis-assigned
status) or a validator gap shows up as a failing Anwendungsfall, with its
skeleton and findings. The fixture snapshot names every verdict that moved.

---

## Step 4 — Validate the set

```bash
cargo xtask validate-profiles            # sources ↔ files, dates and continuity, PIDs, AHB rows ↔ MIG, cited Bedingungen
cargo xtask check-pid-coverage           # the shipped columns against the Anwendungsübersicht
cargo xtask check-release-coverage --date 2027-10-01
```

A Prüfidentifikator carried by the predecessor and absent from the new profile
fails `validate-profiles`. Confirm the retirement in the AHB's Änderungshistorie
and record it in `RETIRED_PIDS` (`xtask/src/validate_profiles.rs`) with the
Änd-ID; without that entry it is an import regression. Update
`pid-overview.json` when BDEW publishes a new Anwendungsübersicht
(`cargo xtask import-pid-overview <xlsx>`).

---

## Step 5 — What the senders must now fill

```bash
cargo run -p edi-energy --all-features --example 07_resolve -- --pruefschablone UTILMD S2.3 55001
```

prints the column: every segment the Anmeldung must carry and every data
element with its operands. Hold the renderers in `makod` and the builders
against it — a new Muss segment in the AHB is a new field the sender has to
fill, and `07_resolve <file>` on a rendered message says which places are still
missing.

---

## Step 6 — PR checklist

- [ ] `sources.json` names the documents by their mirrored file names and the predecessor has its `valid_until`
- [ ] `import-profiles` ran clean; every `warn` line is a documented AHB defect
- [ ] `profile-diff <type> <old fv> <new fv>` read, and its listing in the PR summary
- [ ] `cargo test -p edi-energy --all-features` — skeletons 100 %, snapshot re-blessed with the diff read
- [ ] `validate-profiles`, `check-pid-coverage`, `check-release-coverage --date <new fv>` green
- [ ] retired Prüfidentifikatoren recorded with their Änd-ID
- [ ] `just ci` green

---

## Step 7 — Deploy new binary with both FVs active (zero-downtime rollout)

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

## Step 8 — Run in-flight process migration (online, no downtime)

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

## Step 9 — Retire old FV from adapter registry and redeploy

After a successful migration (Step 8, `errors == []`), remove the old FV from
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
name carry the Anwendungszeitpunkt six months later. `validate-profiles` refuses two
profiles claiming the same day, a gap between consecutive profiles of a track,
and a chain whose newest profile is not open-ended.

Corrections issued in between ("Konsolidierte Lesefassung mit Fehlerkorrektur")
take effect without a further BNetzA Mitteilung and do not move the
Anwendungszeitpunkt; the latest consolidated version is the one to implement.

### Which profile applies when

`profiles/<type>/<fv>/mig.json` is authoritative — `publikationsdatum`,
`valid_from`, `valid_until`. To list them:

```bash
cargo xtask validate-profiles          # prints gaps and overlaps
```

One exception to the six-month rule, by regulation rather than cycle:
`contrl`/`insrpt` run on the ausserordentliche Veröffentlichung of 11.12.2025
(applies 2026-01-01). Those profiles state no `publikationsdatum`.

### Date or wire code?

`UNH DE 0057` identifies the MIG, not the Formatversion — REQOTE AHB 1.1 and 1.2
both carry wire release `1.3c`, so only the date distinguishes them.
`ReleaseRegistry::profile_on` selects the greatest `valid_from ≤ date`, and
`ProcessContext::for_date` does the same for a context. The date is always
supplied by the caller — `makod` states `mako_fristen::heute()`, the German
calendar date, because a Formatversion takes effect at German midnight.

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
