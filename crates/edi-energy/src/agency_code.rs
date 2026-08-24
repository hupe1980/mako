/// EDIFACT DE 3055 — code list responsible agency for market participant identification.
///
/// Used in NAD segments (C082 component 2) and IDE segments to indicate which body
/// issued the party identifier code. Choosing the wrong agency code produces
/// non-conformant EDIFACT that receiving parties may reject.
///
/// In the **German energy market (BDEW `MaKo` / EDI@Energy)** the dominant code is
/// [`AgencyCode::Bdew`] (`"293"`). Nearly all supplier, DSO, and MSB codes carry
/// this agency qualifier — even when the 13-digit number is also a valid GS1 GLN
/// (BDEW is a GS1 member prefix holder).
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
/// assert_eq!(AgencyCode::Etso.as_str(), "305");
///
/// // Parse from a raw NAD segment agency string.
/// assert_eq!(AgencyCode::parse("293"), Some(AgencyCode::Bdew));
/// assert_eq!(AgencyCode::parse("999"), None);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AgencyCode {
    /// `"293"` — the German electricity-industry code list.
    ///
    /// UN/EDIFACT DE 3055 registered this code to the VDEW (Verband der
    /// Elektrizitätswirtschaft); BDEW succeeded the VDEW and the BDEW `MaKo`
    /// documents call it "DE, BDEW". The number is what goes on the wire either
    /// way.
    ///
    /// This is the correct agency code for suppliers (LFN),
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

    /// `"305"` — ETSO, registered in DE 3055 as the European transmission
    /// system operators' association.
    ///
    /// The body became ENTSO-E, which now runs the EIC coding scheme, so this is
    /// the agency for 16-character EIC codes: transmission operators (ÜNB/TSO),
    /// Regelzonen, Bilanzkreise and cross-border participants.
    ///
    /// # NAD wire form
    ///
    /// ```text
    /// NAD+MS+10XDE-EON-NETZ--I::305'
    /// ```
    Etso,

    /// `"332"` — the DVGW gas code list.
    ///
    /// Not legacy: it is the agency on **every** coded value in the DVGW gas
    /// transport formats (see the `dvgw-edi` crate), and Trading Hub Europe's
    /// own market-participant code is issued under it. BDEW EDI@Energy messages
    /// use [`AgencyCode::Bdew`].
    Dvgw,
}

impl AgencyCode {
    /// Default agency code for new outbound EDI@Energy messages.
    ///
    /// `293` is the correct default for all standard German market
    /// participants. Use [`AgencyCode::Etso`] only for parties identified by an
    /// EIC code.
    pub const DEFAULT: Self = Self::Bdew;

    /// Return the wire-format string for the DE 3055 component.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bdew => "293",
            Self::Gs1 => "9",
            Self::Etso => "305",
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
            "305" => Some(Self::Etso),
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
            ("305", AgencyCode::Etso),
            ("332", AgencyCode::Dvgw),
        ] {
            assert_eq!(AgencyCode::parse(code), Some(variant));
            assert_eq!(variant.as_str(), code);
        }
        assert_eq!(AgencyCode::parse("999"), None);
    }
}
