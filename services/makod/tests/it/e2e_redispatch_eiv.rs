//! End to end: mako as Einsatzverantwortlicher against mako as
//! Anschlussnetzbetreiber in the role of the Data Provider.
//!
//! ```text
//!   EIV (tenant A)                                    ANB as DP (tenant B)
//!   ──────────────────────────────────────────────────────────────────────
//!   redispatch.eiv.planungsdaten.senden ─ PRSD A14 ─► redispatch-planungsdaten
//!   (Bestaetigt) ◄──────────── AcknowledgementDocument A01
//!   (Klaerfall)  ◄──────────── AcknowledgementDocument A02 + Z16
//!   redispatch-eiv-abruf ◄──── ActivationDocument A96 (Aufforderungsfall)
//!   redispatch.eiv.abruf.freigeben (a person) ─ A41 ─►
//!   (Bestaetigt) ◄──────────── AcknowledgementDocument A01
//! ```
//!
//! Both sides share one store with a tenant each. The documents are built
//! through `redispatch-xml` and pass its validation on both sides; the
//! `ReceiverRole` is what sends each to the right side.

use std::sync::Arc;

use mako_engine::envelope::Akteur;
use mako_engine::ids::TenantId;
use mako_engine::outbox::OutboxStore;
use mako_engine::process::Process;
use mako_engine::registry::ProcessRegistry;
use mako_engine::store_slatedb::SlateDbStore;
use mako_engine::workflow::Workflow;
use mako_redispatch::eiv;
use makod::commands_api::{CommandsApiState, DispatchOutcome, dispatch_command};
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};
use redispatch_xml::Document;
use redispatch_xml::documents::acknowledgement::{
    AckReason, AckReasonCode, AcknowledgementDocument,
};
use redispatch_xml::documents::activation::{
    ActivationDocType, ActivationDocument, ActivationProcessType, ActivationTimeSeries,
    EicCodingScheme, ResourceObjectCodingScheme, TimeSeriesBusinessType, TimeSeriesStatus,
};
use redispatch_xml::documents::planned_resource_schedule::{
    PlannedResourceScheduleDocument, PlannedResourceTimeSeries, Product, PrsBusinessType,
    PrsDocType, PrsProcessType,
};
use redispatch_xml::types::{
    AttrV, AttrVWithScheme, CodingScheme, ControlZone, Decimal3, Direction, DocumentId,
    DocumentVersion, Interval, MarketParticipantId, MarketRoleType, MeasureUnit, Period,
    TimeInterval, UtcDateTime,
};
use time::macros::datetime;

const EIV_MP_ID: &str = "9900000000003";
const ANB_MP_ID: &str = "9900000000010";
const SR_ID: &str = "C1234567890";

fn mp(id: &str) -> AttrVWithScheme<MarketParticipantId> {
    AttrVWithScheme {
        v: MarketParticipantId::new(id).unwrap(),
        coding_scheme: CodingScheme::Nde,
    }
}

fn doc_id(id: &str) -> AttrV<DocumentId> {
    AttrV {
        v: DocumentId::new(id).unwrap(),
    }
}

fn interval() -> AttrV<TimeInterval> {
    AttrV {
        v: TimeInterval::new(
            datetime!(2026-11-21 23:00:00 UTC),
            datetime!(2026-11-22 23:00:00 UTC),
        )
        .unwrap(),
    }
}

fn period() -> Period {
    Period {
        time_interval: interval(),
        resolution: AttrV {
            v: "PT15M".to_owned(),
        },
        intervals: vec![Interval {
            pos: AttrV { v: 1 },
            qty: AttrV {
                v: Decimal3::new(2.5).unwrap(),
            },
            reasons: vec![],
        }],
    }
}

fn now() -> AttrV<UtcDateTime> {
    AttrV {
        v: UtcDateTime::new(datetime!(2026-11-21 10:00:00 UTC)).unwrap(),
    }
}

fn xml(doc: impl Into<Document>) -> String {
    String::from_utf8(redispatch_xml::serialize(&doc.into()).unwrap()).unwrap()
}

/// The EIV's plan for its SR, to the DP (`PlannedResourceScheduleDocument`
/// AWT 1.0f S. 3, Prozessschritt 1).
fn plan(mrid: &str) -> String {
    xml(PlannedResourceScheduleDocument {
        original_sender_identification: None,
        original_document_identification: None,
        original_document_version: None,
        original_document_date_time: None,
        document_identification: doc_id(mrid),
        document_version: AttrV {
            v: DocumentVersion::new(1).unwrap(),
        },
        document_type: AttrV {
            v: PrsDocType::DayAheadPlan,
        },
        process_type: AttrV {
            v: PrsProcessType::Forecast,
        },
        sender_identification: mp(EIV_MP_ID),
        sender_role: AttrV {
            v: MarketRoleType::ResourceProvider,
        },
        receiver_identification: mp(ANB_MP_ID),
        receiver_role: AttrV {
            v: MarketRoleType::DataProvider,
        },
        document_date_time: now(),
        time_period_covered: interval(),
        time_series: vec![PlannedResourceTimeSeries {
            original_time_series_identification: None,
            requesting_grid_operator: None,
            time_series_identification: doc_id("TS-1"),
            business_type: AttrV {
                v: PrsBusinessType::Production,
            },
            direction: None,
            connecting_area: Some(AttrVWithScheme {
                v: ControlZone::TennetDe,
                coding_scheme: EicCodingScheme::Eic,
            }),
            resource_object: None,
            product: AttrV {
                v: Product::ActivePower,
            },
            acquiring_area: None,
            grid_element: None,
            measurement_unit: AttrV {
                v: MeasureUnit::Megawatt,
            },
            status: None,
            resource_provider: None,
            period: period(),
        }],
    })
}

/// The DP's acknowledgement of `quittiert`, to the EIV.
fn quittung(mrid: &str, quittiert: &str, angenommen: bool) -> String {
    let reasons = if angenommen {
        vec![AckReason {
            code: AttrV {
                v: AckReasonCode::FullyAccepted,
            },
            text: None,
        }]
    } else {
        vec![
            AckReason {
                code: AttrV {
                    v: AckReasonCode::FullyRejected,
                },
                text: None,
            },
            AckReason {
                code: AttrV {
                    v: AckReasonCode::NotPermitted,
                },
                text: Some("nicht zulässig gemäß AWT".to_owned()),
            },
        ]
    };
    xml(AcknowledgementDocument {
        document_identification: doc_id(mrid),
        document_date_time: now(),
        sender_identification: mp(ANB_MP_ID),
        sender_role: AttrV {
            v: MarketRoleType::DataProvider,
        },
        receiver_identification: mp(EIV_MP_ID),
        receiver_role: AttrV {
            v: MarketRoleType::ResourceProvider,
        },
        receiving_document_identification: Some(doc_id(quittiert)),
        receiving_document_version: None,
        receiving_document_type: None,
        receiving_payload_name: None,
        date_time_receiving_document: None,
        time_series_rejections: vec![],
        reasons,
    })
}

/// An `ActivationDocument` between the DP and the EIV.
fn aktivierung(
    mrid: &str,
    typ: ActivationDocType,
    status: TimeSeriesStatus,
    (sender, sender_role): (&str, MarketRoleType),
    (receiver, receiver_role): (&str, MarketRoleType),
) -> String {
    xml(ActivationDocument {
        schedule_time_series: vec![],
        document_identification: doc_id(mrid),
        document_version: AttrV {
            v: DocumentVersion::new(1).unwrap(),
        },
        document_type: AttrV { v: typ },
        process_type: AttrV {
            v: ActivationProcessType::Redispatch,
        },
        sender_identification: mp(sender),
        sender_role: AttrV { v: sender_role },
        receiver_identification: mp(receiver),
        receiver_role: AttrV { v: receiver_role },
        creation_date_time: now(),
        activation_time_interval: interval(),
        order_identification: None,
        order_identification_version: None,
        time_series: vec![ActivationTimeSeries {
            allocation_identification: doc_id("AL-1"),
            resource_provider: None,
            business_type: AttrV {
                v: TimeSeriesBusinessType::InternalRedispatch,
            },
            acquiring_area: AttrVWithScheme {
                v: "10YCB-GERMANY--8".to_owned(),
                coding_scheme: EicCodingScheme::Eic,
            },
            connecting_area: AttrVWithScheme {
                v: ControlZone::TennetDe,
                coding_scheme: EicCodingScheme::Eic,
            },
            measure_unit: AttrV {
                v: MeasureUnit::Megawatt,
            },
            direction: AttrV { v: Direction::Down },
            status: AttrV { v: status },
            resource_object: AttrVWithScheme {
                v: SR_ID.to_owned(),
                coding_scheme: ResourceObjectCodingScheme::Nde,
            },
            senders_document_identification: None,
            senders_document_version: None,
            senders_document_date_time: None,
            senders_time_series_identification: None,
            original_sender_identification: None,
            original_document_identification: None,
            original_document_version: None,
            original_document_date_time: None,
            original_allocation_identification: None,
            period: period(),
        }],
        reason: None,
    })
}

fn command_state(store: &Arc<SlateDbStore>, tenant: TenantId) -> CommandsApiState {
    CommandsApiState {
        tenant_id: tenant,
        sender_party_id: EIV_MP_ID.to_owned(),
        configured_marktrollen: vec!["EIV".to_owned()],
        max_body_bytes: 1_048_576,
        snapshot_interval: 100,
        cedar: Arc::new(
            makod::cedar_authz::CedarAuthorizer::unauthenticated().expect("infallible"),
        ),
        snapshot_store: store.as_snapshot_store(),
        malo_cache: Arc::new(makod::malo_cache::SlateDbMaloCache::new(
            (**store).clone(),
            tenant,
        )),
        maloid_result_cache: makod::malo_cache::MaloIdentResultCache::new((**store).clone()),
        store: Arc::clone(store),
        marktd_client: None,
    }
}

async fn state_of<W>(store: &Arc<SlateDbStore>, tenant: TenantId, key: &str) -> W::State
where
    W: Workflow + WorkflowName,
    W::State: serde::Serialize,
{
    let identity = store
        .as_process_registry()
        .lookup_correlated(tenant, key)
        .await
        .unwrap()
        .into_iter()
        .find(|i| i.workflow_id.name.as_ref() == <W as WorkflowName>::NAME)
        .unwrap_or_else(|| panic!("no process under {key}"));
    Process::<W, Arc<SlateDbStore>>::from_identity(Arc::clone(store), identity)
        .state()
        .await
        .unwrap()
}

/// The name each workflow's processes run under, for [`state_of`].
trait WorkflowName {
    const NAME: &'static str;
}
impl WorkflowName for eiv::PlanungsdatenSendungWorkflow {
    const NAME: &'static str = eiv::planungsdaten::WORKFLOW_NAME;
}
impl WorkflowName for eiv::AbrufWorkflow {
    const NAME: &'static str = eiv::abruf::WORKFLOW_NAME;
}

#[tokio::test]
async fn mako_as_eiv_against_mako_as_anb() {
    let store = Arc::new(SlateDbStore::open_in_memory().await.expect("store"));
    let tenant_eiv = TenantId::from_party_id(EIV_MP_ID);
    let tenant_anb = TenantId::from_party_id(ANB_MP_ID);
    let eiv_state = command_state(&store, tenant_eiv);
    let eiv_ingest = EdifactIngestDispatcher::new(
        Arc::clone(&store),
        store.as_snapshot_store(),
        100,
        tenant_eiv,
    )
    .with_eiv_antwortfenster(Some(time::Duration::minutes(10)));
    let anb_ingest = EdifactIngestDispatcher::new(
        Arc::clone(&store),
        store.as_snapshot_store(),
        100,
        tenant_anb,
    );
    let ingest = |d: &EdifactIngestDispatcher, xml: String| {
        let d = d.clone();
        async move {
            makod::redispatch_xml_ingest::dispatch_redispatch_xml(&d, xml.as_bytes())
                .await
                .expect("ingest")
        }
    };

    // 1. The plan goes out, validated, and reaches the ANB's own workflow.
    let out = dispatch_command(
        &eiv_state,
        Akteur::System,
        "redispatch.eiv.planungsdaten.senden",
        &serde_json::json!({ "dokument": plan("PRSD-1") }),
    )
    .await
    .expect("the plan is accepted");
    assert!(matches!(out, DispatchOutcome::Spawned { .. }), "{out:?}");
    let at_anb = ingest(&anb_ingest, plan("PRSD-1")).await;
    assert!(
        matches!(&at_anb, IngestOutcome::Spawned { workflow_name, .. } if *workflow_name == "redispatch-planungsdaten"),
        "{at_anb:?}"
    );

    // 2. A positive acknowledgement records it.
    let back = ingest(&eiv_ingest, quittung("ACK-1", "PRSD-1", true)).await;
    assert!(matches!(back, IngestOutcome::Dispatched { .. }), "{back:?}");
    assert!(matches!(
        state_of::<eiv::PlanungsdatenSendungWorkflow>(&store, tenant_eiv, "PRSD-1").await,
        eiv::SendungState::Bestaetigt { .. }
    ));

    // 3. A negative one opens a Klärfall naming the document and the reason.
    dispatch_command(
        &eiv_state,
        Akteur::System,
        "redispatch.eiv.planungsdaten.senden",
        &serde_json::json!({ "dokument": plan("PRSD-2") }),
    )
    .await
    .expect("the second plan is accepted");
    ingest(&eiv_ingest, quittung("ACK-2", "PRSD-2", false)).await;
    assert!(matches!(
        state_of::<eiv::PlanungsdatenSendungWorkflow>(&store, tenant_eiv, "PRSD-2").await,
        eiv::SendungState::Klaerfall {
            klaerung: eiv::Klaerung::Abgelehnt { .. },
            ..
        }
    ));
    let pending = OutboxStore::pending(&*store, 100, time::OffsetDateTime::now_utc())
        .await
        .unwrap();
    let notice = pending
        .iter()
        .find(|m| &*m.message_type == mako_engine::outbox::PROCESS_FAILED)
        .expect("the Klärfall is announced");
    let reason = notice.payload["reason"].as_str().unwrap();
    assert!(
        reason.contains("PRSD-2") && reason.contains("Z16"),
        "{reason}"
    );

    // 4. The plan is refused when it is not the EIV's to send.
    let fremd = plan("PRSD-3").replace(r#"<SenderRole v="A27"/>"#, r#"<SenderRole v="A18"/>"#);
    assert!(
        dispatch_command(
            &eiv_state,
            Akteur::System,
            "redispatch.eiv.planungsdaten.senden",
            &serde_json::json!({ "dokument": fremd }),
        )
        .await
        .is_err()
    );

    // 5. An activation addressed to the EIV waits for a person — and a
    //    deployment that is no EIV does not take it for its own.
    let aco = aktivierung(
        "ACO-1",
        ActivationDocType::RedispatchActivation,
        TimeSeriesStatus::Ordered,
        (ANB_MP_ID, MarketRoleType::DataProvider),
        (EIV_MP_ID, MarketRoleType::ResourceProvider),
    );
    let not_ours = ingest(&anb_ingest, aco.clone()).await;
    assert!(
        matches!(not_ours, IngestOutcome::Skipped { .. }),
        "a Netzbetreiber deployment answered as EIV: {not_ours:?}"
    );
    let opened = ingest(&eiv_ingest, aco).await;
    assert!(
        matches!(opened, IngestOutcome::Spawned { .. }),
        "{opened:?}"
    );
    assert!(matches!(
        state_of::<eiv::AbrufWorkflow>(&store, tenant_eiv, "ACO-1").await,
        eiv::AbrufState::EntscheidungOffen(_)
    ));

    // 6. Nothing but a person releases the Rückmeldung zur Umsetzbarkeit.
    let rueckmeldung = aktivierung(
        "ACR-1",
        ActivationDocType::ActivationResponse,
        TimeSeriesStatus::Available,
        (EIV_MP_ID, MarketRoleType::ResourceProvider),
        (ANB_MP_ID, MarketRoleType::DataProvider),
    );
    let body = serde_json::json!({ "abruf_mrid": "ACO-1", "dokument": rueckmeldung });
    assert!(
        dispatch_command(
            &eiv_state,
            Akteur::System,
            "redispatch.eiv.abruf.freigeben",
            &body
        )
        .await
        .is_err(),
        "an unattended caller answered the activation"
    );
    let released = dispatch_command(
        &eiv_state,
        Akteur::Person {
            principal: "disponentin".to_owned(),
        },
        "redispatch.eiv.abruf.freigeben",
        &body,
    )
    .await
    .expect("a person releases it");
    assert!(
        matches!(released, DispatchOutcome::Dispatched { .. }),
        "{released:?}"
    );
    match state_of::<eiv::AbrufWorkflow>(&store, tenant_eiv, "ACO-1").await {
        eiv::AbrufState::Freigegeben(akte) => assert_eq!(
            akte.freigabe.unwrap().freigegeben_von,
            "disponentin",
            "the release names the person"
        ),
        other => panic!("{other:?}"),
    }

    // 7. The DP's acknowledgement of the Rückmeldung closes the activation.
    let back = ingest(&eiv_ingest, quittung("ACK-3", "ACR-1", true)).await;
    assert!(matches!(back, IngestOutcome::Dispatched { .. }), "{back:?}");
    assert!(matches!(
        state_of::<eiv::AbrufWorkflow>(&store, tenant_eiv, "ACO-1").await,
        eiv::AbrufState::Bestaetigt { .. }
    ));
}
