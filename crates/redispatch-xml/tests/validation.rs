//! Targeted validation tests for Kaskade, Stammdaten, NetworkConstraintDocument,
//! and PlannedResourceScheduleDocument.
//!
//! These tests exercise the structural and semantic validation rules that are
//! *separate* from the parse/serialize round-trips covered by `round_trip.rs`.
//! They verify that conformant documents are accepted and that non-conformant
//! documents are rejected with the correct [`ValidationError`] variant.

use redispatch_xml::documents::activation::{
    ControlZoneRef, EicCodingScheme, ResourceObjectCodingScheme, ResourceObjectRef,
};
use redispatch_xml::documents::kaskade::{
    AvailablePeriod, BiddingZoneDomain, CurveType, Kaskade, KaskadeBusinessType, KaskadeMarketRole,
    KaskadeMeasureUnit, KaskadeParticipant, KaskadeReason, KaskadeReasonCode, KaskadeRoleType,
    KaskadeStatus, KaskadeTimeInterval, KaskadeTimeSeries, KaskadeType, QuantityMeasureUnit,
    StatusElement,
};
use redispatch_xml::documents::network_constraint::{
    NcdBusinessType, NcdDocStatus, NcdDocType, NcdProcessType, NetworkConstraintDocument,
    NetworkConstraintTimeSeries,
};
use redispatch_xml::documents::planned_resource_schedule::{
    GridElementCodingScheme, PlannedResourceScheduleDocument, PlannedResourceTimeSeries, Product,
    PrsBusinessType, PrsDocType, PrsProcessType,
};
use redispatch_xml::documents::stammdaten::{
    Codierung, Meldungsstatus, Stammdaten, StammdatenDocType, StammdatenParticipantRef,
    StammdatenReceiverRole, StammdatenSenderRole,
};
use redispatch_xml::types::{
    AttrV, AttrVWithScheme, CodingScheme, ControlZone, Decimal3, Direction, DocumentId,
    DocumentVersion, Interval, MarketParticipantId, MarketRoleType, MeasureUnit, Period,
    RevisionNumber, SimpleContent, TimeInterval, UtcDateTime, UtcMinuteDateTime,
};
use redispatch_xml::validation::{ValidationError, validate};
use redispatch_xml::{Document, serialize_as};
use time::macros::datetime;

// ── Shared helpers ────────────────────────────────────────────────────────────

fn sender() -> AttrVWithScheme<MarketParticipantId> {
    AttrVWithScheme {
        v: MarketParticipantId::new("4045399000008").unwrap(),
        coding_scheme: CodingScheme::Gs1,
    }
}

fn receiver() -> AttrVWithScheme<MarketParticipantId> {
    AttrVWithScheme {
        v: MarketParticipantId::new("4045399000015").unwrap(),
        coding_scheme: CodingScheme::Gs1,
    }
}

fn doc_id() -> AttrV<DocumentId> {
    AttrV {
        v: DocumentId::new("VAL-DOC-001").unwrap(),
    }
}

fn doc_version() -> AttrV<DocumentVersion> {
    AttrV {
        v: DocumentVersion::new(1).unwrap(),
    }
}

fn ts() -> UtcDateTime {
    UtcDateTime::new(datetime!(2025-10-01 06:00:00 UTC)).unwrap()
}

fn interval() -> AttrV<TimeInterval> {
    AttrV {
        v: TimeInterval::new(
            datetime!(2025-10-01 22:00:00 UTC),
            datetime!(2025-10-02 22:00:00 UTC),
        )
        .unwrap(),
    }
}

fn minute_dt(h: u8) -> UtcMinuteDateTime {
    UtcMinuteDateTime::new(datetime!(2025-10-01 00:00:00 UTC) + time::Duration::hours(h as i64))
        .unwrap()
}

fn mrid(s: &str) -> redispatch_xml::types::Mrid {
    DocumentId::new(s).unwrap()
}

fn revision(n: u32) -> RevisionNumber {
    DocumentVersion::new(n).unwrap()
}

fn period() -> Period {
    Period {
        time_interval: interval(),
        resolution: AttrV {
            v: "PT15M".to_string(),
        },
        intervals: vec![Interval {
            pos: AttrV { v: 1 },
            qty: AttrV {
                v: Decimal3::new(100.0).unwrap(),
            },
            reasons: vec![],
        }],
    }
}

fn control_zone() -> ControlZoneRef {
    AttrVWithScheme {
        v: ControlZone::TennetDe,
        coding_scheme: EicCodingScheme::Eic,
    }
}

fn resource_object() -> ResourceObjectRef {
    AttrVWithScheme {
        v: "RESOURCEOBJ001".to_string(),
        coding_scheme: ResourceObjectCodingScheme::Nde,
    }
}

// ── Kaskade ───────────────────────────────────────────────────────────────────

fn valid_kaskade() -> Kaskade {
    Kaskade {
        created_date_time: ts(),
        m_rid: mrid("KAS-VAL-001"),
        revision_number: revision(1),
        status: StatusElement {
            value: KaskadeStatus::Ordered,
        },
        doc_type: KaskadeType::EmergencyMeasures,
        sender_market_participant: KaskadeParticipant {
            m_rid: SimpleContent {
                value: "4045399000008".to_string(),
                coding_scheme: CodingScheme::Gs1,
            },
            market_role: KaskadeMarketRole {
                role_type: KaskadeRoleType::GridOperator,
            },
        },
        receiver_market_participant: KaskadeParticipant {
            m_rid: SimpleContent {
                value: "4045399000015".to_string(),
                coding_scheme: CodingScheme::Gs1,
            },
            market_role: KaskadeMarketRole {
                role_type: KaskadeRoleType::GridOperator,
            },
        },
        time_series: KaskadeTimeSeries {
            m_rid: mrid("KAS-TS-001"),
            senders_document_m_rid: None,
            senders_revision_number: None,
            senders_created_date_time: None,
            business_type: KaskadeBusinessType::Production,
            resource_objects: vec![],
            bidding_zone_domain: BiddingZoneDomain {
                m_rid: SimpleContent {
                    value: "10YDE-EON------1".to_string(),
                    coding_scheme: EicCodingScheme::Eic,
                },
            },
            quantity_measure_unit: QuantityMeasureUnit {
                name: KaskadeMeasureUnit::Megawatt,
            },
            curve_type: CurveType::VariableSizedBlock,
            available_period: AvailablePeriod {
                time_interval: KaskadeTimeInterval {
                    start: Some(minute_dt(6)),
                    end: minute_dt(7),
                },
                resolution: None,
                points: vec![],
            },
            reason: KaskadeReason {
                code: KaskadeReasonCode::LocalGridProblem,
                reason_text: None,
            },
        },
    }
}

#[test]
fn kaskade_valid_document_passes_validation() {
    let doc = Document::Kaskade(Box::new(valid_kaskade()));
    let result = validate(&doc);
    assert!(
        result.is_valid(),
        "expected valid Kaskade to pass: {:?}",
        result.errors
    );
}

#[test]
fn kaskade_revision_number_at_boundary_passes_validation() {
    // revision_number = 999 is the maximum valid value.
    let mut kas = valid_kaskade();
    kas.revision_number = revision(999);
    let doc = Document::Kaskade(Box::new(kas));
    let result = validate(&doc);
    assert!(
        result.is_valid(),
        "revision_number=999 should pass: {:?}",
        result.errors
    );
}

#[test]
fn kaskade_deactivation_status_passes_validation() {
    let mut kas = valid_kaskade();
    kas.status = StatusElement {
        value: KaskadeStatus::Deactivation,
    };
    let doc = Document::Kaskade(Box::new(kas));
    let result = validate(&doc);
    assert!(result.is_valid());
}

#[test]
fn kaskade_test_message_type_passes_validation() {
    let mut kas = valid_kaskade();
    kas.doc_type = KaskadeType::TestMessage;
    let doc = Document::Kaskade(Box::new(kas));
    let result = validate(&doc);
    assert!(result.is_valid());
}

#[test]
fn kaskade_round_trips_through_validate() {
    // Serialize -> parse -> validate: the full pipeline for a Kaskade document.
    let kas = valid_kaskade();
    let xml = serialize_as(&kas, true).unwrap();
    let back: Kaskade = redispatch_xml::parse_as(&xml).unwrap();
    let doc = Document::Kaskade(Box::new(back));
    let result = validate(&doc);
    assert!(result.is_valid(), "re-parsed Kaskade should still be valid");
}

// ── Stammdaten ────────────────────────────────────────────────────────────────

/// Returns a minimal valid `Stammdaten` deactivation (no SR_Objekte required).
fn valid_stammdaten_deactivation() -> Stammdaten {
    Stammdaten {
        document_identification: DocumentId::new("STAMM-VAL-001").unwrap(),
        document_type: StammdatenDocType::Reduced,
        erstellungszeitpunkt: ts(),
        sender: StammdatenParticipantRef {
            code: MarketParticipantId::new("4045399000008").unwrap(),
            codierung: Codierung::Gs1,
        },
        senderrolle: StammdatenSenderRole::ResourceProvider,
        empfaenger: StammdatenParticipantRef {
            code: MarketParticipantId::new("4045399000015").unwrap(),
            codierung: Codierung::Gs1,
        },
        empfaengerrolle: StammdatenReceiverRole::GridOperator,
        ref_dokument_id: None,
        original_sender: None,
        original_dokument_id: None,
        original_erstellungszeitpunkt: None,
        gueltig_ab: ts(),
        meldungsstatus: Meldungsstatus::Deactivation,
        sr_objekte: vec![],
    }
}

#[test]
fn stammdaten_deactivation_without_sr_objekte_passes_validation() {
    // Deactivation is explicitly allowed without SR_Objekte per the semantic rule.
    let doc = Document::Stammdaten(Box::new(valid_stammdaten_deactivation()));
    let result = validate(&doc);
    assert!(
        result.is_valid(),
        "Stammdaten deactivation without SR_Objekte should pass: {:?}",
        result.errors
    );
}

#[test]
fn stammdaten_creation_without_sr_objekte_fails_semantic_validation() {
    let mut doc = valid_stammdaten_deactivation();
    doc.meldungsstatus = Meldungsstatus::Creation;
    let doc = Document::Stammdaten(Box::new(doc));
    let result = validate(&doc);
    assert!(
        !result.is_valid(),
        "Stammdaten creation without SR_Objekte should fail"
    );
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, ValidationError::Semantic(_))),
        "expected a Semantic validation error, got: {:?}",
        result.errors
    );
}

#[test]
fn stammdaten_update_without_sr_objekte_fails_semantic_validation() {
    let mut doc = valid_stammdaten_deactivation();
    doc.meldungsstatus = Meldungsstatus::Update;
    let doc = Document::Stammdaten(Box::new(doc));
    let result = validate(&doc);
    assert!(
        !result.is_valid(),
        "Stammdaten update without SR_Objekte should fail"
    );
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, ValidationError::Semantic(_))),
        "expected a Semantic validation error, got: {:?}",
        result.errors
    );
}

#[test]
fn stammdaten_structural_validates_participant_ids() {
    // Valid participant IDs are exactly 13 decimal digits.
    let doc = Document::Stammdaten(Box::new(valid_stammdaten_deactivation()));
    let result = validate(&doc);
    assert!(
        result.is_valid(),
        "valid participant IDs should pass: {:?}",
        result.errors
    );
}

#[test]
fn stammdaten_round_trips_through_validate() {
    let stamm = valid_stammdaten_deactivation();
    let xml = serialize_as(&stamm, true).unwrap();
    let back: Stammdaten = redispatch_xml::parse_as(&xml).unwrap();
    let doc = Document::Stammdaten(Box::new(back));
    let result = validate(&doc);
    assert!(
        result.is_valid(),
        "re-parsed Stammdaten should still be valid"
    );
}

// ── NetworkConstraintDocument ─────────────────────────────────────────────────

fn valid_ncd_with_time_series() -> NetworkConstraintDocument {
    let ncd_ts = NetworkConstraintTimeSeries {
        time_series_identification: doc_id(),
        business_type: AttrV {
            v: NcdBusinessType::ProductionDispatchable,
        },
        direction: AttrV { v: Direction::Up },
        connecting_area: control_zone(),
        resource_object: resource_object(),
        grid_element: AttrVWithScheme {
            v: "GRID-ELEM-001".to_string(),
            coding_scheme: GridElementCodingScheme::Eic,
        },
        measurement_unit: AttrV {
            v: MeasureUnit::Megawatt,
        },
        status: None,
        period: period(),
    };
    NetworkConstraintDocument {
        document_identification: doc_id(),
        document_version: doc_version(),
        document_type: AttrV {
            v: NcdDocType::NetworkConstraint,
        },
        process_type: AttrV {
            v: NcdProcessType::Forecast,
        },
        sender_identification: sender(),
        sender_role: AttrV {
            v: MarketRoleType::GridOperator,
        },
        receiver_identification: receiver(),
        receiver_role: AttrV {
            v: MarketRoleType::ResourceProvider,
        },
        document_date_time: AttrV { v: ts() },
        time_period_covered: interval(),
        doc_status: None,
        time_series: vec![ncd_ts],
    }
}

#[test]
fn ncd_with_time_series_passes_validation() {
    let doc = Document::NetworkConstraint(Box::new(valid_ncd_with_time_series()));
    let result = validate(&doc);
    assert!(
        result.is_valid(),
        "NCD with time series should pass: {:?}",
        result.errors
    );
}

#[test]
fn ncd_without_time_series_and_no_doc_status_fails_semantic_validation() {
    let mut ncd = valid_ncd_with_time_series();
    ncd.time_series.clear();
    ncd.doc_status = None;
    let doc = Document::NetworkConstraint(Box::new(ncd));
    let result = validate(&doc);
    assert!(
        !result.is_valid(),
        "NCD without time series and no docStatus should fail"
    );
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, ValidationError::Semantic(_))),
        "expected a Semantic error, got: {:?}",
        result.errors
    );
}

#[test]
fn ncd_withdrawal_with_doc_status_passes_validation() {
    let mut ncd = valid_ncd_with_time_series();
    ncd.time_series.clear();
    ncd.doc_status = Some(NcdDocStatus {
        v: "A13".to_string(),
    });
    let doc = Document::NetworkConstraint(Box::new(ncd));
    let result = validate(&doc);
    assert!(
        result.is_valid(),
        "NCD withdrawal (docStatus=A13, no time series) should pass: {:?}",
        result.errors
    );
}

#[test]
fn ncd_round_trips_through_validate() {
    let ncd = valid_ncd_with_time_series();
    let xml = serialize_as(&ncd, true).unwrap();
    let back: NetworkConstraintDocument = redispatch_xml::parse_as(&xml).unwrap();
    let doc = Document::NetworkConstraint(Box::new(back));
    let result = validate(&doc);
    assert!(result.is_valid(), "re-parsed NCD should still be valid");
}

// ── PlannedResourceScheduleDocument ──────────────────────────────────────────

fn valid_prs_with_time_series() -> PlannedResourceScheduleDocument {
    let prs_ts = PlannedResourceTimeSeries {
        time_series_identification: doc_id(),
        business_type: AttrV {
            v: PrsBusinessType::Production,
        },
        direction: Some(AttrV { v: Direction::Up }),
        connecting_area: Some(control_zone()),
        resource_object: None,
        product: AttrV {
            v: Product::ActivePower,
        },
        acquiring_area: None,
        grid_element: None,
        measure_unit: AttrV {
            v: MeasureUnit::Megawatt,
        },
        status: None,
        resource_provider: None,
        period: period(),
    };
    PlannedResourceScheduleDocument {
        document_identification: doc_id(),
        document_version: doc_version(),
        document_type: AttrV {
            v: PrsDocType::DayAheadPlan,
        },
        process_type: AttrV {
            v: PrsProcessType::Forecast,
        },
        sender_identification: sender(),
        sender_role: AttrV {
            v: MarketRoleType::ResourceProvider,
        },
        receiver_identification: receiver(),
        receiver_role: AttrV {
            v: MarketRoleType::GridOperator,
        },
        document_date_time: AttrV { v: ts() },
        time_period_covered: interval(),
        time_series: vec![prs_ts],
    }
}

#[test]
fn prs_with_time_series_passes_validation() {
    let doc = Document::PlannedResourceSchedule(Box::new(valid_prs_with_time_series()));
    let result = validate(&doc);
    assert!(
        result.is_valid(),
        "PRS with time series should pass: {:?}",
        result.errors
    );
}

#[test]
fn prs_without_time_series_fails_semantic_validation() {
    let mut prs = valid_prs_with_time_series();
    prs.time_series.clear();
    let doc = Document::PlannedResourceSchedule(Box::new(prs));
    let result = validate(&doc);
    assert!(
        !result.is_valid(),
        "PlannedResourceScheduleDocument with no time series should fail"
    );
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, ValidationError::Semantic(_))),
        "expected a Semantic error, got: {:?}",
        result.errors
    );
}

#[test]
fn prs_round_trips_through_validate() {
    let prs = valid_prs_with_time_series();
    let xml = serialize_as(&prs, true).unwrap();
    let back: PlannedResourceScheduleDocument = redispatch_xml::parse_as(&xml).unwrap();
    let doc = Document::PlannedResourceSchedule(Box::new(back));
    let result = validate(&doc);
    assert!(result.is_valid(), "re-parsed PRS should still be valid");
}
