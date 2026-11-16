//! The `eb:PartProperties` of an Übertragungsdatei, read from its EDIFACT
//! envelope.
//!
//! Regelungen zum Übertragungsweg für AS4 2.6, Kap. 6 (S. 7) fills four
//! properties for the Marktprozesse and names three that are not used:
//!
//! | Property | Source |
//! |---|---|
//! | `BDEWDocumentType` | „EDIFACT-Name des Nachrichten-Typs gem. UNH DE0065" |
//! | `BDEWApplicationReference` | „Wenn vorhanden, Anwendungsreferenz aus UNB DE0026" |
//! | `BDEWDocumentDate` | „Datumstempel bei Erzeugung im Format YYYY-MM-DD" |
//! | `BDEWDocumentNo` | „Datenaustauschreferenz aus UNB DE0020" |
//!
//! `BDEWFulfillmentDate`, `BDEWSubjectPartyID` and `BDEWSubjectPartyRole` are
//! never set. The date is the interchange's own creation date, `UNB` S004
//! DE0017, so the property and the envelope cannot disagree.
//!
//! BDEW AS4-Profil 1.2 Kap. 2.3.5 (S. 18) gives the receiver no duty: „Eine
//! empfangende Partei KANN die durch diese Eigenschaften gegebenen
//! Informationen nutzen". An inbound message's properties are therefore
//! recorded, never a reason to refuse it.
//!
//! ```rust
//! use mako_as4::part_properties::PartProperties;
//!
//! let unb = b"UNA:+.? 'UNB+UNOC:3+9900000000003:500+9900000000010:500+261001:1200+DAR1++VL'\
//!             UNH+1+MSCONS:D:04B:UN:2.4c'UNT+2+1'UNZ+1+DAR1'";
//! let props = PartProperties::from_interchange(unb).unwrap();
//! assert_eq!(
//!     props.entries(),
//!     vec![
//!         ("BDEWDocumentType", "MSCONS"),
//!         ("BDEWApplicationReference", "VL"),
//!         ("BDEWDocumentDate", "2026-10-01"),
//!         ("BDEWDocumentNo", "DAR1"),
//!     ]
//! );
//! ```

/// `BDEWDocumentType` — UNH DE0065.
pub const DOCUMENT_TYPE: &str = "BDEWDocumentType";
/// `BDEWApplicationReference` — UNB DE0026, when the interchange has one.
pub const APPLICATION_REFERENCE: &str = "BDEWApplicationReference";
/// `BDEWDocumentDate` — the creation date, `YYYY-MM-DD`.
pub const DOCUMENT_DATE: &str = "BDEWDocumentDate";
/// `BDEWDocumentNo` — UNB DE0020.
pub const DOCUMENT_NO: &str = "BDEWDocumentNo";

/// The part properties of one Übertragungsdatei.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartProperties {
    /// UNH DE0065, e.g. `UTILMD`.
    pub document_type: String,
    /// UNB DE0026, absent when the interchange carries none.
    pub application_reference: Option<String>,
    /// UNB S004 DE0017 as `YYYY-MM-DD`.
    pub document_date: String,
    /// UNB DE0020, the Datenaustauschreferenz.
    pub document_no: String,
}

/// An interchange whose envelope does not yield the properties.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PartPropertiesError {
    /// No `UNB` segment.
    #[error("the Übertragungsdatei has no UNB segment")]
    NoUnb,
    /// No `UNH` segment.
    #[error("the Übertragungsdatei has no UNH segment")]
    NoUnh,
    /// A required data element is empty.
    #[error("the Übertragungsdatei has no {0}")]
    Missing(&'static str),
    /// Messages of different types in one interchange: no one
    /// `BDEWDocumentType` names them.
    #[error(
        "the Übertragungsdatei carries messages of the types {0:?}; BDEWDocumentType names one"
    )]
    MixedTypes(Vec<String>),
    /// UNB DE0017 is no date.
    #[error("UNB DE0017 {0:?} is no date")]
    BadDate(String),
}

impl PartProperties {
    /// The properties of the interchange `bytes`.
    ///
    /// # Errors
    ///
    /// No `UNB` or `UNH`, an empty DE0020 or DE0065, messages of several types,
    /// a DE0017 that is no date.
    pub fn from_interchange(bytes: &[u8]) -> Result<Self, PartPropertiesError> {
        let segments = Syntax::segments(bytes);
        let unb = segments
            .iter()
            .find(|s| s.tag() == "UNB")
            .ok_or(PartPropertiesError::NoUnb)?;
        let mut types: Vec<String> = Vec::new();
        for unh in segments.iter().filter(|s| s.tag() == "UNH") {
            let t = unh.component(2, 0);
            if t.is_empty() {
                return Err(PartPropertiesError::Missing("UNH DE0065"));
            }
            if !types.contains(&t) {
                types.push(t);
            }
        }
        let document_type = match types.len() {
            0 => return Err(PartPropertiesError::NoUnh),
            1 => types.remove(0),
            _ => return Err(PartPropertiesError::MixedTypes(types)),
        };
        let document_no = unb.component(5, 0);
        if document_no.is_empty() {
            return Err(PartPropertiesError::Missing("UNB DE0020"));
        }
        let application_reference = Some(unb.component(7, 0)).filter(|r| !r.is_empty());
        let document_date = iso_date(&unb.component(4, 0))?;
        Ok(Self {
            document_type,
            application_reference,
            document_date,
            document_no,
        })
    }

    /// The `(name, value)` pairs in the order of Kap. 6, without an absent
    /// application reference.
    #[must_use]
    pub fn entries(&self) -> Vec<(&'static str, &str)> {
        let mut out = vec![(DOCUMENT_TYPE, self.document_type.as_str())];
        if let Some(r) = &self.application_reference {
            out.push((APPLICATION_REFERENCE, r.as_str()));
        }
        out.push((DOCUMENT_DATE, self.document_date.as_str()));
        out.push((DOCUMENT_NO, self.document_no.as_str()));
        out
    }

    /// `payload` with these properties.
    #[must_use]
    pub fn apply(&self, mut payload: asx::Payload) -> asx::Payload {
        for (name, value) in self.entries() {
            payload = payload.property(name, value);
        }
        payload
    }
}

/// UNB DE0017 — `YYMMDD` (syntax version 3) or `CCYYMMDD` (version 4) — as
/// `YYYY-MM-DD`.
fn iso_date(de0017: &str) -> Result<String, PartPropertiesError> {
    let bad = || PartPropertiesError::BadDate(de0017.to_owned());
    if !de0017.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad());
    }
    let (year, rest) = match de0017.len() {
        6 => (format!("20{}", &de0017[..2]), &de0017[2..]),
        8 => (de0017[..4].to_owned(), &de0017[4..]),
        _ => return Err(bad()),
    };
    let (month, day) = (&rest[..2], &rest[2..]);
    let m: u8 = month.parse().map_err(|_| bad())?;
    let d: u8 = day.parse().map_err(|_| bad())?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return Err(bad());
    }
    Ok(format!("{year}-{month}-{day}"))
}

/// The service characters of an interchange (ISO 9735, `UNA`).
struct Syntax {
    component: u8,
    element: u8,
    release: u8,
    terminator: u8,
}

/// One segment: its elements, each a list of components, unescaped.
struct Segment(Vec<Vec<String>>);

impl Segment {
    fn tag(&self) -> &str {
        self.0
            .first()
            .and_then(|e| e.first())
            .map_or("", String::as_str)
    }

    /// Component `c` of element `e` (the tag is element 0), empty when absent.
    fn component(&self, e: usize, c: usize) -> String {
        self.0
            .get(e)
            .and_then(|el| el.get(c))
            .cloned()
            .unwrap_or_default()
    }
}

impl Syntax {
    fn segments(bytes: &[u8]) -> Vec<Segment> {
        let (syntax, body) = match bytes {
            [b'U', b'N', b'A', c, e, _, r, _, t, rest @ ..] => (
                Syntax {
                    component: *c,
                    element: *e,
                    release: *r,
                    terminator: *t,
                },
                rest,
            ),
            _ => (
                Syntax {
                    component: b':',
                    element: b'+',
                    release: b'?',
                    terminator: b'\'',
                },
                bytes,
            ),
        };
        let mut out = Vec::new();
        let mut elements: Vec<Vec<String>> = Vec::new();
        let mut components: Vec<String> = Vec::new();
        let mut current: Vec<u8> = Vec::new();
        let mut iter = body.iter().copied();
        while let Some(b) = iter.next() {
            if b == syntax.release {
                if let Some(next) = iter.next() {
                    current.push(next);
                }
            } else if b == syntax.component {
                components.push(String::from_utf8_lossy(&current).into_owned());
                current.clear();
            } else if b == syntax.element {
                components.push(String::from_utf8_lossy(&current).into_owned());
                current.clear();
                elements.push(std::mem::take(&mut components));
            } else if b == syntax.terminator {
                components.push(String::from_utf8_lossy(&current).into_owned());
                current.clear();
                elements.push(std::mem::take(&mut components));
                out.push(Segment(std::mem::take(&mut elements)));
            } else if b.is_ascii_whitespace()
                && current.is_empty()
                && components.is_empty()
                && elements.is_empty()
            {
                // A line break between segments belongs to no element.
            } else {
                current.push(b);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(bytes: &str) -> Result<PartProperties, PartPropertiesError> {
        PartProperties::from_interchange(bytes.as_bytes())
    }

    /// Kap. 6: DE0065, DE0020 and the creation date; no application reference
    /// when DE0026 is empty, and nothing of the three unused properties.
    #[test]
    fn the_four_properties_come_from_the_envelope() {
        let p = props(
            "UNA:+.? '\r\nUNB+UNOC:3+9900000000003:500+9900000000010:500+261001:1200+ABC?+1'\r\n\
             UNH+1+UTILMD:D:11A:UN:S2.2'BGM+E01+X'UNT+3+1'UNZ+1+ABC?+1'",
        )
        .unwrap();
        assert_eq!(
            p.entries(),
            vec![
                ("BDEWDocumentType", "UTILMD"),
                ("BDEWDocumentDate", "2026-10-01"),
                ("BDEWDocumentNo", "ABC+1"),
            ]
        );
    }

    #[test]
    fn the_application_reference_is_de0026() {
        let p = props(
            "UNB+UNOC:3+9900000000003:500+9900000000010:500+261231:2300+DAR7++TL'\
             UNH+1+MSCONS:D:04B:UN:2.4c'UNT+2+1'UNZ+1+DAR7'",
        )
        .unwrap();
        assert_eq!(p.application_reference.as_deref(), Some("TL"));
        assert_eq!(p.document_date, "2026-12-31");
    }

    /// Syntax version 4 dates carry the century.
    #[test]
    fn a_ccyymmdd_date_keeps_its_century() {
        let p =
            props("UNB+UNOC:4+A:500+B:500+20270401:0500+D1'UNH+1+APERAK:D:07B:UN:2.1i'").unwrap();
        assert_eq!(p.document_date, "2027-04-01");
    }

    #[test]
    fn several_messages_of_one_type_name_that_type() {
        let p = props(
            "UNB+UNOC:3+A:500+B:500+261001:1200+D1'UNH+1+UTILMD:D:11A:UN:S2.2'UNT+2+1'\
             UNH+2+UTILMD:D:11A:UN:S2.2'UNT+2+2'UNZ+2+D1'",
        )
        .unwrap();
        assert_eq!(p.document_type, "UTILMD");
    }

    #[test]
    fn an_envelope_without_the_sources_is_refused() {
        assert_eq!(
            props("UNH+1+UTILMD:D:11A:UN:S2.2'"),
            Err(PartPropertiesError::NoUnb)
        );
        assert_eq!(
            props("UNB+UNOC:3+A:500+B:500+261001:1200+D1'"),
            Err(PartPropertiesError::NoUnh)
        );
        assert_eq!(
            props("UNB+UNOC:3+A:500+B:500+261001:1200+'UNH+1+UTILMD'"),
            Err(PartPropertiesError::Missing("UNB DE0020"))
        );
        assert!(matches!(
            props("UNB+UNOC:3+A:500+B:500+261001:1200+D1'UNH+1+UTILMD'UNH+2+APERAK'"),
            Err(PartPropertiesError::MixedTypes(_))
        ));
        assert!(matches!(
            props("UNB+UNOC:3+A:500+B:500+261301:1200+D1'UNH+1+UTILMD'"),
            Err(PartPropertiesError::BadDate(_))
        ));
    }

    /// The properties reach the payload `asx-rs` sends.
    #[test]
    fn the_properties_are_set_on_the_payload() {
        let p = props("UNB+UNOC:3+A:500+B:500+261001:1200+D1'UNH+1+UTILMD'").unwrap();
        let payload = p.apply(asx::Payload::new(
            b"x".to_vec(),
            crate::constants::PAYLOAD_MIME_TYPE,
        ));
        let got: Vec<(&str, &str)> = payload
            .properties()
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        assert_eq!(got, p.entries());
    }
}
