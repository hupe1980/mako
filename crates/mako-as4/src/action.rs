//! The EDIFACT message types BDEW market communication carries over AS4.
//!
//! Under BDEW AS4-Profil 1.2 the ebMS3 `eb:Action` is a fixed value the
//! profile sets (§2.3.1.2.2), so the message type travels in the EDIFACT
//! payload and in its file name, not in the AS4 header. [`BdewAction`] names
//! the type and selects the AS4 [`Service`] the message is sent under.

use std::fmt;
use std::str::FromStr;

use crate::service::Service;

/// An EDIFACT message type used in BDEW German energy market communication.
///
/// All 16 standard BDEW EDIFACT message types (UTILMD, APERAK, CONTRL, MSCONS,
/// INVOIC, REMADV, IFTSTA, ORDRSP, ORDERS, ORDCHG, REQOTE, INSRPT, PRICAT,
/// QUOTES, PARTIN, UTILTS) are named variants. Any other type is
/// [`Custom`](Self::Custom), which carries its UNH DE0065 name.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BdewAction {
    /// UTILMD — Stammdaten (GPKE, WiM, GeLi Gas, WiM Gas).
    Utilmd,
    /// APERAK — Anwendungsfehler- und Bestätigungsmeldung.
    Aperak,
    /// CONTRL — Syntax- und Servicebericht.
    Contrl,
    /// MSCONS — Messwerte, Lastgänge, Energiemengen.
    Mscons,
    /// INVOIC — Rechnung.
    Invoic,
    /// REMADV — Zahlungsavis.
    Remadv,
    /// IFTSTA — Statusmeldung.
    Iftsta,
    /// ORDRSP — Bestellantwort.
    Ordrsp,
    /// ORDERS — Bestellung.
    Orders,
    /// ORDCHG — Bestelländerung.
    Ordchg,
    /// REQOTE — Anfrage.
    Reqote,
    /// INSRPT — Prüfbericht.
    Insrpt,
    /// PRICAT — Preisblatt.
    Pricat,
    /// QUOTES — Angebot.
    Quotes,
    /// PARTIN — Kommunikationsdaten (GPKE 37000–37006, GeLi Gas 37008–37014).
    Partin,
    /// UTILTS — Zeitreihen und Konfigurationsdaten.
    Utilts,
    /// Any other EDIFACT type, by its UNH DE0065 name (e.g. `"ALOCAT"`).
    Custom(String),
}

impl BdewAction {
    /// Every standard (non-[`Custom`](Self::Custom)) variant.
    #[must_use]
    pub fn all_standard() -> Vec<Self> {
        vec![
            Self::Utilmd,
            Self::Aperak,
            Self::Contrl,
            Self::Mscons,
            Self::Invoic,
            Self::Remadv,
            Self::Iftsta,
            Self::Ordrsp,
            Self::Orders,
            Self::Ordchg,
            Self::Reqote,
            Self::Insrpt,
            Self::Pricat,
            Self::Quotes,
            Self::Partin,
            Self::Utilts,
        ]
    }

    /// The EDIFACT message-type name (UNH DE0065), e.g. `"UTILMD"`.
    #[must_use]
    pub fn as_edifact_type(&self) -> &str {
        match self {
            Self::Utilmd => "UTILMD",
            Self::Aperak => "APERAK",
            Self::Contrl => "CONTRL",
            Self::Mscons => "MSCONS",
            Self::Invoic => "INVOIC",
            Self::Remadv => "REMADV",
            Self::Iftsta => "IFTSTA",
            Self::Ordrsp => "ORDRSP",
            Self::Orders => "ORDERS",
            Self::Ordchg => "ORDCHG",
            Self::Reqote => "REQOTE",
            Self::Insrpt => "INSRPT",
            Self::Pricat => "PRICAT",
            Self::Quotes => "QUOTES",
            Self::Partin => "PARTIN",
            Self::Utilts => "UTILTS",
            Self::Custom(name) => name.as_str(),
        }
    }

    /// The AS4 service an Übertragungsdatei of this type is sent under.
    ///
    /// Every EDIFACT Übertragungsdatei of the Marktkommunikation is a
    /// Marktprozess (BDEW AS4-Profil 1.2 §2.3.1.2.1). The other four services
    /// carry document families that are not EDIFACT interchanges.
    #[must_use]
    pub fn service(&self) -> Service {
        Service::Marktprozesse
    }
}

/// Formats the EDIFACT type name.
impl fmt::Display for BdewAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_edifact_type())
    }
}

/// Parse an EDIFACT message-type name. Never fails: an unknown name is
/// [`BdewAction::Custom`].
impl FromStr for BdewAction {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "UTILMD" => Self::Utilmd,
            "APERAK" => Self::Aperak,
            "CONTRL" => Self::Contrl,
            "MSCONS" => Self::Mscons,
            "INVOIC" => Self::Invoic,
            "REMADV" => Self::Remadv,
            "IFTSTA" => Self::Iftsta,
            "ORDRSP" => Self::Ordrsp,
            "ORDERS" => Self::Orders,
            "ORDCHG" => Self::Ordchg,
            "REQOTE" => Self::Reqote,
            "INSRPT" => Self::Insrpt,
            "PRICAT" => Self::Pricat,
            "QUOTES" => Self::Quotes,
            "PARTIN" => Self::Partin,
            "UTILTS" => Self::Utilts,
            other => Self::Custom(other.to_owned()),
        })
    }
}

/// The [`BdewAction`] for an EDIFACT message-type name.
#[must_use]
pub fn bdew_action_from_str(message_type: &str) -> BdewAction {
    match message_type.parse() {
        Ok(action) => action,
        Err(never) => match never {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_standard_type_round_trips_through_its_name() {
        let all = BdewAction::all_standard();
        assert_eq!(all.len(), 16);
        for action in all {
            assert_eq!(bdew_action_from_str(action.as_edifact_type()), action);
            assert_eq!(action.to_string(), action.as_edifact_type());
        }
    }

    #[test]
    fn an_unknown_type_is_custom_and_keeps_its_name() {
        let action = bdew_action_from_str("ALOCAT");
        assert_eq!(action, BdewAction::Custom("ALOCAT".to_owned()));
        assert_eq!(action.as_edifact_type(), "ALOCAT");
    }

    /// BDEW AS4-Profil 1.2 §2.3.1.2.1: an EDIFACT Übertragungsdatei of the
    /// Marktkommunikation is sent under the Marktprozesse service.
    #[test]
    fn every_edifact_type_is_a_marktprozess() {
        for action in BdewAction::all_standard()
            .into_iter()
            .chain([BdewAction::Custom("NOMINT".to_owned())])
        {
            assert_eq!(action.service(), Service::Marktprozesse, "{action}");
        }
    }
}
