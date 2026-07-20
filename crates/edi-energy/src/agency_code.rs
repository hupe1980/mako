/// EDIFACT DE 3055 — code list responsible agency for market participant identification.
///
/// Used in NAD segments (C082 component 2) and IDE segments to indicate which body
/// issued the party identifier code. Choosing the wrong agency code produces
/// non-conformant EDIFACT that receiving parties may reject.
///
/// In the **German energy market (BDEW `MaKo` / EDI@Energy)** the dominant code is
/// [`AgencyCode::Bdew`] (`"293"`). Nearly all supplier, DSO, and MSB codes are
/// issued and administered by BDEW and carry this agency qualifier — even when the
/// 13-digit number is also a valid GS1 GLN (BDEW is a GS1 member prefix holder).
///
/// # Wire format
///
/// The agency code appears as the third component of the NAD C082 composite:
///
/// ```text
/// NAD+MS+{party_id}::{agency_code}'
/// ```
///
/// The middle component (code list id, C082/1154) is always empty in EDI@Energy.
///
/// # Example
///
/// ```rust
/// use edi_energy::AgencyCode;
///
/// assert_eq!(AgencyCode::Bdew.as_str(), "293");
/// assert_eq!(AgencyCode::Gs1.as_str(),  "9");
/// assert_eq!(AgencyCode::Entso.as_str(), "305");
///
/// // Parse from a raw NAD segment agency string.
/// assert_eq!(AgencyCode::parse("293"), Some(AgencyCode::Bdew));
/// assert_eq!(AgencyCode::parse("999"), None);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AgencyCode {
    /// `"293"` — BDEW (Bundesverband der Energie- und Wasserwirtschaft).
    ///
    /// Used for all BDEW-issued market participant codes in the German electricity
    /// and gas markets. This is the correct agency code for suppliers (LFN),
    /// distribution system operators (NB/VNB), metering point operators (MSB),
    /// and balance responsible parties (BKV/BRK) registered in the BDEW
    /// Marktteilnehmerverzeichnis.
    ///
    /// # NAD wire form
    ///
    /// ```text
    /// NAD+MS+9900123456789::293'
    /// ```
    Bdew,

    /// `"9"` — GS1 (formerly EAN International).
    ///
    /// Used when a 13-digit GLN is issued directly under GS1's global prefix
    /// scheme rather than through BDEW. Rare in German `MaKo` practice — most
    /// operators use [`AgencyCode::Bdew`] even for GS1-compatible numbers.
    Gs1,

    /// `"305"` — ECOD (ENTSO-E Coding Scheme / European network operator).
    ///
    /// Used for 16-character EIC (Energy Identification Codes) issued by
    /// ENTSO-E. Required for transmission grid operators (ÜNB/TSO), balance
    /// zones (Regelzonen), and cross-border market participants.
    ///
    /// # NAD wire form
    ///
    /// ```text
    /// NAD+MS+10XDE-EON-NETZ--I::305'
    /// ```
    Entso,

    /// `"332"` — DVGW (Deutscher Verein des Gas- und Wasserfaches).
    ///
    /// Occasionally used for gas-sector participants registered in the DVGW
    /// system before the BDEW merger. Modern gas market messages prefer
    /// [`AgencyCode::Bdew`]; this variant is kept for parsing legacy messages.
    Dvgw,
}

impl AgencyCode {
    /// Default agency code for new outbound EDI@Energy messages.
    ///
    /// `293` (BDEW) is the correct default for all standard German market
    /// participants. Change to [`AgencyCode::Entso`] only for TSO/ÜNB parties
    /// that carry an EIC code.
    pub const DEFAULT: Self = Self::Bdew;

    /// Return the wire-format string for the DE 3055 component.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bdew => "293",
            Self::Gs1 => "9",
            Self::Entso => "305",
            Self::Dvgw => "332",
        }
    }

    /// Parse a DE 3055 agency code string.
    ///
    /// Returns `None` for unrecognised codes; callers may fall back to
    /// treating the raw string as an opaque pass-through.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "293" => Some(Self::Bdew),
            "9" => Some(Self::Gs1),
            "305" => Some(Self::Entso),
            "332" => Some(Self::Dvgw),
            _ => None,
        }
    }
}

impl Default for AgencyCode {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl std::fmt::Display for AgencyCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bdew_is_default() {
        assert_eq!(AgencyCode::default(), AgencyCode::Bdew);
        assert_eq!(AgencyCode::DEFAULT.as_str(), "293");
    }

    #[test]
    fn round_trip_from_str() {
        for (code, variant) in [
            ("293", AgencyCode::Bdew),
            ("9", AgencyCode::Gs1),
            ("305", AgencyCode::Entso),
            ("332", AgencyCode::Dvgw),
        ] {
            assert_eq!(AgencyCode::parse(code), Some(variant));
            assert_eq!(variant.as_str(), code);
        }
        assert_eq!(AgencyCode::parse("999"), None);
    }
}
