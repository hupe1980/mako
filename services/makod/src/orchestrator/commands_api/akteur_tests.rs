//! Every command names who acted: the authenticated caller, or — only for a
//! service Cedar grants `ActOnBehalf` — the actor it states in `Mako-Akteur`.

use std::sync::Arc;

use http_body_util::BodyExt as _;
use mako_engine::envelope::{Akteur, Vorschlag};
use mako_engine::event_store::EventStore as _;
use mako_engine::ids::{ProcessId, StreamId};
use secrecy::SecretString;
use tower::ServiceExt as _;

use super::spawn_tests::{anmeldung_55001, lf_state, malo_record, outbox_len};
use super::*;
use crate::cedar_authz::{DefaultPolicy, NamedKey};

/// `sachbearbeiterin` a person's key, `erp` a service without the grant,
/// `processd` the service the operator granted `ActOnBehalf`.
fn authorizer() -> Arc<CedarAuthorizer> {
    let key = |name: &str| NamedKey {
        name: Arc::from(name),
        token: SecretString::new(format!("tok-{name}").into()),
    };
    Arc::new(
        CedarAuthorizer::new(
            vec![key("sachbearbeiterin"), key("erp"), key("processd")],
            Some(
                r#"permit(
  principal == MaKo::Principal::"processd",
  action    == MaKo::Action::"ActOnBehalf",
  resource
);"#
                .to_owned(),
            ),
            None,
            None,
            DefaultPolicy::PermitAll,
        )
        .expect("policies parse"),
    )
}

async fn state() -> Arc<CommandsApiState> {
    let mut state = lf_state().await;
    state.cedar = authorizer();
    state
        .malo_cache
        .upsert(&malo_record())
        .await
        .expect("seed MaLo cache");
    Arc::new(state)
}

async fn post(
    state: &Arc<CommandsApiState>,
    caller: &str,
    akteur: Option<&serde_json::Value>,
) -> (axum::http::StatusCode, serde_json::Value) {
    let mut req = axum::http::Request::post("/api/v1/commands")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer tok-{caller}"))
        .header("idempotency-key", format!("akteur-{caller}"));
    if let Some(akteur) = akteur {
        req = req.header(AKTEUR_HEADER, akteur.to_string());
    }
    let res = router(Arc::clone(state))
        .oneshot(
            req.body(axum::body::Body::from(
                serde_json::json!({
                    "command": "gpke.lieferbeginn.anmelden",
                    "payload": anmeldung_55001(),
                })
                .to_string(),
            ))
            .expect("request"),
        )
        .await
        .expect("answer");
    let status = res.status();
    let body = res.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null),
    )
}

async fn actors(state: &CommandsApiState, body: &serde_json::Value) -> Vec<Akteur> {
    let process = ProcessId::from_uuid(
        body["process_id"]
            .as_str()
            .expect("a process id")
            .parse()
            .expect("uuid"),
    );
    state
        .store
        .load(&StreamId::for_process(state.tenant_id, &process))
        .await
        .expect("load")
        .into_iter()
        .map(|e| e.akteur)
        .collect()
}

#[tokio::test]
async fn a_desk_command_is_attributed_to_its_person() {
    let state = state().await;
    let (status, body) = post(&state, "sachbearbeiterin", None).await;
    assert_eq!(status, axum::http::StatusCode::ACCEPTED, "{body}");
    let actors = actors(&state, &body).await;
    assert!(!actors.is_empty());
    assert!(
        actors.iter().all(|a| *a
            == Akteur::Person {
                principal: "sachbearbeiterin".to_owned()
            }),
        "{actors:?}"
    );
}

#[tokio::test]
async fn a_forged_actor_is_refused_and_nothing_dispatches() {
    let state = state().await;
    let forged = serde_json::json!({
        "principal": "chefin",
        "vorschlag": { "art": "regel", "name": "anmeldung" },
        "unveraendert": true,
    });
    let (status, body) = post(&state, "erp", Some(&forged)).await;
    assert_eq!(status, axum::http::StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "actor_not_permitted", "{body}");
    assert_eq!(outbox_len(&state).await, 0, "nothing is sent");
}

#[tokio::test]
async fn a_granted_service_attributes_the_command_to_the_approval() {
    let state = state().await;
    let asserted = serde_json::json!({
        "principal": "chefin",
        "vorschlag": { "art": "agent", "skill": "anmeldung-pruefen" },
        "unveraendert": false,
    });
    let freigabe = Akteur::Freigabe {
        principal: "chefin".to_owned(),
        vorschlag: Vorschlag::Agent {
            skill: "anmeldung-pruefen".to_owned(),
        },
        unveraendert: false,
        dienst: "processd".to_owned(),
    };
    let (status, body) = post(&state, "processd", Some(&asserted)).await;
    assert_eq!(status, axum::http::StatusCode::ACCEPTED, "{body}");
    let actors = actors(&state, &body).await;
    assert!(
        !actors.is_empty() && actors.iter().all(|a| *a == freigabe),
        "{actors:?}"
    );
}

/// A granted service's own command is the system's; it may relay an approval
/// and nothing else.
#[tokio::test]
async fn a_granted_service_acts_as_the_system_and_names_only_an_approval() {
    let state = state().await;
    let (status, body) = post(&state, "processd", None).await;
    assert_eq!(status, axum::http::StatusCode::ACCEPTED, "{body}");
    let actors = actors(&state, &body).await;
    assert!(
        !actors.is_empty() && actors.iter().all(|a| *a == Akteur::System),
        "{actors:?}"
    );

    // Anything but an approval — the system, a person, a forged service — is
    // refused, and nothing is sent.
    for asserted in [
        serde_json::json!({ "art": "system" }),
        serde_json::json!({ "art": "person", "principal": "chefin" }),
        serde_json::json!({
            "principal": "chefin",
            "vorschlag": { "art": "regel", "name": "r" },
            "unveraendert": true,
            "dienst": "someone-else",
        }),
    ] {
        let state = self::state().await;
        let (status, body) = post(&state, "processd", Some(&asserted)).await;
        assert_eq!(
            status,
            axum::http::StatusCode::FORBIDDEN,
            "{asserted}: {body}"
        );
        assert_eq!(body["code"], "actor_not_permitted", "{body}");
        assert_eq!(outbox_len(&state).await, 0, "nothing is sent");
    }
}

#[tokio::test]
async fn a_door_reached_without_an_actor_writes_nothing() {
    let state = state().await;
    // `dispatch_command` scopes the actor; the inner dispatch alone has none.
    let outcome = super::handler::dispatch_command_als(
        &state,
        "gpke.lieferbeginn.anmelden",
        &anmeldung_55001(),
    )
    .await;
    assert!(outcome.is_err(), "{outcome:?}");
    assert_eq!(outbox_len(&state).await, 0);
}

/// The principal stays in the event store: an ERP notification carries the
/// actor's class as `makoakteur`, and no outbox entry names the person.
#[tokio::test]
async fn no_outbox_entry_carries_the_principal() {
    use mako_engine::outbox::OutboxStore as _;

    let state = state().await;
    let (status, body) = post(&state, "sachbearbeiterin", None).await;
    assert_eq!(status, axum::http::StatusCode::ACCEPTED, "{body}");
    let pending = state.store.pending_now(1_000).await.expect("read outbox");
    assert!(!pending.is_empty(), "the Anmeldung is enqueued");
    for msg in &pending {
        let stored = serde_json::to_string(msg).expect("serialise");
        assert!(!stored.contains("sachbearbeiterin"), "{stored}");
        assert_eq!(msg.akteur, mako_engine::envelope::Eingriffsklasse::Person);
        // The notification the ERP worker would build from an entry of this
        // process: the class travels, the principal does not.
        let mut notice = msg.clone();
        notice.message_type = "ProcessInitiated".into();
        let event = crate::erp_adapter::erp_event(&notice).expect("an ERP notification");
        let ce = crate::erp_adapter::cloud_event_json(&event);
        assert_eq!(ce["makoakteur"], "person", "{ce}");
        assert!(!ce.to_string().contains("sachbearbeiterin"), "{ce}");
    }
}
