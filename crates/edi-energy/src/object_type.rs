/// Domain type for BDEW object-type qualifier codes (EDIFACT DE 7495).
///
/// Used in UTILMD IDE segments to identify the type of the supply-point object.
/// Prefer this over raw qualifier strings to get compile-time safety and
/// self-documenting code.
///
/// # Example
/// ```rust
/// use edi_energy::ObjectType;
///
/// let qualifier = ObjectType::Marktlokation.qualifier_code();
/// assert_eq!(qualifier, "Z18");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ObjectType {
    /// Marktlokation (`MaLo`) — DE 7495 qualifier `"Z18"`.
    ///
    /// A market location is the unit of settlement in the German energy market.
    /// It aggregates one or more Messlokationen.
    Marktlokation,
    /// Messlokation (`MeLo`) — DE 7495 qualifier `"Z19"`.
    ///
    /// A measurement location is a physical metering point.
    Messlokation,
    /// Tranche — DE 7495 qualifier `"Z30"`.
    ///
    /// A tranche is used for load-profile-based settlement.
    Tranche,
    /// Netzlokation — DE 7495 qualifier `"Z31"`.
    ///
    /// A network location for gas market processes.
    Netzlokation,
    /// Technische Ressource — DE 7495 qualifier `"Z32"`.
    ///
    /// A technical resource such as a controllable consumer asset.
    TechnischeRessource,
    /// Steuerbare Ressource — DE 7495 qualifier `"ZE7"`.
    ///
    /// A controllable resource identified by Steuerbarer-Ressourcen-ID.
    SteuerungRessource,
}

impl ObjectType {
    /// Returns the EDIFACT DE 7495 qualifier code for this object type.
    ///
    /// ```rust
    /// use edi_energy::ObjectType;
    ///
    /// assert_eq!(ObjectType::Marktlokation.qualifier_code(), "Z18");
    /// assert_eq!(ObjectType::Messlokation.qualifier_code(), "Z19");
    /// assert_eq!(ObjectType::Tranche.qualifier_code(), "Z30");
    /// assert_eq!(ObjectType::Netzlokation.qualifier_code(), "Z31");
    /// assert_eq!(ObjectType::TechnischeRessource.qualifier_code(), "Z32");
    /// assert_eq!(ObjectType::SteuerungRessource.qualifier_code(), "ZE7");
    /// ```
    #[must_use]
    pub fn qualifier_code(self) -> &'static str {
        match self {
            Self::Marktlokation => "Z18",
            Self::Messlokation => "Z19",
            Self::Tranche => "Z30",
            Self::Netzlokation => "Z31",
            Self::TechnischeRessource => "Z32",
            Self::SteuerungRessource => "ZE7",
        }
    }

    /// Attempt to parse an `ObjectType` from a raw qualifier code string.
    ///
    /// Returns `None` for unknown or extension codes.
    ///
    /// ```rust
    /// use edi_energy::ObjectType;
    ///
    /// assert_eq!(ObjectType::from_qualifier_code("Z18"), Some(ObjectType::Marktlokation));
    /// assert_eq!(ObjectType::from_qualifier_code("Z99"), None);
    /// ```
    #[must_use]
    pub fn from_qualifier_code(code: &str) -> Option<Self> {
        match code {
            "Z18" => Some(Self::Marktlokation),
            "Z19" => Some(Self::Messlokation),
            "Z30" => Some(Self::Tranche),
            "Z31" => Some(Self::Netzlokation),
            "Z32" => Some(Self::TechnischeRessource),
            "ZE7" => Some(Self::SteuerungRessource),
            _ => None,
        }
    }
}

impl std::fmt::Display for ObjectType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.qualifier_code())
    }
}

impl From<ObjectType> for String {
    fn from(ot: ObjectType) -> String {
        ot.qualifier_code().to_owned()
    }
}
