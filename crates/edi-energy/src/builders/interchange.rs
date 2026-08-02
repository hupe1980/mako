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
///     .message(b"UNH+1+UTILMD:D:11A:UN:S2.1'UNT+2+1'".to_vec())
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
    date: String,
    time: String,
    messages: Vec<Vec<u8>>,
}

impl InterchangeBuilder {
    /// Start an interchange from `sender` to `receiver` with Datenaustauschreferenz `dar`.
    ///
    /// The transmission timestamp defaults to `000000`/`0000`; set a real one
    /// with [`transmission`](Self::transmission). It is a parameter rather than
    /// a clock read so that building an interchange stays deterministic —
    /// golden-file tests depend on it.
    pub fn new(
        sender: impl Into<String>,
        receiver: impl Into<String>,
        dar: impl Into<String>,
    ) -> Self {
        Self {
            sender: sender.into(),
            receiver: receiver.into(),
            dar: dar.into(),
            date: "000000".to_owned(),
            time: "0000".to_owned(),
            messages: Vec::new(),
        }
    }

    /// Set the UNB transmission date (`YYMMDD`) and time (`HHMM`).
    #[must_use]
    pub fn transmission(mut self, date: impl Into<String>, time: impl Into<String>) -> Self {
        self.date = date.into();
        self.time = time.into();
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
    /// The `UNZ` message count is derived from the messages actually added, so
    /// it cannot disagree with the payload.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Serialize`] when a field cannot be written — e.g. a
    /// party ID or Datenaustauschreferenz outside the UNOC character set, or
    /// beyond the `an..14` bound of DE 0020.
    pub fn build(self) -> Result<Vec<u8>, Error> {
        let payload_len: usize = self.messages.iter().map(Vec::len).sum();
        let mut w = edifact_rs::Writer::new(Vec::with_capacity(payload_len + 96));
        w.write_composites(
            "UNB",
            &[
                &["UNOC", "3"],
                &[&self.sender, unb_qualifier(&self.sender)],
                &[&self.receiver, unb_qualifier(&self.receiver)],
                &[&self.date, &self.time],
                &[&self.dar],
            ],
        )
        .map_err(|e| Error::Serialize(format!("UNB envelope: {e}")))?;
        let mut bytes = w
            .finish()
            .map_err(|e| Error::Serialize(format!("UNB envelope: {e}")))?;

        for m in &self.messages {
            bytes.extend_from_slice(m);
        }

        let count = self.messages.len().to_string();
        let mut w = edifact_rs::Writer::new(bytes);
        w.write_composites("UNZ", &[&[count.as_str()], &[&self.dar]])
            .map_err(|e| Error::Serialize(format!("UNZ envelope: {e}")))?;
        w.finish()
            .map_err(|e| Error::Serialize(format!("UNZ envelope: {e}")))
    }
}

/// The UNB party-identification qualifier (DE 3055) for a market-partner ID.
///
/// `14` = GS1 GLN, `500` = DE BDEW, `502` = DE DVGW. BDEW-issued 13-digit
/// MP-IDs start with `99` and DVGW-issued with `98`; 16-character EIC codes are
/// issued by BDEW as the German issuing office.
#[must_use]
pub fn unb_qualifier(mp_id: &str) -> &'static str {
    if mp_id.len() == 13 && mp_id.starts_with("99") {
        "500"
    } else if mp_id.len() == 13 && mp_id.starts_with("98") {
        "502"
    } else if mp_id.len() == 13 {
        "14"
    } else {
        "500"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// EDI@Energy `AF61d`: 14 = GS1, 500 = DE BDEW, 502 = DE DVGW.
    #[test]
    fn qualifier_per_af61d() {
        assert_eq!(unb_qualifier("9900123456789"), "500");
        assert_eq!(unb_qualifier("9870123456789"), "502");
        assert_eq!(unb_qualifier("4012345000023"), "14");
        assert_eq!(unb_qualifier("10XDE-EON-NETZ-I"), "500");
    }

    #[test]
    fn unz_count_follows_the_payload() {
        let wire = InterchangeBuilder::new("9900123456789", "9900987654321", "R1")
            .message(b"UNH+1+X'UNT+2+1'".to_vec())
            .message(b"UNH+2+X'UNT+2+2'".to_vec())
            .build()
            .unwrap();
        let text = String::from_utf8(wire).unwrap();
        assert!(text.ends_with("UNZ+2+R1'"), "{text}");
    }

    #[test]
    fn empty_interchange_declares_zero_messages() {
        let wire = InterchangeBuilder::new("9900123456789", "9900987654321", "R1")
            .build()
            .unwrap();
        let text = String::from_utf8(wire).unwrap();
        assert!(text.ends_with("UNZ+0+R1'"), "{text}");
    }
}
