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

/// The one table: variant, wire code, and the Cargo feature that compiles it in.
///
/// Everything else is derived from it, so adding a message type is one row. A
/// missing `from_unh_code` arm would be silent — it turns a supported message
/// into `AnyMessage::Unknown`.
macro_rules! message_types {
    ($(($variant:ident, $code:literal, $feature:literal)),* $(,)?) => {
        impl MessageType {
            /// Returns the EDIFACT type code as it appears in the UNH segment
            /// (e.g. `"UTILMD"`).
            #[must_use]
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $code,)*
                }
            }

            /// Parses the type code from a UNH segment string slice.
            ///
            /// Returns `None` for unrecognised codes.
            #[must_use]
            pub fn from_unh_code(code: &str) -> Option<Self> {
                match code {
                    $($code => Some(Self::$variant),)*
                    _ => None,
                }
            }

            /// The Cargo feature that must be enabled to parse this type.
            #[must_use]
            pub fn feature_name(self) -> &'static str {
                match self {
                    $(Self::$variant => $feature,)*
                }
            }

            /// Returns `true` when this type's Cargo feature is compiled in.
            #[must_use]
            pub fn is_feature_enabled(self) -> bool {
                match self {
                    $(Self::$variant => cfg!(feature = $feature),)*
                }
            }

            /// Every message type, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),*];
        }
    };
}

message_types![
    (Utilmd, "UTILMD", "utilmd"),
    (Mscons, "MSCONS", "mscons"),
    (Aperak, "APERAK", "aperak"),
    (Contrl, "CONTRL", "contrl"),
    (Invoic, "INVOIC", "invoic"),
    (Remadv, "REMADV", "remadv"),
    (Orders, "ORDERS", "orders"),
    (Iftsta, "IFTSTA", "iftsta"),
    (Insrpt, "INSRPT", "insrpt"),
    (Reqote, "REQOTE", "reqote"),
    (Partin, "PARTIN", "partin"),
    (Ordchg, "ORDCHG", "ordchg"),
    (Ordrsp, "ORDRSP", "ordrsp"),
    (Quotes, "QUOTES", "quotes"),
    (Comdis, "COMDIS", "comdis"),
    (Pricat, "PRICAT", "pricat"),
    (Utilts, "UTILTS", "utilts"),
];

impl fmt::Display for MessageType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::MessageType;

    /// Every variant must round-trip through its wire code, and no two may share
    /// one.
    #[test]
    fn the_table_is_a_bijection() {
        let mut codes: Vec<&str> = Vec::new();
        for &mt in MessageType::ALL {
            assert_eq!(MessageType::from_unh_code(mt.as_str()), Some(mt));
            assert_eq!(mt.feature_name(), mt.as_str().to_lowercase());
            codes.push(mt.as_str());
        }
        let total = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), total, "two message types share a wire code");
        assert_eq!(MessageType::from_unh_code("NOMINT"), None);
    }
}
