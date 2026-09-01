//! Human oversight for the live GaBi Gas advisory path.
//!
//! `gabi-gas-agent` no longer claims a correction command that makod cannot
//! dispatch. It confirms the deterministic deadline record read-only and files
//! a triage row when the final ALOCAT is still missing.

use std::sync::Arc;

use agentplane::core::{OnExpiry, Priority, TaskState};
use agentplane::store::RedbStore;
use agentplane::testkit::FakeProvider;
use serde_json::{Value, json};

use agentd::plane::{Activation, Envelope, Plane, PlaneConfig, Stores};

const AGENT: &str = "gabi-gas-agent";
const AUDIENCE: &str = "gas-operations";

fn ce(id: &str) -> Envelope<'_> {
    Envelope {
        id,
        source: "urn:mako:test:tenant:9900357000004",
        event_type: mako_events::gabi::ALOCAT_MISSING,
    }
}

fn event() -> Value {
    json!({
        "gas_day": "2026-08-06",
        "sender_eic": "11XRWENET-----1E",
        "receiver_eic": "11YN00000000TH2M",
        "deadline_label": "gabi-final-allocation",
        "synthetic_pid": "13013"
    })
}

fn plane(provider: &Arc<FakeProvider>) -> Plane {
    let tenant = agentplane::core::TenantId::new("9900357000004").expect("tenant");
    let policy =
        agentd::plane::policy::engine(agentd::plane::policy::DEFAULT_POLICY).expect("policy");
    Plane::new(
        Stores::redb(RedbStore::open_in_memory().expect("store"), &tenant),
        PlaneConfig {
            owner: "agentd-oversight-test",
            tenant: &tenant,
            activated: &Activation::named(vec![AGENT.to_owned()]),
            providers: vec![(
                "anthropic".to_owned(),
                Arc::clone(provider) as Arc<dyn agentplane::model::ModelProvider>,
            )],
            tool_servers: Vec::new(),
            policy,
            keyring: None,
            outbox: None,
            signer: None,
            quota: agentplane::quota::TenantQuota::default(),
        },
    )
    .expect("plane")
}

async fn worklist(plane: &Plane, roles: &[&str]) -> Vec<agentplane::core::Task> {
    let roles = roles
        .iter()
        .map(|role| (*role).to_owned())
        .collect::<Vec<_>>();
    plane
        .runtime()
        .tasks()
        .expect("task store")
        .queue(&roles, 10)
        .await
        .expect("queue")
}

#[tokio::test]
async fn a_missing_final_alocat_completes_and_opens_urgent_triage() {
    let provider = FakeProvider::new();
    let plane = plane(&provider);

    let decision = plane
        .dispatch_one(AGENT, ce("ce-missing"), event())
        .await
        .expect("activated");

    assert_eq!(decision.outcome, "completed", "{}", decision.summary);
    assert_eq!(provider.calls(), 0, "coded triage must not call a model");
    let queue = worklist(&plane, &[AUDIENCE]).await;
    assert_eq!(queue.len(), 1, "the finding must reach gas operations");
    let task = &queue[0];
    assert_eq!(task.state, TaskState::Open);
    assert_eq!(task.priority, Priority::Urgent);
    assert_eq!(task.on_expiry, OnExpiry::Escalate);
    assert!(task.may_decide("user:anna", &[AUDIENCE.to_owned()]));
    assert!(!task.may_decide("user:mallory", &["billing-operations".to_owned()]));
}

#[tokio::test]
async fn a_malformed_event_fails_and_opens_no_triage_row() {
    let provider = FakeProvider::new();
    let plane = plane(&provider);
    let mut malformed = event();
    malformed
        .as_object_mut()
        .expect("object")
        .remove("deadline_label");

    let decision = plane
        .dispatch_one(AGENT, ce("ce-malformed"), malformed)
        .await
        .expect("activated");

    assert_eq!(decision.outcome, "failed");
    assert!(worklist(&plane, &[AUDIENCE]).await.is_empty());
}

#[tokio::test]
async fn the_oversight_surface_mounts_over_the_governed_plane() {
    let provider = FakeProvider::new();
    let plane = plane(&provider);
    let verifier = Arc::new(mako_service::oidc::OidcVerifier::disabled("9900357000004"));

    let _router = agentd::plane::oversight::router(plane.runtime(), verifier)
        .expect("the worklist mounts over a governed plane");
}
