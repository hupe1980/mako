//! Regression coverage for MSCONS 13027 PID routing through SG1 `RFF+Z13`.

use std::sync::Arc;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use mako_engine::{dead_letter::LogDeadLetterSink, ids::TenantId, pid_router::PidRouter};
use secrecy::SecretString;
use tower::ServiceExt as _;

use makod::{
    cedar_authz::{CedarAuthorizer, DefaultPolicy, NamedKey},
    config::PartyConfig,
    edifact_api::{EdifactApiState, router},
    party_registry::MpIdRegistry,
};

const FIXTURE: &[u8] = include_bytes!("fixtures/mscons_13027_rff_z13.edi");
const ESA_MP_ID: &str = "9905550000005";

fn state() -> Arc<EdifactApiState> {
    let mut pid_router = PidRouter::new();
    pid_router.register(13027, "gpke-messwerte");
    let mp_id_registry = MpIdRegistry::from_config(&[
        PartyConfig {
            mp_id: ESA_MP_ID.to_owned(),
            roles: vec!["ESA".to_owned()],
            primary: true,
            agency: None,
        },
        PartyConfig {
            mp_id: "9900357000004".to_owned(),
            roles: vec!["MSB".to_owned()],
            primary: false,
            agency: None,
        },
    ])
    .expect("valid party registry");

    Arc::new(EdifactApiState {
        platform: Arc::new(edi_energy::Platform::with_all_profiles()),
        pid_router,
        mp_id_registry: Arc::new(mp_id_registry),
        cedar: Arc::new(
            CedarAuthorizer::new(
                vec![NamedKey {
                    name: Arc::from("regression-client"),
                    token: SecretString::new("regression-token".to_owned().into()),
                }],
                None,
                None,
                None,
                DefaultPolicy::PermitAll,
            )
            .expect("authorizer construction"),
        ),
        max_body_bytes: 1_048_576,
        partner_store: None,
        tenant_id: TenantId::from_party_id(ESA_MP_ID),
        dl_sink: Arc::new(LogDeadLetterSink),
        dispatcher: None,
        contrl_ack: None,
    })
}

#[tokio::test]
async fn conformant_13027_routes_by_rff_z13_with_a_document_number_in_bgm() {
    let response = router(state())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/edifact")
                .header("authorization", "Bearer regression-token")
                .header("content-type", "application/edifact")
                .body(Body::from(FIXTURE))
                .expect("request construction"),
        )
        .await
        .expect("HTTP service call");

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let result: serde_json::Value = serde_json::from_slice(&body).expect("JSON response");

    assert_eq!(result["accepted"], 1);
    assert_eq!(result["rejected"], 0);
    assert_eq!(result["messages"][0]["message_type"], "MSCONS");
    assert_eq!(result["messages"][0]["pid"], 13027);
    assert_eq!(result["messages"][0]["workflow"], "gpke-messwerte");
    assert_eq!(result["messages"][0]["status"], "routed");
}
