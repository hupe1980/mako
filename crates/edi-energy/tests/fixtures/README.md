# BDEW EDIFACT Test Fixtures

This directory contains EDIFACT message fixtures used by the conformance test
suite (`tests/conformance.rs`).

## Directory Layout

```
fixtures/
  <message_type>/
    valid/       — must parse + validate without errors
    invalid/     — must parse; validation must produce errors matching .expected.json
```

## Fixture Naming Convention

| Prefix          | Meaning                                        |
|-----------------|------------------------------------------------|
| `pid_NNNNN`     | Minimal fixture for a single Prüfidentifikator |
| `beispiel_*`    | Representative Beispielnachricht (see below)   |

A `pid_NNNNN` fixture must sit under a message type whose shipped profiles
**declare that Prüfidentifikator**; `fixture_placement.rs` enforces it.

The directory is part of the assertion, not filing. A fixture placed under the
wrong message type still parses and still counts toward `validate-pruefids`
coverage, so nothing else catches it — while it asserts a message-type/PID
pairing no AHB defines.

The check resolves against the profiles rather than assuming a PID band maps to
one message type, because the bands overlap: 29xxx belongs to **both** APERAK
and COMDIS, so `comdis/valid/pid_29002.edi` and `aperak/…/pid_29002` are each
correct.

## Beispielnachrichten

Fixtures named `beispiel_*` are structured to resemble the _Beispielnachrichten_
published by BDEW in their MIG/AHB documents.  They cover the complete segment
structure required for the named Prüfidentifikator and BDEW format version, with
realistic (but synthetic) market-participant IDs and dates.

### UTILMD (BDEW S2.2, `fv20261001`)

| File                                  | PID    | Description                        |
|---------------------------------------|--------|------------------------------------|
| `beispiel_55001_lieferbeginn.edi`     | 55001  | Lieferbeginn Strom – Anfrage LFN→NB |
| `beispiel_55002_lieferende.edi`       | 55002  | Lieferende Strom – Anfrage LFN→NB  |

### MSCONS (BDEW 2.5, `fv20261001`)

| File                                       | PID   | Description                                |
|--------------------------------------------|-------|--------------------------------------------|
| `beispiel_13002_gas_release_2_5.edi`       | 13002 | Messwerte Zählerstand Gas (release 2.5)    |
| `beispiel_13002_release_2_4c.edi`          | 13002 | Messwerte Zählerstand Strom (release 2.4c) |

### APERAK (BDEW 2.2, `fv20261001`)

| File                                           | PID   | Description                          |
|------------------------------------------------|-------|--------------------------------------|
| `beispiel_29001_verarbeitbarkeitsfehler.edi`   | 29001 | Verarbeitbarkeitsfehler mit FTX       |
| `beispiel_29002_anerkennungsmeldung.edi`       | 29002 | Anerkennungsmeldung                   |

## Which format version a fixture carries

Every release code a counterparty can still send needs at least one fixture
carrying it in `UNH` DE 0057 — `cargo xtask validate-release-codes` fails
otherwise, and the annual-release checklist runs it.

That is more than one code per message type during a transition. MSCONS `2.4c`
runs until 2026-09-30 and `2.5` starts the next day, so both are on the wire
within the same year and both are witnessed here. Only a **superseded** version
needs no fixture: `ReleaseRegistry::is_acceptable_on` refuses it at the BDEW
default receive tolerance of zero days, so retiring its fixtures with it is
correct.

`cargo xtask generate-fixtures` fills gaps for PIDs that have no fixture at all,
stamping each with the release **in force today** — never the newest one shipped.
`valid_from` is a hard edge (Allgemeine Festlegungen 6.1 §2.5): a message stamped
with the next format version is rejected until its Anwendungszeitpunkt, so
generating at the newest release produces a corpus of messages nobody can send
yet and leaves the in-force version unexercised.

## Known Limitations

The current MIG validator uses a **flat segment-sequence** model, which means
that segment tags appearing in multiple segment groups (e.g. `DTM` in both the
message header and the `IDE` group in UTILMD) are treated as a single position
in the expected order.  As a result:

- Fixtures **omit** segment-group-level `DTM`, `NAD`, and similar repeated tags
  that would follow `IDE`/`LOC` in real BDEW Beispielnachrichten.

For two-section messages (containing `UNS`), the detail section now uses a
group-trigger-aware ordering check: when the first tag of the detail section
is seen again (e.g. a second `LOC` in MSCONS), the ordering cursor resets to
allow multiple group occurrences.  Fixtures can therefore include multiple
`LOC` groups in MSCONS messages.

## Market-Participant IDs Used in Fixtures

All IDs are **synthetic** and do not represent real market participants.

| GLN / Code      | Qualifier | Role used in fixtures                   |
|-----------------|-----------|-----------------------------------------|
| `4012345000023` | `14`      | Lieferant / Nachrichten-Sender (MS)     |
| `9900357000004` | `14`      | Netzbetreiber / Empfänger (MR)          |
| `9907317000007` | `14`      | Marktpartner (alternative Sender)       |

## Segment-Count Rules

The `UNT` segment count must equal the number of segments from `UNH` through
`UNT` inclusive (i.e. all segments in the functional group, including `UNH` and
`UNT` themselves).
