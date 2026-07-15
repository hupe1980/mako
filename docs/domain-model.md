---
layout: default
title: Domain Model
nav_order: 13
parent: Architecture
description: >
  BDEW market role model, market objects (MaLo, MeLo, NeLo, NeBe), territory
  definitions, identifier formats with check-digit rules, and EDIFACT encoding
  for all identifiers used in German energy market communication.
---

# Domain Model — Market Roles, Objects, and Identifiers

This page is the definitive reference for the BDEW **Rollenmodell für die
Marktkommunikation im deutschen Energiemarkt** and all identifier types used
across EDI@Energy messages. Every developer working with this library will need
this material to understand what a MaLo is, why the NB sends UTILMD messages on
behalf of the LF, and how to parse a `9900357000004` out of a NAD segment.

**Source documents:**

| Document | Version | Date |
|---|---|---|
| Rollenmodell für die Marktkommunikation im deutschen Energiemarkt | **V2.2** | 2026-01-08 |
| Identifikatoren in der Marktkommunikation | **V1.2** | 2025-02-07 |
| Allgemeine Festlegungen zu den EDIFACT- und XML-Nachrichten | **6.1d** | 2026-04-01 |

---

## Table of Contents

1. [Party Roles (Marktrollen)](#party-roles-marktrollen)
2. [Market Objects (Objekte)](#market-objects-objekte)
3. [Territories (Gebiete)](#territories-gebiete)
4. [Identifier Formats](#identifier-formats)
   - [MP-ID — Marktpartner (13 digits)](#mp-id--marktpartner-13-digits)
   - [MaLo-ID — Marktlokation (11 digits)](#malo-id--marktlokation-11-digits)
   - [MeLo-ID — Messlokation (Zählpunktbezeichnung)](#melo-id--messlokation-zählpunktbezeichnung)
   - [NeLo-ID — Netzlokation (11 chars)](#nelo-id--netzlokation-11-chars)
   - [NeBe-ID — Netzbereich (11 chars)](#nebe-id--netzbereich-11-chars)
   - [Ressourcen-ID — TR / SR / SG / CR (11 chars)](#ressourcen-id--tr--sr--sg--cr-11-chars)
   - [Paket-ID — Netzbetreiberwechsel (11 chars)](#paket-id--netzbetreiberwechsel-11-chars)
5. [Check Digit Algorithms](#check-digit-algorithms)
6. [EDIFACT Encoding](#edifact-encoding)
7. [Rust API](#rust-api)

---

## Party Roles (Marktrollen)

The market role model defines who the actors are. One company can hold multiple
roles simultaneously (e.g., a DSO may also be its own MSB). A distinct MP-ID is
required for each role per commodity (Strom / Gas).

> **Source:** RME V2.2 §3.1

| Abbr. | German Name | English | Strom | Gas | Definition |
|---|---|---|:---:|:---:|---|
| **LF** | Lieferant | Energy Supplier | ✅ | ✅ | Responsible for supplying energy to market locations, settling billing with the DSO, and financially compensating the balance between profiled and metered energy quantities. |
| **NB** | Netzbetreiber | Distribution System Operator (DSO) | ✅ | ✅ | Responsible for grid operation, grid maintenance, and routing of energy. Creates and manages market locations, metering locations, and technical resources within the grid area. Aggregates energy quantities for settlement. Gas: responsible for forwarding metering values to trading partners. |
| **ÜNB** | Übertragungsnetzbetreiber | Transmission System Operator (TSO) | ✅ | — | Responsible for transmission grid stability, EEG allocation time-series, and short-term plausibility checks within the control zone. One TSO = one Regelzone. In MABIS: bilateral MSCONS exchange with BKV (PID 13003). |
| **MSB** | Messstellenbetreiber | Metering Point Operator | ✅ | ✅ | Responsible for installing, operating, and maintaining meters (iMSB = incumbent, gMSB = competitive). Strom: distributes metered values, substitute values, and preliminary values to authorised partners. Gas: determines and forwards metering values to the DSO. |
| **BKV** | Bilanzkreisverantwortlicher | Balance Responsible Party (BRP) | ✅ | ✅ | Responsible for the energetic and financial balance within a Bilanzkreis. Counterparty to the BIKO (Strom) or MGV (Gas). |
| **BIKO** | Bilanzkoordinator | Balance Coordinator | ✅ | — | Responsible for Bilanzkreisabrechnung (balance-circle settlement) and financial settlement between BKVs. See `mako-mabis` (PID 13003). |
| **MGV** | Marktgebietsverantwortlicher | Market Area Manager | — | ✅ | Responsible for gas balance circle settlement and procurement/dispatch of balancing energy. Operates the virtual trading hub. |
| **KN** | Kapazitätsnutzer | Capacity User | — | ✅ | Acquires transport capacity at bookable entry/exit points in the gas entry-exit system and allocates it to Bilanzkreise. |
| **BTR** | Betreiber einer technischen Ressource | Technical Resource Operator | ✅ | — | Installs, operates, and maintains technical resources (generators, controllable loads). Does not change with DSO ownership transfer. |
| **EIV** | Einsatzverantwortlicher | Dispatch Responsible Party | ✅ | — | Responsible for deploying controllable resources. Assigns SR-IDs to steuerable resources. Central actor in Redispatch 2.0. |
| **DP** | Data Provider | Data Provider | ✅ | — | Forwards information to authorised trading partners on behalf of the DSO or MSB. |
| **ESA** | Energieserviceanbieter des Anschlussnutzers | Consumer-side Energy Service Provider | ✅ | — | Acts on behalf of the end-customer (Anschlussnutzer) to request and process metering data, with explicit consumer consent. The ESA must use the data exclusively in the consumer relationship. |
| **RB** | Registerbetreiber | Registry Operator | ✅ | ✅ | Operates a database for energy market data (e.g., the national Marktstammdatenregister). |

### Role pairs in key processes

| Process | Sender (NAD+MS) | Receiver (NAD+MR) | Crate |
|---|---|---|---|
| GPKE Lieferbeginn / Lieferende | LF | NB | `mako-gpke` |
| GPKE Kündigung Lieferbeginn | LF | LFA (outgoing supplier) | `mako-gpke` |
| GPKE Antwort Lieferbeginn | NB | LF | `mako-gpke` |
| GPKE Sperrung / Entsperrung (NB-initiated) | NB | MSB | `mako-gpke` |
| GPKE Sperrung / Entsperrung (LF-initiated, Strom) | LF | NB | `mako-gpke` |
| WiM Gerätewechsel | MSB | NB | `mako-wim` |
| WiM Stammdaten | NB | LF / MSB | `mako-wim` |
| GeLi Gas Lieferbeginn / Lieferende | LF | GNB (gas DSO) | `mako-geli-gas` |
| GeLi Gas Sperrung / Entsperrung (LF-initiated, Gas) | LF | GNB | `mako-geli-gas` |
| WiM Gas Anmeldung / Kündigung gMSB | MSB (gas) | NB (gas) | `mako-wim-gas` |
| MABIS Summenzeitreihe | ÜNB | BKV | `mako-mabis` |
| INVOIC Abrechnung | NB | LF | `mako-gpke` |

---

## Market Objects (Objekte)

Objects are the entities that processes act on. The NB is responsible for
creating and closing objects within the grid area and assigning MP identifiers
to them.

> **Source:** RME V2.2 §3.2; Allgemeine Festlegungen 6.1d §2.15, §2.19

| Abbr. | German | English | Strom | Gas | Definition |
|---|---|---|:---:|:---:|---|
| **MaLo** | Marktlokation | Market Location | ✅ | ✅ | **The central billing entity.** A point where energy is either produced or consumed, connected to a grid via at least one line. The NB is responsible for creating, managing, and closing MaLo. Identified by **MaLo-ID** (11-digit numeric). |
| **MeLo** | Messlokation | Metering Location | ✅ | ✅ | A location where energy is measured, containing all technical equipment required for measurement and value transmission. One MaLo may have one or more MeLo. Each physical quantity is measured at most once per timestamp. Identified by **Zählpunktbezeichnung** (VDE-AR-N 4400 Strom / DVGW G2000 Gas). |
| **NeLo** | Netzlokation | Network Location | ✅ | — | An interconnection point in a grid area. Connects one or more MaLo to the grid via exactly one line. Used for **reactive-power billing** (Blindarbeit) and monitoring power-curve limits. Identified by **NeLo-ID** (11-char alphanumeric, prefix `E`). Introduced by BNetzA BK6-22-128. |
| **NeBe** | Netzbereich | Network Zone | ✅ | — | A sub-area within a grid for managing controllable consumption facilities (§14a EnWG). Identified by **NeBe-ID** (11-char alphanumeric, prefix `F`). Introduced by BNetzA BK6-22-300 / BK8-22/010-A. |
| **BK** | Bilanzkreis | Balance Circle | ✅ | ✅ | An account that balances feed-in and consumption quantities, facilitating energy trading. One BKV manages one or more BK. |
| **NKP** | Netzkopplungspunkt | Grid Coupling Point | ✅ | ✅ | A physical point connecting two grid areas. |
| **TR** | Technische Ressource | Technical Resource | ✅ | — | A physical asset that consumes and/or generates electricity. One TR may be assigned to two MaLo if it both consumes and produces. Identified by **TR-ID** (prefix `D`). |
| **SR** | Steuerbare Ressource | Controllable Resource | ✅ | — | A controllable asset that affects at least one grid connection point. One or more TR are assigned to each SR. Identified by **SR-ID** (prefix `C`). |
| **SG** | Steuergruppe | Control Group | ✅ | — | A grouping of controllable resources for dispatch purposes. Identified by **SG-ID** (prefix `B`). |
| **CR** | Cluster Ressource | Cluster Resource | ✅ | — | A bundle of control groups. Identified by **CR-ID** (prefix `A`). |

### MaLo vs MeLo — the critical distinction

These are **not** interchangeable. Confusion between them is the single most
common domain modelling error in EDI@Energy implementations:

| Aspect | MaLo | MeLo |
|---|---|---|
| **What it models** | Commercial supply point (billing) | Physical measurement device location |
| **Who manages it** | NB — registers and closes | MSB — installs and operates the meter |
| **How many per location** | 1 per supply relationship | 1..n per MaLo |
| **Identifier type** | MaLo-ID (11-digit numeric) | Zählpunktbezeichnung (33-char Strom / 11-char Gas) |
| **EDIFACT context** | UTILMD IDE+Z01, INVOIC | MSCONS, UTILMD (WiM) |
| **Rust type** | `mako_engine::types::MaLo` | `mako_engine::types::MeLo` |

**Business scenario:** When a consumer switches supplier (GPKE Lieferbeginn),
the LF sends a UTILMD referencing the **MaLo-ID**. When the MSB reads the meter
and sends measurements, those go in a MSCONS referencing the **MeLo-ID**
(Zählpunktbezeichnung). They refer to the same physical site but are
structurally different objects in the BDEW role model.

---

## Territories (Gebiete)

Territories are spatial containers. Each DSO operates one or more grid areas.
Each TSO operates one control zone.

> **Source:** RME V2.2 §3.2

| Abbr. | German | English | Strom | Gas | Definition |
|---|---|---|:---:|:---:|---|
| **NG** | Netzgebiet | Grid Area | ✅ | ✅ | A metrologically bounded area within a market area (Gas) or control zone (Strom). May span multiple voltage/pressure levels. Operated by the NB. |
| **BG** | Bilanzierungsgebiet | Balancing Zone | ✅ | — | One or more grid areas consolidated for settlement purposes. The synthetic (SLP) or analytical (RLM) balancing method is applied uniformly within a BG. |
| **MG** | Marktgebiet | Market Area | — | ✅ | Aggregation of gas transport networks sharing a virtual trading hub operated by the MGV. |
| **RZ** | Regelzone | Control Zone | ✅ | — | A bounded area within which one TSO (ÜNB) is responsible for frequency and voltage stability. Each ÜNB operates exactly one RZ. |

---

## Identifier Formats

All market identifiers are:
- **Immutable** once assigned — a MaLo-ID does not change when the DSO changes ownership
- **Centrally issued** by bdew-codes.de (Strom) or codevergabe.dvgw-sc.de (Gas)
- **Locally assigned** to objects by the responsible code holder (NB in most cases)

> **Source:** Identifikatoren in der Marktkommunikation V1.2 (BDEW, 2025-02-07)

---

### MP-ID — Marktpartner (13 digits)

Identifies a trading partner in a specific market role and commodity. One
company holds one MP-ID per role per Sparte.

| Positions | Length | Content |
|---|---|---|
| 1–2 | 2 | Issuer + commodity: `99` = BDEW/Strom, `98` = DVGW/Gas |
| 3 | 1 | Issue mode: `0`–`8` (BDEW), `9` (DVGW) |
| 4–12 | 9 | Sequence number |
| 13 | 1 | Check digit (**Lok- und Waggon-Kennzeichnungsverfahren**) |

**Alternative: GLN (GS1, 13 digits).** When the code holder uses a GS1-issued
Global Location Number, the GS1 check-digit algorithm applies (EAN-13).

**EDIFACT DE3055 qualifier:**
- `293` — BDEW-Codenummer or DVGW-Codenummer
- `9` — GS1 GLN

**Segments:** `UNB DE0004` (sender), `UNB DE0010` (recipient), `NAD DE3035 = MS`
(message sender), `NAD DE3035 = MR` (message recipient).

**Databases:**
- Strom: <https://bdew-codes.de/Codenumbers/BDEWCodes/CodeOverview>
- Gas: <https://codevergabe.dvgw-sc.de/MarketParticipants>

---

### MaLo-ID — Marktlokation (11 digits)

Identifies a supply point (electricity or gas — same pool) for the life of the
market location. The first digit indicates which code authority issued the ID
but does **not** indicate the Sparte (Strom or Gas).

| Position | Length | Content |
|---|---|---|
| 1 | 1 | Issuer: `4`–`9` = BDEW, `1`–`3` = DVGW |
| 2–10 | 9 | Sequence number (auto-assigned) |
| 11 | 1 | Check digit (**Lok- und Waggon-Kennzeichnungsverfahren**) |

**Who assigns:** NB (network operator), who requests blocks of MaLo-IDs from
bdew-codes.de or codevergabe.dvgw-sc.de.

**Key rule:** The same MaLo-ID identifies the location regardless of whether
the grid was transferred to a new DSO — the NB keeps the ID.

**Examples:** `51238696781`, `40130000558`

---

### MeLo-ID — Messlokation (Zählpunktbezeichnung)

The metering location identifier is the **Zählpunktbezeichnung** (metering code).
It is **not** covered by the Identifikatoren AWH — format is defined by
technical standards:

| Commodity | Standard | Typical length | Format description |
|---|---|---|---|
| **Strom** | VDE-AR-N 4400 §6 (MeteringCode) | 33 characters | Country code (2) + issuer (11) + sequence + check char |
| **Gas** | DVGW G2000 | 11 characters | Issuer code + sequence |

**Strom example:** `DE0000123400007002500000000001234`  
(DE = Germany; remainder identifies the DSO grid area and metering point sequence)

**Segments:** Referenced in MSCONS `LOC`, UTILMD WiM `IDE+Z01`, ORDERS/ORDRSP `LOC`.

---

### NeLo-ID — Netzlokation (11 chars)

Strom only. Issued since **15 February 2023** (per BNetzA BK6-22-128).
Used for reactive-power billing and power-curve limit monitoring.

| Position | Length | Content |
|---|---|---|
| 1 | 1 | Type code: always `E` |
| 2–10 | 9 | Alphanumeric sequence (A–Z, 0–9), auto-assigned |
| 11 | 1 | Check digit (**ASCII-Verfahren**) |

**Issuer:** bdew-codes.de only.

---

### NeBe-ID — Netzbereich (11 chars)

Strom only. Issued since **20 February 2025** (per BNetzA BK6-22-300 / BK8-22/010-A).
Used to classify controllable consumption facilities under §14a EnWG.

| Position | Length | Content |
|---|---|---|
| 1 | 1 | Type code: always `F` |
| 2–10 | 9 | Alphanumeric sequence (A–Z, 0–9), auto-assigned |
| 11 | 1 | Check digit (**ASCII-Verfahren**) |

**Issuer:** bdew-codes.de only.

---

### Ressourcen-ID — TR / SR / SG / CR (11 chars)

Used in Redispatch 2.0 and Netzbetreiberkoordination. Four sub-types share one
format, distinguished by the first character.

| Position | Length | Content |
|---|---|---|
| 1 | 1 | Type code: `A` = Cluster Ressource, `B` = Steuergruppe, `C` = Steuerbare Ressource, `D` = Technische Ressource |
| 2–10 | 9 | Alphanumeric sequence (A–Z, 0–9), auto-assigned |
| 11 | 1 | Check digit (**ASCII-Verfahren**) |

**Who assigns:** NB assigns TR-IDs and SG-IDs; EIV assigns SR-IDs.
**Issuer:** bdew-codes.de.

---

### Paket-ID — Netzbetreiberwechsel (11 chars)

Identifies a bundle of market locations affected by a DSO ownership transfer
(Netzbetreiberwechsel). Used in PARTIN messages (`mako-nbw`).

| Position | Length | Content |
|---|---|---|
| 1 | 1 | Type code (assigned by Vergabestelle) |
| 2 | 1 | Sub-type (assigned by Vergabestelle) |
| 3–10 | 8 | Sequence number (auto-assigned) |
| 11 | 1 | Check digit (**ASCII-Verfahren**) |

---

## Check Digit Algorithms

Two algorithms are used across all BDEW market identifiers.

> **Source:** Identifikatoren V1.2 §8

### Lok- und Waggon-Kennzeichnungsverfahren

Used for: **BDEW-Code, DVGW-Code, MaLo-ID**

1. Starting from the leftmost digit, alternately multiply each digit by **2** and **1**.
2. If a product exceeds 9, subtract 9 (equivalent to summing the two digits of the product).
3. Sum all weighted digits.
4. Check digit = `(10 - (sum mod 10)) mod 10`

This is the same as the ISO 6346 (railway wagon numbering) check digit, also
known as the Luhn-like BDEW variant.

### ASCII-Verfahren

Used for: **NeLo-ID, NeBe-ID, Ressourcen-ID, Paket-ID**

Each character is converted to its ASCII code value. The values are weighted by
position and the check digit is computed using modulo arithmetic over the defined
base. The exact weight and base tables are published in Identifikatoren V1.2 §8.2.

---

## EDIFACT Encoding

Key segment positions where identifiers appear in EDI@Energy messages.

> **Source:** Allgemeine Festlegungen 6.1d §2.13, §2.15, §2.16, §2.19

### Interchange level (UNB)

| DE | Role | Content |
|---|---|---|
| `UNB DE0004` | Sender (Absender) | MP-ID of the transmitting party |
| `UNB DE0010` | Receiver (Empfänger) | MP-ID of the receiving party |

The MP-ID in UNB **must be identical** to the MP-ID in the corresponding NAD+MS
/ NAD+MR segment in the enclosed messages. Mismatches are rejected.

### Message level (NAD segment, DE3035)

| Qualifier | Role |
|---|---|
| `MS` | Message sender (Nachrichtenabsender) — always the same party as UNB DE0004 |
| `MR` | Message receiver (Nachrichtenempfänger) — always the same party as UNB DE0010 |

Code scheme (DE3055): `293` for BDEW-Code or DVGW-Code; `9` for GS1 GLN.

### Market / metering location (IDE segment, DE3129 qualifier)

The qualifier in `IDE DE3129` identifies which type of object the `IDE DE3130`
value refers to. Common qualifiers:

| Qualifier | Object type | Identifier format |
|---|---|---|
| `Z01` | Marktlokation (MaLo) | 11-digit MaLo-ID |
| `Z01` | Messlokation (MeLo) / MaBiS-ZP | 33-char Zählpunktbezeichnung (Strom) or 11-char (Gas) |
| `Z08` | Netzlokation (NeLo) | 11-char NeLo-ID (prefix `E`) |

> **Note:** Both MaLo and MeLo use qualifier `Z01` in the IDE segment. The
> object type is determined by context (message type and segment group), not
> the qualifier alone. A UTILMD GPKE message references a MaLo-ID; a UTILMD WiM
> message for Zählerstandsgangmessung references a MeLo-ID.

### File naming convention

Per Allgemeine Festlegungen 6.1d §2.12:

```
{type}_{anwendungsref}_{sender-MP-ID}_{receiver-MP-ID}_{yyyymmdd}_{DAR}.txt
```

Example: `UTILMD__9900123400007_4012345393651_20261001_A177.txt`

---

## Rust API

`mako-engine::types` provides typed newtypes that prevent cross-domain
identifier confusion at compile time.

```rust
use mako_engine::types::{
    MaLo,           // Marktlokations-ID
    MeLo,           // Messlokations-ID (Zählpunktbezeichnung)
    MarktpartnerCode, // MP-ID: BDEW-Code (99...), DVGW-Code (98...), or GLN
    BkvId,          // Bilanzkreisverantwortlicher-ID
    UenbId,         // Übertragungsnetzbetreiber-ID (ÜNB)
    BikoId,         // Bilanzkoordinator-ID (BIKO)
    DeviceId,       // Geräte-ID / Zählernummer (WiM MSB processes)
};

// --- Market location (MaLo) ---
// 11-digit numeric; first digit 4-9 = BDEW-issued, 1-3 = DVGW-issued
let malo: MaLo = MaLo::new("51238696781");   // starts with 5 = BDEW

// --- Metering location (MeLo) = Zählpunktbezeichnung ---
// 33-char Strom (VDE-AR-N 4400) or 11-char Gas (DVGW G2000)
let melo: MeLo = MeLo::new("DE0000123400007002500000000001234"); // Strom

// --- Market participant (MP-ID) ---
// 13-digit numeric; 99... = BDEW/Strom; 98... = DVGW/Gas
let nb:  MarktpartnerCode = MarktpartnerCode::new("9900357000004"); // BDEW-Code, NB
let lf:  MarktpartnerCode = MarktpartnerCode::new("9900357000011"); // BDEW-Code, LF

// --- Balance-circle roles (MABIS) ---
let bkv:  BkvId  = BkvId::new("9900357000004");   // Bilanzkreisverantwortlicher
let uenb: UenbId = UenbId::new("4012345000023");   // ÜNB
let biko: BikoId = BikoId::new("9900357000005");   // Bilanzkoordinator

// --- WiM MSB device identifier ---
// Format depends on meter type and MSB; not standardised as a BDEW ID format
let device: DeviceId = DeviceId::new("1EMH0012345678");
```

All types:
- Wrap `Box<str>` — **immutable, 1-word smaller than `String`**, no heap realloc
- Implement `Serialize` / `Deserialize` as transparent JSON strings
- Implement `Display`, `AsRef<str>`, `From<String>`, `From<&str>`
- Are **not validated at construction** — validation happens at the EDIFACT
  parsing boundary in `edi-energy`. The type system enforces that a `MaLo`
  cannot be passed where a `MeLo` is expected; the format correctness is a
  runtime concern of the adapter layer.

> **Why no validation in the constructor?**  
> The identifier format standards themselves allow for future extensions.
> Validating at construction would require bumping the library version every
> time BDEW extends a format. Format validation belongs at the boundary where
> untrusted input enters the system — i.e., in the EDIFACT parser adapters,
> not in the typed wrappers.

---

## Further Reading

| Topic | Document |
|---|---|
| Full PID table for all process families | [PID Reference](pid-reference) |
| BNetzA rulings governing each process | [BNetzA Regulatory Reference](bnetza) |
| How EDIFACT messages are parsed and validated | [Parsing Guide](../parsing) |
| How the engine routes messages to workflows | [Process Engine](../engine) |
| BO4E objects in ERP integration | [ERP Integration](../erp-integration) |
| Gas balancing domain model | [GaBi Gas domain](#gas-domain--gabi-gas) (below) |

---

## Gas Domain — GaBi Gas

The `mako-gabi-gas` crate provides a dedicated domain vocabulary for the German
gas market, all in `src/domain.rs` and `src/portfolio.rs`. All energy quantities
use `Decimal` — no float arithmetic (**DVGW G 685 requires ≥ 3 decimal places**).

### GasDay — typed gas market day

The German gas day is defined by **DVGW G 2000 §3.2**: it starts and ends at
**06:00 CET** (Central European Time), which is UTC-offset aware:

| Season | Local | UTC |
|---|---|---|
| Winter (CET, UTC+1) | 06:00 CET | 05:00 UTC |
| Summer (CEST, UTC+2) | 06:00 CEST | 04:00 UTC |

DST transitions produce 23-hour (spring forward) or 25-hour (fall back) gas days.
The nomination deadline per KoV §3.2 is **D-1 13:00 CET**.

```rust
let day = GasDay::new(date!(2026-01-15));
assert_eq!(day.start_utc().hour(), 5);          // 05:00 UTC (CET winter)
assert_eq!(day.duration_hours(), 24);
assert_eq!(GasDay::new(date!(2026-03-28)).duration_hours(), 23); // spring-forward day
assert_eq!(GasDay::new(date!(2026-10-24)).duration_hours(), 25); // fall-back day
```

### GasBeschaffenheit + GasQuantity

The DVGW G 685 conversion formula:

$$kWh_{Hs} = m^3 \times H_s \times Z$$

```rust
let beschaffenheit = GasBeschaffenheit {
    brennwert_hs_kwh_per_m3: dec!(10.55),  // Abrechnungsbrennwert from MSCONS PID 13007
    zustandszahl: dec!(0.9764),             // pressure/temperature correction
    quality_class: GasQualityClass::HGas,
    ..
};
let q = GasQuantity::from_m3(dec!(100), beschaffenheit);
assert_eq!(q.energy_kwh_hs, dec!(1030.102));  // rounded to 3 decimal places
```

Gas quality classes per **DVGW G 260**:

| Class | Hs range (kWh/m³) | Usage |
|---|---|---|
| H-Gas | 9.5–13.1 | Most German transmission grids |
| L-Gas | 7.5–10.3 | Parts of northern Germany |
| Biogas | variable | Injected biomethane |

### AllocationVersion — KoV §6.4 correction tracking

ALOCAT messages may be sent as initial, corrected, or final allocations:

| Variant | Meaning |
|---|---|
| `Initial` | First ALOCAT for this gas day — preliminary |
| `Correction(n)` | nth corrected allocation (1-based) |
| `Final` | Binding for imbalance settlement — no further corrections |

### GasMarketRole

| Role | `GasMarketRole` | Notes |
|---|---|---|
| Bilanzkreisverantwortlicher | `Bkv` | Submits NOMINT; receives ALOCAT; subject to IMBNOT |
| Fernleitungsnetzbetreiber | `Fnb` | Sends daily ALOCAT 90001; receives NOMINT |
| Verteilnetzbetreiber | `Vnb` | Sends sub-daily ALOCAT 90003 |
| Marktgebietsverantwortlicher | `Mgv` | Sends monthly ALOCAT 90002; imbalance settlement |
| Lieferant | `Lf` | Supplies end customers; does not submit DVGW nominations directly |
| Händler | `Haendler` | May submit nominations and delivery orders |

### GasPortfolioBalance

`GasPortfolioBalance` aggregates all BKV positions across Bilanzkreise for a
gas day, enabling portfolio-level imbalance management:

```rust
let balance: GasPortfolioBalance = compute_portfolio(bkv_eic, gas_day, positions);
println!("Net imbalance: {} kWh",  balance.net_imbalance_kwh());
println!("Direction: {:?}",         balance.portfolio_direction()); // Mehr/Minder/Balanced
println!("Open positions: {}",      balance.open_imbalance_count());
println!("Fully settled: {}",       balance.is_fully_settled());
```

### Gas identifier formats

| Identifier | Format | Standard | Example |
|---|---|---|---|
| EIC (BKV / FNB / MGV) | 16 chars alphanumeric | ENTSO-E EIC code | `21X000000001368S` |
| Bilanzkreis-EIC | 16 chars | ENTSO-E EIC code | `11YAPG4CTRDNZ--A` |
| DVGW-Codenummer (NB) | 13 digits, starts `98` | DVGW registry | `9800357000001` |
| BDEW-Codenummer (LF) | 13 digits, starts `99` | BDEW registry | `9900357000004` |
| Gas Zählpunkt (MeLo) | 11 chars | DVGW G 2000 | `DE000123400M` |
