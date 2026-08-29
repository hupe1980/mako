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
    /// Fallback agency for a party whose identifier says nothing about its
    /// issuing office.
    ///
    /// Prefer [`AgencyCode::for_mp_id`]: the issuing office is a property of the
    /// MP-ID, and defaulting a Gas party to `293` names a code list the Gas AHB
    /// does not define.
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

    /// The DE 3055 agency for a market-participant identifier.
    ///
    /// The **issuing office is a property of the MP-ID, not of the message**:
    /// BDEW issues 13-digit codes beginning `99` for Strom, DVGW issues `98`
    /// codes for Gas, and everything else 13 characters long is a GS1 GLN. This
    /// is the DE 3055 twin of the UNB DE 0007 rule
    /// ([`unb_qualifier`](crate::builders::unb_qualifier), `500`/`502`/`14`), and
    /// the two must agree: an interchange whose UNB says DVGW while its NAD says
    /// BDEW names two different issuing offices for one party.
    ///
    /// UTILMD AHB Gas G1.1/G1.2 admits only `9` and `332` on a party NAD — a
    /// Gas message carrying `293` states a code list the Anwendungsfall does not
    /// define. (The single exception, PID 44060 „Antwort auf die
    /// Geschäftsdatenanfrage", also admits `293` because the MSB it answers may
    /// be a Strom party; that is a wider set, so deriving from the MP-ID stays
    /// inside it.)
    ///
    /// A 16-character EIC resolves to [`AgencyCode::Bdew`], matching
    /// `unb_qualifier`'s `500`: BDEW is the German EIC issuing office.
    ///
    /// ```rust
    /// use edi_energy::AgencyCode;
    ///
    /// assert_eq!(AgencyCode::for_mp_id("9900123456789"), AgencyCode::Bdew); // BDEW Strom
    /// assert_eq!(AgencyCode::for_mp_id("9870123456789"), AgencyCode::Dvgw); // DVGW Gas
    /// assert_eq!(AgencyCode::for_mp_id("4012345000023"), AgencyCode::Gs1);  // GS1 GLN
    /// ```
    #[must_use]
    pub fn for_mp_id(mp_id: &str) -> Self {
        match mp_id.len() {
            13 if mp_id.starts_with("99") => Self::Bdew,
            13 if mp_id.starts_with("98") => Self::Dvgw,
            13 => Self::Gs1,
            _ => Self::Bdew,
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

    /// The NAD DE 3055 agency and the UNB DE 0007 qualifier name the same
    /// issuing office. A message whose envelope says DVGW and whose NAD says
    /// BDEW is internally inconsistent, and the Gas AHB does not define `293`
    /// on a party NAD at all.
    #[test]
    fn nad_agency_agrees_with_the_unb_qualifier() {
        for (mp_id, agency, unb) in [
            ("9900123456789", "293", "500"),
            ("9870123456789", "332", "502"),
            ("4012345000023", "9", "14"),
            ("10XDE-EON-NETZ-C", "293", "500"),
        ] {
            assert_eq!(AgencyCode::for_mp_id(mp_id).as_str(), agency, "{mp_id}");
            assert_eq!(crate::builders::unb_qualifier(mp_id), unb, "{mp_id}");
        }
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
