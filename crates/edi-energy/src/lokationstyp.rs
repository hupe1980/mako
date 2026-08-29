/// Domain type for the UTILMD **`SG5 LOC` DE 3227** Lokationstyp qualifiers.
///
/// UTILMD names the object a Vorgang is about in `SG5 LOC`, not in `IDE`:
/// `IDE` DE 7495 has exactly two values (`24` Vorgang, `Z01` Liste) and its
/// DE 7402 carries a Vorgangsnummer. The codes below are the ones the MIG
/// lists for `LOC` DE 3227 (Strom S2.2 Zähler 0330 Nr. 00046–00053, Gas G1.2
/// likewise).
///
/// # Example
/// ```rust
/// use edi_energy::Lokationstyp;
///
/// assert_eq!(Lokationstyp::Marktlokation.qualifier_code(), "Z16");
/// assert_eq!(Lokationstyp::Messlokation.qualifier_code(), "Z17");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Lokationstyp {
    /// Meldepunkt — `LOC+172`.
    ///
    /// The Gas qualifier. UTILMD AHB Gas G1.1/G1.2 uses it for every Lokation
    /// and distinguishes Marktlokation from Messlokation by the *format* of
    /// DE 3225 rather than by the qualifier, so this one variant covers both.
    Meldepunkt,
    /// MaBiS-Zählpunkt — `LOC+Z15`.
    MabisZaehlpunkt,
    /// Marktlokation (`MaLo`) — `LOC+Z16`.
    ///
    /// The unit of settlement; aggregates one or more Messlokationen.
    Marktlokation,
    /// Messlokation (`MeLo`) — `LOC+Z17`.
    ///
    /// A physical metering point.
    Messlokation,
    /// Netzlokation — `LOC+Z18`.
    Netzlokation,
    /// Steuerbare Ressource — `LOC+Z19` (§ 14a `EnWG`).
    SteuerbareRessource,
    /// Technische Ressource — `LOC+Z20`.
    TechnischeRessource,
    /// Tranche — `LOC+Z21`.
    Tranche,
    /// Ruhende Marktlokation — `LOC+Z22` (§ 20 Abs. 1d `EnWG` / § 10c EEG).
    RuhendeMarktlokation,
}

impl Lokationstyp {
    /// Returns the EDIFACT `LOC` DE 3227 qualifier code for this Lokationstyp.
    ///
    /// ```rust
    /// use edi_energy::Lokationstyp;
    ///
    /// assert_eq!(Lokationstyp::Marktlokation.qualifier_code(), "Z16");
    /// assert_eq!(Lokationstyp::Messlokation.qualifier_code(), "Z17");
    /// assert_eq!(Lokationstyp::Netzlokation.qualifier_code(), "Z18");
    /// assert_eq!(Lokationstyp::SteuerbareRessource.qualifier_code(), "Z19");
    /// assert_eq!(Lokationstyp::TechnischeRessource.qualifier_code(), "Z20");
    /// assert_eq!(Lokationstyp::Tranche.qualifier_code(), "Z21");
    /// assert_eq!(Lokationstyp::RuhendeMarktlokation.qualifier_code(), "Z22");
    /// ```
    #[must_use]
    pub fn qualifier_code(self) -> &'static str {
        use crate::utilmd_codes::loc;
        match self {
            Self::Meldepunkt => loc::MELDEPUNKT,
            Self::MabisZaehlpunkt => loc::MABIS_ZAEHLPUNKT,
            Self::Marktlokation => loc::MARKTLOKATION,
            Self::Messlokation => loc::MESSLOKATION,
            Self::Netzlokation => loc::NETZLOKATION,
            Self::SteuerbareRessource => loc::STEUERBARE_RESSOURCE,
            Self::TechnischeRessource => loc::TECHNISCHE_RESSOURCE,
            Self::Tranche => loc::TRANCHE,
            Self::RuhendeMarktlokation => loc::RUHENDE_MARKTLOKATION,
        }
    }

    /// Attempt to parse a `Lokationstyp` from a raw `LOC` DE 3227 code.
    ///
    /// Returns `None` for unknown or extension codes.
    ///
    /// ```rust
    /// use edi_energy::Lokationstyp;
    ///
    /// assert_eq!(Lokationstyp::from_qualifier_code("Z16"), Some(Lokationstyp::Marktlokation));
    /// assert_eq!(Lokationstyp::from_qualifier_code("172"), Some(Lokationstyp::Meldepunkt));
    /// assert_eq!(Lokationstyp::from_qualifier_code("Z99"), None);
    /// ```
    #[must_use]
    pub fn from_qualifier_code(code: &str) -> Option<Self> {
        use crate::utilmd_codes::loc;
        match code {
            loc::MELDEPUNKT => Some(Self::Meldepunkt),
            loc::MABIS_ZAEHLPUNKT => Some(Self::MabisZaehlpunkt),
            loc::MARKTLOKATION => Some(Self::Marktlokation),
            loc::MESSLOKATION => Some(Self::Messlokation),
            loc::NETZLOKATION => Some(Self::Netzlokation),
            loc::STEUERBARE_RESSOURCE => Some(Self::SteuerbareRessource),
            loc::TECHNISCHE_RESSOURCE => Some(Self::TechnischeRessource),
            loc::TRANCHE => Some(Self::Tranche),
            loc::RUHENDE_MARKTLOKATION => Some(Self::RuhendeMarktlokation),
            _ => None,
        }
    }
}

impl std::fmt::Display for Lokationstyp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.qualifier_code())
    }
}

impl From<Lokationstyp> for String {
    fn from(lt: Lokationstyp) -> String {
        lt.qualifier_code().to_owned()
    }
}
