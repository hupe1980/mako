//! A spawn and its business key are one write.
//!
//! The duplicate guard ([`super::find_occupying_process`]) finds a running
//! process only through the correlation index. A spawn whose events and
//! outbox entries are durable while its key is not is therefore invisible to
//! the guard, and the ERP's re-issue of the same command spawns a second
//! process and sends a second UTILMD to the NB — which cannot be withdrawn.
//!
//! These tests inject a failure into the key write through
//! [`super::KEY_REGISTRATION_FAULT`], which makes the store refuse the key
//! itself, and assert the door reports the failure and the re-issue sends
//! exactly one message.

use std::sync::Arc;

use mako_engine::ids::TenantId;
use mako_engine::outbox::OutboxStore as _;

use super::*;

const LF_MP_ID: &str = "9905550000005";
const NB_MP_ID: &str = "9900357000004";
const MALO_ID: &str = "51238696012";

async fn lf_state() -> CommandsApiState {
    let store = mako_engine::store_slatedb::SlateDbStore::open_in_memory()
        .await
        .expect("open in-memory SlateDB");
    CommandsApiState {
        tenant_id: TenantId::from_party_id(LF_MP_ID),
        sender_party_id: LF_MP_ID.to_owned(),
        configured_marktrollen: vec!["LF".to_owned()],
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

fn malo_record() -> energy_api::models::electricity::MaloIdentResultPositive {
    serde_json::from_value(serde_json::json!({
        "dataMarketLocation": {
            "maloId": MALO_ID,
            "energyDirection": "consumption",
            "measurementTechnologyClassification": "conventionalMeasuringSystem",
            "optionalChangeForecastBasis": "notPossible",
            "dataMarketLocationProperties": [],
            "dataMarketLocationNetworkOperators": [
                { "marketPartnerId": NB_MP_ID, "executionTimeFrom": "2000-01-01T00:00:00Z" }
            ],
            "dataMarketLocationTransmissionSystemOperators": []
        }
    }))
    .expect("MaLo fixture matches the energy-api schema")
}

async fn outbox_len(state: &CommandsApiState) -> usize {
    state
        .store
        .pending_now(1_000)
        .await
        .expect("read outbox")
        .len()
}

#[tokio::test]
async fn a_failed_key_write_fails_the_spawn_and_the_reissue_sends_one_utilmd() {
    let state = lf_state().await;
    state
        .malo_cache
        .upsert(&state.tenant_id.to_string(), &malo_record())
        .await
        .expect("seed MaLo cache");
    let payload = serde_json::json!({
        "malo_id": MALO_ID,
        "lieferbeginn_datum": "2026-10-01",
        "bilanzkreis": "11XBK-LF-------9",
    });

    let faulted = KEY_REGISTRATION_FAULT
        .scope(
            true,
            dispatch_command(&state, "gpke.lieferbeginn.anmelden", &payload),
        )
        .await;
    // The ERP re-issues the command, as a failed answer tells it to.
    let reissued = dispatch_command(&state, "gpke.lieferbeginn.anmelden", &payload)
        .await
        .expect("the re-issued Anmeldung is accepted");
    assert!(matches!(reissued, DispatchOutcome::Spawned { .. }));
    assert_eq!(
        outbox_len(&state).await,
        1,
        "exactly one UTILMD 55001 may reach the NB for one Anmeldung"
    );
    assert!(
        faulted.is_err(),
        "a spawn whose business key was not written must be reported as failed — the \
         duplicate guard cannot see it; got {faulted:?}"
    );

    // And the spawned process is now findable: a further re-issue is refused.
    let again = dispatch_command(&state, "gpke.lieferbeginn.anmelden", &payload).await;
    assert!(
        matches!(again, Err(DispatchError::DuplicateProcess { .. })),
        "the running Anmeldung must block a further one; got {again:?}"
    );
}

/// The command door serialises on the ingest dispatcher's per-key lock: while
/// another spawn holds the business key, `anmelden` waits instead of running its
/// occupancy check beside it and spawning a second process.
#[tokio::test]
async fn the_command_door_waits_for_the_business_key_lock() {
    let state = Arc::new(lf_state().await);
    state
        .malo_cache
        .upsert(&state.tenant_id.to_string(), &malo_record())
        .await
        .expect("seed MaLo cache");
    let payload = serde_json::json!({
        "malo_id": MALO_ID,
        "lieferbeginn_datum": "2026-10-01",
        "bilanzkreis": "11XBK-LF-------9",
    });

    let held = crate::ingest_dispatcher::business_key_locks()
        .acquire(MALO_ID)
        .await;
    let task = {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            dispatch_command(&state, "gpke.lieferbeginn.anmelden", &payload).await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        !task.is_finished(),
        "anmelden must wait while another spawn holds the MaLo's lock"
    );
    assert_eq!(
        outbox_len(&state).await,
        0,
        "nothing is sent while the lock is held"
    );
    drop(held);
    let outcome = task
        .await
        .expect("join")
        .expect("the Anmeldung is accepted");
    assert!(matches!(outcome, DispatchOutcome::Spawned { .. }));
}
