//! The parse → route pipeline at the transport boundary: a wire document
//! entering over AS4 must resolve to exactly one workflow.
//!
//! makod is the only crate that depends on both halves of the Redispatch
//! boundary (`redispatch-xml` format + `mako-redispatch` engine), so the
//! canonical `DocumentType → RedispatchDocumentKind` mapping and this
//! integration test live here. The mapping is asserted for **all nine**
//! document types so a new type in `redispatch-xml` cannot ship without a
//! routing decision.

use mako_redispatch::{RedispatchDocumentKind, RedispatchModule};
use makod::redispatch_xml_ingest::document_kind;
use redispatch_xml::documents::DocumentType;

/// End-to-end for the hard-real-time document: raw ActivationDocument XML →
/// namespace-checked parse → kind → router → the Aktivierung workflow.
#[test]
fn activation_xml_routes_to_the_aktivierung_workflow() {
    let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<ActivationDocument xmlns="urn:entsoe.eu:wgedi:errp:activationdocument:5:0">
  <DocumentIdentification v="DOC-0001"/>
  <DocumentVersion v="1"/>
  <DocumentType v="A96"/>
  <ProcessType v="A41"/>
  <SenderIdentification v="4012345000001" codingScheme="A10"/>
  <SenderRole v="A18"/>
  <ReceiverIdentification v="4012345000002" codingScheme="A10"/>
  <ReceiverRole v="A27"/>
  <CreationDateTime v="2026-01-15T10:00:00Z"/>
  <ActivationTimeInterval v="2026-01-15T10:00Z/2026-01-15T11:00Z"/>
</ActivationDocument>"#;

    let doc = redispatch_xml::parse(xml).expect("namespace-checked parse");
    let kind = document_kind(doc.document_type());
    assert_eq!(kind, RedispatchDocumentKind::Activation);

    let router = RedispatchModule::build_router();
    let workflow = router.route(kind).expect("activation is routed");
    assert_eq!(workflow, "redispatch-aktivierung");
}

/// Every document type maps and — except the correlation-routed
/// AcknowledgementDocument — routes to a workflow.
#[test]
fn all_nine_document_types_have_a_routing_decision() {
    let router = RedispatchModule::build_router();
    let cases = [
        (DocumentType::Activation, Some("redispatch-aktivierung")),
        (
            DocumentType::PlannedResourceSchedule,
            Some("redispatch-planungsdaten"),
        ),
        // Routed by correlation key (ReceivingDocumentIdentification),
        // deliberately NOT in the type router.
        (DocumentType::Acknowledgement, None),
        (DocumentType::Stammdaten, Some("redispatch-stammdaten")),
        (
            DocumentType::StatusRequest,
            Some("redispatch-statusanfrage"),
        ),
        (
            DocumentType::Unavailability,
            Some("redispatch-verfuegbarkeit"),
        ),
        (DocumentType::Kaskade, Some("redispatch-kaskade")),
        (
            DocumentType::NetworkConstraint,
            Some("redispatch-netzengpass"),
        ),
        (DocumentType::Kostenblatt, Some("redispatch-kostenblatt")),
    ];
    for (dt, expected) in cases {
        let kind = document_kind(dt);
        match expected {
            Some(wf) => assert_eq!(
                router
                    .route(kind)
                    .unwrap_or_else(|_| panic!("{dt:?} must route")),
                wf,
                "{dt:?}"
            ),
            None => assert!(
                router.route(kind).is_err(),
                "{dt:?} must be correlation-routed, not type-routed"
            ),
        }
    }
}
