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

### APERAK (BDEW 2.2, `fv20261001`)

| File                                           | PID   | Description                          |
|------------------------------------------------|-------|--------------------------------------|
| `beispiel_29001_verarbeitbarkeitsfehler.edi`   | 29001 | Verarbeitbarkeitsfehler mit FTX       |
| `beispiel_29002_anerkennungsmeldung.edi`       | 29002 | Anerkennungsmeldung                   |

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
