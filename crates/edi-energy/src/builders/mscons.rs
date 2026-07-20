//! [`MsconsBuilder`] — fluent type-safe builder for MSCONS messages.

use std::marker::PhantomData;

use edifact_rs::Writer;
use rubo4e::identifiers::ObisCode;

use crate::AgencyCode;
use crate::{Error, Pruefidentifikator, Release};

use super::{Set, Unset, bytes_to_segments, today_ccyymmdd};

/// DE 7143 code marking a `PIA` value as an OBIS-Kennzahl (MSCONS AHB 3.2, SG9).
///
/// `Z08` is the sibling code for a Medium.
const PIA_TYPE_OBIS: &str = "SRW";

/// DE 6063 for a summed energy quantity — "Energiemenge summiert (Summenwert,
/// Bilanzsumme)" (MSCONS AHB 3.2, SG10 QTY).
///
/// This is the qualifier a Summenzeitreihe carries. A plain consumption
/// qualifier would describe a single metering point's draw rather than the
/// aggregate of a Bilanzierungsgebiet.
pub const QTY_ENERGIE_SUMMIERT: &str = "79";

/// DE 6063 for a measured ("wahrer") quantity — MSCONS AHB 3.2, SG10 QTY.
pub const QTY_WAHRER_WERT: &str = "220";

/// DE 6063 for a substitute quantity (Ersatzwert) — MSCONS AHB 3.2, SG10 QTY.
pub const QTY_ERSATZWERT: &str = "67";

/// Every DE 6411 Maßeinheit MSCONS defines (MIG 2.5, SG10 QTY).
///
/// The list is closed. Validating against it keeps a typo from reaching the
/// wire, where it becomes a syntactically valid message carrying a unit the
/// receiver cannot interpret.
pub const MSCONS_UNITS: [&str; 4] = ["KWH", "KWT", "D54", "MTS"];

/// `true` when `unit` is a DE 6411 code MSCONS defines.
#[must_use]
pub fn is_valid_mscons_unit(unit: &str) -> bool {
    MSCONS_UNITS.contains(&unit)
}

// ── Inner spec structs ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct QuantitySpec {
    qualifier: String,
    value: String,
    unit: String,
    /// Measurement period the quantity covers, as `(start, end)` in
    /// `CCYYMMDDHHMMZZZ` (EDIFACT format 303).
    ///
    /// A Summenzeitreihe quantity without one has no time reference at all: the
    /// receiver cannot place it on the settlement grid.
    period: Option<(String, String)>,
}

/// Period a `DTM+306` Leistungsperiode covers, with its EDIFACT format code.
#[derive(Debug, Clone)]
struct LeistungsperiodeSpec {
    value: String,
    /// `610` (`CCYYMM`) under a monthly or yearly Leistungspreissystem, `102`
    /// (`CCYYMMDD`) under a daily one.
    format: String,
}

#[derive(Debug, Clone)]
struct LineItemSpec {
    line_number: usize,
    /// OBIS measurement identifier (IEC 62056-61: `[A-B:]C.D[.E][*F]`).
    /// Validated at construction — malformed OBIS codes are rejected.
    obis_code: Option<ObisCode>,
    quantities: Vec<QuantitySpec>,
    /// Period a power maximum fell in (`DTM+306`).
    leistungsperiode: Option<LeistungsperiodeSpec>,
}

#[derive(Debug, Clone)]
struct MeteringPointSpec {
    malo_id: String,
    location_id: Option<String>,
    /// Bilanzierungsmonat as `CCYYMM` (`DTM+492`, format 610).
    balancing_period: Option<String>,
    /// Versionsangabe as `CCYYMMDDHHMMSSZZZ` (`DTM+293`, format 304).
    ///
    /// `MaBiS` identifies a Summenzeitreihe by (`MaBiS`-Zählpunkt,
    /// Bilanzierungsmonat, Version) and requires the version to ascend
    /// (BK6-24-174 Anlage 3 §3.8.2). It is the only thing distinguishing a
    /// correction from the original, since `BGM` DE 1225 is always `9`.
    version: Option<String>,
    line_items: Vec<LineItemSpec>,
}

#[derive(Debug, Clone)]
struct MsconsBuilderInner {
    release: Release,
    pruefidentifikator: Option<Pruefidentifikator>,
    sender_id: Option<String>,
    receiver_id: Option<String>,
    sender_agency: AgencyCode,
    receiver_agency: AgencyCode,
    message_ref: String,
    document_code: String,
    /// BGM DE 1004 Dokumentennummer.
    document_number: String,
    document_date: Option<String>,
    header_references: Vec<(String, String)>,
    metering_points: Vec<MeteringPointSpec>,
}

// ── MsconsBuilder ─────────────────────────────────────────────────────────────

/// Fluent builder for `MSCONS` (Metered Services Consumption Report) messages.
///
/// # Type-state
///
/// [`build`](MsconsBuilder::build) is only available once both
/// [`sender`](MsconsBuilder::sender) and [`receiver`](MsconsBuilder::receiver)
/// have been called.
///
/// # Example
///
/// ```rust,no_run
/// use edi_energy::{Release, Pruefidentifikator};
/// use edi_energy::builders::MsconsBuilder;
///
/// let msg = MsconsBuilder::new(Release::new("2.4c"))
///     .pruefidentifikator(Pruefidentifikator::new(13002).unwrap())
///     .sender("9900111222333")
///     .receiver("9900444555666")
///     .metering_point("DE0001234567890")
///         .location_id("12345678901")
///         .quantity("220", "1000.500", "KWH")
///     .done()
///     .build()?;
///
/// assert_eq!(msg.delivery_points().len(), 1);
/// # Ok::<(), edi_energy::Error>(())
/// ```
#[derive(Debug, Clone)]
#[must_use = "Builder must be consumed via .build() or .serialize()"]
pub struct MsconsBuilder<S = Unset, R = Unset> {
    _ph: PhantomData<fn() -> (S, R)>,
    inner: MsconsBuilderInner,
}

impl MsconsBuilder<Unset, Unset> {
    /// Create a builder targeting the given EDI@Energy release.
    pub fn new(release: Release) -> Self {
        Self {
            _ph: PhantomData,
            inner: MsconsBuilderInner {
                release,
                pruefidentifikator: None,
                sender_id: None,
                receiver_id: None,
                sender_agency: AgencyCode::Bdew,
                receiver_agency: AgencyCode::Bdew,
                message_ref: "1".to_owned(),
                document_code: "7".to_owned(),
                document_number: String::new(),
                document_date: None,
                header_references: Vec::new(),
                metering_points: Vec::new(),
            },
        }
    }
}

impl<S, R> MsconsBuilder<S, R> {
    fn transition<S2, R2>(self) -> MsconsBuilder<S2, R2> {
        MsconsBuilder {
            _ph: PhantomData,
            inner: self.inner,
        }
    }

    /// Set the message sender's market-participant identifier.
    pub fn sender(mut self, id: impl Into<String>) -> MsconsBuilder<Set, R> {
        self.inner.sender_id = Some(id.into());
        self.transition()
    }

    /// Set the message recipient's market-participant identifier.
    pub fn receiver(mut self, id: impl Into<String>) -> MsconsBuilder<S, Set> {
        self.inner.receiver_id = Some(id.into());
        self.transition()
    }

    /// Override the agency code for the sender's party identifier.
    ///
    /// Default: [`AgencyCode::Bdew`] (`"293"`). Use [`AgencyCode::Entso`] (`"305"`)
    /// for TSO/ÜNB parties that carry a 16-char EIC code.
    pub fn sender_agency(mut self, agency: crate::AgencyCode) -> Self {
        self.inner.sender_agency = agency;
        self
    }

    /// Override the agency code for the receiver's party identifier.
    ///
    /// Default: [`AgencyCode::Bdew`] (`"293"`).
    pub fn receiver_agency(mut self, agency: crate::AgencyCode) -> Self {
        self.inner.receiver_agency = agency;
        self
    }

    /// Set the Pruefidentifikator (e.g. `21001`).
    pub fn pruefidentifikator(mut self, pid: Pruefidentifikator) -> Self {
        self.inner.pruefidentifikator = Some(pid);
        self
    }

    /// Override the message reference number.  Defaults to `"1"`.
    pub fn message_ref(mut self, reference: impl Into<String>) -> Self {
        self.inner.message_ref = reference.into();
        self
    }

    /// Set the BGM Dokumentennummer (DE 1004).
    ///
    /// Defaults to the message reference when unset, so the document always
    /// carries a number the sender can correlate on.
    pub fn document_number(mut self, number: impl Into<String>) -> Self {
        self.inner.document_number = number.into();
        self
    }

    /// Override the BGM document name code (DE 1001).  Defaults to `"7"`.
    pub fn document_code(mut self, code: impl Into<String>) -> Self {
        self.inner.document_code = code.into();
        self
    }

    /// Set the document date for DTM+137 (`YYYYMMDD`).
    pub fn document_date(mut self, date: impl Into<String>) -> Self {
        self.inner.document_date = Some(date.into());
        self
    }

    /// Add a header-section reference (RFF segment) before NAD.
    pub fn header_reference(
        mut self,
        qualifier: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.inner
            .header_references
            .push((qualifier.into(), value.into()));
        self
    }

    /// Start configuring a metering-point (NAD+DP) block in the detail section.
    ///
    /// Returns a [`MeteringPointBuilder`] sub-builder. Call
    /// [`done`](MeteringPointBuilder::done) to return to this builder.
    pub fn metering_point(self, malo_id: impl Into<String>) -> MeteringPointBuilder<S, R> {
        MeteringPointBuilder {
            parent: self,
            spec: MeteringPointSpec {
                malo_id: malo_id.into(),
                location_id: None,
                balancing_period: None,
                version: None,
                line_items: Vec::new(),
            },
            current_item: None,
        }
    }
}

impl<S, R> MsconsBuilder<S, R> {
    fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let pid_str = self
            .inner
            .pruefidentifikator
            .map(|p| format!("{:05}", p.as_u32()))
            .unwrap_or_default();
        // Defaults to the Prüfidentifikator, which is what BDEW's examples put
        // in DE 1004 for these use cases.
        let document_number = if self.inner.document_number.is_empty() {
            pid_str.clone()
        } else {
            self.inner.document_number.clone()
        };
        let dtm_val = self
            .inner
            .document_date
            .as_deref()
            .map_or_else(today_ccyymmdd, str::to_owned);

        let mut buf = Vec::new();
        let mut w = Writer::new(&mut buf);

        emit_comp!(
            w,
            "UNH",
            [&self.inner.message_ref],
            ["MSCONS", "D", "04B", "UN", self.inner.release.as_str()]
        );
        // BGM DE 1004 is the Dokumentennummer. The Prüfidentifikator travels in
        // SG1 RFF+Z13 (MSCONS AHB 3.2), which is what the receiver routes on —
        // putting it in BGM leaves the message with no detectable PID.
        emit_seg!(w, "BGM", &self.inner.document_code, &document_number, "9");
        emit_comp!(w, "DTM", ["137", &dtm_val, "102"]);
        if !pid_str.is_empty() {
            emit_comp!(w, "RFF", ["Z13", &pid_str]);
        }
        for (qualifier, value) in &self.inner.header_references {
            emit_comp!(w, "RFF", [qualifier, value]);
        }
        if let Some(id) = &self.inner.sender_id {
            emit_comp!(
                w,
                "NAD",
                ["MS"],
                [id, "", self.inner.sender_agency.as_str()]
            );
        }
        if let Some(id) = &self.inner.receiver_id {
            emit_comp!(
                w,
                "NAD",
                ["MR"],
                [id, "", self.inner.receiver_agency.as_str()]
            );
        }
        if !self.inner.metering_points.is_empty() {
            emit_seg!(w, "UNS", "D");
            for mp in &self.inner.metering_points {
                emit_comp!(w, "NAD", ["DP"], [&mp.malo_id, "", "293"]);
                if !mp.line_items.is_empty() {
                    // LOC+172 is mandatory. Falling back to the metering point
                    // keeps a caller that set no separate location from emitting
                    // a message the profile rejects for a missing segment.
                    let loc_id = mp.location_id.as_deref().unwrap_or(mp.malo_id.as_str());
                    emit_seg!(w, "LOC", "172", loc_id);
                    if let Some(period) = &mp.balancing_period {
                        emit_comp!(w, "DTM", ["492", period, "610"]);
                    }
                    if let Some(version) = &mp.version {
                        emit_comp!(w, "DTM", ["293", version, "304"]);
                    }
                    for item in &mp.line_items {
                        let ln = item.line_number.to_string();
                        emit_seg!(w, "LIN", &ln);
                        if let Some(obis) = &item.obis_code {
                            // DE 7143 `SRW` marks the value as an OBIS-Kennzahl
                            // (MSCONS AHB 3.2, SG9 PIA); `Z08` would mark it as a
                            // Medium. `to_pia_string()` returns the OBIS without
                            // release characters; the writer escapes it for the
                            // active UNA because the component boundary is
                            // structural here, not inferred from `:`.
                            let pia_value = obis.to_pia_string();
                            emit_comp!(w, "PIA", ["5"], [&pia_value, PIA_TYPE_OBIS]);
                        }
                        for qty in &item.quantities {
                            emit_comp!(w, "QTY", [&qty.qualifier, &qty.value, &qty.unit]);
                            // DE 2005 163/164 = Beginn/Ende Messperiode. Emitted
                            // immediately after the QTY they bound, which is what
                            // associates them with that quantity.
                            if let Some((start, end)) = &qty.period {
                                emit_comp!(w, "DTM", ["163", start, "303"]);
                                emit_comp!(w, "DTM", ["164", end, "303"]);
                            }
                        }
                        // The Leistungsperiode belongs to the line item's
                        // maximum, so it follows that item's quantities.
                        if let Some(lp) = &item.leistungsperiode {
                            emit_comp!(w, "DTM", ["306", &lp.value, &lp.format]);
                        }
                    }
                }
            }
        }
        w.finish_unt(&self.inner.message_ref)
            .map_err(Error::Parse)?;
        Ok(buf)
    }
    /// Build and serialize the message to EDIFACT bytes.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if serialization fails.
    pub fn serialize(self) -> Result<Vec<u8>, Error> {
        self.to_bytes()
    }
}

impl MsconsBuilder<Set, Set> {
    /// Build and return a fully-parsed [`crate::messages::mscons::MsconsMessage`].
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if EDIFACT serialization or parsing fails.
    pub fn build(self) -> Result<crate::messages::mscons::MsconsMessage, Error> {
        let pid = self
            .inner
            .pruefidentifikator
            .map(super::super::pruefidentifikator::Pruefidentifikator::as_u32);
        let message_ref = self.inner.message_ref.clone();
        let assoc_code = self.inner.release.as_str().to_owned();
        let segments = bytes_to_segments(&self.to_bytes()?)?;
        Ok(crate::messages::mscons::MsconsMessage::from_parts(
            segments,
            message_ref.as_str(),
            assoc_code.as_str(),
            pid,
        ))
    }
}

// ── MeteringPointBuilder ──────────────────────────────────────────────────────

/// Sub-builder for a metering-point (NAD+DP) block in an MSCONS message.
///
/// Obtained via [`MsconsBuilder::metering_point`]. Call
/// [`done`](MeteringPointBuilder::done) to finalize and return to the parent.
#[derive(Debug)]
#[must_use = "Sub-builder must be finalized with .done()"]
pub struct MeteringPointBuilder<S = Unset, R = Unset> {
    parent: MsconsBuilder<S, R>,
    spec: MeteringPointSpec,
    current_item: Option<LineItemSpec>,
}

impl<S, R> MeteringPointBuilder<S, R> {
    /// Set the grid location / Messlokation ID (LOC+172).
    pub fn location_id(mut self, id: impl Into<String>) -> Self {
        self.flush_item();
        self.spec.location_id = Some(id.into());
        self
    }

    /// Set the OBIS code (PIA+5) for the current line item.
    ///
    /// The code must conform to IEC 62056-61: `[A-B:]C.D[.E][*F]`.
    /// A new line item is created automatically if none is in progress.
    /// Set the Bilanzierungsmonat (`DTM+492`, format 610, `CCYYMM`).
    pub fn balancing_period(mut self, period: impl Into<String>) -> Self {
        self.spec.balancing_period = Some(period.into());
        self
    }

    /// Set the Versionsangabe (`DTM+293`, format 304, `CCYYMMDDHHMMSSZZZ`).
    ///
    /// Required for a `MaBiS` Summenzeitreihe: it completes the identifying
    /// 3-tuple and is what marks a resubmission as a correction rather than a
    /// duplicate.
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.spec.version = Some(version.into());
        self
    }

    /// Set the OBIS-Kennzahl for the current line item.
    pub fn obis(mut self, code: ObisCode) -> Self {
        self.current_item
            .get_or_insert_with(|| LineItemSpec {
                line_number: self.spec.line_items.len() + 1,
                obis_code: None,
                quantities: Vec::new(),
                leistungsperiode: None,
            })
            .obis_code = Some(code);
        self
    }

    /// Start a new line item (LIN segment) with an OBIS code (PIA).
    ///
    /// The code must conform to IEC 62056-61: `[A-B:]C.D[.E][*F]`.
    pub fn line_item(mut self, obis_code: ObisCode) -> Self {
        self.flush_item();
        let idx = self.spec.line_items.len() + 1;
        self.current_item = Some(LineItemSpec {
            line_number: idx,
            obis_code: Some(obis_code),
            quantities: Vec::new(),
            leistungsperiode: None,
        });
        self
    }

    /// Finalize the current line item and start a new one.
    pub fn next_line_item(mut self) -> Self {
        self.flush_item();
        self
    }

    /// Add a quantity (QTY segment) to the current line item.
    ///
    /// If no line item is active, starts a new one without an OBIS code.
    ///
    /// `qualifier` — DE 6063 (e.g. [`QTY_ENERGIE_SUMMIERT`] for a summed
    /// quantity).
    /// `value` — numeric string (e.g. `"1000.500"`).
    /// `unit` — DE 6411 (e.g. `"KWH"`).
    pub fn quantity(
        mut self,
        qualifier: impl Into<String>,
        value: impl Into<String>,
        unit: impl Into<String>,
    ) -> Self {
        if self.current_item.is_none() {
            let idx = self.spec.line_items.len() + 1;
            self.current_item = Some(LineItemSpec {
                line_number: idx,
                obis_code: None,
                quantities: Vec::new(),
                leistungsperiode: None,
            });
        }
        if let Some(item) = &mut self.current_item {
            item.quantities.push(QuantitySpec {
                qualifier: qualifier.into(),
                value: value.into(),
                unit: unit.into(),
                period: None,
            });
        }
        self
    }

    /// Add a quantity covering an explicit measurement period.
    ///
    /// `start` and `end` are EDIFACT format 303 (`CCYYMMDDHHMMZZZ`). Interval
    /// data must use this rather than [`quantity`](Self::quantity): a bare
    /// `QTY` segment carries no time reference, so the receiver cannot place
    /// the value on the settlement grid.
    pub fn quantity_for_period(
        mut self,
        qualifier: impl Into<String>,
        value: impl Into<String>,
        unit: impl Into<String>,
        start: impl Into<String>,
        end: impl Into<String>,
    ) -> Self {
        if self.current_item.is_none() {
            let idx = self.spec.line_items.len() + 1;
            self.current_item = Some(LineItemSpec {
                line_number: idx,
                obis_code: None,
                quantities: Vec::new(),
                leistungsperiode: None,
            });
        }
        if let Some(item) = &mut self.current_item {
            item.quantities.push(QuantitySpec {
                qualifier: qualifier.into(),
                value: value.into(),
                unit: unit.into(),
                period: Some((start.into(), end.into())),
            });
        }
        self
    }

    /// Record the period a power maximum fell in (`DTM+306`).
    ///
    /// `format` is `610` (`CCYYMM`) under a monthly or yearly
    /// Leistungspreissystem and `102` (`CCYYMMDD`) under a daily one
    /// (MSCONS AHB 3.2, SG10 DTM+306). A maximum without it states a magnitude
    /// with no period, which the receiver cannot attribute to a month.
    pub fn leistungsperiode(mut self, value: impl Into<String>, format: impl Into<String>) -> Self {
        if self.current_item.is_none() {
            let idx = self.spec.line_items.len() + 1;
            self.current_item = Some(LineItemSpec {
                line_number: idx,
                obis_code: None,
                quantities: Vec::new(),
                leistungsperiode: None,
            });
        }
        if let Some(item) = &mut self.current_item {
            item.leistungsperiode = Some(LeistungsperiodeSpec {
                value: value.into(),
                format: format.into(),
            });
        }
        self
    }

    fn flush_item(&mut self) {
        if let Some(item) = self.current_item.take() {
            self.spec.line_items.push(item);
        }
    }

    /// Finalize this metering-point block and return to the parent [`MsconsBuilder`].
    pub fn done(mut self) -> MsconsBuilder<S, R> {
        self.flush_item();
        self.parent.inner.metering_points.push(self.spec);
        self.parent
    }
}

#[cfg(test)]
mod summenzeitreihe_tests {
    use super::*;
    use crate::generated::releases;

    /// A `MaBiS` Summenzeitreihe (PID 13003) must carry its identifying 3-tuple
    /// and a time reference for every quantity.
    #[test]
    fn a_summenzeitreihe_carries_its_version_and_interval_bounds() {
        let wire = MsconsBuilder::new(releases::mscons_fv20261001().clone())
            .sender("9900357000004")
            .receiver("9900077000006")
            .pruefidentifikator(Pruefidentifikator::new(13003).expect("13003 is a valid PID"))
            .message_ref("SZR0001")
            .metering_point("11YAPG4CTRDNZ--A")
            .balancing_period("202606")
            .version("20260714050000+00")
            .quantity_for_period(
                QTY_ENERGIE_SUMMIERT,
                "12.5",
                "KWH",
                "202606010000+00",
                "202606010015+00",
            )
            .done()
            .serialize()
            .expect("serialize");
        let wire = String::from_utf8(wire).expect("utf-8");

        assert!(
            wire.contains("DTM+492:202606:610"),
            "Bilanzierungsmonat must be present: {wire}"
        );
        // `+` is the EDIFACT segment separator, so a `+` inside a value is
        // escaped with the release character `?`. The UTC offset in a format-304
        // timestamp therefore appears as `?+00` on the wire.
        assert!(
            wire.contains("DTM+293:20260714050000?+00:304"),
            "Versionsangabe must be present — it is what marks a correction: {wire}"
        );
        assert!(
            wire.contains("QTY+79:12.5:KWH"),
            "quantity must be present: {wire}"
        );
        assert!(
            wire.contains("DTM+163:202606010000?+00:303"),
            "interval start must bound the quantity: {wire}"
        );
        assert!(
            wire.contains("DTM+164:202606010015?+00:303"),
            "interval end must bound the quantity: {wire}"
        );

        // The bounds must follow the quantity they describe, which is what
        // associates them with it.
        let qty_at = wire.find("QTY+79").expect("QTY present");
        let dtm_at = wire.find("DTM+163").expect("DTM present");
        assert!(dtm_at > qty_at, "period must follow its QTY: {wire}");
    }

    /// A plain `quantity` emits no period, so it stays usable for the
    /// non-interval cases without inventing a time reference.
    #[test]
    fn a_bare_quantity_emits_no_period() {
        let wire = MsconsBuilder::new(releases::mscons_fv20261001().clone())
            .sender("9900357000004")
            .receiver("9900077000006")
            .message_ref("M1")
            .metering_point("DE0001234567890")
            .quantity("220", "42", "KWH")
            .done()
            .serialize()
            .expect("serialize");
        let wire = String::from_utf8(wire).expect("utf-8");
        assert!(wire.contains("QTY+220:42:KWH"));
        assert!(!wire.contains("DTM+163"), "no invented period: {wire}");
    }
}
