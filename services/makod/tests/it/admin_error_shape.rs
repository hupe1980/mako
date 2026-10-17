//! The admin and read routes fail with the shared problem body.
//!
//! A failure of makod's own is a 500 that carries an `error_id` and nothing of
//! the cause: the cause names streams, keys and process ids, and the
//! identifier is what finds its log line. A caller's mistake is a sentence in
//! `detail` under the status's own `error`.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use secrecy::SecretString;
use tower::ServiceExt as _;

use mako_engine::{
    envelope::NewEvent,
    event_store::{AppendBatch, AtomicAppend as _, ExpectedVersion},
    ids::{ConversationId, CorrelationId, ProcessId, StreamId, TenantId},
    store_slatedb::SlateDbStore,
    version::WorkflowId,
};
use makod::cedar_authz::{CedarAuthorizer, DefaultPolicy, NamedKey};
use makod::invoic_api::{self, InvoicApiState};

const TENANT: &str = "9900357000004";

fn authorizer() -> Arc<CedarAuthorizer> {
    Arc::new(
        CedarAuthorizer::new(
            vec![NamedKey {
                name: Arc::from("operator"),
                token: SecretString::new("s3cret".to_owned().into()),
            }],
            None,
            None,
            None,
            DefaultPolicy::PermitAll,
        )
        .expect("authorizer construction"),
    )
}

/// A store holding one process whose `WimInvoicReceived` carries no
/// `rechnung` — a stream the adapter never writes.
async fn malformed_invoic_stream() -> (Arc<SlateDbStore>, TenantId, ProcessId) {
    let store = SlateDbStore::open_in_memory()
        .await
        .expect("in-memory store");
    let tenant = TenantId::from_party_id(TENANT);
    let process = ProcessId::from_uuid(uuid::Uuid::new_v4());
    let stream = StreamId::for_process(tenant, &process);
    store
        .append(AppendBatch::new(
            &stream,
            ExpectedVersion::Any,
            &[NewEvent::new(
                CorrelationId::new(),
                None,
                ConversationId::new(),
                process,
                tenant,
                WorkflowId::new("wim-invoic", "FV2026-04-01"),
                "WimInvoicReceived",
                1,
                serde_json::json!({ "data": {} }),
            )],
        ))
        .await
        .expect("append");
    (Arc::new(store), tenant, process)
}

async fn get(app: axum::Router, uri: &str, token: Option<&str>) -> (StatusCode, serde_json::Value) {
    let mut req = Request::builder().uri(uri);
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let resp = app
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .expect("answered");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).expect("a JSON problem body"),
    )
}

async fn invoic_app() -> (axum::Router, ProcessId) {
    let (store, tenant_id, process) = malformed_invoic_stream().await;
    let app = invoic_api::router(Arc::new(InvoicApiState {
        store,
        tenant_id,
        cedar: authorizer(),
        tenant: TENANT.to_owned(),
    }));
    (app, process)
}

#[tokio::test]
async fn a_malformed_stream_is_a_500_with_an_error_id_and_no_cause() {
    let (app, process) = invoic_app().await;
    let (status, body) = get(
        app,
        &format!("/api/v1/invoic/{process}/rechnung"),
        Some("s3cret"),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(body["detail"], "internal server error", "{body}");
    assert!(
        body["error_id"]
            .as_str()
            .is_some_and(|id| id.parse::<uuid::Uuid>().is_ok()),
        "{body}"
    );
    let text = body.to_string();
    assert!(
        !text.contains(&process.to_string()) && !text.contains("WimInvoicReceived"),
        "the cause stays in the log: {body}"
    );
}

#[tokio::test]
async fn a_caller_mistake_is_a_sentence_under_the_status() {
    let (app, process) = invoic_app().await;
    let (status, body) = get(
        app.clone(),
        &format!("/api/v1/invoic/{process}/rechnung"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"], "Unauthorized", "{body}");

    let (status, body) = get(
        app.clone(),
        "/api/v1/invoic/not-a-uuid/rechnung",
        Some("s3cret"),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(
        body["detail"].as_str().is_some_and(|d| d.contains("UUID")),
        "{body}"
    );

    let (status, body) = get(
        app,
        &format!("/api/v1/invoic/{}/rechnung", uuid::Uuid::new_v4()),
        Some("s3cret"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "Not Found", "{body}");
}
