//! The GeLi Gas Stammdatenänderung has a door in both directions.
//!
//! „Die berechtigten Marktpartner antworten mit dem Prüfidentifikator 44115"
//! to the NB's 44112 (UTILMD AHB Gas 1.2 Kap. 5.10.1, S. 97), „spätestens
//! jedoch bis zum Ablauf des 10. WT nach Eingang der Änderung" (AWH GeLi Gas
//! V1.2 Kap. 4.3.2 Nr. 2, S. 78). An inbound 44112 opens a process waiting for
//! that Antwort; without `geli.stammdaten-aenderung.beantworten` nothing can
//! send it and the window lapses on a process nobody can close. The outbound
//! side, `geli.stammdaten-aenderung.senden`, sends the LF's 44109 (Kap.
//! 5.11.3) and the NB's 44112 with the groups that changed, and the 44120,
//! 44123 and 44175 whole.

use std::sync::Arc;

use mako_engine::ids::TenantId;
use mako_engine::outbox::OutboxStore as _;
use mako_markt::commands::{
    GELI_STAMMDATEN_AENDERUNG_BEANTWORTEN, GELI_STAMMDATEN_AENDERUNG_SENDEN,
};

use super::*;

/// DVGW-Codenummern: a Gas UTILMD admits NAD DE 3055 `9` or `332` only.
const OWN_MP_ID: &str = "9807317000007";
const PARTNER_MP_ID: &str = "9870012345678";
const MALO_ID: &str = "51238696012";

async fn state_for(role: &str) -> CommandsApiState {
    let store = mako_engine::store_slatedb::SlateDbStore::open_in_memory()
        .await
        .expect("open in-memory SlateDB");
    CommandsApiState {
        tenant_id: TenantId::from_party_id(OWN_MP_ID),
        sender_party_id: OWN_MP_ID.to_owned(),
        configured_marktrollen: vec![role.to_owned()],
        max_body_bytes: 1_048_576,
        snapshot_interval: 100,
        cedar: Arc::new(CedarAuthorizer::unauthenticated().expect("infallible")),
        snapshot_store: store.as_snapshot_store(),
        malo_cache: Arc::new(SlateDbMaloCache::new(store.clone())),
        maloid_result_cache: MaloIdentResultCache::new(store.clone()),
        store: Arc::new(store),
        marktd_client: None,
    }
}

/// The NB's 44112 arrived at the LF and passed its structural check.
async fn seed_44112(state: &CommandsApiState) {
    spawn_process::<mako_geli_gas::GeliGasStammdatenaenderungWorkflow>(
        &state.store,
        state.tenant_id,
        WorkflowId::new(
            mako_geli_gas::stammdatenaenderung::WORKFLOW_NAME,
            latest_format_version(),
        ),
        mako_geli_gas::GasStammdatenCommand::ReceiveAenderung {
            pid: Pruefidentifikator::const_new(44112),
            sender: MarktpartnerCode::new(PARTNER_MP_ID),
            receiver: MarktpartnerCode::new(OWN_MP_ID),
            location_id: MaLo::new(MALO_ID),
            aenderungsdatum: "20261215".to_owned(),
            patch: serde_json::json!({}),
            vertragsbeginn: Some("20250101".to_owned()),
            vorgangsnummer: Some("AEND-VG-1".to_owned()),
            dokumentenart: Some("E03".to_owned()),
            message_ref: MessageRef::new("AEND-1"),
            validation_passed: true,
            validation_errors: Vec::new(),
            received_at: time::macros::datetime!(2026-11-09 10:00 UTC),
        },
        &[],
        &[MALO_ID],
    )
    .await
    .expect("seed the inbound 44112");
}

async fn utilmd(state: &CommandsApiState) -> Vec<mako_engine::outbox::OutboxMessage> {
    state
        .store
        .pending_now(1_000)
        .await
        .expect("read outbox")
        .into_iter()
        .filter(|m| m.message_type.as_ref() == "UTILMD")
        .collect()
}

/// The message passes the column the pre-send gate checks it against.
fn passes_its_profile(state: &CommandsApiState, msg: &mako_engine::outbox::OutboxMessage) {
    let pending = mako_engine::outbox::PendingOutbox::new(
        "UTILMD",
        msg.recipient.as_ref(),
        msg.payload.clone(),
    );
    let findings = crate::orchestrator::edifact_renderer::utilmd_ahb_findings(
        &pending,
        state.tenant_id,
        &latest_format_version(),
    )
    .expect("renders");
    assert!(findings.is_empty(), "{}: {findings:#?}", msg.payload["pid"]);
}

#[tokio::test]
async fn a_received_44112_is_answered_with_a_44115_the_gate_passes() {
    let state = state_for("LFG").await;
    seed_44112(&state).await;
    dispatch_command(
        &state,
        GELI_STAMMDATEN_AENDERUNG_BEANTWORTEN,
        &serde_json::json!({ "malo_id": MALO_ID, "antwort_code": "E15" }),
    )
    .await
    .expect("the Berechtigter answers the 44112");
    let out = utilmd(&state).await;
    assert_eq!(out.len(), 1, "one Antwort");
    assert_eq!(out[0].recipient.as_ref(), PARTNER_MP_ID);
    assert_eq!(out[0].payload["pid"], 44115);
    assert_eq!(out[0].payload["antwort_code"], "E15");
    assert_eq!(out[0].payload["antwort_codeliste"], "G_0016");
    passes_its_profile(&state, &out[0]);
}

/// `G_0016` publishes `E15` alone (EBD 4.3 Kap. 13.10): an Ablehnungscode of
/// another column is refused naming the list, and nothing is written.
#[tokio::test]
async fn a_code_outside_the_columns_codeliste_is_refused_by_name() {
    let state = state_for("LFG").await;
    seed_44112(&state).await;
    let err = dispatch_command(
        &state,
        GELI_STAMMDATEN_AENDERUNG_BEANTWORTEN,
        &serde_json::json!({ "malo_id": MALO_ID, "antwort_code": "E13" }),
    )
    .await
    .expect_err("E13 is not a 44115 code");
    let text = format!("{err:?}");
    assert!(text.contains("E13") && text.contains("G_0016"), "{text}");
    assert!(utilmd(&state).await.is_empty());

    let err = dispatch_command(
        &state,
        GELI_STAMMDATEN_AENDERUNG_BEANTWORTEN,
        &serde_json::json!({ "malo_id": MALO_ID, "antwort_code": "E15", "grund": "x" }),
    )
    .await
    .expect_err("an undeclared key is refused");
    assert!(format!("{err:?}").contains("grund"), "{err:?}");
}

fn aenderung_44109(gruppen: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "malo_id": MALO_ID,
        "pid": 44109,
        "empfaenger_mp_id": PARTNER_MP_ID,
        "aenderungsdatum": "20261215",
        "vertragsbeginn": "20250101",
        "gruppen": gruppen,
        "abrechnungsintervall_monate": 12,
        "haushaltskunde": true,
        "kunde": {
            "vorname": "Erika",
            "nachname": "Mustermann",
            "anschrift": {
                "strasse": "Hauptstraße", "hausnummer": "1",
                "postleitzahl": "50667", "ort": "Köln", "landescode": "DE"
            }
        }
    })
}

/// The LF's 44109 carries the groups that changed and passes its column
/// (UTILMD AHB Gas 1.2 Kap. 5.11.3, S. 120–123); the process then waits for
/// the NB's 44111.
#[tokio::test]
async fn the_lf_sends_a_44109_its_column_accepts() {
    let state = state_for("LFG").await;
    dispatch_command(
        &state,
        GELI_STAMMDATEN_AENDERUNG_SENDEN,
        &aenderung_44109(&[
            "ABRECHNUNGSINTERVALL_LF",
            "GRUPPENZUORDNUNG",
            "KUNDE_DES_LF",
            "KORRESPONDENZANSCHRIFT_KUNDE_LF",
        ]),
    )
    .await
    .expect("the LF sends its Änderung");
    let out = utilmd(&state).await;
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].recipient.as_ref(), PARTNER_MP_ID);
    assert_eq!(out[0].payload["pid"], 44109);
    passes_its_profile(&state, &out[0]);

    // A second Änderung on the same Marktlokation waits for the first.
    let err = dispatch_command(
        &state,
        GELI_STAMMDATEN_AENDERUNG_SENDEN,
        &aenderung_44109(&["GRUPPENZUORDNUNG"]),
    )
    .await
    .expect_err("one open Stammdatenänderung per Marktlokation");
    assert!(
        matches!(err, DispatchError::DuplicateProcess { .. }),
        "{err:?}"
    );
}

#[tokio::test]
async fn an_aenderung_naming_no_changed_group_or_a_foreign_one_is_refused() {
    let state = state_for("LFG").await;
    for (gruppen, nennt) in [
        (&[][..], "gruppen is empty"),
        (&["MARKTGEBIET"][..], "MARKTGEBIET"),
        (&["UNBEKANNT"][..], "UNBEKANNT"),
    ] {
        let err = dispatch_command(
            &state,
            GELI_STAMMDATEN_AENDERUNG_SENDEN,
            &aenderung_44109(gruppen),
        )
        .await
        .expect_err("refused");
        assert!(format!("{err:?}").contains(nennt), "{nennt}: {err:?}");
    }
    // The 44120 carries its SG8 whole, named by no group.
    let mut bila = aenderung_44109(&["GRUPPENZUORDNUNG"]);
    bila["pid"] = serde_json::json!(44120);
    let err = dispatch_command(&state, GELI_STAMMDATEN_AENDERUNG_SENDEN, &bila)
        .await
        .expect_err("a 44120 names no group");
    assert!(format!("{err:?}").contains("GRUPPENZUORDNUNG"), "{err:?}");
    // The MSB's Änderungen are not sent from here.
    let mut msb = aenderung_44109(&[]);
    msb["pid"] = serde_json::json!(44159);
    let err = dispatch_command(&state, GELI_STAMMDATEN_AENDERUNG_SENDEN, &msb)
        .await
        .expect_err("a 44159 is not assembled");
    assert!(matches!(err, DispatchError::NotImplemented(_)), "{err:?}");
    // The 44112 is the GNB's.
    let mut nb = aenderung_44109(&["MARKTGEBIET"]);
    nb["pid"] = serde_json::json!(44112);
    let err = dispatch_command(&state, GELI_STAMMDATEN_AENDERUNG_SENDEN, &nb)
        .await
        .expect_err("an LF deployment does not send a 44112");
    assert!(format!("{err:?}").contains("GNB"), "{err:?}");
    assert!(utilmd(&state).await.is_empty());
}
