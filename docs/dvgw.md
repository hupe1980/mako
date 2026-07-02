---
layout: default
title: DVGW EDI
nav_order: 40
parent: Architecture
description: >
  dvgw-edi: parsing ALOCAT, NOMINT, NOMRES, SCHEDL, IMBNOT, TRANOT, DELORD, and DELRES
  for GaBi Gas 2.0. Covers regulatory basis, message taxonomy, version management,
  profile schema, parsing architecture, and GaBi Gas workflow integration.
---

# DVGW EDI

The `dvgw-edi` crate implements EDIFACT parsing for the German gas transport and
balancing market (GaBi Gas 2.0, BNetzA BK7-14-020). It is the DVGW counterpart
to the `edi-energy` crate, which covers the BDEW EDI@Energy retail-market layer.

---

## 1. Regulatory Basis

### 1.1 Statutory framework

| Document | Significance |
|---|---|
| **GasNZV** (Gasnetzzugangsverordnung) | Statutory basis for gas network access and balancing; delegates technical implementation to the BNetzA |
| **GaBi Gas 2.0** (BNetzA **BK7-14-020**) | Current ruling. Introduced the two-market-area model, simplified exit-zone products, and mandatory DVGW-format electronic exchange. All production implementations must comply with BK7-14-020. |
| **Kooperationsvereinbarung Gas** (KoV) | Industry agreement between all German gas network operators (§ 20 Abs. 1b EnWG), mandating the use of DVGW EDIFACT formats for balancing and transport processes |
| **DVGW G 685** | Technical standard for gas metering and allocation calculations |

### 1.2 Governance authority

DVGW Projektkreis (PK) Datenaustausch develops, maintains, and publishes all
DVGW EDIFACT message types under the label **EDI-DVGW**. The DVGW Service &
Consulting GmbH (DVGW S&C) hosts the canonical publication portal:

> <https://www.dvgw-sc.de/leistungen/it-dienstleistungen/datenaustausch-gas>

**Key distinction from EDI@Energy:** BDEW EDI@Energy governs retail gas market
communication (UTILMD G, GeLi Gas, WiM Gas). DVGW governs the *transport and
balancing* layer — the wholesale TSO/MGV/BKV processes that BDEW does not cover.

---

## 2. Message Taxonomy

### 2.1 GaBi Gas balancing messages

All use EDIG@S-derived UN/EDIFACT segment vocabulary.

| Message | Version | Valid from | UN/EDIFACT base | Description |
|---|---|---|---|---|
| **ALOCAT** | 5.11a | 2024-10-01 | D03A | Allokationsnachricht — gas quantity allocation per exit zone, entry point, or measurement point |
| **NOMINT** | 4.6 FK | 2026-02-01 | D01B | Nominierungsintegration — aggregated nomination submitted by BKV to FNB/MGV |
| **NOMRES** | 4.7 FK | 2026-02-01 | D01B | Nominierungsantwort — FNB/MGV response confirming or rejecting a nomination |
| **SCHEDL** | 4.4 FK | 2026-02-01 | D01B | Schedulingnachricht — transport schedule for a gas day |
| **IMBNOT** | 5.7a | 2023-10-01 | D03A | Imbalance notification — intraday balance status communicated by MGV/BKV |
| **TRANOT** | 5.8b | 2023-10-01 | D01B | Transport notification (FNB → BKV) |
| **DELORD** | 4.5 FK | 2026-02-01 | D01B | Delivery order (BKV → FNB) |
| **DELRES** | 4.6 FK | 2026-02-01 | D01B | Delivery response (FNB → BKV) |
| **CHACAP** | 4.6 FK | 2026-02-01 | D01B | Capacity change notification |
| **NÜVOR** | 1.1 FK | 2024-02-01 | — | Netznutzungsvoraussetzungserfüllung |
| **SSQNOT** | 5.7 FK | 2021-12-01 | D03A | Storage sequence notification |
| **SLPASP** | 1.1 FK | 2019-12-01 | — | SLP Speiserichtung |

**FK** = Fehlerkorrektur — editorial correction only; no structural change to the parser.

### 2.2 Acknowledgement layer (shared with `edi-energy`)

DVGW adopted the BDEW CONTRL/APERAK pattern starting 2009:

| Message | Version | Role |
|---|---|---|
| **CONTRL** | 1.3b | Syntax-level interchange acknowledgement |
| **APERAK** | 2.0b | Application-level acknowledgement / error response |

These are specified in `edi-energy` profiles and are **not** reimplemented in
`dvgw-edi`. See "Ergänzungsblatt zur APERAK und CONTRL für die Nutzung in GaBi
Prozessen" on [edi-energy.de](http://www.edi-energy.de/).

### 2.3 Deprecated formats (out of scope)

DVGW explicitly states these formats are no longer maintained:
`AVAILY`, `REQUEST`, `REQRES`, `CAPNOT`, `CAPRES`, `INTORD`, `INTRES`

They have no governing process description and will never be updated. They appear
in the DVGW document archive for historical reference only.

---

## 3. Version Management

### 3.1 Release cycle

DVGW uses biannual implementation cutover dates:

- **1 April, 06:00 CET** and **1 October, 06:00 CET**

All market participants must use the package current at the time of transmission.
There is no multi-year coexistence period analogous to the BDEW `FV2025-10-01` /
`FV2026-10-01` split — only the latest active version applies.

### 3.2 Version vs. release numbering

| Change type | Number bumped | Example |
|---|---|---|
| Structural (codelist change, new segments, new UN/EDIFACT directory) | **Version** (X.Y) | NOMINT 4.5 → NOMINT 4.6 |
| Editorial (wording, layout, documentation only) | **FK suffix** | NOMINT 4.6 → NOMINT 4.6 FK |

`FK` (Fehlerkorrektur) means the release was incremented for editorial reasons
only — no parser changes are required. The profile content is updated in-place
since the segment structure is unchanged.

### 3.3 Profile management in `dvgw-edi`

Unlike `edi-energy` (which uses `FV<YYYY>-<MM>-<DD>` as the profile directory
key), `dvgw-edi` profiles are keyed per message type and version:

```
crates/dvgw-edi/profiles/
  alocat/
    v5_11a/
      mig.json
      ahb.json
  nomint/
    v4_6/
      mig.json
      ahb.json
  nomres/
    v4_7/
      mig.json
      ahb.json
  schedl/
    v4_4/
      mig.json
  imbnot/
    v5_7a/
      mig.json
  tranot/
    v5_8b/
      mig.json
  delord/
    v4_5/
      mig.json
  delres/
    v4_6/
      mig.json
```

A `valid_from` field in each `mig.json` records when the version became mandatory.
FK corrections update the profile content in-place.

---

## 4. Profile Schema

### 4.1 Schema design

`dvgw-edi` profiles use the **exact same `mig.json` / `ahb.json` schema** as
`edi-energy` (`schema_version: 1`). Two DVGW-specific `mig.json` fields:

```json
{
  "schema_version": 1,
  "message_type": "ALOCAT",
  "release": "5.11a",
  "valid_from": "2024-10-01",
  "dvgw_source": "ALOCAT 5.11a Stand 02.04.2024",
  "segments": [ /* ... */ ],
  "segment_groups": [ /* ... */ ]
}
```

- `dvgw_source`: literal document title from the DVGW publication, for traceability
- `valid_from`: ISO-8601 date the version became mandatory

### 4.2 Synthetic PID routing

DVGW messages carry no BGM DE 1004 Prüfidentifikator. The routing discriminant
is `(message_type, role_qualifier)` — the sender/receiver EIC type from NAD+MS/MR.

To keep the `mako-engine` PID router uniform, a synthetic PID encodes this pair:

| Synthetic PID | Message | Role / Direction |
|---|---|---|
| 90001 | ALOCAT | FNB → BKV (daily allocation) |
| 90002 | ALOCAT | MGV → BKV (monthly allocation) |
| 90003 | ALOCAT | VNB → FNB (sub-daily allocation) |
| 90011 | NOMINT | BKV → FNB (nomination) |
| 90012 | NOMINT | BKV → MGV (nomination) |
| 90021 | NOMRES | FNB → BKV (nomination response) |
| 90022 | NOMRES | MGV → BKV (nomination response) |
| 90031 | SCHEDL | FNB → BKV (schedule) |
| 90041 | IMBNOT | MGV → BKV (intraday imbalance) |
| 90051 | TRANOT | FNB → BKV (transport notification) |
| 90061 | DELORD | BKV → FNB (delivery order) |
| 90062 | DELRES | FNB → BKV (delivery response) |

Range `90000–90999` is reserved for DVGW synthetic PIDs. It will never collide
with BDEW PIDs (10000–99999, documented in PID 3.3 / PID 4.0).

Use `AnyDvgwMessage::detect_pid(role_qualifier)` in application code:

```rust
use dvgw_edi::DvgwPlatform;

let msg = DvgwPlatform::default().parse(&raw_bytes)?;
let pid = msg.detect_pid(Some("Z01")); // BKV → FNB nomination → Some(90011)
```

---

## 5. Parsing Architecture

### 5.1 EDIFACT tokeniser

`dvgw-edi` does not contain its own EDIFACT tokeniser. It depends on
`edifact-rs` for the segment iterator. The key API:

```toml
[dependencies]
edifact-rs = { workspace = true }
thiserror  = { workspace = true }
```

`DvgwPlatform::parse(&[u8])` tokenises with `edifact_rs::from_bytes_owned_with_config`,
extracts the UNH message type, and dispatches to the appropriate typed message
constructor. This ensures consistent EDIFACT parsing rules and DoS limits
(`ReaderConfig`) across all message families.

### 5.2 ALOCAT segment structure

ALOCAT 5.11a is the most structurally complex DVGW format, with up to
**7 nesting levels** (SG1…SG14). Key groups:

| Group | Trigger segment | Description |
|---|---|---|
| SG1 | RFF | Reference (clearing number, contract ref) |
| SG2 | NAD | Market participant (FNB, BKV, MGV) |
| SG3 | DTM | Period reference |
| SG4 | LOC | Entry/exit zone or measurement point |
| SG5 | QTY | Allocated quantity per period |
| SG6 | DTM | Quantity-level time window |
| SG7 | STS | Status qualifier (e.g. preliminary / final) |
| SG8–SG14 | Various | Measurement point details, contract refs |

The current parser extracts the flat LOC/QTY/STS/DTM groups via the
`AlocatMessage::quantities` field. Full group-hierarchy parsing is added when
profile JSON is committed.

### 5.3 NOMINT/NOMRES correlation

Nominations use a two-message round-trip correlated by document reference:

1. **NOMINT** — the `nomination_ref` field holds the BGM document number
   (BGM element 1, composite C106, component 0). This is the NOMINT's own
   reference and the key the NOMRES will cite.

2. **NOMRES** — the `nomination_ref` field holds the `RFF+Z13:<value>` that
   back-references the originating NOMINT.

Correlate `nomres.nomination_ref == nomint.nomination_ref` to route the response
to the correct outbound nomination workflow via `ProcessRegistry::lookup_by_correlation`.

Nomination window deadlines are gas-day-specific per the Kooperationsvereinbarung
Gas (KoV): submission by **D-1 13:00 CET**, re-nomination by **D+0 10:00 CET**.
These are enforced in `mako-gabi-gas` by the workflow deadline layer, not by
the parser.

---

## 6. GaBi Gas Workflow Integration

### 6.1 INVOIC billing (live)

`GaBiGasInvoicWorkflow` in `mako-gabi-gas` handles PID 31010:

| PID | Process | Direction | Crate |
|---|---|---|---|
| 31010 | Kapazitätsrechnung (capacity billing) | FNB/VNB → BKV | `mako-gabi-gas` |

> **PID 31011 is NOT a GaBi Gas billing.** PID 31011 (Rechnung sonstige Leistung,
> AWH Sperrprozesse Gas, NB → LF) is the GeLi Gas billing for grid operator
> charges incurred during gas disconnection processes. It belongs to `mako-geli-gas`
> per BK7-24-01-009. The distinction matters: GaBi Gas (BK7-14-020) covers transport
> and balancing between FNB/MGV/BKV; GeLi Gas (BK7-24-01-009) covers retail gas
> market communication between LFG/GNB.

PID 31010 uses the standard BDEW INVOIC format handled by `edi-energy`'s INVOIC
profile. It is independent of the DVGW formats (ALOCAT, NOMINT, NOMRES, etc.).

### 6.2 Implementation patterns

The `dvgw-edi` / `mako-gabi-gas` crates follow the same conventions as all
other domain workflow crates in this workspace:

| Concern | Reference |
|---|---|
| Workflow state machine | `crates/mako-gabi-gas/src/invoic.rs` |
| `on_deadline` dispatch | `services/makod/src/deadline_dispatch.rs` |
| Adapter registry | `services/makod/src/adapters.rs` |
| Startup validation | `services/makod/src/main.rs` — `adapter.validate_policy()` |
| `DISPATCH_TABLE` enforcement | `deadline_dispatch::assert_dispatch_coverage()` |
| Profile JSON schema | `crates/edi-energy/profiles/mscons/fv20251001/mig.json` |
| AHB JSON schema | `crates/edi-energy/profiles/mscons/fv20251001/ahb.json` |

---

## References

| Resource | URL / Path |
|---|---|
| DVGW GaBi Gas message index | <https://www.dvgw-sc.de/leistungen/it-dienstleistungen/datenaustausch-gas/gabi-gastransport> |
| DVGW document archive | <https://www.dvgw-sc.de/leistungen/it-dienstleistungen/datenaustausch-gas/dokumentenarchiv> |
| DVGW version management rules | <https://www.dvgw-sc.de/leistungen/it-dienstleistungen/datenaustausch-gas/gabi-versionsmanagement> |
| ALOCAT 5.11a PDF | [docs/pdfs/dvgw/ALOCAT_5.11a_Stand_2024-04-02.pdf](pdfs/dvgw/ALOCAT_5.11a_Stand_2024-04-02.pdf) |
| NOMINT 4.6 FK PDF | [docs/pdfs/dvgw/NOMINT_4.6_Stand_2026-02-01_Fehlerkorrektur.pdf](pdfs/dvgw/NOMINT_4.6_Stand_2026-02-01_Fehlerkorrektur.pdf) |
| NOMRES 4.7 FK PDF | [docs/pdfs/dvgw/NOMRES_4.7_Stand_2026-02-01_Fehlerkorrektur.pdf](pdfs/dvgw/NOMRES_4.7_Stand_2026-02-01_Fehlerkorrektur.pdf) |
| BNetzA BK7-14-020 (GaBi Gas 2.0) | [docs/pdfs/bentza/](pdfs/bentza/) |
| `dvgw-edi` source | [crates/dvgw-edi/](../crates/dvgw-edi/) |
| `mako-gabi-gas` source | [crates/mako-gabi-gas/](../crates/mako-gabi-gas/) |
| Process engine guide | [docs/engine.md](engine.md) |

---
