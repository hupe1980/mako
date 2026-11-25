+++
title = "Annual Release Workflow"
description = "The engineering playbook for a new BDEW release: mirror the documents, import and validate the profiles, check PID coverage, roll out, migrate."
weight = 12
+++
The steps for bringing a new BDEW release into the workspace, in order. Cutovers
are staggered per message type — 1 January, 1 April and 1 October all carry
format versions — so this runs once per message type that moves.

A cutover starts from the BNetzA Mitteilung that publishes it, and a Mitteilung
can publish nothing for EDIFACT. [Mitteilung Nr. 58](https://www.bundesnetzagentur.de/DE/Beschlusskammern/BK06/BK6_83_Zug_Mess/835_mitteilungen_datenformate/Mitteilung_58/Mitteilung_Nr_58.html) (01.10.2026) is
one: for **01.04.2027** „Zum 1. Oktober 2026 erfolgt keine Veröffentlichung
neuer Dokumente. Damit ergibt sich zum 1. April 2027 kein Anpassungsbedarf" —
for EDIFACT, the API-Webdienste and the EBD alike. That cycle binds only
**ActivationDocument AWT 1.1g** (Redispatch XML, `redispatch-xml`) and
**Regelungen zum Übertragungsweg 1.11** (certificate use for mTLS), neither of
which is a profile under `crates/edi-energy/profiles`, so this playbook does not
run for it. The same Mitteilung keeps **UTILTS** („Die UTILTS bleibt damit
erhalten"), limits the API-Webdienste to MaLo-Ident and Steuern, and puts the
MaBiS-Hub Anwendungsfälle in EDIFACT.

---

## Prerequisites

- The BDEW PDFs live in a local mirror that `cargo xtask sync-regulatories
  --download` populates (Step 0); they are third-party publications and not in
  the repository.

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
>
> BDEW issues a correction in two ways, and the report names both. Under the
> **same catalogue id** with a new `fileId` or `correctionDate`, the entry is
> *reissued upstream*, and `--download` fetches it over the file it corrects.
> Under a **new catalogue id** with an unchanged title — the Redispatch
> Änderungshistorie is republished this way — it is *in force upstream but not
> mirrored*: a file the manifest already gives to another id never stands in for
> it, and the new edition lands beside the old one.

---

## Before the documents bind — the consultation as a candidate

A consultation publishes the next release's documents as a
**Konsultationsfassung** months before they bind. It is imported as a
**candidate**, never as a profile: everything under
`crates/edi-energy/profiles/` is embedded by `build.rs` as a selectable
Release of the published crate, and a Konsultationsfassung is never binding.
Candidates live in `candidates/` at the workspace root and are read at runtime
only.

1. Name it in `candidates/sources.json` — the same fields as a profile entry,
   plus `status` and `against`:

   ```json
   {
     "candidates": {
       "utilmd/fv20270401": {
         "status": "Konsultationsfassung",
         "against": "fv20261001",
         "release": "S2.3",
         "valid_from": "2027-04-01",
         "ahb_version": "2.3",
         "mig": "<the consultation MIG PDF in the mirror>",
         "ahb": "<the consultation AHB PDF in the mirror>"
       }
     }
   }
   ```

2. Import it with the binding importer:

   ```bash
   cargo xtask import-profiles --candidate utilmd/fv20270401
   ```

   This writes `candidates/utilmd/fv20270401/{mig,ahb,candidate}.json`.
   `candidate.json` records the SHA-256 of each consultation PDF and the
   Prüfidentifikatoren that changed, appeared and went away against `against`
   — the list `cargo xtask profile-diff utilmd fv20261001 fv20270401
   --candidate --json` prints. A column counts as changed when a status or an
   operand differs, or when it cites a Bedingung or Paket whose text differs.

3. `cargo xtask validate-profiles` holds the line: it refuses a
   `candidate.json` under `profiles/`, a binding entry carrying `status` or
   `against`, a candidate whose `<type>/<fv>` is also binding, and one
   without its status or a SHA-256 per PDF.

4. Read which of the operator's own messages the candidate would refuse. makod
   keeps no full copy of an exchanged interchange, so the report reads the
   operator's interchange archive — one interchange per file, named per
   Allgemeine Festlegungen 6.1d Kap. 2.12
   (`<MsgType>_<Sender>_<Receiver>_<YYMMDD>_<HHMM>_<Ref>.txt`) — over the last
   `window_days` (default 90). Each message of a changed PID is validated
   under its binding profile and under the candidate, and only a refusal the
   candidate adds is listed: PID, direction, counterparty, date, file, the
   rule with the candidate AHB's status and Bedingung text, the segment and
   its position, and for an outbound message the builder in mako that must
   change. Compressed or unnamed files, messages without a binding profile
   and PIDs the candidate does not change are counted, not listed. makod's
   `format_impact` module computes the report, as JSON or as a `;`-separated
   CSV for the IT provider or the counterparty.

When the binding documents are published, the release runs through Step 1
onwards like any other, and the candidate directory is deleted.

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

When the Mitteilung gives the release a publication date off the Allgemeine
Festlegungen 6.1d Kap. 2.5 timetable, add `"publikation_quelle"` naming it —
`"BNetzA Mitteilung Nr. <n> (<Datum>)"` or the Mitteilung's
bundesnetzagentur.de URL. `validate-profiles` refuses an off-timetable date
without it, and a `publikation_quelle` that names no Mitteilung.

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

A document defect the importer works around is printed as `warn` — the places
where AHB and profile differ. Two are evidenced and handled: an `SG27 MOA` row
without a status in INVOIC AHB 1.0b takes it from the MIG, and an operand cell
without its letter (UTILTS AHB 1.1 `SG8 DTM` DE 2379) takes the letter its
sibling column prints. Any other defect, and any Bedingung cited without its
text, fails the import.

Then read the delta against the predecessor:

```bash
cargo xtask profile-diff utilmd fv20261001 fv20271001
cargo xtask profile-diff utilmd fv20261001 fv20271001 --pid 55001
```

It lists Prüfidentifikatoren that appeared or were withdrawn and, per PID, the
places whose status changed, codes that gained or lost an operand, and rewritten
Bedingungen and Pakete. Places are named by position and MIG name (`SG4
Vorgangs-Identifikation / STS Transaktionsgrund`), since the MIG renumbers its
segments between versions. Put it in the PR summary.

---

## Step 3 — Prove it

```bash
cargo test -p edi-energy --all-features
```

`tests/it/skeletons.rs` generates the minimal message of every column and
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

`validate-profiles` also counts status expressions the extraction cut in half;
the count is **zero**, pinned in `xtask/profile-expression-defects.json`, and
may only shrink. It cannot see a lost *operator*: `(…) ⊻ ([177] ∧ [178])` with
the `⊻` on its own line reads as `(…) ([177] ∧ [178])`, a conjunction. Read the
`profile-diff` output for operators that appear or vanish, and check a
suspicious column against `pdf-grid`.

A Prüfidentifikator carried by the predecessor and absent from the new profile
fails `validate-profiles`. Confirm the retirement in the AHB's Änderungshistorie
and record it in `RETIRED_PIDS` (`xtask/src/validate_profiles.rs`) with the
Änd-ID; without that entry it is an import regression. Update
`pid-overview.json` when BDEW publishes a new Anwendungsübersicht, from its
PDF — the Disclaimer on its S. 1 makes the PDF the binding edition and the
.xlsx informational:

```bash
cargo xtask import-pid-overview regulatories/bdew-mako/<Anwendungsübersicht>.pdf \
    --oracle <.xlsx of the same Stand>
```

The import reads Tabelle 1 „Prüfidentifikator zu Prozessschritt" by column
position, names the PDF by title and SHA-256 in `source_document`, and with
`--oracle` reports every Prüfidentifikator, Sparte or Zuordnung the workbook
states differently; it never takes a value from it. Then run
`cargo xtask check-pid-coverage` and reconcile `COVERED_FLOOR` and
`SHIPPED_NOT_PUBLISHED` with what the new edition adds or removes.

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

## Step 7 — Deploy the new binary

A binary carries every profile `sources.json` names, and every known format
version must be covered by an adapter —
`makod::startup::adapter_befunde` refuses to boot otherwise. There is
no per-FV switch: in-flight processes keep the FV on their `WorkflowId`, and the
inbound message's own FV picks the adapter.

```bash
kubectl set image deployment/makod makod=registry.example/makod:<version>
kubectl rollout status deployment/makod
```

### Watch for format-version substitution

When `makod` cannot derive the format version from an inbound message — an
unknown message type, an unparseable release, or a release with **no registered
profile for today's date** — it falls back to the newest known FV and logs at
`WARN`:

```
format version could not be derived from the message — validating against the
newest known release instead; the AHB rules applied are not necessarily those of
the release the message claims
  reason="no profile registered for this release on today's date"
  substituted_fv="FV2026-10-01"
```

Validation still derives its profile from the message's own release; the
substituted FV selects the `MessageAdapter` and the spawned process's
`WorkflowId`. Alert on `reason="no profile registered for this release on
today's date"`: a burst during a cutover means a profile is missing from the
deployed binary. The other reasons indicate a malformed message.

---

## Step 8 — Run in-flight process migration (online, no downtime)

The migration endpoint repoints in-flight processes' snapshots from the old FV
to the new one, inside the running daemon — `makod` holds an exclusive lock on
its data directory, so no second process could open the store.

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
  "workflows": ["esa-wertebestellung", "gabi-gas-allocation", "…"]
}
```

**Assert `errors == []`** (the endpoint answers `207` otherwise); investigate
each failed stream. A pair outside `KNOWN_FV_TRANSITIONS` (`migration_api.rs`,
one entry per release cycle) is `422`.
`workflows` names every workflow the run covered — each registered
`WorkflowDriver`.

If a workflow's state schema changed between FVs, add a bespoke `StateMigration`
implementation in the domain crate (see
[`mako_engine::migration::StateMigration`](https://github.com/hupe1980/mako/blob/main/crates/mako-engine/src/migration.rs))
and run it for that workflow in `services/makod/src/api/migration_api.rs`.

---

## Step 9 — Retire a profile

A profile stays in `sources.json` — and compiled in — while any deployment can
still receive its format; it is removed from `sources.json` and its directory
deleted when none can.

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

See [Transition Window Handling](@/docs/compliance/release-lifecycle.md#transition-window-handling).

---

## Appendix C — Formatversion effective dates

### Publikationsdatum is not the Anwendungszeitpunkt

Allgemeine Festlegungen 6.1d Kap. 2.5.1/Kap. 2.5.2 give the cycle three instants. For the
October changeover: consultation documents 01.02., **Veröffentlichung der
konsultierten Dokumente 01.04.**, **Anwendungszeitpunkt 01.10.** The April
changeover mirrors it — published 01.10., applies 01.04. of the next year. The
six months between are the *Umsetzungsphase*, and throughout them the previous
format is the binding one.

The document's `Publikationsdatum` goes in `publikationsdatum`; `valid_from`
and the directory name carry the Anwendungszeitpunkt. `validate-profiles`
refuses two profiles on the same day, a gap within a track, and a chain whose
newest profile is not open-ended.

Corrections issued in between ("Konsolidierte Lesefassung mit Fehlerkorrektur")
take effect without a further BNetzA Mitteilung and do not move the
Anwendungszeitpunkt; the latest consolidated version is the one to implement.

### Which profile applies when

`profiles/<type>/<fv>/mig.json` is authoritative — `publikationsdatum`,
`valid_from`, `valid_until`. To list them:

```bash
cargo xtask validate-profiles          # prints gaps and overlaps
```

The six-month gap is Kap. 2.5's own timetable, so `publikationsdatum` is entailed
by `valid_from` rather than looked up, and `validate-profiles` refuses a regular
Anwendungszeitpunkt whose profile states no date or one that disagrees. A date
off the timetable is admitted only with a `publikation_quelle` that names the
BNetzA Mitteilung — „Mitteilung Nr. <n>“ or its bundesnetzagentur.de page — and
no publication may fall on or after its own `valid_from`. `contrl`/`insrpt` run
on an ausserordentliche Veröffentlichung (applies 2026-01-01), which neither
Kap. 2.5.1 nor Kap. 2.5.2 covers; those profiles state no `publikationsdatum`, which
the rule admits off the timetable.

### Date or wire code?

`UNH DE 0057` identifies the MIG, not the Formatversion — REQOTE AHB 1.1 and 1.2
both carry wire release `1.3c`, so only the date distinguishes them.
`ReleaseRegistry::profile_on` selects the greatest `valid_from ≤ date`, and
`ProcessContext::for_date` does the same for a context. The date is always
supplied by the caller — `makod` states `mako_fristen::heute()`, the German
calendar date, because a Formatversion takes effect at German midnight.


## Appendix D — Release naming conventions

| Profile directory | Wire release code | Rule |
|---|---|---|
| `fv20271001` | e.g. `S2.3` | UTILMD Strom |
| `fv20271001_gas` | e.g. `G1.3` | UTILMD Gas |
| `fv20271001` | e.g. `2.5b` | MSCONS |

The wire release code is `UNH` DE 0057 and must match `release` in `mig.json`.

---

## The lead time

```bash
cargo xtask lead-time
```

Per profile, the days from the release's `publikationsdatum` to the first
commit that names its key in `sources.json` (found with
`git log -S'"<type>/fv<date>"' --reverse`), and from that commit to
`valid_from`. A profile committed after it was already binding reads negative.
Only profiles added after the cut count — the commit that added the command, or
`--cut <rev>` — because the repository's history opens after most existing
profiles were binding, and their figure would be the repository's age. A
shallow clone is refused: it has no first commit to find.

## Read the candidate preview

Use makod’s authorized `GET /api/v1/format-candidates/{fv}/impact` to compare the candidate with binding profiles on the local interchange archive. `revalidated` counts messages, and `refusals` contains one row per new rule finding. In `not_read`, compressed, unnamed and unparseable input counts files. A missing binding profile counts messages. A truncated interchange counts as one unread file, even when a complete message precedes the truncation. A sample with zero assessed messages supplies no evidence about changed verdicts.

An unavailable catalogue returns `503 candidate_catalog_unavailable`. An incomplete import returns `422 candidate_invalid`. The archive must contain regular files and directories. An unreadable tree, including a link or special file in it, returns `503 archive_unavailable`.
