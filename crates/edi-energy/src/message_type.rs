use std::fmt;

/// EDIFACT message type codes used in the German energy market (EDI@Energy).
///
/// All variants are always present regardless of enabled features; feature gates
/// control which concrete message structs and profile data are compiled in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MessageType {
    /// UTILMD — Utilities Master Data.\
    /// BDEW message for grid-connection processes (switchover, registration, etc.).
    Utilmd,
    /// MSCONS — Metered Services Consumption Report.\
    /// Meter value transmission between grid operator and balance-group manager.
    Mscons,
    /// APERAK — Application Error and Acknowledgement.\
    /// Technical rejection or acknowledgement of a previously received message.
    Aperak,
    /// CONTRL — Interchange Control Structure.\
    /// Syntax acknowledgement at interchange level.
    Contrl,
    /// INVOIC — Invoice.
    Invoic,
    /// REMADV — Remittance Advice.
    Remadv,
    /// ORDERS — Purchase Order.
    Orders,
    /// IFTSTA — International Multimodal Status Report Message.
    Iftsta,
    /// INSRPT — Inspection Report.
    Insrpt,
    /// REQOTE — Request for Quotation.
    Reqote,
    /// PARTIN — Party Information.
    Partin,
    /// ORDCHG — Purchase Order Change.
    Ordchg,
    /// ORDRSP — Purchase Order Response.
    Ordrsp,
    /// QUOTES — Quotation.
    Quotes,
    /// COMDIS — Commercial Dispute (Handelsunstimmigkeit).
    Comdis,
    /// PRICAT — Price/Sales Catalogue (Preisliste).
    Pricat,
    /// UTILTS — Übertragung technischer Stammdaten (Technical Master Data).
    Utilts,
}

impl MessageType {
    /// Returns the EDIFACT type code as it appears in the UNH segment (e.g. `"UTILMD"`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Utilmd => "UTILMD",
            Self::Mscons => "MSCONS",
            Self::Aperak => "APERAK",
            Self::Contrl => "CONTRL",
            Self::Invoic => "INVOIC",
            Self::Remadv => "REMADV",
            Self::Orders => "ORDERS",
            Self::Iftsta => "IFTSTA",
            Self::Insrpt => "INSRPT",
            Self::Reqote => "REQOTE",
            Self::Partin => "PARTIN",
            Self::Ordchg => "ORDCHG",
            Self::Ordrsp => "ORDRSP",
            Self::Quotes => "QUOTES",
            Self::Comdis => "COMDIS",
            Self::Pricat => "PRICAT",
            Self::Utilts => "UTILTS",
        }
    }

    /// Parses the type code from a UNH segment string slice.
    ///
    /// Returns `None` for unrecognised codes.
    #[must_use]
    pub fn from_unh_code(code: &str) -> Option<Self> {
        match code {
            "UTILMD" => Some(Self::Utilmd),
            "MSCONS" => Some(Self::Mscons),
            "APERAK" => Some(Self::Aperak),
            "CONTRL" => Some(Self::Contrl),
            "INVOIC" => Some(Self::Invoic),
            "REMADV" => Some(Self::Remadv),
            "ORDERS" => Some(Self::Orders),
            "IFTSTA" => Some(Self::Iftsta),
            "INSRPT" => Some(Self::Insrpt),
            "REQOTE" => Some(Self::Reqote),
            "PARTIN" => Some(Self::Partin),
            "ORDCHG" => Some(Self::Ordchg),
            "ORDRSP" => Some(Self::Ordrsp),
            "QUOTES" => Some(Self::Quotes),
            "COMDIS" => Some(Self::Comdis),
            "PRICAT" => Some(Self::Pricat),
            "UTILTS" => Some(Self::Utilts),
            _ => None,
        }
    }
}

impl fmt::Display for MessageType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
