//! BDEW segment layouts for the user segments EDI@Energy messages carry.
//!
//! `edifact-rs` ships the ISO 9735 service segments (`UNB`…`UNZ`, `UCI`…`UCD`)
//! because the syntax standard fixes them. The user segments below come from
//! the separately-licensed UN/EDIFACT directory, so they are declared here.
//!
//! # These follow the MIG, not the directory
//!
//! The two differ, and for EDI@Energy the MIG wins:
//!
//! - **An unused element keeps its slot.** A MIG may mark a composite *nicht
//!   benutzt*, but it cannot renumber the ones after it — `C502` is unused in
//!   `CCI` and still occupies element 2, which is why the Merkmal sits at
//!   element 3 (`CCI+15++BI1'`).
//! - **A composite may be restricted.** `C240` carries only DE 7037 and `C901`
//!   only DE 9321 in the BDEW profile; the directory defines more.
//! - **`STS` is polymorphic in its Statuskategorie.** DE 9015 selects whether
//!   `C555` or `C556` carries the value, and the MIG marks the other unused in
//!   each case. Each data element still resolves to exactly one slot, so one
//!   definition addresses all three — but *which* is populated is a runtime
//!   question. See [`crate::messages::segments::Sts::code`].
//!
//! Positions are **one-based**, as `ElementRef`/`ComponentRef` require;
//! `element_slot`/`component_slot` return the zero-based index the accessors
//! use.
//!
//! Sources: BDEW EDI@Energy `UTILMD MIG Strom S2.2`, `UTILMD MIG Gas G1.2`,
//! `MSCONS MIG 2.5`, `ORDERS MIG 1.4c`, `INVOIC MIG 2.8e`, `APERAK MIG 2.2`.

use edifact_rs::{ComponentRef, ElementRef, SegmentDefinition, Status};

use Status::{Conditional as C, Mandatory as M};

// ── composite data elements ───────────────────────────────────────────────────

/// C002 — Dokumenten-/Nachrichtenname (`BGM`).
const C002: &[ComponentRef] = &[ComponentRef::new(1, "1001", C)];

/// C106 — Dokumenten-/Nachrichten-Identifikation (`BGM`).
const C106: &[ComponentRef] = &[ComponentRef::new(1, "1004", C)];

/// C507 — Datum/Uhrzeit/Zeitspanne (`DTM`).
const C507: &[ComponentRef] = &[
    ComponentRef::new(1, "2005", M),
    ComponentRef::new(2, "2380", C),
    ComponentRef::new(3, "2379", C),
];

/// C082 — Identifikation des Beteiligten (`NAD`).
const C082: &[ComponentRef] = &[
    ComponentRef::new(1, "3039", M),
    ComponentRef::new(2, "1131", C),
    ComponentRef::new(3, "3055", C),
];

/// C058 — Name und Anschrift (`NAD`). Five interchangeable free-text lines.
const C058: &[ComponentRef] = &[ComponentRef::repeated(1, "3124", C, 5)];

/// C080 — Name des Beteiligten (`NAD`).
const C080: &[ComponentRef] = &[ComponentRef::repeated(1, "3036", M, 5)];

/// C506 — Referenz (`RFF`).
const C506: &[ComponentRef] = &[
    ComponentRef::new(1, "1153", M),
    ComponentRef::new(2, "1154", C),
];

/// C206 — Identifikationsnummer (`IDE`).
const C206: &[ComponentRef] = &[ComponentRef::new(1, "7402", C)];

/// C517 — Ortsangabe (`LOC`).
const C517: &[ComponentRef] = &[
    ComponentRef::new(1, "3225", C),
    ComponentRef::new(2, "1131", C),
    ComponentRef::new(3, "3055", C),
    ComponentRef::new(4, "3224", C),
];

/// C901 — Anwendungsfehler (`ERC`). BDEW carries only DE 9321.
const C901: &[ComponentRef] = &[ComponentRef::new(1, "9321", M)];

/// C107 — Textreferenz (`FTX`).
const C107: &[ComponentRef] = &[ComponentRef::new(1, "4441", M)];

/// C108 — Text (`FTX`). Five interchangeable lines; the first carries the text.
const C108: &[ComponentRef] = &[ComponentRef::repeated(1, "4440", M, 5)];

/// C186 — Mengenangaben (`QTY`).
const C186: &[ComponentRef] = &[
    ComponentRef::new(1, "6063", M),
    ComponentRef::new(2, "6060", M),
    ComponentRef::new(3, "6411", C),
];

/// C212 — Waren-/Leistungsnummer (`PIA`).
const C212: &[ComponentRef] = &[
    ComponentRef::new(1, "7140", C),
    ComponentRef::new(2, "7143", C),
];

/// C502 — Einzelheiten zu Maßangaben (`CCI`). *Nicht benutzt*, keeps its slot.
const C502: &[ComponentRef] = &[ComponentRef::new(1, "6313", C)];

/// C240 — Merkmalsbeschreibung (`CCI`). BDEW carries only DE 7037.
const C240: &[ComponentRef] = &[ComponentRef::new(1, "7037", M)];

/// C601 — Statuskategorie (`STS`).
const C601: &[ComponentRef] = &[ComponentRef::new(1, "9015", M)];

/// C555 — Status (`STS`). Carries the value for Statuskategorie `Z18` / `10`.
const C555: &[ComponentRef] = &[ComponentRef::new(1, "4405", M)];

/// C556 — Statusanlass (`STS`). Carries the value for Statuskategorie `7` / `Z33`.
const C556: &[ComponentRef] = &[ComponentRef::new(1, "9013", M)];

/// C056 — Abteilung oder Bearbeiter (`CTA`).
const C056: &[ComponentRef] = &[
    ComponentRef::new(1, "3413", C),
    ComponentRef::new(2, "3412", C),
];

/// C076 — Kommunikationsverbindung (`COM`).
const C076: &[ComponentRef] = &[
    ComponentRef::new(1, "3148", M),
    ComponentRef::new(2, "3155", M),
];

// ── segment definitions ───────────────────────────────────────────────────────

const BGM_ELEMENTS: &[ElementRef] = &[
    ElementRef::composite(1, "C002", C, 1, C002),
    ElementRef::composite(2, "C106", C, 1, C106),
    ElementRef::new(3, "1225", C, 1),
];
/// `BGM` — Beginn der Nachricht.
pub const BGM: SegmentDefinition =
    SegmentDefinition::new("BGM", "Beginn der Nachricht", BGM_ELEMENTS);

const DTM_ELEMENTS: &[ElementRef] = &[ElementRef::composite(1, "C507", M, 1, C507)];
/// `DTM` — Datum/Uhrzeit/Zeitspanne.
pub const DTM: SegmentDefinition =
    SegmentDefinition::new("DTM", "Datum/Uhrzeit/Zeitspanne", DTM_ELEMENTS);

const NAD_ELEMENTS: &[ElementRef] = &[
    ElementRef::new(1, "3035", M, 1),
    ElementRef::composite(2, "C082", C, 1, C082),
    ElementRef::composite(3, "C058", C, 1, C058),
    ElementRef::composite(4, "C080", C, 1, C080),
];
/// `NAD` — Name und Adresse.
pub const NAD: SegmentDefinition = SegmentDefinition::new("NAD", "Name und Adresse", NAD_ELEMENTS);

const RFF_ELEMENTS: &[ElementRef] = &[ElementRef::composite(1, "C506", M, 1, C506)];
/// `RFF` — Referenz.
pub const RFF: SegmentDefinition = SegmentDefinition::new("RFF", "Referenz", RFF_ELEMENTS);

const IDE_ELEMENTS: &[ElementRef] = &[
    ElementRef::new(1, "7495", M, 1),
    ElementRef::composite(2, "C206", C, 1, C206),
];
/// `IDE` — Identifikation.
pub const IDE: SegmentDefinition = SegmentDefinition::new("IDE", "Identifikation", IDE_ELEMENTS);

const LOC_ELEMENTS: &[ElementRef] = &[
    ElementRef::new(1, "3227", M, 1),
    ElementRef::composite(2, "C517", C, 1, C517),
];
/// `LOC` — Ortsangabe.
pub const LOC: SegmentDefinition = SegmentDefinition::new("LOC", "Ortsangabe", LOC_ELEMENTS);

const ERC_ELEMENTS: &[ElementRef] = &[ElementRef::composite(1, "C901", M, 1, C901)];
/// `ERC` — Anwendungsfehler.
pub const ERC: SegmentDefinition = SegmentDefinition::new("ERC", "Anwendungsfehler", ERC_ELEMENTS);

const FTX_ELEMENTS: &[ElementRef] = &[
    ElementRef::new(1, "4451", M, 1),
    ElementRef::new(2, "4453", C, 1),
    ElementRef::composite(3, "C107", C, 1, C107),
    ElementRef::composite(4, "C108", C, 1, C108),
];
/// `FTX` — Freier Text. The text is `C108` at element 4 (`FTX+AAO+++Text'`).
pub const FTX: SegmentDefinition = SegmentDefinition::new("FTX", "Freier Text", FTX_ELEMENTS);

const QTY_ELEMENTS: &[ElementRef] = &[ElementRef::composite(1, "C186", M, 1, C186)];
/// `QTY` — Mengenangaben.
pub const QTY: SegmentDefinition = SegmentDefinition::new("QTY", "Mengenangaben", QTY_ELEMENTS);

const LIN_ELEMENTS: &[ElementRef] = &[
    ElementRef::new(1, "1082", C, 1),
    ElementRef::new(2, "1229", C, 1),
];
/// `LIN` — Positionsdaten.
pub const LIN: SegmentDefinition = SegmentDefinition::new("LIN", "Positionsdaten", LIN_ELEMENTS);

const PIA_ELEMENTS: &[ElementRef] = &[
    ElementRef::new(1, "4347", M, 1),
    ElementRef::composite(2, "C212", M, 1, C212),
];
/// `PIA` — Zusätzliche Produkt-ID.
pub const PIA: SegmentDefinition =
    SegmentDefinition::new("PIA", "Zusätzliche Produkt-ID", PIA_ELEMENTS);

const CCI_ELEMENTS: &[ElementRef] = &[
    ElementRef::new(1, "7059", C, 1),
    ElementRef::composite(2, "C502", C, 1, C502),
    ElementRef::composite(3, "C240", C, 1, C240),
];
/// `CCI` — Merkmal/Klasse. `C502` is unused and still occupies element 2.
pub const CCI: SegmentDefinition = SegmentDefinition::new("CCI", "Merkmal/Klasse", CCI_ELEMENTS);

const STS_ELEMENTS: &[ElementRef] = &[
    ElementRef::composite(1, "C601", C, 1, C601),
    ElementRef::composite(2, "C555", C, 1, C555),
    ElementRef::composite(3, "C556", C, 1, C556),
];
/// `STS` — Status. Polymorphic in DE 9015; see the module docs.
pub const STS: SegmentDefinition = SegmentDefinition::new("STS", "Status", STS_ELEMENTS);

const CTA_ELEMENTS: &[ElementRef] = &[
    ElementRef::new(1, "3139", C, 1),
    ElementRef::composite(2, "C056", C, 1, C056),
];
/// `CTA` — Ansprechpartner.
pub const CTA: SegmentDefinition = SegmentDefinition::new("CTA", "Ansprechpartner", CTA_ELEMENTS);

const COM_ELEMENTS: &[ElementRef] = &[ElementRef::composite(1, "C076", M, 1, C076)];
/// `COM` — Kommunikationsverbindung.
pub const COM: SegmentDefinition =
    SegmentDefinition::new("COM", "Kommunikationsverbindung", COM_ELEMENTS);
