//! Semantic domain type wrappers for identifiers used across all MaKo process families.
//!
//! All types in this module wrap `Box<str>` rather than `String` — they are
//! **immutable** identifiers that are never mutated after construction.
//! `Box<str>` is one pointer word smaller than `String` on the stack and avoids
//! the extra capacity bookkeeping.
//!
//! ## Why newtypes instead of `String`?
//!
//! Domain commands and events have many identifier fields:
//!
//! ```text
//! ReceiveUtilmd {
//!     sender:        String,  // GLN
//!     receiver:      String,  // GLN
//!     location_id:   String,  // MaLo / EIC
//!     document_date: String,  // YYYYMMDD
//!     message_ref:   String,  // EDIFACT reference
//! }
//! ```
//!
//! Passing `location_id` where `sender` is expected is a compile-time no-op
//! when all fields are `String`. Typed wrappers turn that into a type error.
//!
//! ## Construction
//!
//! All types implement `From<String>` and `From<&str>` for ergonomic
//! construction without `.into()` gymnastics:
//!
//! ```rust
//! use mako_engine::types::{MaLo, MarktpartnerCode};
//!
//! let malo:   MaLo            = MaLo::new("DE00123456789012345678901234567890");
//! let sender: MarktpartnerCode = MarktpartnerCode::new("9900123456789");
//! ```
//!
//! ## Serde
//!
//! All types serialize/deserialize as plain JSON strings, keeping event
//! payloads human-readable in SlateDB and log output.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! domain_id {
    (
        $(#[$attr:meta])*
        $name:ident,
        $doc:literal
    ) => {
        $(#[$attr])*
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Box<str>);

        impl $name {
            /// Construct a new identifier from any string-like value.
            #[must_use]
            pub fn new(s: impl Into<Box<str>>) -> Self {
                Self(s.into())
            }

            /// Borrow the underlying string slice.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s.into_boxed_str())
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.into())
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> Self {
                id.0.into()
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

domain_id!(
    /// Marktlokations-ID (MaLo).
    ///
    /// Identifies a supply point for electricity or gas in the German energy
    /// market. EIC format (33-char) or legacy 13-digit format; exact format
    /// depends on the process family and format version.
    MaLo,
    "Marktlokations-ID — supply point identifier (EIC / MaLo format)"
);

domain_id!(
    /// Messlokations-ID (MeLo).
    ///
    /// Identifies a metering point in the WiM (Wechselprozesse im Messwesen)
    /// process family. Distinct from a MaLo — one supply point may have
    /// multiple metering points.
    MeLo,
    "Messlokations-ID — metering point identifier"
);

domain_id!(
    /// Market-participant identifier (Marktpartner-Code).
    ///
    /// Identifies a trading partner in the German energy market. Three code
    /// schemes are in active use:
    ///
    /// | Scheme | Digits | EDIFACT DE 3055 | Typical holders |
    /// |--------|--------|-----------------|-----------------|
    /// | **BDEW code** | 13 numeric | `"293"` | Suppliers (LFN), DSOs (NB/VNB), MSBs, BKVs — the dominant scheme |
    /// | **GLN** (GS1) | 13 numeric | `"9"` | Global GS1 scheme; rare in German MaKo |
    /// | **EIC** (ENTSO-E) | 16 alphanumeric | `"305"` | TSOs (ÜNB), Regelzonen, cross-border |
    ///
    /// Used as `sender` and `receiver` in EDIFACT message headers and as
    /// domain party identifiers in all MaKo process commands. The numeric
    /// value is stored without the agency qualifier — use
    /// [`edi_energy::AgencyCode`] when rendering outbound NAD segments.
    MarktpartnerCode,
    "Marktpartner-Code — BDEW code (293), GS1 GLN (9), or EIC (305) market-participant identifier"
);

domain_id!(
    /// EDIFACT message reference.
    ///
    /// Corresponds to the BGM/C106 reference number in UTILMD, APERAK,
    /// MSCONS, and REMADV messages. Used to correlate responses back to the
    /// originating message and to detect duplicate deliveries.
    MessageRef,
    "EDIFACT message reference (BGM/C106 document number)"
);

domain_id!(
    /// Geräte-ID / Zählernummer.
    ///
    /// Identifies a physical metering device in the WiM Gerätewechsel
    /// process. Assigned by the Messstellenbetreiber; format varies by
    /// device manufacturer.
    DeviceId,
    "Geräte-ID — physical metering device identifier (Zählernummer)"
);

domain_id!(
    /// Bilanzkreisverantwortlicher-ID (BKV).
    ///
    /// Identifies the balance circle responsible party in MaBiS billing
    /// processes. Used in Prüfmitteilung and billing settlement messages.
    BkvId,
    "Bilanzkreisverantwortlicher-ID — balance circle responsible party"
);

domain_id!(
    /// Übertragungsnetzbetreiber-ID (ÜNB).
    ///
    /// Identifies the transmission grid operator. Kept for use in contexts
    /// outside MaBiS billing (e.g. GaBi Gas, Redispatch).
    UenbId,
    "Übertragungsnetzbetreiber-ID — transmission grid operator identifier"
);

domain_id!(
    /// Bilanzkoordinator-ID (BIKO).
    ///
    /// Identifies the Bilanzkoordinator in MaBiS processes. The BIKO is the
    /// central actor in Bilanzkreisabrechnung Strom: it calculates and sends
    /// the `Abrechnungssummenzeitreihe` to BKV, NB, and ÜNB, and receives
    /// the `Prüfmitteilung` back from BKV. The BKV must respond with a
    /// Prüfmitteilung within **1 Werktag** of receiving the Abrechnungs-
    /// summenzeitreihe (MaBiS BK6-24-174, §13.8).
    BikoId,
    "Bilanzkoordinator-ID — balance coordinator identifier (BIKO)"
);

domain_id!(
    /// Abrechnungszeitraum (billing period).
    ///
    /// Represents the billing period as a string in `YYYYMM` or `YYYYMMDD–YYYYMMDD`
    /// format, depending on the context and AHB version. Kept as an opaque
    /// string rather than a date range to avoid coupling to a specific calendar
    /// representation.
    BillingPeriod,
    "Abrechnungszeitraum — billing period identifier string"
);

// ── Pruefidentifikator ────────────────────────────────────────────────────────

/// A validated BDEW process-type code (Prüfidentifikator, PID).
///
/// Prüfidentifikatoren are 5-digit decimal codes in the range `10000–99999`
/// that identify the business process variant of an EDI@Energy message
/// (e.g. `55001` for GPKE Lieferbeginn, `11001` for WiM Zählerstand).
///
/// # Serde representation
///
/// Serialises as a plain JSON number (`u32`), matching the wire format of
/// `edi_energy::Pruefidentifikator` (which is also `#[serde(transparent)]`
/// over `u32`). Stored event payloads are therefore fully compatible with both
/// representations — no migration needed.
///
/// # Why this lives in `mako-engine` and not `edi-energy`
///
/// Domain event structs and workflow state must only depend on `mako-engine`,
/// not on the stateless parsing library `edi-energy`. Moving the PID type here
/// removes the `edi-energy` dependency from all domain crates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Pruefidentifikator(u32);

impl Pruefidentifikator {
    /// The inclusive lower bound of the valid PID range.
    pub const MIN: u32 = 10_000;
    /// The inclusive upper bound of the valid PID range.
    pub const MAX: u32 = 99_999;

    /// Construct a `Pruefidentifikator`, validating that `code` is in range.
    ///
    /// # Errors
    ///
    /// Returns an error string if `code < 10000` or `code > 99999`.
    pub fn new(code: u32) -> Result<Self, String> {
        if (Self::MIN..=Self::MAX).contains(&code) {
            Ok(Self(code))
        } else {
            Err(format!(
                "invalid Pruefidentifikator {code}: must be a 5-digit code in 10000–99999"
            ))
        }
    }

    /// Returns the numeric code.
    #[must_use]
    pub fn as_u32(self) -> u32 {
        self.0
    }
}

impl fmt::Display for Pruefidentifikator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:05}", self.0)
    }
}

impl std::str::FromStr for Pruefidentifikator {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u32>()
            .map_err(|_| format!("Pruefidentifikator is not a decimal integer: {s:?}"))
            .and_then(Self::new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn malo_roundtrip_display_and_serde() {
        let m = MaLo::new("DE00123456789012345678901234567890");
        assert_eq!(m.to_string(), "DE00123456789012345678901234567890");
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v, json!("DE00123456789012345678901234567890"));
        let back: MaLo = serde_json::from_value(v).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn from_string_and_str() {
        let from_string: MarktpartnerCode = MarktpartnerCode::from(String::from("4012345000009"));
        let from_str: MarktpartnerCode = MarktpartnerCode::from("4012345000009");
        assert_eq!(from_string, from_str);
    }

    #[test]
    fn into_string() {
        let mid = MessageRef::new("UTILMD-2025-001");
        let s: String = mid.into();
        assert_eq!(s, "UTILMD-2025-001");
    }

    #[test]
    fn distinct_types_are_not_interchangeable() {
        // This test is a compile-time proof: the following would NOT compile:
        // let malo: MaLo = MeLo::new("X");
        // let _: MaLo = MarktpartnerCode::new("X");
        let malo_val = MaLo::new("A");
        let messlokation = MeLo::new("A");
        // Different types even though same inner value:
        let _: MaLo = malo_val;
        let _: MeLo = messlokation;
    }

    #[test]
    fn as_str_and_as_ref() {
        let g = MarktpartnerCode::new("4012345000009");
        assert_eq!(g.as_str(), "4012345000009");
        let s: &str = g.as_ref();
        assert_eq!(s, "4012345000009");
    }
}
