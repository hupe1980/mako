//! End-to-end loopback: ESA Wertebestellung handshake (WiM Strom Teil 2 Kap. 4)
//! driven through the **real** command dispatcher, outbox, EDIFACT renderer and
//! ingest dispatcher — proving the LOC-less ORDRSP answer correlates back to the
//! ESA's process by the echoed order reference (`RFF+ACW`), not by MaLo.
//!
//! ```text
//!   ESA (tenant A)                                   MSB (tenant B)
//!   ─────────────────────────────────────────────────────────────────
//!   esa.werteanfrage.stellen ─ REQOTE 35002 ─►   (spawn wim-wertebestellung)
//!            (resume) ◄─ QUOTES 15003 ─ wim.wertebestellung.anbieten
//!   esa.bestellung.beauftragen ─ ORDERS 17007 ─► (resume; index by Belegnummer)
//!            (resume by RFF+ACW) ◄─ ORDRSP 19011 ─ …bestellung-beantworten
//! ```
//!
//! Both roles share one store; each role owns its own tenant, command state and
//! ingest dispatcher, exactly as two market partners would. Party identifiers
//! are `mp_id`s (BDEW/DVGW market-participant codes), never "GLN".

use std::sync::Arc;

use edi_energy::AnyMessage;
use mako_engine::{ids::TenantId, outbox::OutboxStore, store_slatedb::SlateDbStore};
use makod::commands_api::{CommandsApiState, DispatchOutcome, dispatch_command};
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

const ESA_MP_ID: &str = "9905550000005";
const MSB_MP_ID: &str = "9900357000004";
const MALO_ID: &str = "51238696780"; // 11-digit Marktlokations-ID

/// Build a `CommandsApiState` over the shared store for one market role.
fn command_state(
    store: &Arc<SlateDbStore>,
    tenant: TenantId,
    sender_mp_id: &str,
    marktrolle: &str,
) -> CommandsApiState {
    CommandsApiState {
        tenant_id: tenant,
        sender_party_id: sender_mp_id.to_owned(),
        configured_marktrollen: vec![marktrolle.to_uppercase()],
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

fn party_registry() -> makod::party_registry::MpIdRegistry {
    use makod::config::PartyConfig;
    makod::party_registry::MpIdRegistry::from_config(&[
        PartyConfig {
            mp_id: ESA_MP_ID.to_owned(),
            roles: vec!["ESA".to_owned()],
            primary: true,
            agency: None,
        },
        PartyConfig {
            mp_id: MSB_MP_ID.to_owned(),
            roles: vec!["MSB".to_owned()],
            primary: false,
            agency: None,
        },
    ])
    .expect("valid registry")
}

/// Drain every pending outbox entry, render it to wire, parse it, and hand it to
/// the matching role's ingest dispatcher. Returns the ingest outcomes in order.
///
/// This exercises the true relay path: workflow → outbox → renderer → parser →
/// ingest, with the `workflow_name` the runtime's `PidRouter` would assign.
async fn relay_pending(
    store: &Arc<SlateDbStore>,
    registry: &makod::party_registry::MpIdRegistry,
    dispatcher_esa: &EdifactIngestDispatcher,
    dispatcher_msb: &EdifactIngestDispatcher,
) -> Vec<IngestOutcome> {
    let now = time::OffsetDateTime::now_utc() + time::Duration::days(1);
    let pending = OutboxStore::pending(&**store, 100, now)
        .await
        .expect("pending outbox");
    let mut outcomes = Vec::new();
    for om in &pending {
        let wire = makod::edifact_renderer::render_to_wire_bytes(om, registry)
            .unwrap_or_else(|e| panic!("render {} failed: {e:?}", om.message_type));
        let msg = edi_energy::parse(&wire.bytes)
            .unwrap_or_else(|e| panic!("parse {} failed: {e:?}", om.message_type));
        let pid = detect_pid(&msg);
        // Route to (workflow_name, target dispatcher) the way the PidRouter would.
        let (workflow_name, dispatcher) = match (&*om.message_type, pid) {
            ("REQOTE", 35002) => ("wim-preisanfrage", dispatcher_msb),
            ("QUOTES", 15003) => ("esa-wertebestellung", dispatcher_esa),
            ("ORDERS", 17007 | 17008) => ("wim-wertebestellung", dispatcher_msb),
            ("ORDCHG", 39002) => ("wim-wertebestellung", dispatcher_msb),
            ("ORDRSP", 19011..=19014) => ("esa-wertebestellung", dispatcher_esa),
            other => panic!("unexpected outbox message {other:?}"),
        };
        let outcome = dispatcher
            .dispatch(&msg, workflow_name, pid)
            .await
            .unwrap_or_else(|e| panic!("dispatch {} failed: {e:?}", om.message_type));
        outcomes.push(outcome);
        OutboxStore::acknowledge(&**store, om.message_id)
            .await
            .expect("acknowledge");
    }
    outcomes
}

fn detect_pid(msg: &AnyMessage) -> u32 {
    use edi_energy::EdiEnergyMessage as _;
    msg.detect_pruefidentifikator()
        .expect("PID detectable")
        .as_u32()
}

#[tokio::test]
async fn esa_ordrsp_answer_correlates_by_order_reference_not_malo() {
    let store = Arc::new(
        SlateDbStore::open_in_memory()
            .await
            .expect("in-memory store"),
    );
    let tenant_esa = TenantId::from_party_id(ESA_MP_ID);
    let tenant_msb = TenantId::from_party_id(MSB_MP_ID);

    let esa = command_state(&store, tenant_esa, ESA_MP_ID, "ESA");
    let msb = command_state(&store, tenant_msb, MSB_MP_ID, "MSB");

    let dispatcher_esa = EdifactIngestDispatcher::new(
        Arc::clone(&store),
        store.as_snapshot_store(),
        100,
        tenant_esa,
    );
    // The MSB must recognise the ESA as an ESA counterparty to classify the
    // shared-PID REQOTE 35002 as a Werteanfrage rather than a Preisanfrage.
    let dispatcher_msb = EdifactIngestDispatcher::new(
        Arc::clone(&store),
        store.as_snapshot_store(),
        100,
        tenant_msb,
    )
    .with_esa_partners([ESA_MP_ID.to_owned()]);

    let registry = party_registry();

    // 1. ESA originates the Werteanfrage (REQOTE 35002).
    let out = dispatch_command(
        &esa,
        "esa.werteanfrage.stellen",
        &serde_json::json!({ "msb_mp_id": MSB_MP_ID, "malo_id": MALO_ID }),
    )
    .await
    .expect("werteanfrage");
    assert!(matches!(out, DispatchOutcome::Spawned { .. }), "{out:?}");

    // REQOTE relayed to the MSB spawns its wim-wertebestellung process.
    let outcomes = relay_pending(&store, &registry, &dispatcher_esa, &dispatcher_msb).await;
    assert!(
        matches!(outcomes.as_slice(), [IngestOutcome::Spawned { .. }]),
        "REQOTE must spawn the MSB process; got {outcomes:?}"
    );

    // 2. MSB answers with a QUOTES 15003 Angebot; the ESA resumes by MaLo (LOC).
    dispatch_command(
        &msb,
        "wim.wertebestellung.anbieten",
        &serde_json::json!({ "malo_id": MALO_ID }),
    )
    .await
    .expect("anbieten");
    let outcomes = relay_pending(&store, &registry, &dispatcher_esa, &dispatcher_msb).await;
    assert!(
        matches!(outcomes.as_slice(), [IngestOutcome::Dispatched { .. }]),
        "QUOTES must resume the ESA process by MaLo; got {outcomes:?}"
    );

    // 3. ESA places the Bestellung (ORDERS 17007); the ESA process is indexed
    //    under the order Belegnummer for the LOC-less answer to come.
    dispatch_command(
        &esa,
        "esa.bestellung.beauftragen",
        &serde_json::json!({ "malo_id": MALO_ID }),
    )
    .await
    .expect("bestellung");
    let outcomes = relay_pending(&store, &registry, &dispatcher_esa, &dispatcher_msb).await;
    assert!(
        matches!(outcomes.as_slice(), [IngestOutcome::Dispatched { .. }]),
        "ORDERS must resume the MSB process; got {outcomes:?}"
    );

    // 4. MSB confirms the Bestellung with an ORDRSP 19011 — which carries NO LOC.
    //    It must still resume the ESA process, via the RFF+ACW order reference.
    dispatch_command(
        &msb,
        "wim.wertebestellung.bestellung-beantworten",
        &serde_json::json!({ "malo_id": MALO_ID, "accept": true }),
    )
    .await
    .expect("bestellung-beantworten");
    let outcomes = relay_pending(&store, &registry, &dispatcher_esa, &dispatcher_msb).await;
    assert!(
        matches!(
            outcomes.as_slice(),
            [IngestOutcome::Dispatched {
                workflow_name: "esa-wertebestellung",
                ..
            }]
        ),
        "the LOC-less ORDRSP 19011 must resume the ESA process by RFF+ACW; got {outcomes:?}"
    );

    // 5. ESA cancels the not-yet-delivered Bestellung (ORDCHG 39002). The
    //    Stornierung carries NO LOC — it references the original Bestellung in
    //    RFF+ON, and must resume the *same* MSB subscription process (39002 is
    //    consolidated onto the wertebestellung lifecycle, not a standalone flow).
    dispatch_command(
        &esa,
        "esa.stornierung.beauftragen",
        &serde_json::json!({ "malo_id": MALO_ID }),
    )
    .await
    .expect("stornierung");
    let outcomes = relay_pending(&store, &registry, &dispatcher_esa, &dispatcher_msb).await;
    assert!(
        matches!(
            outcomes.as_slice(),
            [IngestOutcome::Dispatched {
                workflow_name: "wim-wertebestellung",
                ..
            }]
        ),
        "the LOC-less ORDCHG 39002 must resume the MSB process by RFF+ON; got {outcomes:?}"
    );

    // 6. MSB confirms the Stornierung (ORDRSP 19013) — again no LOC; the ESA
    //    resumes by the ORDCHG's Belegnummer echoed in RFF+ACW.
    dispatch_command(
        &msb,
        "wim.wertebestellung.stornierung-beantworten",
        &serde_json::json!({ "malo_id": MALO_ID, "accept": true }),
    )
    .await
    .expect("stornierung-beantworten");
    let outcomes = relay_pending(&store, &registry, &dispatcher_esa, &dispatcher_msb).await;
    assert!(
        matches!(
            outcomes.as_slice(),
            [IngestOutcome::Dispatched {
                workflow_name: "esa-wertebestellung",
                ..
            }]
        ),
        "the LOC-less ORDRSP 19013 must resume the ESA process by RFF+ACW; got {outcomes:?}"
    );
}
