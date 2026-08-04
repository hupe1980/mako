+++
title = "DVGW EDI"
description = "dvgw-edi: parsing ALOCAT, NOMINT, NOMRES, SCHEDL, IMBNOT, TRANOT, DELORD, and DELRES for GaBi Gas 2.1. Covers regulatory basis, message taxonomy, version management, semantic validation, parsing architecture, and GaBi Gas workflow integration."
weight = 14
+++
# DVGW EDI

The `dvgw-edi` crate implements EDIFACT parsing for the German gas transport and
balancing market (GaBi Gas 2.1, BNetzA BK7-24-01-008). It is the DVGW counterpart
to the `edi-energy` crate, which covers the BDEW EDI@Energy retail-market layer.

---

## 1. Regulatory Basis

### 1.1 Statutory framework

| Document | Significance |
|---|---|
| **§20 Abs. 3 EnWG** | Festlegungskompetenz for gas network access and balancing; exercised through the BK7 Festlegungen (GasNZV was repealed with effect from the end of 31.12.2025) |
| **GaBi Gas 2.1** (BNetzA **BK7-24-01-008**) | Current ruling. Introduced the two-market-area model, simplified exit-zone products, and mandatory DVGW-format electronic exchange. All production implementations must comply with BK7-24-01-008. |
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

### 3.3 Version handling in `dvgw-edi`

Unlike `edi-energy` (which keys compiled-in MIG/AHB JSON profiles by an
`FV<YYYY>-<MM>-<DD>` directory), `dvgw-edi` has **no profile-JSON layer** and no
per-version profile directory. There is exactly one typed constructor per message
family, and it is version-agnostic within that family:

- `DvgwPlatform::parse` tokenises the interchange, reads the message type from the
  UNH segment (DE 0065, component 0) via `DvgwMessageType::from_unh_code`, and
  dispatches to the single concrete constructor for that type (see
  [section 5.1](#5-1-edifact-tokeniser)).
- The wire version string in UNH DE 0057 (association assigned code) is captured
  as [`DvgwVersion`](https://github.com/hupe1980/mako/blob/main/crates/dvgw-edi/src/version.rs) — `DvgwVersion::parse`
  accepts any non-empty value and round-trips the raw string faithfully — but it
  does **not** select a different code path.

Because the typed constructors read the segment fields directly, FK
(Fehlerkorrektur) editorial corrections require no code change, and the biannual
structural releases are absorbed by the same constructor as long as the field
positions the constructor reads are unchanged.

---

## 4. Validation & PID Routing

### 4.1 Semantic rule packs

`dvgw-edi` has no compiled-in MIG/AHB JSON profiles. Conformance checking is done
by in-code **semantic rule packs** built in
[`crates/dvgw-edi/src/validate.rs`](https://github.com/hupe1980/mako/blob/main/crates/dvgw-edi/src/validate.rs).

`DvgwPlatform::validate` runs two passes:

1. **Envelope validation** — when a UNB/UNZ interchange wrapper is present, it is
   checked with `edifact_rs::validate_envelope_lenient_owned`. A structurally
   unrecoverable interchange returns `Err(Error::Parse(…))`; count-only mismatches
   are folded into the report as issues (rule id `ENVELOPE-COUNT-MISMATCH`).
2. **Semantic validation** — a per-message-type `edifact_rs::ProfileRulePack` is
   built by a `*_pack()` function (`alocat_pack`, `nomint_pack`, `nomres_pack`,
   `schedl_pack`, `imbnot_pack`, `tranot_pack`, `delord_pack`, `delres_pack`), each
   gated behind its message-type Cargo feature. The pack registers stateless rule
   closures that check DVGW mandatory elements — BGM presence, the `NAD+MS` /
   `NAD+MR` role segments, message-specific `DTM` timing qualifiers (e.g.
   `DTM+137` Gasdatum for ALOCAT/NOMINT), and correlation references. The pack is
   fed to `ValidationContext::builder().with_profile_pack(pack)…validate_lenient_owned`.

Findings are returned as `DvgwIssue` items in a [`DvgwReport`](https://github.com/hupe1980/mako/blob/main/crates/dvgw-edi/src/report.rs)
rather than as hard errors, so a message that parses but violates a semantic rule
still yields a struct plus a list of issues. Rule closures emit
`ValidationSeverity::Error` / `Warning` with a stable `rule_id`
(e.g. `SEM-ALOCAT-DTM-137-REQUIRED`, `SEM-ALOCAT-LOC-EXPECTED`).

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

The parser extracts the flat LOC/QTY/STS/DTM groups via the
`AlocatMessage::quantities` field, exposing each allocated quantity with its
status qualifier and time window.

### 5.3 NOMINT/NOMRES correlation

Nominations use a two-message round-trip correlated by document reference:

1. **NOMINT** — the `nomination_ref` field holds the BGM document number
   (BGM element 1, composite C106, component 0). This is the NOMINT's own
   reference and the key the NOMRES will cite.

2. **NOMRES** — the `nomination_ref` field holds the `RFF+Z13:<value>` that
   back-references the originating NOMINT.

Correlate `nomres.nomination_ref == nomint.nomination_ref` to route the response
to the correct outbound nomination workflow via `ProcessRegistry::lookup_by_correlation`.

#### KoV deadline enforcement

Nomination window deadlines are gas-day-specific per the Kooperationsvereinbarung
Gas (KoV). `mako-gabi-gas` exposes these as typed helper methods on `GasDay`:

| Deadline | Per KoV | Helper method |
|---|---|---|
| NOMINT submission | D-1 13:00 CET | `GasDay::nomination_deadline_utc()` |
| NOMRES response window | D-1 15:00 CET | `GasDay::nomres_deadline_utc()` |
| Initial ALOCAT | D+3 12:00 CET (KoV §6.4) | `GasDay::initial_alocat_deadline_utc()` |
| Final ALOCAT | M+2 last day (KoV §6.4) | `GasDay::final_alocat_deadline_utc()` |

These are enforced in `mako-gabi-gas` by the workflow deadline layer, not by
the parser.

### 5.4 Decimal-safe quantity parsing

Gas energy quantities require high precision (DVGW G 685 §7 mandates ≥ 3 decimal
places), and binary floating point cannot represent those decimal fractions
exactly. Every quantity type therefore exposes exactly one accessor, returning a
`Decimal`:

```rust
// Returns Option<Decimal> — exact, no float rounding
if let Some(kwh) = qty.quantity_decimal() { ... }
```

The `decimal` feature (enabled by default) provides `quantity_decimal()` on
`AlocatQuantity`, `NomintQuantity`, `NomresQuantity`, `SchedlQuantity`,
`ImbalanceEntry`, `DeliveryOrderLine` and `DeliveryResponseLine`. Feed its value
directly into `GasQuantity` or `GasBeschaffenheit::to_kwh_hs()`.

---

## 6. GaBi Gas Workflow Integration

### 6.1 INVOIC billing (live)

`GaBiGasInvoicWorkflow` in `mako-gabi-gas` handles the INVOIC PIDs:

| PID | Process | Direction | Crate |
|---|---|---|---|
| 31010 | Kapazitätsrechnung (capacity billing) | FNB/VNB → BKV | `mako-gabi-gas` |
| 31007 | Aggreg. MMM-Rechnung Gas | NB → MGV | `mako-gabi-gas` |
| 31008 | MMM-Rechnung Gas selbst ausgestellt | NB → MGV | `mako-gabi-gas` |

> **PID 31011 is NOT a GaBi Gas billing.** PID 31011 (Rechnung sonstige Leistung,
> AWH Sperrprozesse Gas, NB → LF) is the GeLi Gas billing for grid operator
> charges incurred during gas disconnection processes. It belongs to `mako-geli-gas`
> per BK7-24-01-009. The distinction matters: GaBi Gas (BK7-24-01-008) covers transport
> and balancing between FNB/MGV/BKV; GeLi Gas (BK7-24-01-009) covers retail gas
> market communication between LFG/GNB.

### 6.2 Gas domain model (`mako-gabi-gas`)

The `mako-gabi-gas` crate exposes a rich, regulation-accurate domain vocabulary:

| Type | Purpose |
|---|---|
| `GasDay` | Typed gas market day (DST-aware, 06:00 CET start, 23/25-hour DST days) |
| `GasQuantity` | Decimal-precision kWh_Hs with m³ + conversion metadata |
| `GasBeschaffenheit` | Brennwert (Hs/Hu) + Zustandszahl; `.validate()` checks DVGW G 260 ranges |
| `GasQualityFlag` | 7-state quality flag (Measured/Estimated/Substituted/Calculated/Corrected/Rejected/Unknown) per § 60 Abs. 2 MsbG |
| `AllocationVersion` | Initial/Correction(n)/Final per KoV §6.4 |
| `GasMarketRole` | 9-role typed enum (LF, NB, FNB, VNB, BKV, MGV, MSB, Händler, TNB) |
| `GasImbalanceSaldo` | Mehr/Minder/Balanced with `ausgleichsenergie_price_ct_per_kwh` per KoV §9 |
| `GasPortfolioBalance` | BKV portfolio across Bilanzkreise; `conservation_check()` per GaBi Gas 2.1 (BK7-24-01-008) |
| `cloud_events` | Typed `de.gabi.*` CloudEvent constants for all 12 domain events |
| `dvgw_versions` | Biannual DVGW format version tracking (ALOCAT 5.11a / NOMINT 4.6 FK / …) |

### 6.3 Implementation patterns

The `dvgw-edi` / `mako-gabi-gas` crates follow the same workflow conventions as
all other domain workflow crates in this workspace:

| Concern | Reference |
|---|---|
| Workflow state machine | `crates/mako-gabi-gas/src/invoic.rs` |
| `on_deadline` dispatch | `services/makod/src/orchestrator/deadline_dispatch.rs` |
| Adapter registry | `services/makod/src/orchestrator/adapters/mod.rs` |
| Startup validation | `services/makod/src/main.rs` — `adapter.validate_policy()` |
| `DISPATCH_TABLE` enforcement | `deadline_dispatch::assert_dispatch_coverage()` |

**Two validation layers, two crates.** `mako-gabi-gas` also carries the
edi-energy EDIFACT message types (MSCONS 13013 MMMA, INVOIC), which *are*
validated through the AHB/MIG profile-JSON layer
(`crates/edi-energy/profiles/mscons/fv20251001/{mig,ahb}.json`). The DVGW
transport messages in `dvgw-edi` itself (ALOCAT, NOMINT, NOMRES, …) have **no
profile-JSON layer** — they are parsed and validated entirely in typed Rust code
(see §3.3, §4.1).

---

## References

| Resource | URL / Path |
|---|---|
| DVGW GaBi Gas message index | <https://www.dvgw-sc.de/leistungen/it-dienstleistungen/datenaustausch-gas/gabi-gastransport> |
| DVGW document archive | <https://www.dvgw-sc.de/leistungen/it-dienstleistungen/datenaustausch-gas/dokumentenarchiv> |
| DVGW version management rules | <https://www.dvgw-sc.de/leistungen/it-dienstleistungen/datenaustausch-gas/gabi-versionsmanagement> |
| ALOCAT specification | ALOCAT 5.11a, Stand 2024-04-02 (DVGW Dokumentenarchiv) |
| NOMINT specification | NOMINT 4.6, Stand 2026-02-01 Fehlerkorrektur (DVGW Dokumentenarchiv) |
| NOMRES specification | NOMRES 4.7, Stand 2026-02-01 Fehlerkorrektur (DVGW Dokumentenarchiv) |
| GaBi Gas 2.1 Festlegung | BNetzA BK7-24-01-008 |
| `dvgw-edi` source | [crates/dvgw-edi/](https://github.com/hupe1980/mako/tree/main/crates/dvgw-edi) |
| `mako-gabi-gas` source | [crates/mako-gabi-gas/](https://github.com/hupe1980/mako/tree/main/crates/mako-gabi-gas) |
| Process engine guide | [docs/engine.md](engine.md) |

---
