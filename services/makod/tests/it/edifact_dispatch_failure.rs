//! `POST /edifact` reports a dispatch failure and dead-letters it.
//!
//! A message that parses and routes but whose workflow command fails has not
//! landed: there is no process and no § 147 AO record of it unless the door
//! writes one. Answered `status: routed, error: null`, the gateway takes it as
//! delivered and never redelivers it. The AS4 door dead-letters the same
//! failure, so both doors must agree.

use std::sync::{Arc, Mutex};

use http_body_util::BodyExt as _;
use mako_engine::dead_letter::{DeadLetterReason, DeadLetterSink};
use mako_engine::{ids::TenantId, store_slatedb::SlateDbStore};
use makod::ingest_dispatcher::EdifactIngestDispatcher;
use tower::ServiceExt as _;

const OWN_MP: &str = "9900001000001";
const PID: u32 = 55001;
const WORKFLOW: &str = "gpke-supplier-change";

/// A dead-letter sink that keeps what it was given.
#[derive(Default)]
struct CapturingSink(Mutex<Vec<String>>);

impl DeadLetterSink for CapturingSink {
    fn reject<'a>(
        &'a self,
        reason: &'a DeadLetterReason,
    ) -> mako_engine::dead_letter::RejectFuture<'a> {
        self.0
            .lock()
            .expect("not poisoned")
            .push(format!("{reason:?}"));
        Box::pin(std::future::ready(Ok(())))
    }
}

fn fixture() -> String {
    use edi_energy::profile::SkeletonParties;
    let code = edi_energy::Pruefidentifikator::new(PID).expect("five digits");
    let profile = edi_energy::ReleaseRegistry::global()
        .profiles_for(edi_energy::MessageType::Utilmd)
        .filter(|p| p.has_anwendungsfall(code))
        .max_by_key(|p| p.valid_from())
        .unwrap_or_else(|| panic!("no UTILMD profile carries {PID}"));
    let af = profile.anwendungsfall(PID).expect("carried");
    let bytes = profile
        .skeleton_interchange(
            af,
            &SkeletonParties {
                sender: "4012345000023".to_owned(),
                receiver: OWN_MP.to_owned(),
            },
        )
        .unwrap_or_else(|e| panic!("skeleton for {PID}: {e}"));
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Post the fixture to a door whose store refuses every append, so its
/// command fails after parsing and routing succeeded.
async fn post_with_failing_store(
    sink: Arc<dyn DeadLetterSink>,
) -> (axum::http::StatusCode, serde_json::Value) {
    // A store that refuses every append: the spawn the message asks for fails
    // in the store, after parsing and routing succeeded.
    let store = SlateDbStore::open_in_memory()
        .await
        .expect("in-memory store")
        .with_max_stream_events(0);
    let tenant = TenantId::from_party_id(OWN_MP);
    let dispatcher = Arc::new(EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        tenant,
    ));
    let mut pid_router = mako_engine::pid_router::PidRouter::new();
    pid_router.register(PID, WORKFLOW);
    let state = Arc::new(makod::edifact_api::EdifactApiState {
        platform: Arc::new(edi_energy::Platform::with_all_profiles()),
        pid_router,
        mp_id_registry: Arc::new(
            makod::party_registry::MpIdRegistry::from_config(&[makod::config::PartyConfig {
                mp_id: OWN_MP.to_owned(),
                roles: vec!["NB".to_owned()],
                primary: true,
                agency: None,
            }])
            .expect("valid registry"),
        ),
        cedar: Arc::new(
            makod::cedar_authz::CedarAuthorizer::unauthenticated().expect("infallible"),
        ),
        max_body_bytes: 1024 * 1024,
        partner_store: None,
        tenant_id: tenant,
        dl_sink: sink,
        dispatcher: Some(dispatcher),
        contrl_ack: None,
    });

    let response = makod::edifact_api::router(state)
        .oneshot(
            axum::http::Request::post("/edifact")
                .header("content-type", "application/edifact")
                .body(axum::body::Body::from(fixture()))
                .expect("request"),
        )
        .await
        .expect("infallible");
    let status = response.status();
    let body: serde_json::Value = serde_json::from_slice(
        &response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("JSON");

    (status, body)
}

#[tokio::test]
async fn a_dispatch_failure_is_reported_and_dead_lettered() {
    let sink = Arc::new(CapturingSink::default());
    let (status, body) =
        post_with_failing_store(Arc::clone(&sink) as Arc<dyn DeadLetterSink>).await;

    let message = &body["messages"][0];
    assert_eq!(
        message["status"], "dispatch_failed",
        "a message whose command failed did not land; got {body}"
    );
    assert!(
        message["error"].as_str().is_some_and(|e| !e.is_empty()),
        "the response names the failure; got {body}"
    );
    assert_eq!(body["accepted"], 0, "{body}");
    assert_eq!(body["rejected"], 1, "{body}");
    assert_eq!(status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);

    let letters = sink.0.lock().expect("not poisoned");
    assert!(
        letters.iter().any(|l| l.contains("dispatch_failed")),
        "the failure is dead-lettered; got {letters:?}"
    );
}

/// A sink whose every write fails, as a closed or unreachable store does.
struct FailingSink;

impl DeadLetterSink for FailingSink {
    fn reject<'a>(
        &'a self,
        _reason: &'a DeadLetterReason,
    ) -> mako_engine::dead_letter::RejectFuture<'a> {
        Box::pin(std::future::ready(Err(
            mako_engine::error::EngineError::dead_letter("store closed"),
        )))
    }
}

/// A refusal whose dead letter could not be written has no record, so the door
/// must not answer it as handled: the caller is told the request failed and
/// resubmits it.
#[tokio::test]
async fn an_unrecorded_dead_letter_fails_the_request() {
    let (status, body) = post_with_failing_store(Arc::new(FailingSink)).await;
    assert_eq!(
        status,
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "the dead letter was not written; got {body}"
    );
}
