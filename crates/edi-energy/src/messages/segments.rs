//! Typed EDIFACT segment structs shared across all EDI@Energy message types.
//!
//! Each struct maps directly to one EDIFACT segment via the `#[edifact(segment = "TAG")]`
//! derive attribute and implements both [`EdifactDeserialize`] and [`EdifactSerialize`].
//!
//! # Element / component indexing
//!
//! EDIFACT uses 0-based indices:
//! - `#[edifact(element = N)]` — selects component 0 of the Nth element.
//! - `#[edifact(element = N, component = M)]` — selects component M of the Nth element.
//!
//! For composite data elements (e.g. C507 in DTM) you need multiple fields
//! sharing the same `element = N` but with different `component` indices.
//!
//! [`EdifactDeserialize`]: edifact_rs::EdifactDeserialize
//! [`EdifactSerialize`]: edifact_rs::EdifactSerialize

use edifact_rs::{EdifactDeserialize, EdifactSerialize};

// ── BGM ───────────────────────────────────────────────────────────────────────

/// `BGM` — Beginning of Message.
///
/// Structure: `BGM+<document_code>+<document_id>+<function>'`
///
/// | Element | DE   | Meaning                                                  |
/// |---------|------|----------------------------------------------------------|
/// | 0       | 1001 | Document / message name code (e.g. `E01`, `1000`)        |
/// | 1       | 1004 | Document / message number (Pruefidentifikator value)     |
/// | 2       | 1225 | Message function, coded (e.g. `9` = original)            |
#[derive(Debug, Clone, PartialEq, Eq, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "BGM")]
pub struct Bgm {
    /// DE 1001 — document / message name code.  In EDI@Energy this is the
    /// message category identifier (e.g. `"E01"` for grid-feed-in UTILMD,
    /// `"1000"` for a positive APERAK).
    #[edifact(element = 0)]
    pub document_code: String,
    /// DE 1004 — document / message number.  Carries the Pruefidentifikator
    /// value (e.g. `"11001"`) or an 8-digit document reference number.
    #[edifact(element = 1)]
    pub document_id: Option<String>,
    /// DE 1225 — message function, coded (e.g. `"9"` = original).
    #[edifact(element = 2)]
    pub function: Option<String>,
}

impl Bgm {
    /// Parse `document_id` as a [`crate::Pruefidentifikator`].
    ///
    /// Returns `None` when the field is absent or the value is not a valid
    /// 5-digit Pruefidentifikator.
    #[must_use]
    pub fn pruefidentifikator(&self) -> Option<crate::Pruefidentifikator> {
        self.document_id
            .as_deref()
            .and_then(|s| s.parse::<u32>().ok())
            .and_then(|code| crate::Pruefidentifikator::new(code).ok())
    }
}

// ── DTM ───────────────────────────────────────────────────────────────────────

/// `DTM` — Date/Time/Period.
///
/// Structure: `DTM+<qualifier>:<value>:<format>'`  (C507 composite in element 0)
///
/// | Element | Component | DE   | Meaning                                 |
/// |---------|-----------|------|-----------------------------------------|
/// | 0       | 0         | 2005 | Date/time/period function qualifier     |
/// | 0       | 1         | 2380 | Date/time/period text value             |
/// | 0       | 2         | 2379 | Date/time/period format qualifier       |
#[derive(Debug, Clone, PartialEq, Eq, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "DTM")]
pub struct Dtm {
    /// DE 2005 — date/time/period qualifier (e.g. `"137"` = document date,
    /// `"163"` = beginning of delivery period).
    #[edifact(element = 0)]
    pub qualifier: String,
    /// DE 2380 — date/time/period value (e.g. `"20230101"`).
    #[edifact(element = 0, component = 1)]
    pub value: Option<String>,
    /// DE 2379 — date/time/period format qualifier (e.g. `"102"` = CCYYMMDD).
    #[edifact(element = 0, component = 2)]
    pub format: Option<String>,
}

impl Dtm {
    /// Returns `true` when this is the document date / creation timestamp
    /// (`qualifier == "137"`).
    #[must_use]
    pub fn is_document_date(&self) -> bool {
        self.qualifier == "137"
    }

    /// Returns `true` when this marks the beginning of a supply/delivery period
    /// (`qualifier == "163"`).
    #[must_use]
    pub fn is_period_start(&self) -> bool {
        self.qualifier == "163"
    }

    /// Returns `true` when this marks the end of a supply/delivery period
    /// (`qualifier == "164"`).
    #[must_use]
    pub fn is_period_end(&self) -> bool {
        self.qualifier == "164"
    }

    /// Returns the date/time value as a `&str`, if present.
    #[must_use]
    pub fn value_str(&self) -> Option<&str> {
        self.value.as_deref()
    }
}

// ── NAD ───────────────────────────────────────────────────────────────────────

/// `NAD` — Name and Address, qualified by element 0 (DE 3035).
///
/// Structure: `NAD+<qualifier>+<party_id>::<agency>'`
///
/// | Element | Component | DE   | Meaning                              |
/// |---------|-----------|------|--------------------------------------|
/// | 0       | 0         | 3035 | Party function qualifier             |
/// | 1       | 0         | 3039 | Party identification                 |
/// | 1       | 2         | 3055 | Code list responsible agency         |
/// | 3       | 0         | 3036 | Party name                           |
///
/// Common qualifiers:
/// - `"MS"` — message sender (DE 3035)
/// - `"MR"` — message recipient (DE 3035)
/// - `"AG"` — authorised/requesting agent
#[derive(Debug, Clone, PartialEq, Eq, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "NAD", qualifier_from = 0)]
pub struct Nad {
    /// DE 3035 — party function qualifier (e.g. `"MS"` = message sender).
    #[edifact(element = 0)]
    pub qualifier: String,
    /// DE 3039 — party identification code (GLN / BDEW code), component 0 of C082.
    #[edifact(element = 1)]
    pub party_id: Option<String>,
    /// DE 3055 — code list responsible agency, component 2 of C082.
    ///
    /// Common values in EDI@Energy:
    /// - `"293"` — BDEW (the standard for German `MaKo` market participants)
    /// - `"9"` — GS1/EAN (global GLN scheme, rare in German `MaKo`)
    /// - `"305"` — ECOD/ENTSO-E (EIC codes for TSOs and Regelzonen)
    /// - `"332"` — DVGW (legacy gas-sector codes)
    ///
    /// Use [`crate::AgencyCode`] to parse or format this value.
    #[edifact(element = 1, component = 2)]
    pub agency_code: Option<String>,
    /// DE 3036 — party name, component 0 of C080.
    #[edifact(element = 3)]
    pub party_name: Option<String>,
}

// ── RFF ───────────────────────────────────────────────────────────────────────

/// `RFF` — Reference.
///
/// Structure: `RFF+<qualifier>:<reference>'`  (C506 composite in element 0)
///
/// | Element | Component | DE   | Meaning                             |
/// |---------|-----------|------|-------------------------------------|
/// | 0       | 0         | 1153 | Reference function qualifier        |
/// | 0       | 1         | 1154 | Reference identifier                |
///
/// Common qualifiers in EDI@Energy:
/// - `"ACW"` — acknowledgement reference (APERAK)
/// - `"TN"` — transaction reference number
/// - `"Z13"` — Pruefidentifikator of referenced message
#[derive(Debug, Clone, PartialEq, Eq, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "RFF")]
pub struct Rff {
    /// DE 1153 — reference function qualifier (e.g. `"ACW"`, `"TN"`, `"Z13"`).
    #[edifact(element = 0)]
    pub qualifier: String,
    /// DE 1154 — reference identifier value.
    #[edifact(element = 0, component = 1)]
    pub reference: Option<String>,
}

// ── IDE ───────────────────────────────────────────────────────────────────────

/// `IDE` — Identity.
///
/// Used in UTILMD to carry market location IDs (Marktlokation / Messlokation).
///
/// | Element | Component | DE   | Meaning                             |
/// |---------|-----------|------|-------------------------------------|
/// | 0       | 0         | 7495 | Object type qualifier               |
/// | 1       | 0         | 7402 | Identity number (MaLo / MeLo ID)    |
#[derive(Debug, Clone, PartialEq, Eq, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "IDE")]
pub struct Ide {
    /// DE 7495 — object type qualifier (e.g. `"Z18"` = Marktlokation,
    /// `"Z19"` = Messlokation).
    #[edifact(element = 0)]
    pub qualifier: String,
    /// DE 7402 — object identity number (component 0 of C206).
    /// Must be exactly 11 upper-case alphanumeric characters for EDI@Energy.
    #[edifact(element = 1)]
    pub object_id: Option<String>,
}

// ── LOC ───────────────────────────────────────────────────────────────────────

/// `LOC` — Place/Location Identification.
///
/// | Element | Component | DE   | Meaning                             |
/// |---------|-----------|------|-------------------------------------|
/// | 0       | 0         | 3227 | Location function qualifier         |
/// | 1       | 0         | 3225 | Location name code                  |
#[derive(Debug, Clone, PartialEq, Eq, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "LOC")]
pub struct Loc {
    /// DE 3227 — location function qualifier (e.g. `"172"` = measurement point).
    #[edifact(element = 0)]
    pub qualifier: String,
    /// DE 3225 — location name code / identifier, component 0 of C517.
    #[edifact(element = 1)]
    pub location_id: Option<String>,
}

// ── ERC ───────────────────────────────────────────────────────────────────────

/// `ERC` — Application Error Information.
///
/// Used in APERAK to carry application-level error codes.
///
/// | Element | Component | DE   | Meaning                             |
/// |---------|-----------|------|-------------------------------------|
/// | 0       | 0         | 9321 | Application error code              |
#[derive(Debug, Clone, PartialEq, Eq, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "ERC")]
pub struct Erc {
    /// DE 9321 — application error code (component 0 of C901).
    #[edifact(element = 0)]
    pub error_code: String,
    /// DE 1131 — code list identification (component 1 of C901), if present.
    #[edifact(element = 0, component = 1)]
    pub code_list_id: Option<String>,
    /// DE 3055 — responsible agency code (component 2 of C901), if present.
    #[edifact(element = 0, component = 2)]
    pub agency_code: Option<String>,
}

// ── FTX ───────────────────────────────────────────────────────────────────────

/// `FTX` — Free Text.
///
/// Used for human-readable notes and, in APERAK, for error descriptions.
///
/// | Element | Component | DE   | Meaning                             |
/// |---------|-----------|------|-------------------------------------|
/// | 0       | 0         | 4451 | Text subject qualifier              |
/// | 1       | 0         | 4453 | Text function, coded                |
/// | 3       | 0         | 4440 | Free text (first line)              |
#[derive(Debug, Clone, PartialEq, Eq, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "FTX")]
pub struct Ftx {
    /// DE 4451 — text subject qualifier (e.g. `"AAI"` = general information,
    /// `"AIM"` = APERAK error text, `"ZZZ"` = mutually defined).
    #[edifact(element = 0)]
    pub qualifier: String,
    /// DE 4440 — free text, component 0 of C108 (element 3).
    #[edifact(element = 3)]
    pub text: Option<String>,
}

// ── QTY ───────────────────────────────────────────────────────────────────────

/// `QTY` — Quantity.
///
/// Used in MSCONS to carry metered quantity values.
///
/// | Element | Component | DE   | Meaning                             |
/// |---------|-----------|------|-------------------------------------|
/// | 0       | 0         | 6063 | Quantity type code qualifier        |
/// | 0       | 1         | 6060 | Quantity value                      |
/// | 0       | 2         | 6411 | Measurement unit code               |
#[derive(Debug, Clone, PartialEq, Eq, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "QTY")]
pub struct Qty {
    /// DE 6063 — quantity type code qualifier (e.g. `"220"` = metered quantity).
    #[edifact(element = 0)]
    pub qualifier: String,
    /// DE 6060 — quantity value as string (e.g. `"1234.5"`).
    #[edifact(element = 0, component = 1)]
    pub value: Option<String>,
    /// DE 6411 — measurement unit code (e.g. `"KWH"`, `"MWH"`, `"M3"`).
    #[edifact(element = 0, component = 2)]
    pub unit: Option<String>,
}

impl Qty {
    /// Parse `value` as an `f64`.
    ///
    /// Returns `None` when the field is absent or the string is not a valid
    /// decimal number.  The EDIFACT decimal mark may be either `.` or `,`.
    #[must_use]
    pub fn value_f64(&self) -> Option<f64> {
        self.value
            .as_deref()
            .map(|s| s.replace(',', "."))
            .and_then(|s| s.parse::<f64>().ok())
    }

    /// Returns `true` when this is a metered quantity (`qualifier == "220"`).
    #[must_use]
    pub fn is_metered(&self) -> bool {
        self.qualifier == "220"
    }
}

// ── UCI ───────────────────────────────────────────────────────────────────────

/// `UCI` — Interchange Response.
///
/// The mandatory segment in CONTRL messages.  Carries the interchange
/// control reference and the acknowledgement action code.
///
/// | Element | Component | DE   | Meaning                              |
/// |---------|-----------|------|--------------------------------------|
/// | 0       | 0         | 0020 | Interchange control reference        |
/// | 1       | 0         | 0004 | Sender identification                |
/// | 2       | 0         | 0010 | Recipient identification             |
/// | 3       | 0         | 0083 | Action, coded (4/7/8)                |
#[derive(Debug, Clone, PartialEq, Eq, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "UCI")]
pub struct Uci {
    /// DE 0020 — interchange control reference.
    #[edifact(element = 0)]
    pub interchange_ref: String,
    /// DE 0004 — sender identification (S002 component 0).
    #[edifact(element = 1)]
    pub sender: Option<String>,
    /// DE 0010 — recipient identification (S003 component 0).
    #[edifact(element = 2)]
    pub recipient: Option<String>,
    /// DE 0083 — action, coded: `"4"` = acknowledged, `"7"` = rejected (group),
    /// `"8"` = rejected (interchange).
    #[edifact(element = 3)]
    pub action_code: Option<String>,
}

// ── UNH ───────────────────────────────────────────────────────────────────────

/// `UNH` — Message Header.
///
/// | Element | Component | DE   | Meaning                              |
/// |---------|-----------|------|--------------------------------------|
/// | 0       | 0         | 0062 | Message reference number             |
/// | 1       | 0         | 0065 | Message type identifier              |
/// | 1       | 4         | 0057 | Association assigned code (release)  |
#[derive(Debug, Clone, PartialEq, Eq, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "UNH")]
pub struct Unh {
    /// DE 0062 — message reference number.
    #[edifact(element = 0)]
    pub message_ref: String,
    /// DE 0065 — message type identifier (e.g. `"UTILMD"`, `"MSCONS"`).
    #[edifact(element = 1)]
    pub message_type: String,
    /// DE 0057 — association assigned code, component 4 of S009
    /// (e.g. `"5.5.3a"`).  This is the EDI@Energy release identifier.
    #[edifact(element = 1, component = 4)]
    pub assoc_code: Option<String>,
}

// ── UNT ───────────────────────────────────────────────────────────────────────

/// `UNT` — Message Trailer.
///
/// | Element | Component | DE   | Meaning                              |
/// |---------|-----------|------|--------------------------------------|
/// | 0       | 0         | 0074 | Number of segments in message        |
/// | 1       | 0         | 0062 | Message reference number             |
#[derive(Debug, Clone, PartialEq, Eq, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "UNT")]
pub struct Unt {
    /// DE 0074 — number of segments in the message (including UNH and UNT).
    #[edifact(element = 0)]
    pub segment_count: String,
    /// DE 0062 — message reference number (must match UNH element 0).
    #[edifact(element = 1)]
    pub message_ref: String,
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Find the first `NAD` segment with the given `qualifier` in a segment slice.
///
/// Used by message constructors to extract sender (`"MS"`) and receiver (`"MR"`).
pub(crate) fn find_nad(segments: &[edifact_rs::Segment<'_>], qualifier: &str) -> Option<Nad> {
    segments
        .iter()
        .filter(|s| s.tag == "NAD")
        .find(|s| s.element_str(0) == Some(qualifier))
        .and_then(|seg| Nad::edifact_deserialize(std::slice::from_ref(seg)).ok())
}

/// Find all `DTM` segments in a segment slice.
pub(crate) fn collect_dtm(segments: &[edifact_rs::Segment<'_>]) -> Vec<Dtm> {
    segments
        .iter()
        .filter(|s| s.tag == "DTM")
        .filter_map(|seg| Dtm::edifact_deserialize(std::slice::from_ref(seg)).ok())
        .collect()
}

/// Find the first `BGM` segment.
pub(crate) fn find_bgm(segments: &[edifact_rs::Segment<'_>]) -> Option<Bgm> {
    segments
        .iter()
        .filter(|s| s.tag == "BGM")
        .find_map(|seg| Bgm::edifact_deserialize(std::slice::from_ref(seg)).ok())
}

/// Find the first `UCI` segment.
pub(crate) fn find_uci(segments: &[edifact_rs::Segment<'_>]) -> Option<Uci> {
    segments
        .iter()
        .filter(|s| s.tag == "UCI")
        .find_map(|seg| Uci::edifact_deserialize(std::slice::from_ref(seg)).ok())
}

/// Find the first `RFF` segment with the given qualifier.
pub(crate) fn find_rff(segments: &[edifact_rs::Segment<'_>], qualifier: &str) -> Option<Rff> {
    segments
        .iter()
        .filter(|s| s.tag == "RFF")
        .find(|s| s.element_str(0) == Some(qualifier))
        .and_then(|seg| Rff::edifact_deserialize(std::slice::from_ref(seg)).ok())
}

/// Collect all `COM` segments from a segment slice.
///
/// Used by `PartinMessage` to extract communication channels
/// (AS4 endpoint, email, phone) declared by the described party.
#[cfg(feature = "partin")]
pub(crate) fn collect_com(segments: &[edifact_rs::Segment<'_>]) -> Vec<Com> {
    segments
        .iter()
        .filter(|s| s.tag == "COM")
        .filter_map(|seg| Com::edifact_deserialize(std::slice::from_ref(seg)).ok())
        .collect()
}

// ── UCM ───────────────────────────────────────────────────────────────────────

/// `UCM` — Message Response (CONTRL SG1).
///
/// Acknowledges or rejects one specific message within the interchange.
///
/// | Element | Component | DE   | Meaning                              |
/// |---------|-----------|------|--------------------------------------|
/// | 0       | 0         | 0062 | Message reference number             |
/// | 1       | 0         | 0065 | Message type identifier              |
/// | 1       | 4         | 0057 | Association assigned code            |
/// | 2       | 0         | 0083 | Action, coded (`"4"` = acknowledged, `"7"` = rejected) |
/// | 3       | 0         | 0085 | Syntax error code (if rejected)      |
#[derive(Debug, Clone, PartialEq, Eq, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "UCM")]
pub struct Ucm {
    /// DE 0062 — message reference number (matches UNH element 0).
    #[edifact(element = 0)]
    pub message_ref: String,
    /// DE 0065 — message type identifier (e.g. `"UTILMD"`).
    #[edifact(element = 1)]
    pub message_type: String,
    /// DE 0057 — association assigned code (component 4 of S009).
    #[edifact(element = 1, component = 4)]
    pub assoc_code: Option<String>,
    /// DE 0083 — action, coded: `"4"` = acknowledged, `"7"` = rejected.
    #[edifact(element = 2)]
    pub action_code: String,
    /// DE 0085 — syntax error code (component 0 of S011), present when rejected.
    #[edifact(element = 3)]
    pub syntax_error: Option<String>,
}

// ── UCS ───────────────────────────────────────────────────────────────────────

/// `UCS` — Segment Identification (CONTRL SG2).
///
/// Identifies the erroneous segment within a message.
///
/// | Element | Component | DE   | Meaning                             |
/// |---------|-----------|------|-------------------------------------|
/// | 0       | 0         | 0096 | Segment position in message body    |
/// | 0       | 1         | 0135 | Service segment tag (if applicable) |
/// | 1       | 0         | 0085 | Syntax error code                   |
#[derive(Debug, Clone, PartialEq, Eq, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "UCS")]
pub struct Ucs {
    /// DE 0096 — segment position in message body (component 0 of S011).
    #[edifact(element = 0)]
    pub segment_position: String,
    /// DE 0135 — service segment tag (component 1 of S011).
    #[edifact(element = 0, component = 1)]
    pub segment_tag: Option<String>,
    /// DE 0085 — syntax error, coded.
    #[edifact(element = 1)]
    pub error_code: Option<String>,
}

// ── UCD ───────────────────────────────────────────────────────────────────────

/// `UCD` — Data Element Error Identification (CONTRL SG3).
///
/// | Element | Component | DE   | Meaning                             |
/// |---------|-----------|------|-------------------------------------|
/// | 0       | 0         | 0085 | Syntax error code                   |
/// | 1       | 0         | 0098 | Data element position               |
/// | 1       | 1         | 0104 | Component position (optional)       |
#[derive(Debug, Clone, PartialEq, Eq, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "UCD")]
pub struct Ucd {
    /// DE 0085 — syntax error, coded.
    #[edifact(element = 0)]
    pub error_code: String,
    /// DE 0098 — data element position (component 0 of C085).
    #[edifact(element = 1)]
    pub element_position: Option<String>,
    /// DE 0104 — component data element position (component 1 of C085).
    #[edifact(element = 1, component = 1)]
    pub component_position: Option<String>,
}

// ── LIN ───────────────────────────────────────────────────────────────────────

/// `LIN` — Line Item (MSCONS SG9).
///
/// Marks the start of a new metered-quantity line item.
///
/// | Element | Component | DE   | Meaning                             |
/// |---------|-----------|------|-------------------------------------|
/// | 0       | 0         | 1082 | Line item number (sequential)       |
#[derive(Debug, Clone, PartialEq, Eq, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "LIN")]
pub struct Lin {
    /// DE 1082 — line item number.
    #[edifact(element = 0)]
    pub line_number: Option<String>,
}

// ── PIA ───────────────────────────────────────────────────────────────────────

/// `PIA` — Additional Product ID (MSCONS SG9).
///
/// Carries the OBIS code or metering location identifier.
///
/// `PIA` — Additional product ID (MSCONS SG8).
///
/// In MSCONS, carries the OBIS measurement identifier for a line item.
///
/// `PIA` — Additional Product ID (MSCONS SG9, item identification).
///
/// Carries the OBIS measurement identifier for the current line item.
///
/// | Element | Component | DE   | Meaning                             |
/// |---------|-----------|------|-------------------------------------|
/// | 0       | —         | 4347 | Product ID function qualifier       |
/// | 1       | 0         | 7140 | OBIS code (full, e.g. `1-0:1.8.0`) |
/// | 1       | 1         | 7143 | Item type code (`Z12`, `SRW`, …)   |
///
/// ## EDIFACT release characters and OBIS codes
///
/// OBIS codes (IEC 62056-61) use `:` as part of their notation, which is also
/// the EDIFACT composite component separator.  The BDEW MSCONS AHB (since
/// FV2025-10-01) mandates the **EDIFACT release character `?`** to escape each
/// `:` inside the OBIS value:
///
/// ```text
/// PIA+5+1-1?:1.9.1:SRW'
///          ^^         ^^ release-char escapes the OBIS colon
///                     ^^ unescaped colon → component separator → DE 7143 = "SRW"
/// ```
///
/// `edifact-rs` correctly processes release characters: after parsing,
/// `item_number` contains the full clean OBIS string (`"1-1:1.9.1"`)
/// and `item_type` contains the DE 7143 qualifier (`"SRW"` or `"Z12"`).
///
/// **Builder note:** always use `Writer::escape_value()` when constructing the
/// PIA composite to ensure the OBIS colons are correctly escaped.
#[derive(Debug, Clone, PartialEq, Eq, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "PIA")]
pub struct Pia {
    /// DE 4347 — product ID function qualifier (e.g. `"5"` = product ID).
    #[edifact(element = 0)]
    pub qualifier: String,
    /// DE 7140 — full OBIS code (e.g. `"1-0:1.8.0"`).
    ///
    /// This field contains the complete OBIS identifier because the BDEW AHB
    /// requires the `:` inside OBIS values to be escaped with `?:`.
    /// `edifact-rs` strips the release character during parsing.
    #[edifact(element = 1)]
    pub item_number: Option<String>,
    /// DE 7143 — item type code (e.g. `"Z12"` for OBIS, `"SRW"` for
    /// Strom-Richtung-Wirkleistung).
    #[edifact(element = 1, component = 1)]
    pub item_type: Option<String>,
}

impl Pia {
    /// Return the OBIS code from DE 7140.
    ///
    /// Convenience accessor for `item_number`.  Returns the full OBIS string
    /// (e.g. `"1-0:1.8.0"`) after release-character processing by `edifact-rs`.
    ///
    /// # Example
    ///
    /// ```
    /// # use edi_energy::messages::segments::Pia;
    /// let pia = Pia {
    ///     qualifier: "5".to_owned(),
    ///     item_number: Some("1-0:1.8.0".to_owned()),
    ///     item_type: Some("Z12".to_owned()),
    /// };
    /// assert_eq!(pia.obis_code().as_deref(), Some("1-0:1.8.0"));
    /// assert_eq!(pia.item_type.as_deref(), Some("Z12"));
    /// ```
    #[must_use]
    pub fn obis_code(&self) -> Option<&str> {
        self.item_number.as_deref()
    }
}

// ── CCI ───────────────────────────────────────────────────────────────────────

/// `CCI` — Characteristic/Class ID (MSCONS SG8).
///
/// In MSCONS, carries the time-series type (Zeitreihentyp).
///
/// | Element | Component | DE   | Meaning                              |
/// |---------|-----------|------|--------------------------------------|
/// | 0       | 0         | 7059 | Property class code (optional)       |
/// | 2       | 0         | 7037 | Characteristic ID (Zeitreihentyp)    |
/// | 2       | 1         | 1131 | Code list identification             |
/// | 2       | 2         | 3055 | Responsible agency code              |
#[derive(Debug, Clone, PartialEq, Eq, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "CCI")]
pub struct Cci {
    /// DE 7059 — property class code (element 0), usually empty in MSCONS.
    #[edifact(element = 0)]
    pub category: Option<String>,
    /// DE 7037 — characteristic identifier (component 0 of C240, element 2).
    /// In MSCONS: time-series type code, e.g. `"Z05"` = measured quantity.
    #[edifact(element = 2)]
    pub characteristic_id: Option<String>,
    /// DE 1131 — code list identification (component 1 of C240).
    #[edifact(element = 2, component = 1)]
    pub code_list_id: Option<String>,
    /// DE 3055 — responsible agency code (component 2 of C240).
    #[edifact(element = 2, component = 2)]
    pub agency_code: Option<String>,
}

// ── STS ───────────────────────────────────────────────────────────────────────

/// `STS` — Status (MSCONS SG10).
///
/// Carries the quality / validation status of a quantity reading.
///
/// | Element | Component | DE   | Meaning                             |
/// |---------|-----------|------|-------------------------------------|
/// | 0       | 0         | 9015 | Status category code                |
/// | 1       | 0         | 9011 | Status value code                   |
/// | 1       | 1         | 4405 | Status value sub-qualifier          |
/// | 1       | 2         | 3055 | Agency code for status value        |
#[derive(Debug, Clone, PartialEq, Eq, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "STS")]
pub struct Sts {
    /// DE 9015 — status category (component 0 of C601, e.g. `"7"` = measurement).
    #[edifact(element = 0)]
    pub category: Option<String>,
    /// DE 9011 — status value code (component 0 of C555, e.g. `"Z03"` = validated).
    #[edifact(element = 1)]
    pub status_code: Option<String>,
    /// DE 4405 — status value sub-qualifier (component 1 of C555).
    ///
    /// Present in APERAK and CONTRL `STS` segments that carry a two-level qualifier,
    /// e.g. `STS+E10:ZF3'` where `ZF3` is the sub-qualifier.
    #[edifact(element = 1, component = 1)]
    pub sub_qualifier: Option<String>,
    /// DE 3055 — agency code (component 2 of C555).
    #[edifact(element = 1, component = 2)]
    pub agency_code: Option<String>,
}

// ── UNS ───────────────────────────────────────────────────────────────────────

/// `UNS` — Section Control (MSCONS).
///
/// Marks the transition from the header section to the detail section.
///
/// | Element | Component | DE   | Meaning                             |
/// |---------|-----------|------|-------------------------------------|
/// | 0       | 0         | 0081 | Section identification              |
#[derive(Debug, Clone, PartialEq, Eq, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "UNS")]
pub struct Uns {
    /// DE 0081 — section identification (`"D"` = detail section).
    #[edifact(element = 0)]
    pub section_id: String,
}

// ── CTA ───────────────────────────────────────────────────────────────────────

/// `CTA` — Contact Information (MSCONS SG4).
///
/// | Element | Component | DE   | Meaning                             |
/// |---------|-----------|------|-------------------------------------|
/// | 0       | 0         | 3139 | Contact function code               |
/// | 1       | 0         | 3413 | Department / employee name          |
#[derive(Debug, Clone, PartialEq, Eq, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "CTA")]
pub struct Cta {
    /// DE 3139 — contact function, coded.
    #[edifact(element = 0)]
    pub function_code: Option<String>,
    /// DE 3413 — department or employee name (component 0 of C056).
    #[edifact(element = 1)]
    pub name: Option<String>,
}

// ── COM ───────────────────────────────────────────────────────────────────────

/// `COM` — Communication Contact (MSCONS SG4).
///
/// | Element | Component | DE   | Meaning                             |
/// |---------|-----------|------|-------------------------------------|
/// | 0       | 0         | 3148 | Communication number                |
/// | 0       | 1         | 3155 | Communication channel qualifier     |
#[derive(Debug, Clone, PartialEq, Eq, EdifactDeserialize, EdifactSerialize)]
#[edifact(segment = "COM")]
pub struct Com {
    /// DE 3148 — communication number (component 0 of C076).
    #[edifact(element = 0)]
    pub number: Option<String>,
    /// DE 3155 — communication channel qualifier (component 1 of C076,
    /// e.g. `"EM"` = email, `"TE"` = telephone).
    #[edifact(element = 0, component = 1)]
    pub channel: Option<String>,
}

// ── additional helpers ────────────────────────────────────────────────────────

/// Deserialize a single segment tag from a segment slice, returning `None`
/// on failure.  Convenience wrapper used by group parsers.
pub(crate) fn try_deserialize<T: edifact_rs::EdifactDeserialize>(
    seg: &edifact_rs::Segment<'_>,
) -> Option<T> {
    T::edifact_deserialize(std::slice::from_ref(seg)).ok()
}
