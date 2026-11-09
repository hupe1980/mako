//! Whether an energy invoice's Rechnungsempfänger sits in Germany — the only
//! case mako taxes.
//!
//! Every invoice mako issues settles a supply of gas, electricity or
//! heating/cooling and is taxed as a domestic supply. Whether an operator
//! supplies across a border is the operator's fact, read from the recipient's
//! address: a Rechnungsempfänger in another Member State of the EU is a
//! cross-border supply whose treatment under Art. 369aa MwStSystRL (the OSS) is
//! not built, and one outside the EU is an export supply whose treatment is not
//! built either. Both are refused rather than taxed as domestic — a refusal the
//! operator can act on, where a domestic invoice to a foreign recipient reads
//! exactly like a correct one downstream.
//!
//! The Member-State refusal stands when Art. 369aa MwStSystRL applies from
//! 01.01.2027 (Richtlinie (EU) 2025/516 Art. 2 Nr. 12 and Art. 6(2)): the
//! article admits such a supply to the One-Stop-Shop return of Art. 369b
//! without prescribing invoice content, and the VAT stays that of „the Member
//! State in which the goods are subject to VAT", whose rates mako does not
//! hold.
//!
//! The rule reads the country code (ISO 3166-1 alpha-2, the EN 16931 BT-55
//! term) and nothing else. An address without one is refused: the decision is
//! not taken on the caller's behalf. A service that documents its own default
//! for an absent code applies it **before** asking, so that the country the
//! rule decides on is the country the document states.

/// The Member States of the European Union other than Germany, as ISO 3166-1
/// alpha-2 codes — plus `EL`, the prefix Greek VAT identification numbers
/// carry beside the ISO `GR`.
const ANDERE_MITGLIEDSTAATEN: [&str; 27] = [
    "AT", "BE", "BG", "CY", "CZ", "DK", "EE", "EL", "ES", "FI", "FR", "GR", "HR", "HU", "IE", "IT",
    "LT", "LU", "LV", "MT", "NL", "PL", "PT", "RO", "SE", "SI", "SK",
];

/// Why the invoice is refused: the recipient is not in Germany, or where it is
/// is not known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NichtInland {
    /// The recipient sits in another Member State of the EU.
    Mitgliedstaat {
        /// The country code, upper-cased.
        land: String,
    },
    /// The recipient sits outside the EU.
    Drittland {
        /// The country code, upper-cased.
        land: String,
    },
    /// The recipient's address carries no country code.
    LandFehlt,
    /// The country code is not two letters.
    LandUngueltig {
        /// The value as received.
        wert: String,
    },
}

impl NichtInland {
    /// The stable machine code a service answers the refusal with.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Mitgliedstaat { .. } => "CROSS_BORDER_SUPPLY_UNSUPPORTED",
            Self::Drittland { .. } => "EXPORT_SUPPLY_UNSUPPORTED",
            Self::LandFehlt => "RECIPIENT_COUNTRY_MISSING",
            Self::LandUngueltig { .. } => "RECIPIENT_COUNTRY_INVALID",
        }
    }
}

impl std::fmt::Display for NichtInland {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mitgliedstaat { land } => write!(
                f,
                "der Rechnungsempfänger sitzt in {land}, einem anderen Mitgliedstaat der EU — \
                 mako besteuert eine grenzüberschreitende Lieferung von Gas, Elektrizität, \
                 Wärme oder Kälte nicht als Inlandsumsatz, und ihre OSS-Behandlung nach \
                 Art. 369aa MwStSystRL ist nicht gebaut"
            ),
            Self::Drittland { land } => write!(
                f,
                "der Rechnungsempfänger sitzt in {land}, außerhalb der EU — mako besteuert \
                 eine Lieferung an ihn nicht als Inlandsumsatz, und die Behandlung einer \
                 Ausfuhr ist nicht gebaut"
            ),
            Self::LandFehlt => f.write_str(
                "die Anschrift des Rechnungsempfängers trägt keinen Ländercode — ob die \
                 Lieferung ein Inlandsumsatz ist, wird nicht unterstellt",
            ),
            Self::LandUngueltig { wert } => write!(
                f,
                "der Ländercode {wert:?} des Rechnungsempfängers ist kein ISO-3166-1-alpha-2-Code \
                 — ob die Lieferung ein Inlandsumsatz ist, wird nicht unterstellt"
            ),
        }
    }
}

impl std::error::Error for NichtInland {}

/// Whether an invoice to a Rechnungsempfänger with this country code is a
/// domestic supply mako may tax: `DE`, case-insensitively, and nothing else.
///
/// # Errors
///
/// [`NichtInland`] — another Member State, a third country, or a code that is
/// absent or not two letters.
pub fn pruefe_empfaengerland(land: Option<&str>) -> Result<(), NichtInland> {
    let wert = land.map(str::trim).filter(|w| !w.is_empty());
    let Some(wert) = wert else {
        return Err(NichtInland::LandFehlt);
    };
    if wert.len() != 2 || !wert.bytes().all(|b| b.is_ascii_alphabetic()) {
        return Err(NichtInland::LandUngueltig {
            wert: wert.to_owned(),
        });
    }
    let land = wert.to_ascii_uppercase();
    if land == "DE" {
        Ok(())
    } else if ANDERE_MITGLIEDSTAATEN.contains(&land.as_str()) {
        Err(NichtInland::Mitgliedstaat { land })
    } else {
        Err(NichtInland::Drittland { land })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Rechnungsempfänger in Austria is a cross-border supply and refused,
    /// naming the OSS treatment that is not built.
    #[test]
    fn a_recipient_in_austria_is_refused() {
        let err = pruefe_empfaengerland(Some("AT")).expect_err("AT is another Member State");
        assert_eq!(err.code(), "CROSS_BORDER_SUPPLY_UNSUPPORTED");
        assert!(err.to_string().contains("Art. 369aa MwStSystRL"), "{err}");
    }

    #[test]
    fn germany_is_domestic_in_any_case() {
        for de in ["DE", "de", " DE "] {
            pruefe_empfaengerland(Some(de)).expect("domestic");
        }
    }

    /// Every other Member State is refused as cross-border, never as an export.
    #[test]
    fn every_other_member_state_is_cross_border() {
        assert_eq!(ANDERE_MITGLIEDSTAATEN.len(), 27, "26 Member States and EL");
        assert!(!ANDERE_MITGLIEDSTAATEN.contains(&"DE"));
        for land in ANDERE_MITGLIEDSTAATEN {
            assert_eq!(
                pruefe_empfaengerland(Some(&land.to_lowercase())),
                Err(NichtInland::Mitgliedstaat {
                    land: land.to_owned()
                }),
            );
        }
    }

    /// A country outside the EU is not domestic either: refused as an export,
    /// not guessed.
    #[test]
    fn a_third_country_is_refused_as_an_export() {
        for land in ["CH", "GB", "NO", "US"] {
            let err = pruefe_empfaengerland(Some(land)).expect_err("not domestic");
            assert_eq!(err.code(), "EXPORT_SUPPLY_UNSUPPORTED", "{land}");
        }
    }

    /// A missing country is a refusal, not a silent `DE`.
    #[test]
    fn an_absent_or_malformed_country_is_refused() {
        assert_eq!(
            pruefe_empfaengerland(None).map_err(|e| e.code()),
            Err("RECIPIENT_COUNTRY_MISSING")
        );
        assert_eq!(
            pruefe_empfaengerland(Some("  ")).map_err(|e| e.code()),
            Err("RECIPIENT_COUNTRY_MISSING")
        );
        for wert in ["DEU", "Deutschland", "D", "4X"] {
            assert_eq!(
                pruefe_empfaengerland(Some(wert)).map_err(|e| e.code()),
                Err("RECIPIENT_COUNTRY_INVALID"),
                "{wert}"
            );
        }
    }
}
