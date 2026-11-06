//! The INVOIC-issuing commands start their process and send the invoice.
//!
//! Each command builds the INVOIC from its `rechnung`, validates it, and spawns
//! the issuing process in the write that records `InvoicSent` and enqueues the
//! message. A second command for the same `invoice_ref` is the same invoice and
//! reaches the same process.

use std::sync::Arc;

use axum::{Router, extract::Path, response::IntoResponse as _, routing::get};
use mako_engine::outbox::OutboxStore as _;
use secrecy::SecretString;

use super::*;
use crate::orchestrator::invoic_ausgang::fixtures::{
    LF, NB, abschlagsrechnung, mmm_selbst_ausgestellt, nn_rechnung, stornorechnung,
};
use mako_markt::marktd_client::MarktdClient;

/// A stand-in marktd recording every partner as an extractable E-Rechnung
/// partner with Zustimmung, so the § 14 UStG check admits the INVOIC.
async fn marktd() -> MarktdClient {
    let app = Router::new().route(
        "/api/v1/partners/{mp_id}",
        get(|Path(mp_id): Path<String>| async move {
            axum::Json(serde_json::json!({
                "mp_id": mp_id, "edi_zustimmung": true, "edi_extrahierbar": true,
            }))
            .into_response()
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind stand-in marktd");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    MarktdClient::new(
        format!("http://{addr}"),
        SecretString::from("test-key"),
        reqwest::Client::new(),
    )
}

async fn nb_state() -> CommandsApiState {
    let store = mako_engine::store_slatedb::SlateDbStore::open_in_memory()
        .await
        .expect("open in-memory SlateDB");
    CommandsApiState {
        tenant_id: mako_engine::ids::TenantId::from_party_id(NB),
        sender_party_id: NB.to_owned(),
        configured_marktrollen: vec!["NB".into(), "MSB".into()],
        max_body_bytes: 1_048_576,
        snapshot_interval: 100,
        cedar: Arc::new(CedarAuthorizer::unauthenticated().expect("infallible")),
        snapshot_store: store.as_snapshot_store(),
        malo_cache: Arc::new(SlateDbMaloCache::new(store.clone())),
        maloid_result_cache: MaloIdentResultCache::new(store.clone()),
        store: Arc::new(store),
        marktd_client: Some(Arc::new(marktd().await)),
    }
}

/// The payload `netzbilanzd` sends for `rechnung`.
fn payload(rechnung: &rubo4e::current::Rechnung) -> serde_json::Value {
    serde_json::json!({
        "invoice_ref":      rechnung.rechnungsnummer,
        "sender_mp_id":     NB,
        "recipient_mp_id":  LF,
        "msb_mp_id":        NB,
        "nb_mp_id":         LF,
        "zugehoeriges_dokument": "MSCONSLS0001",
        "preise_mp_id":     NB,
        "rechnung":         rechnung,
    })
}

async fn outbox(
    state: &CommandsApiState,
    message_type: &str,
) -> Vec<mako_engine::outbox::OutboxMessage> {
    state
        .store
        .pending_now(1_000)
        .await
        .expect("read outbox")
        .into_iter()
        .filter(|m| m.message_type.as_ref() == message_type)
        .collect()
}

/// Render an outbox entry as makod sends it and parse and validate the
/// interchange against its own AHB column.
fn wire_valid(msg: &mako_engine::outbox::OutboxMessage) -> String {
    let registry = crate::core::party_registry::MpIdRegistry::from_config(&[
        crate::core::config::PartyConfig {
            mp_id: NB.to_owned(),
            roles: vec!["NB".to_owned()],
            primary: true,
            agency: None,
            as4: None,
        },
    ])
    .expect("registry");
    let rendered = crate::orchestrator::edifact_renderer::render_to_wire_bytes(msg, &registry)
        .unwrap_or_else(|e| panic!("INVOIC renders: {e}"));
    assert_eq!(&*rendered.sender_mp_id, NB);
    assert_eq!(&*rendered.receiver_mp_id, LF);
    let wire = String::from_utf8_lossy(&rendered.bytes).into_owned();
    edi_energy::EdiEnergyMessage::validate(&edi_energy::parse(wire.as_bytes()).expect("parse"))
        .expect("validate")
        .into_error_result()
        .unwrap_or_else(|e| panic!("must be AHB-conformant: {e:#?}\n{wire}"));
    wire
}

/// `invoic.nne.stellen` (31002) spawns the `gpke-abrechnung`
/// process, which leaves exactly one INVOIC outbox entry whose interchange
/// parses and validates; the same `invoice_ref` again does not spawn a second
/// process and sends no second INVOIC.
#[tokio::test]
async fn invoic_nne_stellen_spawns_once_and_sends_one_valid_invoic() {
    let state = nb_state().await;
    let p = payload(&nn_rechnung());
    let first = dispatch_command(&state, "invoic.nne.stellen", &p).await;
    let Ok(DispatchOutcome::Spawned { process_id }) = first else {
        panic!("expected a spawn, got {first:?}");
    };
    let sent = outbox(&state, "INVOIC").await;
    assert_eq!(sent.len(), 1, "{sent:?}");
    let wire = wire_valid(&sent[0]);
    assert!(wire.contains("RFF+Z13:31002'"), "{wire}");
    assert!(wire.contains("MOA+203:80'"), "{wire}");

    let again = dispatch_command(&state, "invoic.nne.stellen", &p).await;
    match again {
        Err(DispatchError::DuplicateInvoice {
            process_id: existing,
            invoice_ref,
        }) => {
            assert_eq!(existing, process_id);
            assert_eq!(invoice_ref, "NNE-2026-0001");
        }
        other => panic!("expected the first process, got {other:?}"),
    }
    assert_eq!(outbox(&state, "INVOIC").await.len(), 1, "no second INVOIC");
}

/// `invoic.nne-abschlag.stellen` (31001): the Abschlagsrechnung spawns and sends a valid INVOIC.
#[tokio::test]
async fn invoic_nne_abschlag_stellen_spawns_and_sends_a_valid_invoic() {
    let state = nb_state().await;
    let mut p = payload(&abschlagsrechnung());
    // The 31001 column has no Lieferschein and no Preise des Netzbetreibers.
    p.as_object_mut().map(|o| {
        o.remove("zugehoeriges_dokument");
        o.remove("preise_mp_id")
    });
    let r = dispatch_command(&state, "invoic.nne-abschlag.stellen", &p).await;
    assert!(matches!(r, Ok(DispatchOutcome::Spawned { .. })), "{r:?}");
    let sent = outbox(&state, "INVOIC").await;
    assert_eq!(sent.len(), 1, "{sent:?}");
    let wire = wire_valid(&sent[0]);
    assert!(wire.contains("RFF+Z13:31001'"), "{wire}");
}

/// `invoic.mmm.stellen` sends the NB's selbst ausgestellte Rechnung — the
/// Mehrmenge — as 31006 (`BGM+389`), the Mindermenge as 31005: „… 31005 …
/// zur Abrechnung der Mindermenge und … 31006 … zur Abrechnung der
/// Mehrmenge" (INVOIC AHB 1.0b Kap. 3, S. 5).
#[tokio::test]
async fn invoic_mmm_stellen_sends_a_mehrmenge_as_31006() {
    let state = nb_state().await;
    let mut p = payload(&mmm_selbst_ausgestellt());
    p.as_object_mut().map(|o| {
        o.remove("zugehoeriges_dokument");
        o.remove("preise_mp_id")
    });
    let r = dispatch_command(&state, "invoic.mmm.stellen", &p).await;
    assert!(matches!(r, Ok(DispatchOutcome::Spawned { .. })), "{r:?}");
    let sent = outbox(&state, "INVOIC").await;
    assert_eq!(sent.len(), 1, "{sent:?}");
    let wire = wire_valid(&sent[0]);
    assert!(wire.contains("BGM+389+MMS-2026-0001+9'"), "{wire}");
    assert!(wire.contains("RFF+Z13:31006'"), "{wire}");
}

/// A Stornorechnung goes out as 31004 — `BGM+457`, `RFF+OI` — whichever
/// door it came through, under the workflow that owns the PID.
#[tokio::test]
async fn a_storno_through_invoic_nne_stellen_goes_out_as_31004() {
    let state = nb_state().await;
    let original = nn_rechnung();
    let mut p = payload(&stornorechnung(&original));
    p["original_rechnungsdatum"] = "2026-02-15T00:00:00Z".into();
    let r = dispatch_command(&state, "invoic.nne.stellen", &p).await;
    let Ok(DispatchOutcome::Spawned { process_id }) = r else {
        panic!("expected a spawn, got {r:?}");
    };
    let sent = outbox(&state, "INVOIC").await;
    assert_eq!(sent.len(), 1, "{sent:?}");
    let wire = wire_valid(&sent[0]);
    assert!(wire.contains("BGM+457+STO-2026-0001+9'"), "{wire}");
    assert!(wire.contains("RFF+Z13:31004'"), "{wire}");
    let ids = state
        .store
        .as_process_registry()
        .lookup_correlated(state.tenant_id, "STO-2026-0001")
        .await
        .expect("lookup");
    assert_eq!(ids.len(), 1);
    assert_eq!(ids[0].process_id, process_id);
    assert_eq!(
        ids[0].workflow_id.name.as_ref(),
        mako_wim::invoic::WORKFLOW_NAME
    );
}

/// No issuing command answers `process_not_found` for a new
/// `invoice_ref`. Each reaches the builder: the 31002-shaped Rechnung is a
/// spawn under its own column or a refusal naming the places the other
/// columns require.
#[tokio::test]
async fn no_issuing_command_answers_process_not_found() {
    for command in [
        "invoic.nne-abschlag.stellen",
        "invoic.nne.stellen",
        "invoic.mmm.stellen",
        "wim.msb-rechnung.stellen",
        "invoic.sonstige-leistung.stellen",
    ] {
        let state = nb_state().await;
        let r = dispatch_command(&state, command, &payload(&nn_rechnung())).await;
        assert!(
            matches!(
                r,
                Ok(DispatchOutcome::Spawned { .. })
                    | Err(DispatchError::InvoicNichtDarstellbar { .. })
            ),
            "{command}: {r:?}"
        );
    }
}

/// A Rechnung the AHB refuses is refused to the caller with its
/// places, and nothing is spawned or sent.
#[tokio::test]
async fn an_invalid_rechnung_is_refused_and_nothing_is_sent() {
    let state = nb_state().await;
    let r = rubo4e::current::Rechnung {
        marktlokation: None,
        ..nn_rechnung()
    };
    match dispatch_command(&state, "invoic.nne.stellen", &payload(&r)).await {
        Err(DispatchError::InvoicNichtDarstellbar { places, .. }) => {
            assert!(places.contains(&"00029".to_owned()), "{places:?}");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert!(outbox(&state, "INVOIC").await.is_empty());
    let ids = state
        .store
        .as_process_registry()
        .lookup_correlated(state.tenant_id, "NNE-2026-0001")
        .await
        .expect("lookup");
    assert!(ids.is_empty(), "{ids:?}");
}

/// The process is found by `invoice_ref` and the REMADV answers the printed
/// Rechnungsnummer, so the two must be one value.
#[tokio::test]
async fn an_invoice_ref_that_is_not_the_rechnungsnummer_is_refused() {
    let state = nb_state().await;
    let mut p = payload(&nn_rechnung());
    p["invoice_ref"] = "SOMETHING-ELSE".into();
    let r = dispatch_command(&state, "invoic.nne.stellen", &p).await;
    assert!(matches!(r, Err(DispatchError::InvalidPayload(_))), "{r:?}");
}
