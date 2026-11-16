//! UNB/UNZ interchange envelope construction.
//!
//! Every message builder in this module produces a *message* (`UNH`…`UNT`). A
//! message is not sendable on its own: the wire unit a market partner receives
//! over AS4 is an *interchange*, which wraps one or more messages in a `UNB`
//! header and a `UNZ` trailer.
//!
//! The envelope is a format concern, so it belongs here next to the message
//! builders rather than in each sender. Both `makod`'s outbound renderer and
//! the `makotest` toolkit build interchanges through this type.

use crate::Error;

/// Builds a UNB/UNZ interchange envelope around one or more messages.
///
/// ```
/// use edi_energy::builders::InterchangeBuilder;
///
/// let wire = InterchangeBuilder::new("9900123456789", "9900987654321", "REF001")
///     .transmission("260802", "0915")
///     .message(b"UNH+1+UTILMD:D:11A:UN:S2.1'BGM+E01'UNT+3+1'".to_vec())
///     .build()
///     .unwrap();
///
/// let text = String::from_utf8(wire).unwrap();
/// assert!(text.starts_with("UNB+UNOC:3+9900123456789:500+9900987654321:500+260802:0915+REF001'"));
/// assert!(text.ends_with("UNZ+1+REF001'"));
/// ```
#[derive(Debug, Clone)]
pub struct InterchangeBuilder {
    sender: String,
    receiver: String,
    /// UNB DE0020 / UNZ DE0036 Datenaustauschreferenz (`an..14`).
    dar: String,
    /// UNB S004 as written: `YYMMDD` and `HHMM`.
    transmission: Option<(String, String)>,
    /// UNB DE 0026 Anwendungsreferenz, where the AHB fixes one.
    application_reference: Option<String>,
    messages: Vec<Vec<u8>>,
}

impl InterchangeBuilder {
    /// Start an interchange from `sender` to `receiver` with Datenaustauschreferenz `dar`.
    ///
    /// The transmission timestamp is a parameter rather than a clock read so
    /// that building an interchange stays deterministic — golden-file tests
    /// depend on it — and [`build`](Self::build) refuses an interchange
    /// without one.
    pub fn new(
        sender: impl Into<String>,
        receiver: impl Into<String>,
        dar: impl Into<String>,
    ) -> Self {
        Self {
            sender: sender.into(),
            receiver: receiver.into(),
            dar: dar.into(),
            transmission: None,
            application_reference: None,
            messages: Vec::new(),
        }
    }

    /// Set the UNB transmission date (`YYMMDD`) and time (`HHMM`).
    #[must_use]
    pub fn transmission(mut self, date: impl Into<String>, time: impl Into<String>) -> Self {
        self.transmission = Some((date.into(), time.into()));
        self
    }

    /// Set the UNB **application reference** (DE 0026).
    ///
    /// Not decoration: the MSCONS AHB fixes it per use case — `TL` „Lastgang,
    /// beliebiger Zeitraum" is Muss on the Werte-nach-Typ-2 interchange — and a
    /// receiver may route on it before the message is parsed at all.
    #[must_use]
    pub fn application_reference(mut self, reference: impl Into<String>) -> Self {
        self.application_reference = Some(reference.into());
        self
    }

    /// Append one serialized message (`UNH`…`UNT`).
    #[must_use]
    pub fn message(mut self, bytes: Vec<u8>) -> Self {
        self.messages.push(bytes);
        self
    }

    /// Render the interchange.
    ///
    /// `edifact::InterchangeBuilder` writes the envelope: it computes the `UNZ`
    /// count from the messages actually added, checks each message's `UNT`
    /// count and reference, and encodes the payload into the `UNOC` repertoire
    /// the header declares.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Serialize`] when the interchange cannot be written — no
    /// transmission timestamp or no message, a party ID or
    /// Datenaustauschreferenz outside the UNOC character set or beyond the
    /// `an..14` bound of DE 0020, or a message whose `UNT` disagrees with it.
    pub fn build(self) -> Result<Vec<u8>, Error> {
        let (date, time) = self
            .transmission
            .as_ref()
            .ok_or_else(|| Error::Serialize("UNB S004: no transmission date and time".into()))?;
        let prepared = preparation_date_time(date, time)?;
        let party = |id: &str| -> Result<edifact::Party, Error> {
            Ok(edifact::Party::new(id).with_qualifier(unb_qualifier_or_refuse(id)?))
        };
        let mut header = edifact::InterchangeHeader::new(
            INTERCHANGE_CHARSET,
            edifact::SyntaxVersion::V3,
            party(&self.sender)?,
            party(&self.receiver)?,
            prepared,
            self.dar,
        );
        if let Some(app_ref) = self.application_reference {
            header = header.with_application_ref(app_ref);
        }
        // The message builders render `UNH`…`UNT` as UTF-8, because their own
        // `build()` parses the bytes straight back into a typed message; the
        // envelope encodes them into the declared repertoire.
        let mut builder = edifact::InterchangeBuilder::new(header);
        for m in self.messages {
            let text = String::from_utf8(m)
                .map_err(|e| Error::Serialize(format!("message payload is not UTF-8: {e}")))?;
            builder = builder.message(text);
        }
        builder.build().map_err(Error::write)
    }
}

/// UNB S004 from `YYMMDD` and `HHMM`, the syntax version 3 representation.
fn preparation_date_time(date: &str, time: &str) -> Result<edifact::PreparationDateTime, Error> {
    let refuse = || Error::Serialize(format!("UNB S004 {date:?}:{time:?} is not YYMMDD:HHMM"));
    let field = |s: &str, at: usize| -> Result<u8, Error> {
        s.get(at..at + 2)
            .filter(|f| f.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|f| f.parse().ok())
            .ok_or_else(refuse)
    };
    if date.len() != 6 || time.len() != 4 {
        return Err(refuse());
    }
    edifact::PreparationDateTime::from_two_digit_year(
        field(date, 0)?,
        field(date, 2)?,
        field(date, 4)?,
        field(time, 0)?,
        field(time, 2)?,
    )
    .map_err(Error::write)
}

/// The character repertoire every BDEW `MaKo` interchange declares in `UNB`
/// S001 DE 0001 — `UNOC`, which is ISO 8859-1 rather than UTF-8.
///
/// Declared once so the header and the payload encoding cannot disagree.
const INTERCHANGE_CHARSET: edifact::Charset = edifact::Charset::UnoC;

/// The UNB party-identification qualifier (DE 3055) for a market-partner ID.
///
/// `14` = GS1 GLN, `500` = DE BDEW, `502` = DE DVGW (Allgemeine Festlegungen
/// 6.1d S. 50). BDEW-issued 13-digit MP-IDs start with `99` and DVGW-issued
/// with `98`. An identifier that is no 13-digit MP-ID — an EIC among them — has
/// no qualifier: `None`, the twin of [`crate::AgencyCode::for_mp_id`].
#[must_use]
pub fn unb_qualifier(mp_id: &str) -> Option<&'static str> {
    crate::AgencyCode::for_mp_id(mp_id).map(|a| match a {
        crate::AgencyCode::Bdew => "500",
        crate::AgencyCode::Dvgw => "502",
        crate::AgencyCode::Gs1 => "14",
    })
}

/// [`unb_qualifier`], refused by name for an identifier that is no MP-ID.
pub(crate) fn unb_qualifier_or_refuse(mp_id: &str) -> Result<&'static str, Error> {
    unb_qualifier(mp_id).ok_or_else(|| {
        Error::Serialize(format!(
            "{mp_id:?} is no MP-ID (BDEW-/DVGW-Codenummer or GLN, 13 digits): UNB DE 0007 has \
             no qualifier for it"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// EDI@Energy `AF61d`: 14 = GS1, 500 = DE BDEW, 502 = DE DVGW.
    #[test]
    fn qualifier_per_af61d() {
        assert_eq!(unb_qualifier("9900123456789"), Some("500"));
        assert_eq!(unb_qualifier("9870123456789"), Some("502"));
        assert_eq!(unb_qualifier("4012345000023"), Some("14"));
        assert_eq!(unb_qualifier("10XDE-EON-NETZ-C"), None);
    }

    #[test]
    fn unz_count_follows_the_payload() {
        let wire = InterchangeBuilder::new("9900123456789", "9900987654321", "R1")
            .transmission("260802", "0915")
            .message(b"UNH+1+X:D:11A:UN'BGM+7'UNT+3+1'".to_vec())
            .message(b"UNH+2+X:D:11A:UN'BGM+7'UNT+3+2'".to_vec())
            .build()
            .unwrap();
        let text = String::from_utf8(wire).unwrap();
        assert!(text.ends_with("UNZ+2+R1'"), "{text}");
    }

    /// A message whose `UNT` miscounts its segments is refused, not wrapped.
    #[test]
    fn a_miscounted_message_is_refused() {
        let result = InterchangeBuilder::new("9900123456789", "9900987654321", "R1")
            .transmission("260802", "0915")
            .message(b"UNH+1+X:D:11A:UN'BGM+7'UNT+2+1'".to_vec())
            .build();
        assert!(matches!(result, Err(Error::Serialize(_))), "{result:?}");
    }

    #[test]
    fn an_interchange_without_a_transmission_time_is_refused() {
        let result = InterchangeBuilder::new("9900123456789", "9900987654321", "R1")
            .message(b"UNH+1+X:D:11A:UN'BGM+7'UNT+3+1'".to_vec())
            .build();
        assert!(matches!(result, Err(Error::Serialize(_))), "{result:?}");
    }

    #[test]
    fn the_application_reference_follows_an_empty_s005() {
        let wire = InterchangeBuilder::new("9900123456789", "9900987654321", "R1")
            .transmission("260802", "0915")
            .application_reference("TL")
            .message(b"UNH+1+X:D:11A:UN'BGM+7'UNT+3+1'".to_vec())
            .build()
            .unwrap();
        let text = String::from_utf8(wire).unwrap();
        assert!(
            text.starts_with("UNB+UNOC:3+9900123456789:500+9900987654321:500+260802:0915+R1++TL'"),
            "{text}"
        );
    }
}
