//! Round-trip tests: serialize a document, parse it back, check identity.
//!
//! These tests are smoke tests for the serialization/deserialization pipeline.
//! They do not validate business rules — see `validation.rs` for those.

use redispatch_xml::documents::acknowledgement::{
    AckReason, AckReasonCode, AcknowledgementDocument,
};
use redispatch_xml::documents::activation::{
    ActivationDocType, ActivationDocument, ActivationProcessType, ActivationTimeSeries,
    ControlZoneRef, EicCodingScheme, ResourceObjectCodingScheme, ResourceObjectRef,
    TimeSeriesBusinessType, TimeSeriesStatus,
};
use redispatch_xml::documents::kaskade::{
    AvailablePeriod, BiddingZoneDomain, CurveType, Kaskade, KaskadeBusinessType, KaskadeMarketRole,
    KaskadeMeasureUnit, KaskadeParticipant, KaskadeReason, KaskadeReasonCode, KaskadeRoleType,
    KaskadeStatus, KaskadeTimeInterval, KaskadeTimeSeries, KaskadeType, QuantityMeasureUnit,
    StatusElement,
};
use redispatch_xml::documents::kostenblatt::{
    CostBusinessType, CostTimeSeries, Kostenblatt, KostenblattDocType, KostenblattProcessType,
};
use redispatch_xml::documents::network_constraint::{
    NcdBusinessType, NcdProcessType, NetworkConstraintDocument, NetworkConstraintTimeSeries,
};
use redispatch_xml::documents::planned_resource_schedule::{
    GridElementCodingScheme, PlannedResourceScheduleDocument, PlannedResourceTimeSeries, Product,
    PrsBusinessType, PrsDocType, PrsProcessType,
};
use redispatch_xml::documents::stammdaten::{
    Codierung, Meldungsstatus, Stammdaten, StammdatenDocType, StammdatenParticipantRef,
    StammdatenReceiverRole, StammdatenSenderRole,
};
use redispatch_xml::documents::status_request::{
    StatusRequestDocType, StatusRequestMarketDocument, StatusRequestReceiver,
    StatusRequestReceiverMarketRole, StatusRequestReceiverRole, StatusRequestSender,
    StatusRequestSenderMarketRole, StatusRequestSenderRole,
};
use redispatch_xml::documents::unavailability::{
    UnavailabilityDocType, UnavailabilityMarketDocument, UnavailabilityMarketRole,
    UnavailabilityMarketRoleType, UnavailabilityParticipant, UnavailabilityProcessType,
    UnavailabilityTimeInterval, UnavailabilityTimePeriod,
};
use redispatch_xml::types::{
    AttrV, AttrVWithScheme, CodingScheme, ControlZone, Decimal3, Direction, DocumentId,
    DocumentVersion, Interval, MarketParticipantId, MarketRoleType, MeasureUnit, Mrid, Period,
    RevisionNumber, SimpleContent, TimeInterval, UtcDateTime, UtcMinuteDateTime,
};
use time::macros::datetime;

// ── Shared helpers ────────────────────────────────────────────────────────────

fn sample_sender() -> AttrVWithScheme<MarketParticipantId> {
    AttrVWithScheme {
        v: MarketParticipantId::new("4045399000008").unwrap(),
        coding_scheme: CodingScheme::Gs1,
    }
}

fn sample_receiver() -> AttrVWithScheme<MarketParticipantId> {
    AttrVWithScheme {
        v: MarketParticipantId::new("4045399000015").unwrap(),
        coding_scheme: CodingScheme::Gs1,
    }
}

fn sample_doc_id() -> AttrV<DocumentId> {
    AttrV {
        v: DocumentId::new("DOC-0001").unwrap(),
    }
}

fn sample_doc_version() -> AttrV<DocumentVersion> {
    AttrV {
        v: DocumentVersion::new(1).unwrap(),
    }
}

fn sample_ts() -> UtcDateTime {
    UtcDateTime::new(datetime!(2025-10-01 06:00:00 UTC)).unwrap()
}

fn sample_interval() -> AttrV<TimeInterval> {
    AttrV {
        v: TimeInterval::new(
            datetime!(2025-10-01 22:00:00 UTC),
            datetime!(2025-10-02 22:00:00 UTC),
        )
        .unwrap(),
    }
}

fn sample_period() -> Period {
    Period {
        time_interval: sample_interval(),
        resolution: AttrV {
            v: "PT15M".to_string(),
        },
        intervals: vec![Interval {
            pos: AttrV { v: 1 },
            qty: AttrV {
                v: Decimal3::new(50.0).unwrap(),
            },
            reasons: vec![],
        }],
    }
}

fn sample_control_zone() -> ControlZoneRef {
    AttrVWithScheme {
        v: ControlZone::TennetDe,
        coding_scheme: EicCodingScheme::Eic,
    }
}

fn sample_resource_object() -> ResourceObjectRef {
    AttrVWithScheme {
        v: "RESOURCEOBJ001".to_string(),
        coding_scheme: ResourceObjectCodingScheme::Nde,
    }
}

fn sample_mrid(s: &str) -> Mrid {
    DocumentId::new(s).unwrap()
}

fn sample_revision() -> RevisionNumber {
    DocumentVersion::new(1).unwrap()
}

fn sample_participant_mrid(code: &str) -> SimpleContent<String> {
    SimpleContent {
        value: code.to_string(),
        coding_scheme: CodingScheme::Gs1,
    }
}

fn sample_minute_dt() -> UtcMinuteDateTime {
    UtcMinuteDateTime::new(datetime!(2025-10-01 22:00:00 UTC)).unwrap()
}

// ── Acknowledgement round-trip ────────────────────────────────────────────────

#[test]
fn acknowledgement_round_trip() {
    let doc = AcknowledgementDocument {
        document_identification: sample_doc_id(),
        document_date_time: AttrV { v: sample_ts() },
        sender_identification: sample_sender(),
        sender_role: AttrV {
            v: MarketRoleType::GridOperator,
        },
        receiver_identification: sample_receiver(),
        receiver_role: AttrV {
            v: MarketRoleType::ResourceProvider,
        },
        receiving_document_identification: None,
        receiving_document_version: None,
        receiving_document_type: None,
        receiving_payload_name: None,
        date_time_receiving_document: None,
        time_series_rejections: vec![],
        reasons: vec![AckReason {
            code: AttrV {
                v: AckReasonCode::FullyAccepted,
            },
            text: None,
        }],
    };

    let xml = redispatch_xml::serialize_as(&doc, true).unwrap();
    let xml_str = std::str::from_utf8(&xml).unwrap();
    assert!(xml_str.contains("AcknowledgementDocument"));
    assert!(xml_str.contains("4045399000008"));

    let back: AcknowledgementDocument = redispatch_xml::parse_as(&xml).unwrap();
    assert_eq!(back.document_identification.v.as_str(), "DOC-0001");
    assert_eq!(back.sender_identification.v.as_str(), "4045399000008");
}

// ── ActivationDocument round-trip ─────────────────────────────────────────────

fn sample_activation_ts() -> ActivationTimeSeries {
    ActivationTimeSeries {
        allocation_identification: sample_doc_id(),
        resource_provider: None,
        business_type: AttrV {
            v: TimeSeriesBusinessType::SystemOperatorRedispatching,
        },
        acquiring_area: AttrVWithScheme {
            v: "10YCB-GERMANY--8".to_string(),
            coding_scheme: EicCodingScheme::Eic,
        },
        connecting_area: sample_control_zone(),
        measure_unit: AttrV {
            v: MeasureUnit::Megawatt,
        },
        direction: AttrV { v: Direction::Up },
        status: AttrV {
            v: TimeSeriesStatus::Ordered,
        },
        resource_object: sample_resource_object(),
        senders_document_identification: None,
        senders_document_version: None,
        senders_document_date_time: None,
        senders_time_series_identification: None,
        original_sender_identification: None,
        original_document_identification: None,
        original_document_version: None,
        original_document_date_time: None,
        original_allocation_identification: None,
        period: sample_period(),
    }
}

#[test]
fn activation_document_round_trip() {
    let doc = ActivationDocument {
        document_identification: sample_doc_id(),
        document_version: sample_doc_version(),
        document_type: AttrV {
            v: ActivationDocType::RedispatchActivation,
        },
        process_type: AttrV {
            v: ActivationProcessType::Redispatch,
        },
        sender_identification: sample_sender(),
        sender_role: AttrV {
            v: MarketRoleType::GridOperator,
        },
        receiver_identification: sample_receiver(),
        receiver_role: AttrV {
            v: MarketRoleType::ResourceProvider,
        },
        creation_date_time: AttrV { v: sample_ts() },
        activation_time_interval: sample_interval(),
        order_identification: None,
        order_identification_version: None,
        time_series: vec![sample_activation_ts()],
        reason: None,
    };

    let xml = redispatch_xml::serialize_as(&doc, true).unwrap();
    let xml_str = std::str::from_utf8(&xml).unwrap();
    assert!(xml_str.contains("ActivationDocument"));

    let back: ActivationDocument = redispatch_xml::parse_as(&xml).unwrap();
    assert_eq!(back.document_identification.v.as_str(), "DOC-0001");
    assert_eq!(back.time_series.len(), 1);
}

// ── PlannedResourceScheduleDocument round-trip ───────────────────────────────

#[test]
fn planned_resource_schedule_round_trip() {
    let ts = PlannedResourceTimeSeries {
        time_series_identification: sample_doc_id(),
        business_type: AttrV {
            v: PrsBusinessType::Production,
        },
        direction: Some(AttrV { v: Direction::Up }),
        connecting_area: Some(sample_control_zone()),
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
        period: sample_period(),
    };
    let doc = PlannedResourceScheduleDocument {
        document_identification: sample_doc_id(),
        document_version: sample_doc_version(),
        document_type: AttrV {
            v: PrsDocType::DayAheadPlan,
        },
        process_type: AttrV {
            v: PrsProcessType::Forecast,
        },
        sender_identification: sample_sender(),
        sender_role: AttrV {
            v: MarketRoleType::ResourceProvider,
        },
        receiver_identification: sample_receiver(),
        receiver_role: AttrV {
            v: MarketRoleType::GridOperator,
        },
        document_date_time: AttrV { v: sample_ts() },
        time_period_covered: sample_interval(),
        time_series: vec![ts],
    };

    let xml = redispatch_xml::serialize_as(&doc, true).unwrap();
    let xml_str = std::str::from_utf8(&xml).unwrap();
    assert!(xml_str.contains("PlannedResourceScheduleDocument"));
    assert!(xml_str.contains("8716867000016"));

    let back: PlannedResourceScheduleDocument = redispatch_xml::parse_as(&xml).unwrap();
    assert_eq!(back.document_identification.v.as_str(), "DOC-0001");
    assert_eq!(back.time_series.len(), 1);
    assert_eq!(back.time_series[0].product.v, Product::ActivePower);
}

// ── NetworkConstraintDocument round-trip ─────────────────────────────────────

#[test]
fn network_constraint_round_trip() {
    use redispatch_xml::documents::network_constraint::NcdDocType;

    let ts = NetworkConstraintTimeSeries {
        time_series_identification: sample_doc_id(),
        business_type: AttrV {
            v: NcdBusinessType::ProductionDispatchable,
        },
        direction: AttrV { v: Direction::Up },
        connecting_area: sample_control_zone(),
        resource_object: sample_resource_object(),
        grid_element: AttrVWithScheme {
            v: "GRID-ELM-001".to_string(),
            coding_scheme: GridElementCodingScheme::Eic,
        },
        measurement_unit: AttrV {
            v: MeasureUnit::Megawatt,
        },
        status: None,
        period: sample_period(),
    };
    let doc = NetworkConstraintDocument {
        document_identification: sample_doc_id(),
        document_version: sample_doc_version(),
        document_type: AttrV {
            v: NcdDocType::NetworkConstraint,
        },
        process_type: AttrV {
            v: NcdProcessType::Forecast,
        },
        sender_identification: sample_sender(),
        sender_role: AttrV {
            v: MarketRoleType::GridOperator,
        },
        receiver_identification: sample_receiver(),
        receiver_role: AttrV {
            v: MarketRoleType::ResourceProvider,
        },
        document_date_time: AttrV { v: sample_ts() },
        time_period_covered: sample_interval(),
        doc_status: None,
        time_series: vec![ts],
    };

    let xml = redispatch_xml::serialize_as(&doc, true).unwrap();
    let xml_str = std::str::from_utf8(&xml).unwrap();
    assert!(xml_str.contains("NetworkConstraintDocument"));
    assert!(xml_str.contains("GRID-ELM-001"));

    let back: NetworkConstraintDocument = redispatch_xml::parse_as(&xml).unwrap();
    assert_eq!(back.document_identification.v.as_str(), "DOC-0001");
    assert_eq!(back.time_series.len(), 1);
}

// ── Kostenblatt round-trip ────────────────────────────────────────────────────

#[test]
fn kostenblatt_round_trip() {
    let ts = CostTimeSeries {
        time_series_identification: sample_doc_id(),
        business_type: AttrV {
            v: CostBusinessType::ProductionEnergy,
        },
        direction: Some(AttrV { v: Direction::Up }),
        product: AttrV {
            v: Product::ActivePower,
        },
        connecting_area: Some(sample_control_zone()),
        resource_object: Some(sample_resource_object()),
        period: sample_period(),
    };
    let doc = Kostenblatt {
        document_identification: sample_doc_id(),
        document_version: sample_doc_version(),
        document_type: AttrV {
            v: KostenblattDocType::Kostenblatt,
        },
        process_type: AttrV {
            v: KostenblattProcessType::Forecast,
        },
        sender_identification: sample_sender(),
        sender_role: AttrV {
            v: MarketRoleType::GridOperator,
        },
        receiver_identification: sample_receiver(),
        receiver_role: AttrV {
            v: MarketRoleType::ResourceProvider,
        },
        document_date_time: AttrV { v: sample_ts() },
        time_period_covered: sample_interval(),
        time_series: vec![ts],
    };

    let xml = redispatch_xml::serialize_as(&doc, true).unwrap();
    let xml_str = std::str::from_utf8(&xml).unwrap();
    assert!(xml_str.contains("Kostenblatt"));
    assert!(xml_str.contains("Z05"));

    let back: Kostenblatt = redispatch_xml::parse_as(&xml).unwrap();
    assert_eq!(back.document_identification.v.as_str(), "DOC-0001");
    assert_eq!(back.time_series.len(), 1);
}

// ── Stammdaten round-trip ─────────────────────────────────────────────────────

#[test]
fn stammdaten_round_trip() {
    let doc = Stammdaten {
        document_identification: DocumentId::new("STAMM-0001").unwrap(),
        document_type: StammdatenDocType::Reduced,
        erstellungszeitpunkt: UtcDateTime::new(datetime!(2025-10-01 06:00:00 UTC)).unwrap(),
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
        gueltig_ab: UtcDateTime::new(datetime!(2025-10-01 00:00:00 UTC)).unwrap(),
        meldungsstatus: Meldungsstatus::Creation,
        sr_objekte: vec![],
    };

    let xml = redispatch_xml::serialize_as(&doc, true).unwrap();
    let xml_str = std::str::from_utf8(&xml).unwrap();
    assert!(xml_str.contains("Stammdaten"));
    assert!(xml_str.contains("4045399000008"));
    assert!(xml_str.contains("Z02")); // Reduced doc type

    let back: Stammdaten = redispatch_xml::parse_as(&xml).unwrap();
    assert_eq!(back.document_identification.as_str(), "STAMM-0001");
    assert_eq!(back.sender.code.as_str(), "4045399000008");
    assert_eq!(back.meldungsstatus, Meldungsstatus::Creation);
}

// ── StatusRequest_MarketDocument round-trip ───────────────────────────────────

#[test]
fn status_request_round_trip() {
    let doc = StatusRequestMarketDocument {
        m_rid: sample_mrid("SR-DOC-001"),
        doc_type: StatusRequestDocType::StatusRequest,
        sender_market_participant: StatusRequestSender {
            m_rid: sample_participant_mrid("4045399000008"),
            market_role: StatusRequestSenderMarketRole {
                role_type: StatusRequestSenderRole::GridOperator,
            },
        },
        receiver_market_participant: StatusRequestReceiver {
            m_rid: sample_participant_mrid("4045399000015"),
            market_role: StatusRequestReceiverMarketRole {
                role_type: StatusRequestReceiverRole::ResourceProvider,
            },
        },
        created_date_time: sample_ts(),
        attributes: vec![],
        mkt_activity_records: vec![],
    };

    let xml = redispatch_xml::serialize_as(&doc, true).unwrap();
    let xml_str = std::str::from_utf8(&xml).unwrap();
    assert!(xml_str.contains("StatusRequest_MarketDocument"));
    assert!(xml_str.contains("4045399000008"));

    let back: StatusRequestMarketDocument = redispatch_xml::parse_as(&xml).unwrap();
    assert_eq!(back.m_rid.as_str(), "SR-DOC-001");
    assert_eq!(back.sender_market_participant.m_rid.value, "4045399000008");
}

// ── Unavailability_MarketDocument round-trip ──────────────────────────────────

#[test]
fn unavailability_round_trip() {
    let doc = UnavailabilityMarketDocument {
        m_rid: sample_mrid("UNAV-DOC-001"),
        revision_number: sample_revision(),
        doc_type: UnavailabilityDocType::PlannedUnavailability,
        process_type: UnavailabilityProcessType::Forecast,
        created_date_time: sample_ts(),
        sender_market_participant: UnavailabilityParticipant {
            m_rid: sample_participant_mrid("4045399000008"),
            market_role: UnavailabilityMarketRole {
                role_type: UnavailabilityMarketRoleType::ResourceProvider,
            },
        },
        receiver_market_participant: UnavailabilityParticipant {
            m_rid: sample_participant_mrid("4045399000015"),
            market_role: UnavailabilityMarketRole {
                role_type: UnavailabilityMarketRoleType::GridOperator,
            },
        },
        unavailability_time_period: UnavailabilityTimePeriod {
            time_interval: UnavailabilityTimeInterval {
                start: sample_minute_dt(),
                end: UtcMinuteDateTime::new(datetime!(2025-10-02 22:00:00 UTC)).unwrap(),
            },
        },
        doc_status: None,
        time_series: vec![],
    };

    let xml = redispatch_xml::serialize_as(&doc, true).unwrap();
    let xml_str = std::str::from_utf8(&xml).unwrap();
    assert!(xml_str.contains("Unavailability_MarketDocument"));
    assert!(xml_str.contains("A67")); // PlannedUnavailability

    let back: UnavailabilityMarketDocument = redispatch_xml::parse_as(&xml).unwrap();
    assert_eq!(back.m_rid.as_str(), "UNAV-DOC-001");
    assert_eq!(back.doc_type, UnavailabilityDocType::PlannedUnavailability);
}

// ── Kaskade round-trip ────────────────────────────────────────────────────────

#[test]
fn kaskade_round_trip() {
    let doc = Kaskade {
        created_date_time: sample_ts(),
        m_rid: sample_mrid("KAS-DOC-001"),
        revision_number: sample_revision(),
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
            m_rid: sample_mrid("KAS-TS-001"),
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
                    start: Some(sample_minute_dt()),
                    end: UtcMinuteDateTime::new(datetime!(2025-10-02 22:00:00 UTC)).unwrap(),
                },
                resolution: None,
                points: vec![],
            },
            reason: KaskadeReason {
                code: KaskadeReasonCode::LocalGridProblem,
                reason_text: None,
            },
        },
    };

    let xml = redispatch_xml::serialize_as(&doc, true).unwrap();
    let xml_str = std::str::from_utf8(&xml).unwrap();
    assert!(xml_str.contains("Kaskade"));
    assert!(xml_str.contains("Z16")); // EmergencyMeasures

    let back: Kaskade = redispatch_xml::parse_as(&xml).unwrap();
    assert_eq!(back.m_rid.as_str(), "KAS-DOC-001");
    assert_eq!(back.doc_type, KaskadeType::EmergencyMeasures);
}

// ── parse_and_validate convenience ───────────────────────────────────────────

#[test]
fn parse_and_validate_rejects_invalid_xml() {
    let result = redispatch_xml::parse_and_validate(b"<not valid xml");
    assert!(result.is_err());
}

#[test]
fn parse_and_validate_accepts_valid_activation() {
    // Build an ActivationDocument, serialize it, then manually inject the required namespace
    // since quick-xml serde does not emit xmlns from struct definitions.
    let doc = ActivationDocument {
        document_identification: sample_doc_id(),
        document_version: sample_doc_version(),
        document_type: AttrV {
            v: ActivationDocType::RedispatchActivation,
        },
        process_type: AttrV {
            v: ActivationProcessType::Redispatch,
        },
        sender_identification: sample_sender(),
        sender_role: AttrV {
            v: MarketRoleType::GridOperator,
        },
        receiver_identification: sample_receiver(),
        receiver_role: AttrV {
            v: MarketRoleType::ResourceProvider,
        },
        creation_date_time: AttrV { v: sample_ts() },
        activation_time_interval: sample_interval(),
        order_identification: None,
        order_identification_version: None,
        time_series: vec![sample_activation_ts()],
        reason: None,
    };

    let xml = redispatch_xml::serialize_as(&doc, true).unwrap();
    let xml_str = std::str::from_utf8(&xml).unwrap();
    // Inject the expected xmlns so parse_and_validate's namespace check passes.
    let xml_with_ns = xml_str.replace(
        "<ActivationDocument",
        "<ActivationDocument xmlns=\"urn:entsoe.eu:wgedi:errp:activationdocument:5:0\"",
    );
    let result = redispatch_xml::parse_and_validate(xml_with_ns.as_bytes());
    assert!(result.is_ok(), "expected valid document: {result:?}");
}

// ── From<T> for Document conversions ─────────────────────────────────────────

#[test]
fn from_impl_for_document() {
    let ack = AcknowledgementDocument {
        document_identification: sample_doc_id(),
        document_date_time: AttrV { v: sample_ts() },
        sender_identification: sample_sender(),
        sender_role: AttrV {
            v: MarketRoleType::GridOperator,
        },
        receiver_identification: sample_receiver(),
        receiver_role: AttrV {
            v: MarketRoleType::ResourceProvider,
        },
        receiving_document_identification: None,
        receiving_document_version: None,
        receiving_document_type: None,
        receiving_payload_name: None,
        date_time_receiving_document: None,
        time_series_rejections: vec![],
        reasons: vec![],
    };
    let doc = redispatch_xml::Document::from(ack);
    assert!(matches!(doc, redispatch_xml::Document::Acknowledgement(_)));
}

// ── ValidationResult Display ──────────────────────────────────────────────────

#[test]
fn validation_result_display_ok() {
    let result = redispatch_xml::validation::ValidationResult::default();
    assert_eq!(result.to_string(), "ok");
}

// ── AttrV ergonomics ──────────────────────────────────────────────────────────

#[test]
fn attr_v_from_impl() {
    let v: AttrV<u32> = AttrV::from(42u32);
    assert_eq!(v.v, 42);
}

#[test]
fn attr_v_display() {
    let v = AttrV::new("hello".to_string());
    assert_eq!(v.to_string(), "hello");
}

// ── TimeInterval Display ──────────────────────────────────────────────────────

#[test]
fn time_interval_display() {
    let iv = TimeInterval::new(
        datetime!(2025-10-01 22:00:00 UTC),
        datetime!(2025-10-02 22:00:00 UTC),
    )
    .unwrap();
    assert_eq!(iv.to_string(), "2025-10-01T22:00Z/2025-10-02T22:00Z");
}

// ── DocumentId ergonomics ─────────────────────────────────────────────────────

#[test]
fn document_id_try_from() {
    let id = DocumentId::try_from("TEST-DOC-1").unwrap();
    assert_eq!(id.as_str(), "TEST-DOC-1");
    assert_eq!(id.as_ref(), "TEST-DOC-1");

    let from_string = DocumentId::try_from("TEST-DOC-2".to_string()).unwrap();
    assert_eq!(from_string.as_str(), "TEST-DOC-2");

    assert!(DocumentId::try_from("").is_err());
    assert!(DocumentId::try_from("X".repeat(36).as_str()).is_err());
}

// ── MarketParticipantId ergonomics ────────────────────────────────────────────

#[test]
fn market_participant_id_try_from() {
    let id = MarketParticipantId::try_from("4045399000008").unwrap();
    assert_eq!(id.as_str(), "4045399000008");
    assert_eq!(id.as_ref(), "4045399000008");

    let from_string = MarketParticipantId::try_from("4045399000015".to_string()).unwrap();
    assert_eq!(from_string.as_str(), "4045399000015");

    assert!(MarketParticipantId::try_from("123456789012").is_err()); // 12 digits
    assert!(MarketParticipantId::try_from("12345678901234").is_err()); // 14 digits
    assert!(MarketParticipantId::try_from("404539900000X").is_err()); // non-digit
}

// ── Namespace-checked round trip ─────────────────────────────────────────────

/// `parse(serialize(doc))` must survive the namespace-checked path — the
/// serializer injects the XSD's default `xmlns`, so a serialized document is
/// a valid wire document, not just `parse_as`-compatible.
#[test]
fn serialized_activation_survives_the_namespace_checked_parse() {
    let doc = ActivationDocument {
        document_identification: sample_doc_id(),
        document_version: sample_doc_version(),
        document_type: AttrV {
            v: ActivationDocType::RedispatchActivation,
        },
        process_type: AttrV {
            v: ActivationProcessType::Redispatch,
        },
        sender_identification: sample_sender(),
        sender_role: AttrV {
            v: MarketRoleType::GridOperator,
        },
        receiver_identification: sample_receiver(),
        receiver_role: AttrV {
            v: MarketRoleType::ResourceProvider,
        },
        creation_date_time: AttrV { v: sample_ts() },
        activation_time_interval: sample_interval(),
        order_identification: None,
        order_identification_version: None,
        time_series: vec![sample_activation_ts()],
        reason: None,
    };

    let xml = redispatch_xml::serialize(&redispatch_xml::Document::from(doc)).unwrap();
    let xml_str = std::str::from_utf8(&xml).unwrap();
    assert!(
        xml_str.contains("xmlns=\"urn:entsoe.eu:wgedi:errp:activationdocument:5:0\""),
        "serializer must emit the ERRP namespace, got: {}",
        &xml_str[..200.min(xml_str.len())]
    );

    // The strict, namespace-checked entry point — not parse_as.
    let back = redispatch_xml::parse(&xml).expect("namespace-checked parse succeeds");
    assert!(matches!(back, redispatch_xml::Document::Activation(_)));
}
