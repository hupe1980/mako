//! The `eb:Service` values of BDEW AS4-Profil 1.2.

/// A business service of BDEW AS4-Profil 1.2 §2.3.1.2.1 (S. 14).
///
/// „Im BDEW AS4-Profil ist das AS4-Protokoll vom Geschäftsprozess entkoppelt.
/// Daher ERFORDERT dieses Profil einen vordefinierten Wert für den Service beim
/// Austausch von Nutzdaten". The test service (§2.3.6) and the path-switch
/// service (§2.3.7) are the profile's own and set by `asx-rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Service {
    /// Marktprozesse — every EDIFACT Übertragungsdatei of the Marktkommunikation.
    Marktprozesse,
    /// Fahrplan.
    Fahrplan,
    /// Redispatch 2.0 Prozessdaten.
    Redispatch,
    /// KWEP.
    Kwep,
    /// System Operation Guideline.
    SystemOperation,
}

impl Service {
    /// Every service, in the order §2.3.1.2.1 lists them.
    pub const ALL: [Self; 5] = [
        Self::Marktprozesse,
        Self::Fahrplan,
        Self::Redispatch,
        Self::Kwep,
        Self::SystemOperation,
    ];

    /// The `eb:Service` URI.
    #[must_use]
    pub const fn uri(self) -> &'static str {
        match self {
            Self::Marktprozesse => "https://www.bdew.de/as4/communication/services/MP",
            Self::Fahrplan => "https://www.bdew.de/as4/communication/services/FP",
            Self::Redispatch => "https://www.bdew.de/as4/communication/services/RD",
            Self::Kwep => "https://www.bdew.de/as4/communication/services/KW",
            Self::SystemOperation => "https://www.bdew.de/as4/communication/services/SO",
        }
    }

    /// The service named by `uri`, if it is one of the five.
    #[must_use]
    pub fn from_uri(uri: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.uri() == uri)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The values are quoted from §2.3.1.2.1; a receiving MSH routes on them,
    /// so a typo is a delivery failure the sender cannot see.
    #[test]
    fn the_service_uris_are_the_profiles() {
        assert_eq!(
            Service::ALL.map(Service::uri),
            [
                "https://www.bdew.de/as4/communication/services/MP",
                "https://www.bdew.de/as4/communication/services/FP",
                "https://www.bdew.de/as4/communication/services/RD",
                "https://www.bdew.de/as4/communication/services/KW",
                "https://www.bdew.de/as4/communication/services/SO",
            ]
        );
        for s in Service::ALL {
            assert_eq!(Service::from_uri(s.uri()), Some(s));
        }
        assert_eq!(Service::from_uri("urn:bdew:as4:service"), None);
    }
}
