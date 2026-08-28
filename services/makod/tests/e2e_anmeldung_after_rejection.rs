//! A rejected Anmeldung must not block the MaLo forever.
//!
//! When the NB rejects a Lieferbeginn (UTILMD 55003 with a negative
//! Antwortstatus), the LF corrects whatever the NB objected to and sends a new
//! Anmeldung. That is a normal GPKE flow, not a retry — and it has to be
//! allowed.
//!
//! The duplicate guard used to refuse it. `register_correlated` writes an entry
//! to the correlation index when a process spawns, nothing ever calls
//! `remove_correlated`, and the guard refused on the mere *presence* of an
//! entry. So the index — which is append-only — permanently recorded that this
//! MaLo had "an active Anmeldung", and every later attempt was answered with
//! `409 duplicate_process` naming a process that had already terminated.
//!
//! Callers treat `duplicate_process` as an idempotent success (see
//! `mako_markt::makod_client::classify_conflict`), so the failure was silent in
//! the worst way: `vertragd` marked the contract component `ANGEMELDET` against
//! the dead process id and no UTILMD ever went out.
//!
//! The guard now rehydrates each candidate and blocks only on `Pending` or
//! `Active`.

use std::sync::Arc;

use mako_engine::{ids::TenantId, registry::ProcessRegistry as _, store_slatedb::SlateDbStore};
use makod::commands_api::{CommandsApiState, DispatchOutcome, dispatch_command};

const LF_MP_ID: &str = "9905550000005";
const NB_MP_ID: &str = "9900357000004";
/// Checksum-valid MaLo — `energy_api::MaloId` validates the check digit on
/// deserialization, so an invented id is rejected before the test can run.
const MALO_ID: &str = "51238696012";

fn command_state(store: &Arc<SlateDbStore>, tenant: TenantId) -> CommandsApiState {
    CommandsApiState {
        tenant_id: tenant,
        sender_party_id: LF_MP_ID.to_owned(),
        configured_marktrollen: vec!["LF".to_owned()],
        max_body_bytes: 1_048_576,
        snapshot_interval: 100,
        cedar: Arc::new(
            makod::cedar_authz::CedarAuthorizer::unauthenticated().expect("infallible"),
        ),
        snapshot_store: store.as_snapshot_store(),
        malo_cache: Arc::new(makod::malo_cache::SlateDbMaloCache::new((**store).clone())),
        maloid_result_cache: makod::malo_cache::MaloIdentResultCache::new((**store).clone()),
        store: Arc::clone(store),
        marktd_client: None,
    }
}

/// Minimal MaLo master-data record — enough for the dispatcher to resolve the
/// NB from `dataMarketLocationNetworkOperators`.
fn malo_record() -> energy_api::models::electricity::MaloIdentResultPositive {
    serde_json::from_value(serde_json::json!({
        "dataMarketLocation": {
            "maloId": MALO_ID,
            "energyDirection": "consumption",
            "measurementTechnologyClassification": "conventionalMeasuringSystem",
            "optionalChangeForecastBasis": "notPossible",
            "dataMarketLocationProperties": [],
            "dataMarketLocationNetworkOperators": [
                {
                    "marketPartnerId": NB_MP_ID,
                    "executionTimeFrom": "2000-01-01T00:00:00Z"
                }
            ],
            "dataMarketLocationTransmissionSystemOperators": []
        }
    }))
    .expect("MaLo fixture matches the energy-api schema")
}

/// Drive `process_id` to `Rejected`, as the AS4 ingest layer does on an inbound
/// UTILMD 55003.
async fn reject(
    store: &Arc<SlateDbStore>,
    tenant: TenantId,
    process_id: mako_engine::ids::ProcessId,
) {
    use mako_gpke::lf_anmeldung::{GpkeLfAnmeldungWorkflow, LfAnmeldungCommand};
    let identity = store
        .as_process_registry()
        .lookup_correlated(tenant, MALO_ID)
        .await
        .expect("correlation lookup")
        .into_iter()
        .find(|id| id.process_id == process_id)
        .expect("process is in the correlation index");
    mako_engine::process::Process::<GpkeLfAnmeldungWorkflow, Arc<SlateDbStore>>::from_identity(
        Arc::clone(store),
        identity,
    )
    .execute(LfAnmeldungCommand::HandleAntwort {
        response_pid: mako_engine::types::Pruefidentifikator::new(55003).expect("valid PID"),
        accepted: false,
        reason: Some("Zaehlpunkt nicht zugeordnet".to_owned()),
        response_ref: mako_engine::types::MessageRef::new("NB-ANTWORT-1"),
    })
    .await
    .expect("rejection is a legal transition from Pending");
}

#[tokio::test]
async fn a_rejected_anmeldung_does_not_block_the_malo_forever() {
    let store = Arc::new(
        SlateDbStore::open_in_memory()
            .await
            .expect("in-memory store"),
    );
    let tenant = TenantId::from_party_id(LF_MP_ID);
    let state = command_state(&store, tenant);

    state
        .malo_cache
        .upsert(&tenant.to_string(), &malo_record())
        .await
        .expect("seed MaLo cache");

    let payload = serde_json::json!({
        "malo_id": MALO_ID,
        "lieferbeginn_datum": "2026-10-01",
        // `SG8 SEQ+Z79` Produktpaket — Muss on a 55001 (UTILMD AHB Strom 2.2
        // Kap. 5.3); the workflow refuses an Anmeldung without one.
        "bilanzkreis": "11XBK-LF-------9",
    });

    // ── First Anmeldung ───────────────────────────────────────────────────────
    let first = dispatch_command(&state, "gpke.lieferbeginn.anmelden", &payload)
        .await
        .expect("first Anmeldung is accepted");
    let DispatchOutcome::Spawned {
        process_id: first_id,
    } = first
    else {
        panic!("expected the first Anmeldung to spawn a process, got {first:?}");
    };

    // A second attempt while the first is still Pending is a genuine duplicate —
    // the NB has not answered yet, so two open Anmeldungen for one MaLo would be
    // a protocol error. This half of the guard must keep working.
    let while_pending = dispatch_command(&state, "gpke.lieferbeginn.anmelden", &payload).await;
    assert!(
        matches!(
            while_pending,
            Err(makod::commands_api::DispatchError::DuplicateProcess { .. })
        ),
        "a second Anmeldung while the first is still Pending must be refused, got {while_pending:?}"
    );

    // ── NB rejects ────────────────────────────────────────────────────────────
    //
    // The NB's answer arrives as an inbound UTILMD 55003, which the AS4 ingest
    // layer turns into `HandleAntwort { accepted: false }`. Dispatched directly
    // here so the test does not need a full AS4 loopback — this is the same
    // command the ingest path issues.
    {
        use mako_gpke::lf_anmeldung::{GpkeLfAnmeldungWorkflow, LfAnmeldungCommand};
        let identity = store
            .as_process_registry()
            .lookup_correlated(tenant, MALO_ID)
            .await
            .expect("correlation lookup")
            .into_iter()
            .find(|id| id.process_id == first_id)
            .expect("the first Anmeldung is in the correlation index");
        let process = mako_engine::process::Process::<
            GpkeLfAnmeldungWorkflow,
            Arc<SlateDbStore>,
        >::from_identity(Arc::clone(&store), identity);
        process
            .execute(LfAnmeldungCommand::HandleAntwort {
                response_pid: mako_engine::types::Pruefidentifikator::new(55003)
                    .expect("55003 is a valid PID"),
                accepted: false,
                reason: Some("Zaehlpunkt nicht zugeordnet".to_owned()),
                response_ref: mako_engine::types::MessageRef::new("NB-ANTWORT-1"),
            })
            .await
            .expect("the NB rejection is a legal transition from Pending");
    }

    // ── Corrected Anmeldung ───────────────────────────────────────────────────
    // The defect: this used to fail with DuplicateProcess naming `first_id`,
    // permanently, because the correlation entry outlives the process.
    let second = dispatch_command(&state, "gpke.lieferbeginn.anmelden", &payload)
        .await
        .expect(
            "after the NB rejected, a corrected Anmeldung must be accepted — \
             the MaLo is not permanently blocked by the terminated process",
        );
    let DispatchOutcome::Spawned {
        process_id: second_id,
    } = second
    else {
        panic!("expected the corrected Anmeldung to spawn a process, got {second:?}");
    };

    assert_ne!(
        first_id, second_id,
        "the corrected Anmeldung must be a new process, not the rejected one"
    );
}

/// The regression the prune exists to prevent.
///
/// Allowing a second process is only useful if the follow-up commands still
/// reach it. `dispatch_to_process_keyed` resolves a business key to **exactly
/// one** process and returns `AmbiguousProcess` on two — so if the rejected
/// process's correlation entry survived, every `bestaetigen` / `ablehnen` /
/// `aktivieren` on this MaLo would fail from here on.
#[tokio::test]
async fn a_follow_up_command_still_resolves_after_a_replacement_process_spawns() {
    let store = Arc::new(
        SlateDbStore::open_in_memory()
            .await
            .expect("in-memory store"),
    );
    let tenant = TenantId::from_party_id(LF_MP_ID);
    let state = command_state(&store, tenant);
    state
        .malo_cache
        .upsert(&tenant.to_string(), &malo_record())
        .await
        .expect("seed MaLo cache");

    let payload = serde_json::json!({
        "malo_id": MALO_ID,
        "lieferbeginn_datum": "2026-10-01",
        // `SG8 SEQ+Z79` Produktpaket — Muss on a 55001 (UTILMD AHB Strom 2.2
        // Kap. 5.3); the workflow refuses an Anmeldung without one.
        "bilanzkreis": "11XBK-LF-------9",
    });

    let DispatchOutcome::Spawned {
        process_id: first_id,
    } = dispatch_command(&state, "gpke.lieferbeginn.anmelden", &payload)
        .await
        .expect("first Anmeldung")
    else {
        panic!("expected a spawn");
    };

    reject(&store, tenant, first_id).await;

    let DispatchOutcome::Spawned {
        process_id: second_id,
    } = dispatch_command(&state, "gpke.lieferbeginn.anmelden", &payload)
        .await
        .expect("corrected Anmeldung")
    else {
        panic!("expected a spawn");
    };

    // Exactly one live process must remain indexed under the MaLo.
    let indexed: Vec<_> = store
        .as_process_registry()
        .lookup_correlated(tenant, MALO_ID)
        .await
        .expect("correlation lookup")
        .into_iter()
        .filter(|id| id.workflow_id.name.as_ref() == mako_gpke::lf_anmeldung::WORKFLOW_NAME)
        .map(|id| id.process_id)
        .collect();
    assert_eq!(
        indexed,
        vec![second_id],
        "only the live Anmeldung may remain in the correlation index; \
         the rejected one ({first_id}) must have been retired"
    );

    // And the follow-up command resolves to it rather than failing ambiguous.
    let activated = dispatch_command(
        &state,
        "gpke.lieferbeginn.bestaetigen",
        &serde_json::json!({
            "malo_id":      MALO_ID,
            "response_pid": 55002,
        }),
    )
    .await;
    assert!(
        !matches!(
            activated,
            Err(makod::commands_api::DispatchError::AmbiguousProcess { .. })
        ),
        "the follow-up command must resolve to the live process, got {activated:?}"
    );
}
