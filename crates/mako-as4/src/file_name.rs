//! The file name of an Übertragungsdatei.

/// The Übertragungsdatei name of Allgemeine Festlegungen V6.1d §2.12:
/// `Nachrichtentyp_Anwendungsreferenz_von_an_yyyymmdd_DAR.txt`.
///
/// - `message_type` — UNH DE0065 (`UTILMD`, `APERAK`, …)
/// - `anwendungsreferenz` — UNB DE0026, empty for most types; an empty value
///   gives a double underscore
/// - `sender`, `receiver` — the UNB DE0004 / DE0010 MP-IDs
/// - `year`, `month`, `day` — the creation date **in UTC**
/// - `dar` — the UNB DE0020 Datenaustauschreferenz
///
/// The name rides the payload part (`Payload::named`); `asx-rs` refuses the
/// send when it is not a valid part file name rather than dropping it.
#[must_use]
pub fn uebertragungsdatei_name(
    message_type: &str,
    anwendungsreferenz: &str,
    sender: &str,
    receiver: &str,
    (year, month, day): (i32, u8, u8),
    dar: &str,
) -> String {
    format!(
        "{message_type}_{anwendungsreferenz}_{sender}_{receiver}_{year:04}{month:02}{day:02}_{dar}.txt"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_anwendungsreferenz_gives_a_double_underscore() {
        assert_eq!(
            uebertragungsdatei_name(
                "UTILMD",
                "",
                "9900000000003",
                "9900000000010",
                (2026, 10, 1),
                "DAR123"
            ),
            "UTILMD__9900000000003_9900000000010_20261001_DAR123.txt"
        );
    }

    #[test]
    fn an_anwendungsreferenz_is_carried() {
        assert_eq!(
            uebertragungsdatei_name("MSCONS", "TL", "A", "B", (2027, 1, 9), "7"),
            "MSCONS_TL_A_B_20270109_7.txt"
        );
    }
}
