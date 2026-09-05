+++
title = "Profile Files"
description = "What a profile is: mig.json and ahb.json, generated from the BDEW MIG and AHB PDFs, keyed by the MIG's running segment number."
weight = 10
+++
A profile is one MIG and one AHB of one format version, as two files under
`crates/edi-energy/profiles/<type>/<fvYYYYMMDD>/`. Both are **generated** by
`cargo xtask import-profiles` from the BDEW PDFs named in
`profiles/sources.json`, and both carry `schema_version: 2`.

## `mig.json` — the Nachrichtenbeschreibung

| Field | Content |
|---|---|
| `message_type`, `release`, `track` | `UTILMD`, the `UNH` DE 0057 wire code, `Strom`/`Gas` where one message type has two MIGs |
| `valid_from`, `valid_until`, `publikationsdatum`, `ahb_version` | the Anwendungszeitpunkt window and the AHB the MIG pairs with |
| `pid_source`, `pid_exempt` | where the Prüfidentifikator travels — `rff_z13` for fifteen of the seventeen message types, `bgm_de1004` for APERAK and CONTRL. `pid_exempt` marks a profile that publishes none at all (CONTRL), which `check-release-coverage` reads |
| `source` | the PDF's file name, title and SHA-256 |
| `structure` | the Nachrichtenstruktur as a tree of groups (`group`, `status`, `max`, `children`) and segments (`nr`, `tag`, `status`, `max`, `elements`) |
| `envelope` | the `UNB`/`UNZ` layouts, outside the message |

Every segment carries the MIG's running number `Nr` (`00047`) and its
Segmentlayout: data elements with BDEW status (`M`, `R`, `C`, `D`, `O`, `N`;
groups and segments use `M`/`R`/`D`/`O`), format
(`an..35`, `n11`), note and admitted codes, composites with their components.
The `Nr` is what tells two places of the same tag apart — `SG5 LOC+Z16`
Marktlokation and `SG5 LOC+Z17` Messlokation are two nodes with two layouts.

## `ahb.json` — the Anwendungshandbuch

| Field | Content |
|---|---|
| `conditions` | every Bedingung as printed, by number: Voraussetzungen `1`–`499`, Hinweise `500`–`899`, Formatbedingungen `901`–`999`, Wiederholbarkeiten `2000`–`2499`, `UB1`–`UB3` |
| `packages` | Pakete (`1P`) with their Paketvoraussetzung |
| `anwendungsfaelle` | one entry per AHB column — the Prüfschablone |

A Prüfschablone has `pid`, `name`, `communication`, `chapter`, `rows` (the
status of each segment `nr` or group — `Muss`, `Muss [10]`, `Soll [3] ∧ [4]`)
and `elements` (per `nr` and data element, the operands on its codes or on the
value — `X`, `X [UB1]`, `M [7]`). **CONTRL** is the one message type whose AHB
publishes no Prüfidentifikatoren: its three columns carry no `pid` and are named
`col1`–`col3`, selected by best fit. APERAK does publish them — `29001`
Annahme and `29002` Ablehnung.

## What the runtime does with them

- `Structure` compiles the MIG into a resolver that assigns every segment of a
  message to its `Nr` — the leading qualifier tells places apart, a stray
  segment is reported and skipped rather than derailing the rest.
- `Profile::validate` runs the MIG checks (structure, cardinality, layout) and
  the column's Prüfschablone, and evaluates Voraussetzungen against the message
  (`Wenn SG5 LOC+Z17 nicht vorhanden`, `Wenn SG10 QTY DE6063 mit Wert 67
  vorhanden`, `mehr als einmal vorhanden`).
- `Profile::skeleton` generates the minimal message a column admits; every
  Anwendungsfall's skeleton validating against its own column is the test that
  extraction and validator agree (`tests/skeletons.rs`).
- `Profile::pruefschablone(pid)` prints the column for a reader.

## Versioning

`schema_version` is the shape of these files. A new shape is a new number, a
re-import of every profile and a matching change in `crates/edi-energy/src/profile/model.rs`;
nothing reads an older shape. Additive fields are `#[serde(default)]` and need
no bump.

`validate-profiles` holds the committed files against `sources.json` (release,
dates, AHB version, document names), against each other (every AHB row names a
`Nr` the MIG has, every column lists `UNH`, Prüfidentifikatoren are five digits
and unique) and, where the document mirror is present, against the mirror's
SHA-256. `import-profiles --check` re-reads the PDFs and fails on any drift.
