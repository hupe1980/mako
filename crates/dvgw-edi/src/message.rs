//! [`DvgwMessage`] — a parsed DVGW message and the walk that builds it.

use edifact_rs::OwnedSegment;
use time::{OffsetDateTime, UtcOffset};

use crate::{
    datetime::{self, DtmFormat, DtmValue, DvgwPeriod},
    document::{Carrier, DvgwDocument, DvgwMessageType},
    error::{Error, sanitize_code},
    model::{ItemDescription, LineItem, LocationGroup, Party, Quantity, Reference, nad, rff},
    pruefidentifikator::Pruefidentifikator,
    version::DvgwVersion,
};

/// A parsed DVGW message.
///
/// One type serves ALOCAT, NOMINT and NOMRES: the three share a structure and
/// differ only in which qualifiers are legal, which is a validation concern.
/// Match on [`message_type`](Self::message_type) or
/// [`document`](Self::document) when the family matters.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DvgwMessage {
    /// The logical family, derived from the document code.
    pub message_type: DvgwMessageType,
    /// `BGM` C002 DE 1001 — what this message *is*.
    pub document: DvgwDocument,
    /// `UNH` DE 0065 — the UN/EDIFACT carrier the document rode in on.
    pub carrier: Carrier,
    /// `UNH` S009 DE 0057 — the DVGW package code or message version.
    pub version: Option<DvgwVersion>,
    /// `UNH` DE 0062 message reference.
    pub message_ref: String,
    /// `BGM` C106 DE 1004 Dokumentennummer.
    pub document_number: Option<String>,
    /// `SG1 RFF+Z13` Prüfidentifikator.
    pub pruefidentifikator: Option<Pruefidentifikator>,
    /// The zone `DTM+Z05` declares, as a whole-hour offset. Defaults to UTC,
    /// which is what `DTM+Z05:0:805` says and what every shipped package uses.
    pub timezone: UtcOffset,
    /// `DTM+137` — Datum und Zeit der Nachricht.
    pub message_datetime: Option<OffsetDateTime>,
    /// `DTM+Z01` — Gültigkeitszeitraum der Nachricht.
    ///
    /// For ALOCAT and NOMINT this is the gas day the message reports on.
    pub validity_period: Option<DvgwPeriod>,
    /// Header `RFF` segments in wire order.
    pub references: Vec<Reference>,
    /// Header parties in wire order (`NAD+MS`, `NAD+MR`, `NAD+ZSY`).
    pub parties: Vec<Party>,
    /// The `LIN` positions.
    pub items: Vec<LineItem>,
    /// Header `DTM` segments that were present but could not be decoded against
    /// their own format code. Carried so validation can report them precisely.
    pub(crate) undecodable_dtm: Vec<String>,
    /// The raw segments, authoritative for serialization.
    segments: Vec<OwnedSegment>,
}

impl DvgwMessage {
    /// The sender from `NAD+MS`.
    #[must_use]
    pub fn sender(&self) -> Option<&Party> {
        self.party(nad::ABSENDER)
    }

    /// The receiver from `NAD+MR`.
    #[must_use]
    pub fn receiver(&self) -> Option<&Party> {
        self.party(nad::EMPFAENGER)
    }

    /// The first header party with the given `NAD` role.
    #[must_use]
    pub fn party(&self, role: &str) -> Option<&Party> {
        self.parties.iter().find(|p| p.role == role)
    }

    /// The first header reference with the given `RFF` qualifier.
    #[must_use]
    pub fn reference(&self, qualifier: &str) -> Option<&str> {
        self.references
            .iter()
            .find(|r| r.qualifier == qualifier)
            .map(|r| r.value.as_str())
    }

    /// `RFF+ANX` — the ALOCAT Clearingnummer.
    #[must_use]
    pub fn clearingnummer(&self) -> Option<&str> {
        self.reference(rff::CLEARINGNUMMER)
    }

    /// `RFF+AGO` — the NOMINT reference to the nomination this one corrects.
    ///
    /// This is the correlation key for a re-nomination chain. `RFF+Z13` is the
    /// Prüfidentifikator and correlates nothing.
    #[must_use]
    pub fn original_nomination_ref(&self) -> Option<&str> {
        self.reference(rff::ORIGINAL_NOMINIERUNG)
    }

    /// Every quantity in the message, flattened across positions and locations.
    pub fn quantities(&self) -> impl Iterator<Item = &Quantity> {
        self.items.iter().flat_map(LineItem::quantities)
    }

    /// Energy totals in kWh, one per `QTY` qualifier.
    ///
    /// Each quantity is integrated over its own period
    /// ([`Quantity::energy_kwh`]) and summed within its qualifier — the
    /// qualifier is the direction (`Z02` in, `Z03` out), so a figure across them
    /// would be a net position.
    ///
    /// A quantity that cannot be converted is **omitted**;
    /// [`energy_is_complete`](Self::energy_is_complete) reports whether any were.
    #[must_use]
    pub fn energy_by_qualifier(&self) -> crate::model::EnergyByQualifier {
        self.energy_by_qualifier_where(|_| true)
    }

    /// [`energy_by_qualifier`](Self::energy_by_qualifier) over the positions
    /// `keep` selects.
    ///
    /// For NOMRES, which reports **both** sides of a match: `IMD` `17G` labels
    /// the quantities the recipient nominated, `18G` the counterparty's, `16G`
    /// the matched result.
    #[must_use]
    pub fn energy_by_qualifier_where(
        &self,
        keep: impl Fn(&LineItem) -> bool,
    ) -> crate::model::EnergyByQualifier {
        let mut totals = crate::model::EnergyByQualifier::new();
        for quantity in self
            .items
            .iter()
            .filter(|item| keep(item))
            .flat_map(LineItem::quantities)
        {
            if let Some(kwh) = quantity.energy_kwh() {
                *totals.entry(quantity.qualifier.clone()).or_default() += kwh;
            }
        }
        totals
    }

    /// The single energy total this message states, in kWh, or `None`.
    ///
    /// `None` when the selected positions carry more than one `QTY` qualifier —
    /// `Z02` in and `Z03` out make a net position, not a total — and `None` when
    /// nothing could be integrated or some quantity was dropped.
    /// [`energy_by_qualifier_where`](Self::energy_by_qualifier_where) gives the
    /// per-direction figures.
    #[must_use]
    pub fn single_energy_kwh(
        &self,
        keep: impl Fn(&LineItem) -> bool + Copy,
    ) -> Option<rust_decimal::Decimal> {
        let selected: Vec<&LineItem> = self.items.iter().filter(|i| keep(i)).collect();
        if selected.is_empty() {
            return None;
        }
        // Every selected quantity must have contributed, or the total is a floor.
        let complete = selected
            .iter()
            .flat_map(|i| i.quantities())
            .all(|q| q.energy_kwh().is_some());
        if !complete {
            return None;
        }
        let totals = self.energy_by_qualifier_where(keep);
        match totals.len() {
            1 => totals.into_values().next(),
            _ => None,
        }
    }

    /// `true` when every quantity in the message contributed to
    /// [`energy_by_qualifier`](Self::energy_by_qualifier).
    ///
    /// `false` means at least one was dropped, so the totals are a floor rather
    /// than a figure — check this before booking one.
    #[must_use]
    pub fn energy_is_complete(&self) -> bool {
        let mut any = false;
        for quantity in self.quantities() {
            any = true;
            if quantity.energy_kwh().is_none() {
                return false;
            }
        }
        any
    }

    /// The raw segments (`UNH` … `UNT`, plus any envelope that was parsed with them).
    #[must_use]
    pub fn segments(&self) -> &[OwnedSegment] {
        &self.segments
    }

    /// Render the message back to EDIFACT bytes.
    ///
    /// Serialization replays the raw segments, so edits to the typed fields are
    /// **not** reflected. Build an outbound message with
    /// [`MessageBuilder`](crate::MessageBuilder) instead of mutating a parsed one.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Serialize`] when a segment value cannot be encoded.
    pub fn serialize(&self) -> Result<Vec<u8>, Error> {
        edifact_rs::segments_to_bytes_owned(&self.segments)
            .map_err(|e| Error::Serialize(e.to_string()))
    }

    // ── Construction ─────────────────────────────────────────────────────────

    /// Identify and parse one message from its segments.
    ///
    /// # Errors
    ///
    /// - [`Error::MissingSegment`] — no `UNH` or no `BGM`.
    /// - [`Error::UnknownDocumentCode`] — `BGM` DE 1001 is not a DVGW code.
    /// - [`Error::CarrierMismatch`] — `UNH` DE 0065 contradicts the document code.
    pub(crate) fn from_segments(segments: Vec<OwnedSegment>) -> Result<Self, Error> {
        let unh = find(&segments, "UNH").ok_or(Error::MissingSegment("UNH"))?;
        let message_ref = unh.element_str(0).unwrap_or_default().to_owned();
        let carrier_code = unh.component_str(1, 0).unwrap_or_default().to_owned();
        let version = unh.component_str(1, 4).and_then(DvgwVersion::parse);

        let bgm = find(&segments, "BGM").ok_or(Error::MissingSegment("BGM"))?;
        let document_code = bgm.component_str(0, 0).unwrap_or_default();
        let document =
            DvgwDocument::from_code(document_code).ok_or_else(|| Error::UnknownDocumentCode {
                raw_code: sanitize_code(document_code),
            })?;
        let document_number = bgm.component_str(1, 0).map(str::to_owned);

        // The carrier is a cross-check on the identity, not the identity itself.
        let expected = document.carrier();
        let carrier = match Carrier::from_unh_code(&carrier_code) {
            Some(c) if c == expected => c,
            _ => {
                return Err(Error::CarrierMismatch {
                    document: document.code(),
                    expected: expected.as_str(),
                    raw_code: sanitize_code(&carrier_code),
                });
            }
        };

        // `DTM+Z05` declares the zone every other timestamp is read in, so it is
        // resolved before any of them.
        let mut undecodable_dtm = Vec::new();
        let timezone = header_dtm(&segments, "Z05", UtcOffset::UTC, &mut undecodable_dtm)
            .and_then(DtmValue::as_hours)
            .and_then(|h| UtcOffset::from_hms(h, 0, 0).ok())
            .unwrap_or(UtcOffset::UTC);

        let message_datetime = header_dtm(&segments, "137", timezone, &mut undecodable_dtm)
            .and_then(DtmValue::as_instant);
        let validity_period = header_dtm(&segments, "Z01", timezone, &mut undecodable_dtm)
            .and_then(DtmValue::as_period);

        let header_end = segments
            .iter()
            .position(|s| s.tag == "LIN")
            .unwrap_or(segments.len());
        let header = &segments[..header_end];

        let references: Vec<Reference> = header
            .iter()
            .filter(|s| s.tag == "RFF")
            .filter_map(read_reference)
            .collect();
        let parties: Vec<Party> = header
            .iter()
            .filter(|s| s.tag == "NAD")
            .filter_map(read_party)
            .collect();
        let pruefidentifikator = references
            .iter()
            .find(|r| r.qualifier == rff::PRUEFIDENTIFIKATOR)
            .and_then(|r| r.value.parse::<Pruefidentifikator>().ok());

        let items = parse_items(&segments[header_end..], timezone, &mut undecodable_dtm);

        Ok(Self {
            message_type: document.message_type(),
            document,
            carrier,
            version,
            message_ref,
            document_number,
            pruefidentifikator,
            timezone,
            message_datetime,
            validity_period,
            references,
            parties,
            items,
            undecodable_dtm,
            segments,
        })
    }
}

// ── Segment readers ───────────────────────────────────────────────────────────

fn find<'a>(segments: &'a [OwnedSegment], tag: &str) -> Option<&'a OwnedSegment> {
    segments.iter().find(|s| s.tag == tag)
}

fn read_reference(seg: &OwnedSegment) -> Option<Reference> {
    Some(Reference {
        qualifier: seg.component_str(0, 0)?.to_owned(),
        value: seg.component_str(0, 1).unwrap_or_default().to_owned(),
    })
}

fn read_party(seg: &OwnedSegment) -> Option<Party> {
    Some(Party {
        role: seg.element_str(0)?.to_owned(),
        id: seg.component_str(1, 0).unwrap_or_default().to_owned(),
        agency: seg
            .component_str(1, 2)
            .filter(|a| !a.is_empty())
            .map(str::to_owned),
    })
}

fn read_item_description(seg: &OwnedSegment) -> ItemDescription {
    ItemDescription {
        characteristic: seg
            .element_str(1)
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        code: seg
            .component_str(2, 0)
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
    }
}

/// Decode a `DTM` against its own format code.
///
/// A `DTM` whose value does not match its declared format is recorded in
/// `undecodable` rather than dropped, so validation can name it.
fn read_dtm(
    seg: &OwnedSegment,
    offset: UtcOffset,
    undecodable: &mut Vec<String>,
) -> Option<DtmValue> {
    let qualifier = seg.component_str(0, 0)?;
    let value = seg.component_str(0, 1).unwrap_or_default();
    let format = seg.component_str(0, 2).and_then(DtmFormat::from_code);
    let Some(format) = format else {
        undecodable.push(qualifier.to_owned());
        return None;
    };
    let decoded = datetime::decode(value, format, offset);
    if decoded.is_none() {
        undecodable.push(qualifier.to_owned());
    }
    decoded
}

fn header_dtm(
    segments: &[OwnedSegment],
    qualifier: &str,
    offset: UtcOffset,
    undecodable: &mut Vec<String>,
) -> Option<DtmValue> {
    let header_end = segments
        .iter()
        .position(|s| s.tag == "LIN")
        .unwrap_or(segments.len());
    let seg = segments[..header_end]
        .iter()
        .find(|s| s.tag == "DTM" && s.component_str(0, 0) == Some(qualifier))?;
    read_dtm(seg, offset, undecodable)
}

/// Walk the `LIN` loops, keeping the `LOC` → `DTM` → `QTY` → `STS` nesting.
///
/// The walk is a small state machine over the segment order rather than a scan
/// for tags, because the meaning of a `DTM` or a `NAD` depends entirely on which
/// group is open when it appears.
fn parse_items(
    segments: &[OwnedSegment],
    offset: UtcOffset,
    undecodable: &mut Vec<String>,
) -> Vec<LineItem> {
    let mut items: Vec<LineItem> = Vec::new();
    // The `DTM+2` currently in effect inside the open `LOC` group.
    let mut current_period: Option<DvgwPeriod> = None;

    for seg in segments {
        match seg.tag.as_str() {
            "LIN" => {
                items.push(LineItem {
                    number: seg
                        .element_str(0)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned),
                    // C212 sits in element 2; DE 7143 is its second component
                    // (`LIN+1++:Z01::332` — the Zeitreihentyp).
                    item_type: seg
                        .component_str(2, 1)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned),
                    descriptions: Vec::new(),
                    locations: Vec::new(),
                    parties: Vec::new(),
                });
                current_period = None;
            }
            "IMD" => {
                if let Some(item) = items.last_mut() {
                    item.descriptions.push(read_item_description(seg));
                }
            }
            "LOC" => {
                if let Some(item) = items.last_mut() {
                    item.locations.push(LocationGroup {
                        qualifier: seg.element_str(0).unwrap_or_default().to_owned(),
                        code: seg
                            .component_str(1, 0)
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned),
                        agency: seg
                            .component_str(1, 2)
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned),
                        quantities: Vec::new(),
                    });
                }
                current_period = None;
            }
            // Inside a position, `DTM+2` sets the period for the quantities that
            // follow it — several may alternate to transmit a profile.
            "DTM" => {
                if let Some(period) =
                    read_dtm(seg, offset, undecodable).and_then(DtmValue::as_period)
                {
                    current_period = Some(period);
                }
            }
            "QTY" => {
                let raw_value = seg.component_str(0, 1).unwrap_or_default().to_owned();
                let quantity = Quantity {
                    qualifier: seg.component_str(0, 0).unwrap_or_default().to_owned(),
                    value: raw_value.parse().ok(),
                    raw_value,
                    unit: seg
                        .component_str(0, 2)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned),
                    period: current_period,
                    status: Vec::new(),
                };
                if let Some(location) = items.last_mut().and_then(|i| i.locations.last_mut()) {
                    location.quantities.push(quantity);
                }
            }
            "STS" => {
                let code = seg.component_str(0, 0).filter(|s| !s.is_empty());
                let target = items
                    .last_mut()
                    .and_then(|i| i.locations.last_mut())
                    .and_then(|l| l.quantities.last_mut());
                if let (Some(code), Some(quantity)) = (code, target) {
                    quantity.status.push(code.to_owned());
                }
            }
            "NAD" => {
                if let (Some(item), Some(party)) = (items.last_mut(), read_party(seg)) {
                    item.parties.push(party);
                }
            }
            _ => {}
        }
    }
    items
}
