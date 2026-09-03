//! Writing DVGW messages.
//!
//! A BKV that only parses cannot nominate, and a Netzbetreiber that only
//! parses cannot report its Mehr-/Mindermengen. [`MessageBuilder`] renders the
//! header and `LIN` loops the Nachrichtenbeschreibungen prescribe, so the same
//! crate that reads a NOMRES can produce the NOMINT it answers and the SSQNOT
//! the MGV expects.
//!
//! Every coded value is stamped with the agency the Segmentlayout names —
//! `332` (DVGW) by default, `9` (GS1) where a party is a GLN — and a profile
//! is written the way the DVGW column caps it: one `DTM+2` and one `QTY` per
//! `LOC` group, the `LOC` repeated per period.
//!
//! ```rust
//! use dvgw_edi::{DvgwDocument, DvgwPeriod, MessageBuilder, Position};
//! use time::macros::datetime;
//!
//! let gas_day = DvgwPeriod {
//!     start: datetime!(2026-03-01 05:00 UTC),
//!     end:   datetime!(2026-03-02 05:00 UTC),
//! };
//!
//! let wire = MessageBuilder::new(DvgwDocument::NominierungTransportkunde)
//!     .message_ref("1")
//!     .document_number("NOMINT00052")
//!     .version("DVGW17")
//!     .pruefidentifikator(70030)
//!     .message_datetime(datetime!(2026-02-28 20:56 UTC))
//!     .validity_period(gas_day)
//!     .sender("9870009700005")
//!     .receiver("9870009700006")
//!     .position(
//!         Position::new()
//!             .location("Z19", Some("ABCD1234"))
//!             .quantity("Z03", "6782", gas_day)
//!             .party("ZEU", "BK-CODE-1")
//!             .party("ZES", "BK-CODE-2"),
//!     )
//!     .build()?;
//!
//! assert!(String::from_utf8_lossy(&wire).contains("BGM+01G::332+NOMINT00052'"));
//! # Ok::<(), dvgw_edi::Error>(())
//! ```

use time::{OffsetDateTime, UtcOffset};

use crate::{
    datetime::{DvgwPeriod, format_instant, format_period},
    document::{DVGW_AGENCY_CODE, DvgwDocument},
    error::Error,
    model::{nad, rff},
};

/// One `LIN` position under construction.
#[derive(Debug, Clone, Default)]
pub struct Position {
    number: Option<String>,
    item_type: Option<String>,
    description: Option<String>,
    locations: Vec<LocationDraft>,
    parties: Vec<PartyDraft>,
}

#[derive(Debug, Clone)]
struct PartyDraft {
    role: String,
    code: String,
    agency: String,
}

#[derive(Debug, Clone)]
struct LocationDraft {
    qualifier: String,
    code: Option<String>,
    quantities: Vec<QuantityDraft>,
}

#[derive(Debug, Clone)]
struct QuantityDraft {
    qualifier: String,
    value: String,
    unit: Option<String>,
    period: DvgwPeriod,
    status: Vec<String>,
}

impl Position {
    /// An empty position.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set `LIN` DE 1082 explicitly. Positions are numbered from 1 otherwise.
    #[must_use]
    pub fn number(mut self, number: impl Into<String>) -> Self {
        self.number = Some(number.into());
        self
    }

    /// Set `LIN` C212 DE 7143 — the Zeitreihentyp in ALOCAT.
    #[must_use]
    pub fn item_type(mut self, code: impl Into<String>) -> Self {
        self.item_type = Some(code.into());
        self
    }

    /// Set the `IMD` DE 7009 code — NOMRES labels which side a position reports.
    #[must_use]
    pub fn description(mut self, code: impl Into<String>) -> Self {
        self.description = Some(code.into());
        self
    }

    /// Open a `LOC` group. Pass `None` as the code for `LOC+Z99`, which ALOCAT
    /// sends when the message needs no specific place.
    #[must_use]
    pub fn location(mut self, qualifier: impl Into<String>, code: Option<&str>) -> Self {
        self.locations.push(LocationDraft {
            qualifier: qualifier.into(),
            code: code.map(str::to_owned),
            quantities: Vec::new(),
        });
        self
    }

    /// Add a quantity to the open `LOC` group, with the period it applies to,
    /// in the family's default unit (`KW1` kWh/h; `KWH` for SSQNOT).
    ///
    /// Repeat to transmit a profile: the DVGW column admits one `DTM+2` and
    /// one `QTY` per `LOC` group, so every quantity after the first is
    /// written under a repeated `LOC`.
    ///
    /// # Panics
    ///
    /// Panics when no [`location`](Self::location) has been opened yet — a
    /// quantity outside a `LOC` group has nowhere to go on the wire.
    #[must_use]
    pub fn quantity(
        self,
        qualifier: impl Into<String>,
        value: impl Into<String>,
        period: DvgwPeriod,
    ) -> Self {
        self.push_quantity(qualifier.into(), value.into(), None, period)
    }

    /// [`quantity`](Self::quantity) in an explicit C186 DE 6411 unit —
    /// `KW2` (kWh/d) on an ALOCAT, `KWH` on a nomination.
    ///
    /// # Panics
    ///
    /// As [`quantity`](Self::quantity).
    #[must_use]
    pub fn quantity_in(
        self,
        qualifier: impl Into<String>,
        value: impl Into<String>,
        unit: impl Into<String>,
        period: DvgwPeriod,
    ) -> Self {
        self.push_quantity(qualifier.into(), value.into(), Some(unit.into()), period)
    }

    fn push_quantity(
        mut self,
        qualifier: String,
        value: String,
        unit: Option<String>,
        period: DvgwPeriod,
    ) -> Self {
        let location = self
            .locations
            .last_mut()
            .expect("call Position::location before Position::quantity");
        location.quantities.push(QuantityDraft {
            qualifier,
            value,
            unit,
            period,
            status: Vec::new(),
        });
        self
    }

    /// Attach an `STS` DE 9015 code to the last quantity — the Zeitreihentyp
    /// of an ALOCAT (`09G`, `14G`, …), the Verfahren of a SSQNOT (`A1G`/`A2G`).
    ///
    /// # Panics
    ///
    /// Panics when no quantity has been added yet.
    #[must_use]
    pub fn status(mut self, code: impl Into<String>) -> Self {
        self.locations
            .last_mut()
            .and_then(|l| l.quantities.last_mut())
            .expect("call Position::quantity before Position::status")
            .status
            .push(code.into());
        self
    }

    /// Add a position-level `NAD` — Bilanzkreis, Netzkonto, VHP, Netzbetreiber —
    /// coded under the DVGW agency (`332`).
    #[must_use]
    pub fn party(self, role: impl Into<String>, code: impl Into<String>) -> Self {
        self.party_coded(role, code, DVGW_AGENCY_CODE)
    }

    /// [`party`](Self::party) under an explicit DE 3055 agency — `9` for a GLN,
    /// which ALOCAT admits on the `ZSO`/`VHP` row.
    #[must_use]
    pub fn party_coded(
        mut self,
        role: impl Into<String>,
        code: impl Into<String>,
        agency: impl Into<String>,
    ) -> Self {
        self.parties.push(PartyDraft {
            role: role.into(),
            code: code.into(),
            agency: agency.into(),
        });
        self
    }
}

/// Escape the EDIFACT service characters in a value.
///
/// A value containing `\'` would otherwise close the segment early and have
/// everything after it read as further segments — outbound messages are
/// assembled from counterparty-supplied identifiers.
fn esc(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if matches!(c, '+' | ':' | '\'' | '?') {
            out.push('?');
        }
        out.push(c);
    }
    out
}

/// Builds a complete DVGW message.
///
/// The document code decides the family, the carrier and the `UNH` header, so
/// there is one builder rather than one per message type.
#[derive(Debug, Clone)]
pub struct MessageBuilder {
    document: DvgwDocument,
    message_ref: String,
    document_number: String,
    version: Option<String>,
    timezone: UtcOffset,
    pruefidentifikator: Option<u32>,
    message_datetime: Option<OffsetDateTime>,
    validity_period: Option<DvgwPeriod>,
    clearingnummer: Option<String>,
    original_nomination: Option<(String, OffsetDateTime)>,
    references: Vec<(String, String)>,
    parties: Vec<PartyDraft>,
    positions: Vec<Position>,
}

impl MessageBuilder {
    /// Start a message of the given document type.
    #[must_use]
    pub fn new(document: DvgwDocument) -> Self {
        Self {
            document,
            message_ref: "1".to_owned(),
            document_number: String::new(),
            version: None,
            timezone: UtcOffset::UTC,
            pruefidentifikator: None,
            message_datetime: None,
            validity_period: None,
            clearingnummer: None,
            original_nomination: None,
            references: Vec::new(),
            parties: Vec::new(),
            positions: Vec::new(),
        }
    }

    /// `UNH` DE 0062 message reference (mirrored in `UNT`).
    #[must_use]
    pub fn message_ref(mut self, value: impl Into<String>) -> Self {
        self.message_ref = value.into();
        self
    }

    /// `BGM` C106 DE 1004 Dokumentennummer.
    #[must_use]
    pub fn document_number(mut self, value: impl Into<String>) -> Self {
        self.document_number = value.into();
        self
    }

    /// `UNH` S009 DE 0057 — the package code (`DVGW17`) or version (`5.11a`).
    /// Defaults to what the family's Nachrichtenbeschreibung prescribes
    /// ([`DvgwMessageType::anwendungscode`](crate::DvgwMessageType::anwendungscode)).
    #[must_use]
    pub fn version(mut self, value: impl Into<String>) -> Self {
        self.version = Some(value.into());
        self
    }

    /// `SG1 RFF+Z13` Prüfidentifikator.
    #[must_use]
    pub fn pruefidentifikator(mut self, pid: u32) -> Self {
        self.pruefidentifikator = Some(pid);
        self
    }

    /// `DTM+137` Datum und Zeit der Nachricht.
    #[must_use]
    pub fn message_datetime(mut self, value: OffsetDateTime) -> Self {
        self.message_datetime = Some(value);
        self
    }

    /// `DTM+Z01` Gültigkeitszeitraum — the gas day for ALOCAT, NOMINT and
    /// NOMRES, the Abrechnungszeitraum for SSQNOT.
    #[must_use]
    pub fn validity_period(mut self, period: DvgwPeriod) -> Self {
        self.validity_period = Some(period);
        self
    }

    /// `NAD+MS` Absender, coded under the DVGW agency (`332`).
    #[must_use]
    pub fn sender(self, code: impl Into<String>) -> Self {
        self.party(nad::ABSENDER, code)
    }

    /// `NAD+MS` Absender under an explicit DE 3055 agency (`9` for a GLN).
    #[must_use]
    pub fn sender_coded(self, code: impl Into<String>, agency: impl Into<String>) -> Self {
        self.party_coded(nad::ABSENDER, code, agency)
    }

    /// `NAD+MR` Empfänger, coded under the DVGW agency (`332`).
    #[must_use]
    pub fn receiver(self, code: impl Into<String>) -> Self {
        self.party(nad::EMPFAENGER, code)
    }

    /// `NAD+MR` Empfänger under an explicit DE 3055 agency (`9` for a GLN).
    #[must_use]
    pub fn receiver_coded(self, code: impl Into<String>, agency: impl Into<String>) -> Self {
        self.party_coded(nad::EMPFAENGER, code, agency)
    }

    /// Any further header `NAD`, e.g. `ZSY` (zusätzlicher BKV), under `332`.
    #[must_use]
    pub fn party(self, role: impl Into<String>, code: impl Into<String>) -> Self {
        self.party_coded(role, code, DVGW_AGENCY_CODE)
    }

    /// [`party`](Self::party) under an explicit DE 3055 agency.
    #[must_use]
    pub fn party_coded(
        mut self,
        role: impl Into<String>,
        code: impl Into<String>,
        agency: impl Into<String>,
    ) -> Self {
        self.parties.push(PartyDraft {
            role: role.into(),
            code: code.into(),
            agency: agency.into(),
        });
        self
    }

    /// `RFF+ANX` Clearingnummer (ALOCAT), written first in `SG1` as the
    /// Nachrichtenstruktur orders it.
    #[must_use]
    pub fn clearingnummer(mut self, value: impl Into<String>) -> Self {
        self.clearingnummer = Some(value.into());
        self
    }

    /// `RFF+AGO` — the nomination this one corrects (NOMINT) — with the
    /// `DTM+9` Bearbeitungsdatum NOMINT 4.6 marks Erforderlich beside it.
    #[must_use]
    pub fn original_nomination(
        mut self,
        value: impl Into<String>,
        processed_at: OffsetDateTime,
    ) -> Self {
        self.original_nomination = Some((value.into(), processed_at));
        self
    }

    /// Any further header `RFF`, written after the ones the structure orders.
    #[must_use]
    pub fn reference(mut self, qualifier: impl Into<String>, value: impl Into<String>) -> Self {
        self.references.push((qualifier.into(), value.into()));
        self
    }

    /// Append a position.
    #[must_use]
    pub fn position(mut self, position: Position) -> Self {
        self.positions.push(position);
        self
    }

    /// Render the message as `UNH`…`UNT` EDIFACT bytes.
    ///
    /// The interchange envelope is deliberately not written: the AS4 layer owns
    /// `UNB`/`UNZ` and its control reference, and a second writer would have to
    /// guess at both.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Serialize`] when a mandatory field was never set — the
    /// Dokumentennummer, the Prüfidentifikator, the two timestamps, both parties,
    /// or at least one position.
    pub fn build(&self) -> Result<Vec<u8>, Error> {
        let missing = |what: &str| Error::Serialize(format!("{what} is required but was not set"));

        if self.document_number.is_empty() {
            return Err(missing("BGM C106 DE 1004 Dokumentennummer"));
        }
        let pid = self
            .pruefidentifikator
            .ok_or_else(|| missing("SG1 RFF+Z13 Prüfidentifikator"))?;
        let message_datetime = self
            .message_datetime
            .ok_or_else(|| missing("DTM+137 Datum und Zeit der Nachricht"))?;
        let validity = self
            .validity_period
            .ok_or_else(|| missing("DTM+Z01 Gültigkeitszeitraum"))?;
        for role in [nad::ABSENDER, nad::EMPFAENGER] {
            if !self.parties.iter().any(|p| p.role == role) {
                return Err(missing(&format!("NAD+{role}")));
            }
        }
        if self.positions.is_empty() {
            return Err(missing("at least one LIN position"));
        }

        let agency = DVGW_AGENCY_CODE;
        let mut segments: Vec<String> = Vec::new();

        let family = self.document.message_type();
        let version = self
            .version
            .as_deref()
            .unwrap_or_else(|| family.anwendungscode());
        segments.push(format!(
            "UNH+{}+{}:D:07A:UN:{}",
            esc(&self.message_ref),
            self.document.carrier().as_str(),
            esc(version)
        ));
        segments.push(format!(
            "BGM+{}::{agency}+{}",
            self.document.code(),
            esc(&self.document_number)
        ));
        // The zone must precede the timestamps it governs.
        segments.push(format!("DTM+Z05:{}:805", self.timezone.whole_hours()));
        segments.push(format!(
            "DTM+137:{}:203",
            format_instant(message_datetime, self.timezone)
        ));
        segments.push(format!(
            "DTM+Z01:{}:719",
            format_period(validity, self.timezone)
        ));
        // `SG1` in the order the Nachrichtenstrukturen list it: ALOCAT puts
        // the Clearingnummer before the Prüfidentifikator, NOMINT the
        // Original-Nominierung (with its `DTM+9`) after it.
        if let Some(clearing) = &self.clearingnummer {
            segments.push(format!("RFF+{}:{}", rff::CLEARINGNUMMER, esc(clearing)));
        }
        segments.push(format!("RFF+{}:{pid}", rff::PRUEFIDENTIFIKATOR));
        if let Some((original, processed_at)) = &self.original_nomination {
            segments.push(format!(
                "RFF+{}:{}",
                rff::ORIGINAL_NOMINIERUNG,
                esc(original)
            ));
            segments.push(format!(
                "DTM+9:{}:203",
                format_instant(*processed_at, self.timezone)
            ));
        }
        for (qualifier, value) in &self.references {
            segments.push(format!("RFF+{}:{}", esc(qualifier), esc(value)));
        }
        for party in &self.parties {
            segments.push(format!(
                "NAD+{}+{}::{}",
                esc(&party.role),
                esc(&party.code),
                esc(&party.agency)
            ));
        }

        let default_unit = family.admitted_units()[0];
        for (index, position) in self.positions.iter().enumerate() {
            self.render_position(position, index, agency, default_unit, &mut segments);
        }

        segments.push("UNS+S".to_owned());
        // UNT DE 0074 counts UNH…UNT inclusive: everything rendered so far plus
        // the UNT itself.
        segments.push(format!(
            "UNT+{}+{}",
            segments.len() + 1,
            esc(&self.message_ref)
        ));

        let mut out = String::new();
        for segment in segments {
            out.push_str(&segment);
            out.push('\'');
        }
        Ok(out.into_bytes())
    }

    /// Render one `LIN` loop.
    fn render_position(
        &self,
        position: &Position,
        index: usize,
        agency: &str,
        default_unit: &str,
        segments: &mut Vec<String>,
    ) {
        {
            let number = position
                .number
                .clone()
                .unwrap_or_else(|| (index + 1).to_string());
            let number = esc(&number);
            match &position.item_type {
                Some(item_type) => {
                    segments.push(format!("LIN+{number}++:{}::{agency}", esc(item_type)));
                }
                None => segments.push(format!("LIN+{number}")),
            }
            if let Some(code) = &position.description {
                segments.push(format!("IMD++05G+{}::{agency}", esc(code)));
            }
            for location in &position.locations {
                let loc = match &location.code {
                    Some(code) => {
                        format!("LOC+{}+{}::{agency}", esc(&location.qualifier), esc(code))
                    }
                    None => format!("LOC+{}", esc(&location.qualifier)),
                };
                // One `DTM+2` and one `QTY` per `LOC` group (the DVGW MaxWdh),
                // so a profile repeats the `LOC`.
                for quantity in &location.quantities {
                    segments.push(loc.clone());
                    segments.push(format!(
                        "DTM+2:{}:719",
                        format_period(quantity.period, self.timezone)
                    ));
                    segments.push(format!(
                        "QTY+{}:{}:{}",
                        esc(&quantity.qualifier),
                        esc(&quantity.value),
                        esc(quantity.unit.as_deref().unwrap_or(default_unit))
                    ));
                    for status in &quantity.status {
                        segments.push(format!("STS+{}::{agency}", esc(status)));
                    }
                }
                if location.quantities.is_empty() {
                    segments.push(loc);
                }
            }
            for party in &position.parties {
                segments.push(format!(
                    "NAD+{}+{}::{}",
                    esc(&party.role),
                    esc(&party.code),
                    esc(&party.agency)
                ));
            }
        }
    }
}
