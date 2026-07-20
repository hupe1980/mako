use edifact_rs::{
    EdifactDeserialize, EdifactSerialize, EventEmitter, OwnedSegment, ProfileRulePack,
    ValidationIssue, ValidationSeverity,
};

use crate::{
    MessageType,
    messages::{
        core::MessageCore,
        segments::{
            Bgm, Cci, Dtm, Lin, Loc, Nad, Pia, Qty, Rff, Sts, collect_dtm, find_bgm, find_nad,
            try_deserialize,
        },
    },
};

// ── Segment group types ───────────────────────────────────────────────────────

/// A header-section reference group (MSCONS SG1: RFF + optional DTM).
///
/// Carries the Pruefidentifikator, MMMA reference, and similar header
/// reference codes before the section delimiter (`UNS+D`).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct MsconsReference {
    /// RFF — reference qualifier and identifier.
    pub rff: Rff,
    /// DTM — date/version for this reference (optional in SG1).
    pub dtm: Vec<Dtm>,
}

/// A delivery / receipt point group (MSCONS SG5: NAD + SG6 sub-groups).
///
/// Each instance represents one metering location or delivery point
/// described by a `NAD` segment after the `UNS+D` section delimiter.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct MsconsDeliveryPoint {
    /// NAD — location / delivery-point identification.
    pub nad: Nad,
    /// SG6 — one or more time-series / measurement objects for this location.
    pub time_series: Vec<MsconsTimeSeries>,
}

/// A measurement-object time series (MSCONS SG6: LOC + nested SG7/SG8/SG9).
///
/// Identified by a `LOC` segment (balance zone, measurement point, etc.).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct MsconsTimeSeries {
    /// LOC — identifies the measurement object or balance zone.
    pub loc: Loc,
    /// DTM — delivery-period dates for this time series (SG6 level).
    pub dtm: Vec<Dtm>,
    /// SG7 — references for this measurement object (e.g. device number).
    pub references: Vec<Rff>,
    /// SG8 — time-series type (Zeitreihentyp), from `CCI`.
    pub time_series_type: Option<Cci>,
    /// SG9 — line items (metered interval values) for this time series.
    pub items: Vec<MsconsLineItem>,
}

/// A line-item group (MSCONS SG9: LIN + PIA + SG10).
///
/// One `LIN` segment plus optional OBIS code (`PIA`) and one or more
/// quantity readings (`SG10`).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct MsconsLineItem {
    /// LIN — sequential line-item number.
    pub lin: Lin,
    /// PIA — OBIS code or other product identification (optional).
    pub pia: Option<Pia>,
    /// SG10 — quantity readings for this line item.
    pub quantities: Vec<MsconsQuantity>,
}

/// A quantity reading (MSCONS SG10: QTY + DTM + STS).
///
/// The leaf of the hierarchy: one metered value with its period and status.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct MsconsQuantity {
    /// QTY — metered quantity value and unit.
    pub qty: Qty,
    /// DTM — begin/end of the measurement interval.
    pub dtm: Vec<Dtm>,
    /// STS — quality / validation status codes for this reading.
    pub status: Vec<Sts>,
}

// ── MsconsMessage ─────────────────────────────────────────────────────────────

/// MSCONS — Metered Services Consumption Report.
///
/// Transmits meter readings and consumption values between grid operators
/// and balance-group managers in the German energy market.
///
/// # Typed access
///
/// | Field      | Segment | Meaning                                       |
/// |------------|---------|-----------------------------------------------|
/// | `bgm`             | BGM     | Document code and Pruefidentifikator          |
/// | `dtm`             | DTM     | Message-level DTM segments (e.g. DTM+137)     |
/// | `sender`          | NAD+MS  | Message sender                                |
/// | `receiver`        | NAD+MR  | Message recipient                             |
/// | `references`      | SG1/RFF | Header references (Pruefidentifikator, MMMA)  |
/// | `delivery_points` | SG5/NAD | Metering / delivery point groups              |
///
/// The delivery-point hierarchy provides fully typed access to the metered
/// values: `delivery_points[i].time_series[j].items[k].quantities[l].qty`.
#[derive(Debug, Clone)]
pub struct MsconsMessage {
    pub(crate) core: MessageCore,
    /// BGM — beginning of message.
    bgm: Option<Bgm>,
    /// DTM — message-level date/time segments (before UNS).
    dtm: Vec<Dtm>,
    /// NAD+MS — message sender.
    sender: Option<Nad>,
    /// NAD+MR — message recipient.
    receiver: Option<Nad>,
    /// SG1 — header references (Pruefidentifikator, MMMA allocation list, etc.).
    references: Vec<MsconsReference>,
    /// SG5 — delivery / metering point groups (after `UNS+D`).
    delivery_points: Vec<MsconsDeliveryPoint>,
}

impl MsconsMessage {
    pub(crate) fn from_parts(
        segments: Vec<OwnedSegment>,
        message_ref: impl Into<Box<str>>,
        assoc_code: impl Into<Box<str>>,
        pruefidentifikator: Option<u32>,
    ) -> Self {
        let (bgm, dtm, sender, receiver, references, delivery_points) = {
            let borrowed: Vec<edifact_rs::Segment<'_>> =
                segments.iter().map(|s| s.as_borrowed()).collect();
            (
                find_bgm(&borrowed),
                collect_dtm_header(&borrowed),
                find_nad(&borrowed, "MS"),
                find_nad(&borrowed, "MR"),
                parse_references(&borrowed),
                parse_delivery_points(&borrowed),
            )
        };
        Self {
            core: MessageCore::new(
                segments,
                message_ref,
                assoc_code,
                pruefidentifikator,
                MessageType::Mscons,
            ),
            bgm,
            dtm,
            sender,
            receiver,
            references,
            delivery_points,
        }
    }

    /// The EDI@Energy release / association code from UNH (DE 0057).
    #[must_use]
    pub fn assoc_code(&self) -> &str {
        &self.core.assoc_code
    }

    /// Raw parsed segments (authoritative for validation and serialization).
    #[must_use]
    pub fn segments(&self) -> &[OwnedSegment] {
        &self.core.segments
    }

    /// BGM — beginning of message.  Returns `None` when absent or malformed.
    #[must_use]
    pub fn bgm(&self) -> Option<&Bgm> {
        self.bgm.as_ref()
    }

    /// DTM — message-level date/time segments.
    #[must_use]
    pub fn dtm(&self) -> &[Dtm] {
        &self.dtm
    }

    /// NAD+MS — message sender.  Returns `None` when absent or malformed.
    #[must_use]
    pub fn sender(&self) -> Option<&Nad> {
        self.sender.as_ref()
    }

    /// NAD+MR — message recipient.  Returns `None` when absent or malformed.
    #[must_use]
    pub fn receiver(&self) -> Option<&Nad> {
        self.receiver.as_ref()
    }

    /// SG1 — header references (Pruefidentifikator, MMMA allocation list, etc.).
    #[must_use]
    pub fn references(&self) -> &[MsconsReference] {
        &self.references
    }

    /// SG5 — delivery / metering point groups (after `UNS+D`).
    #[must_use]
    pub fn delivery_points(&self) -> &[MsconsDeliveryPoint] {
        &self.delivery_points
    }
}

// ── EdifactDeserialize ────────────────────────────────────────────────────────

impl EdifactDeserialize for MsconsMessage {
    fn edifact_deserialize(
        segments: &[edifact_rs::Segment<'_>],
    ) -> Result<Self, edifact_rs::EdifactError> {
        let (message_ref, assoc_code) = MessageCore::extract_unh_fields(segments)?;
        let pid = MessageCore::extract_bgm_pid(segments);
        let owned: Vec<OwnedSegment> = segments.iter().cloned().map(OwnedSegment::from).collect();
        Ok(Self::from_parts(owned, message_ref, assoc_code, pid))
    }
}

// ── EdifactSerialize ──────────────────────────────────────────────────────────

impl EdifactSerialize for MsconsMessage {
    fn edifact_serialize<E: EventEmitter>(
        &self,
        emitter: &mut E,
    ) -> Result<(), edifact_rs::EdifactError> {
        self.core.emit_segments(emitter)
    }
}
impl_edi_energy_message!(MsconsMessage, sem = mscons_semantic_pack());

// ── segment group parsers ─────────────────────────────────────────────────────

/// Collect DTM segments from the header section only (before `UNS`).
fn collect_dtm_header(segments: &[edifact_rs::Segment<'_>]) -> Vec<Dtm> {
    let end = segments
        .iter()
        .position(|s| s.tag == "UNS")
        .unwrap_or(segments.len());
    collect_dtm(&segments[..end])
}

/// Parse SG1 reference groups (RFF + optional DTM) from the header section.
fn parse_references(segments: &[edifact_rs::Segment<'_>]) -> Vec<MsconsReference> {
    let end = segments
        .iter()
        .position(|s| s.tag == "UNS")
        .unwrap_or(segments.len());
    let header = &segments[..end];

    let mut result = Vec::new();
    let mut i = 0;
    while i < header.len() {
        if header[i].tag != "RFF" {
            i += 1;
            continue;
        }
        let Some(rff) = try_deserialize::<Rff>(&header[i]) else {
            i += 1;
            continue;
        };
        let mut dtm = Vec::new();
        let mut j = i + 1;
        while j < header.len() && header[j].tag == "DTM" {
            if let Some(d) = try_deserialize::<Dtm>(&header[j]) {
                dtm.push(d);
            }
            j += 1;
        }
        result.push(MsconsReference { rff, dtm });
        i = j;
    }
    result
}

/// Parse SG5 delivery-point groups (NAD + SG6 time-series) from the detail
/// section (after `UNS+D`).
fn parse_delivery_points(segments: &[edifact_rs::Segment<'_>]) -> Vec<MsconsDeliveryPoint> {
    let start = match segments.iter().position(|s| s.tag == "UNS") {
        Some(pos) => pos + 1,
        None => return Vec::new(),
    };
    let detail = &segments[start..];

    let mut result = Vec::new();
    let mut i = 0;

    while i < detail.len() {
        if detail[i].tag != "NAD" {
            i += 1;
            continue;
        }
        let Some(nad) = try_deserialize::<Nad>(&detail[i]) else {
            i += 1;
            continue;
        };
        i += 1;

        // Collect SG6 groups (LOC-headed) that belong to this NAD.
        let (time_series, next_i) = parse_sg6_groups(detail, i);
        i = next_i;

        result.push(MsconsDeliveryPoint { nad, time_series });
    }
    result
}

/// Segment tags that terminate any SG6 (LOC-headed) group.
const SG6_TERMINATORS: &[&str] = &["NAD", "UNT"];
/// Segment tags that terminate any SG9 (LIN-headed) group.
const SG9_TERMINATORS: &[&str] = &["LIN", "LOC", "NAD", "UNT"];
/// Segment tags that terminate any SG10 (QTY-headed) group.
const SG10_TERMINATORS: &[&str] = &["QTY", "LIN", "LOC", "NAD", "UNT"];

/// Parse all SG6 time-series groups from `detail[from..]` until a top-level
/// boundary (NAD or UNT).  Returns `(groups, next_index)`.
fn parse_sg6_groups(
    detail: &[edifact_rs::Segment<'_>],
    from: usize,
) -> (Vec<MsconsTimeSeries>, usize) {
    let mut series = Vec::new();
    let mut i = from;

    while i < detail.len() {
        if SG6_TERMINATORS.iter().any(|t| &detail[i].tag == t) {
            break;
        }
        if detail[i].tag != "LOC" {
            i += 1;
            continue;
        }
        let Some(loc) = try_deserialize::<Loc>(&detail[i]) else {
            i += 1;
            continue;
        };
        i += 1;

        let mut dtm = Vec::new();
        let mut references = Vec::new();
        let mut time_series_type: Option<Cci> = None;

        // Consume DTM / RFF (SG7) / CCI (SG8) before any LIN.
        while i < detail.len() && !SG6_TERMINATORS.iter().any(|t| &detail[i].tag == t) {
            match detail[i].tag {
                "DTM" => {
                    if let Some(d) = try_deserialize::<Dtm>(&detail[i]) {
                        dtm.push(d);
                    }
                    i += 1;
                }
                "RFF" => {
                    if let Some(r) = try_deserialize::<Rff>(&detail[i]) {
                        references.push(r);
                    }
                    i += 1;
                }
                "CCI" => {
                    time_series_type = try_deserialize::<Cci>(&detail[i]);
                    i += 1;
                }
                "LIN" | "LOC" => break, // next SG6 group starts
                _ => {
                    i += 1;
                }
            }
        }

        // Consume SG9 (LIN-headed) line items.
        let (items, next_i) = parse_sg9_items(detail, i);
        i = next_i;

        series.push(MsconsTimeSeries {
            loc,
            dtm,
            references,
            time_series_type,
            items,
        });
    }

    (series, i)
}

/// Parse all SG9 line-item groups from `detail[from..]` until an SG6 boundary.
/// Returns `(items, next_index)`.
fn parse_sg9_items(
    detail: &[edifact_rs::Segment<'_>],
    from: usize,
) -> (Vec<MsconsLineItem>, usize) {
    let mut items = Vec::new();
    let mut i = from;

    while i < detail.len() {
        if SG9_TERMINATORS[1..].iter().any(|t| &detail[i].tag == t) {
            // LOC / NAD / UNT — SG9 section ends.
            break;
        }
        if detail[i].tag != "LIN" {
            i += 1;
            continue;
        }
        let Some(lin) = try_deserialize::<Lin>(&detail[i]) else {
            i += 1;
            continue;
        };
        i += 1;

        // Optional PIA immediately after LIN.
        let pia = if i < detail.len() && detail[i].tag == "PIA" {
            let p = try_deserialize::<Pia>(&detail[i]);
            i += 1;
            p
        } else {
            None
        };

        // SG10 quantity groups.
        let (quantities, next_i) = parse_sg10_quantities(detail, i);
        i = next_i;

        items.push(MsconsLineItem {
            lin,
            pia,
            quantities,
        });
    }

    (items, i)
}

/// Parse all SG10 quantity groups from `detail[from..]` until an SG9 boundary.
/// Returns `(quantities, next_index)`.
fn parse_sg10_quantities(
    detail: &[edifact_rs::Segment<'_>],
    from: usize,
) -> (Vec<MsconsQuantity>, usize) {
    let mut quantities = Vec::new();
    let mut i = from;

    while i < detail.len() {
        if SG10_TERMINATORS[1..].iter().any(|t| &detail[i].tag == t) {
            break;
        }
        if detail[i].tag != "QTY" {
            i += 1;
            continue;
        }
        let Some(qty) = try_deserialize::<Qty>(&detail[i]) else {
            i += 1;
            continue;
        };
        i += 1;

        let mut dtm = Vec::new();
        let mut status = Vec::new();

        while i < detail.len() && !SG10_TERMINATORS.iter().any(|t| &detail[i].tag == t) {
            match detail[i].tag {
                "DTM" => {
                    if let Some(d) = try_deserialize::<Dtm>(&detail[i]) {
                        dtm.push(d);
                    }
                }
                "STS" => {
                    if let Some(s) = try_deserialize::<Sts>(&detail[i]) {
                        status.push(s);
                    }
                }
                _ => {}
            }
            i += 1;
        }

        quantities.push(MsconsQuantity { qty, dtm, status });
    }

    (quantities, i)
}

// ── Layer 5: MSCONS semantic rule pack ───────────────────────────────────────

/// Build the MSCONS semantic rule pack (Layer 5).
///
/// Rules:
/// - [`rule_sem_melo_format`]: `LOC+172` metering-point IDs must be exactly 11
///   upper-case alphanumeric characters ([A-Z0-9]{11}).
/// - [`rule_sem_period_order`]: when both a start-of-period (`DTM 163`) and an
///   end-of-period (`DTM 164`) are present, the start must not be after the end.
/// - [`rule_sem_unit_unknown`]: the unit-of-measure code in `QTY C186` component 2
///   must be from the EDI@Energy approved set.
fn mscons_semantic_pack() -> ProfileRulePack {
    ProfileRulePack::new("MSCONS-SEM")
        .for_message_type("MSCONS")
        .with_stateless_rule_fn(rule_sem_melo_format)
        .with_stateless_rule_fn(rule_sem_period_order)
        .with_stateless_rule_fn(rule_sem_unit_unknown)
}

/// `SEM-MSCONS-MELO-FORMAT` — Metering-point IDs in `LOC+172` must be exactly
/// 11 upper-case alphanumeric characters ([A-Z0-9]{11}).
fn rule_sem_melo_format(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    for seg in segments.iter().filter(|s| s.tag == "LOC") {
        // LOC: element[0] = 3227 (location qualifier), element[1] = C517 composite.
        // C517 component[0] = 3225 (location id code / metering point ID).
        let qualifier = seg.element_str(0).unwrap_or("");
        if qualifier != "172" {
            continue; // Only check Marktlokation (172) identifiers.
        }
        let id = seg
            .get_element(1)
            .and_then(|e| e.get_component(0))
            .unwrap_or("");
        if id.is_empty() {
            continue;
        }
        if !super::common::is_valid_location_id(id) {
            issues.push(
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    "LOC+172 element 3225 (C517 component 0): value does not match the \
                     Messlokations-ID format [A-Z0-9]{11}"
                        .to_owned(),
                )
                .with_span(seg.span)
                .with_rule_id("SEM-MSCONS-MELO-FORMAT")
                .with_segment("LOC")
                .with_suggestion(
                    "Messlokations-IDs in LOC+172 must be exactly 11 upper-case \
                     alphanumeric characters matching [A-Z0-9]{11}",
                ),
            );
        }
    }
}

/// `SEM-MSCONS-PERIOD-ORDER` — When both a period-start (`DTM+163`) and a
/// period-end (`DTM+164`) are present in the message, the start date must not
/// be lexicographically greater than the end date.
///
/// Date strings are in `YYYYMMDD` (format 102) or `YYYYMMDDHHmm` (format 203),
/// both of which sort chronologically as strings.
fn rule_sem_period_order(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    // Track both value and source span so we can point diagnostics at the
    // out-of-order start segment.
    let mut start: Option<(&str, edifact_rs::Span)> = None;
    let mut end: Option<(&str, edifact_rs::Span)> = None;

    for seg in segments.iter().filter(|s| s.tag == "DTM") {
        // DTM element[0] = C507 composite:
        //   component[0] = 2005 (date/time qualifier)
        //   component[1] = date/time value
        let Some(c507) = seg.get_element(0) else {
            continue;
        };
        let qualifier = c507.get_component(0).unwrap_or("");
        let value = c507.get_component(1).unwrap_or("");
        match qualifier {
            "163" => start = Some((value, seg.span)),
            "164" => end = Some((value, seg.span)),
            _ => {}
        }
    }

    if let (Some((start_val, start_span)), Some((end_val, _))) = (start, end) {
        if !start_val.is_empty() && !end_val.is_empty() && start_val > end_val {
            issues.push(
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    "DTM: period-start (qualifier 163) is after period-end (qualifier 164)"
                        .to_owned(),
                )
                .with_span(start_span)
                .with_rule_id("SEM-MSCONS-PERIOD-ORDER")
                .with_segment("DTM")
                .with_suggestion(
                    "Ensure DTM+163 (Beginn Lieferzeitraum) is not later than \
                     DTM+164 (Ende Lieferzeitraum) — date values must be in \
                     ascending chronological order",
                ),
            );
        }
    }
}

/// EDI@Energy approved unit-of-measure codes for MSCONS metering values.
///
/// Source: BDEW MSCONS Application Handbook, Appendix A — Code List 6411.
/// DE 6411 codes MSCONS admits.
///
/// `KWT` (Kilowatt), `D54` (Watt pro Quadratmeter) and `MTS` (Meter pro
/// Sekunde) are the codes MIG 2.5 lists for SG10 QTY; `D54` and `MTS` carry the
/// meteorological values of Redispatch 2.0 PID 13021, and `KWT` carries a power
/// maximum. Omitting them rejects messages the MIG defines.
const APPROVED_UNITS: &[&str] = &[
    "KWH", "MWH", "GWH", "KW", "KWT", "MW", "GW", "KVA", "MVA", "KVAR", "MVAR", "M3", "M3H", "HM3",
    "GJ", "MJ", "J", "D54", "MTS", "Z03", "Z12", "Z14",
];

/// `SEM-MSCONS-UNIT-UNKNOWN` — The unit-of-measure code in `QTY C186`
/// component 2 (data element 6411) must be from the EDI@Energy approved set.
fn rule_sem_unit_unknown(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    for seg in segments.iter().filter(|s| s.tag == "QTY") {
        // QTY element[0] = C186 composite:
        //   component[0] = 6063 (quantity type qualifier)
        //   component[1] = quantity value
        //   component[2] = 6411 (measure unit code)
        let unit = seg
            .get_element(0)
            .and_then(|e| e.get_component(2))
            .unwrap_or("");
        if unit.is_empty() {
            continue; // Unit is optional for some qualifier types.
        }
        if !APPROVED_UNITS.contains(&unit) {
            issues.push(
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    "QTY C186 component 2 (DE 6411): unit-of-measure code is not \
                     in the EDI@Energy approved set for MSCONS"
                        .to_owned(),
                )
                .with_span(seg.span)
                .with_rule_id("SEM-MSCONS-UNIT-UNKNOWN")
                .with_segment("QTY")
                .with_suggestion(
                    "Use one of the EDI@Energy MSCONS approved units (Code List 6411): \
                     KWH MWH GWH KW KWT MW GW KVA MVA KVAR MVAR M3 M3H HM3 GJ MJ J \
                     D54 MTS Z03 Z12 Z14",
                ),
            );
        }
    }
}
