# BDEW EDIFACT Test Fixtures

EDIFACT messages the conformance suite (`tests/conformance.rs`) reads.

## Directory Layout

```
fixtures/
  <message_type>/
    valid/       — must parse + validate without errors
    invalid/     — must parse; validation must produce errors matching .expected.json
```

| Directory | Asserted by | Claim |
|---|---|---|
| `valid/` | `conformance.rs` | mako reads this Anwendungsfall and finds nothing wrong |
| `invalid/` | `conformance.rs` + `.expected.json` | mako refuses it, for the stated reason |

The corpus is curated, not exhaustive: **every** Anwendungsfall of every profile
is witnessed by `tests/skeletons.rs`, which generates the minimal message of
each column from the Prüfschablone and validates it against that column. A
fixture here is an example a reader can copy — a Beispielnachricht with
realistic (synthetic) MP-IDs, Lokations-IDs and dates — or a defect that must
be refused.

## Corpus-wide invariants

`validation_snapshot.txt` records each fixture's verdict, rule id by rule id,
and fails when one moves. Two things it cannot express are asserted separately:

- **`validation_snapshot.rs::every_fixture_parses_and_is_judged`** — no fixture
  may fail to parse or to validate.
- **`demo_fixtures.rs`** — the fixtures under `demos/` are outside this corpus
  and are validated there instead.
- **`party_agency_code.rs`** — every `NAD` stamps the agency its MP-ID range
  implies: `99…` is `293` (BDEW), `98…` is `332` (DVGW), a GLN is `9`. Gas
  columns admit only `9`/`332`, so a Gas fixture carries `98…` ids in `UNB`
  and `NAD` alike.

## Naming

| Prefix | Meaning |
|---|---|
| `beispiel_<PID>_<name>` | a Beispielnachricht of one Prüfidentifikator |
| `pid_<PID>_<variant>` | a variant of one Prüfidentifikator (an older format version, a Sparte) |
| `<scenario>` (invalid/) | one defect, named by what is wrong; its `.expected.json` names the rule ids that must fire |

A fixture must sit under a message type whose shipped profiles **declare that
Prüfidentifikator**; `fixture_placement.rs` enforces it, resolving against the
profiles because PID bands overlap (29xxx belongs to both APERAK and COMDIS).

## Writing one

Start from the column's own skeleton and put real values in:

```bash
cargo run -p edi-energy --all-features --example 07_resolve -- --skeleton UTILMD S2.2 55001
cargo run -p edi-energy --all-features --example 07_resolve -- tests/fixtures/utilmd/valid/beispiel_55001_lieferbeginn.edi
```

The second command shows where every segment landed in the Nachrichtenstruktur
and every finding, so a fixture is never edited blind. The interchange parties
(`UNB`) must be the `NAD+MS`/`NAD+MR` of the message (Allgemeine Festlegungen
§2.13); the Prüfidentifikator travels where the AHB puts it — `SG6 RFF+Z13` in
UTILMD, `SG1 RFF+Z13` in most other types — and `BGM` DE 1004 is the
Dokumentennummer.

## Which format version a fixture carries

A fixture carries the release it is an example of, in `UNH` DE 0057; the
conformance suite validates it on the newest `valid_from` any profile states.
Both releases of a transition (MSCONS `2.4c` until 2026-09-30, `2.5` from the
next day) are on the wire in the same year and both are represented here.
