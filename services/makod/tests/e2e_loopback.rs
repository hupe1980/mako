//! Combined-role (VIU) loopback round-trip and FV-coexistence dispatch.
//!
//! A combined-role deployment sends messages to itself: `BdewAs4Sender`
//! detects `recipient == own MP-ID`, renders the Übertragungsdatei, re-parses
//! it, and dispatches in-process — no HTTP, no self-signed-cert failure.
//! This test drives the **real** sender loopback branch end to end and
//! asserts a process exists afterwards.
//!
//! The FV-coexistence tests prove the property the annual release window
//! depends on: one running instance dispatches messages under both active
//! format versions without a restart, and an unknown format version is a
//! structured error, not a panic.

use std::sync::Arc;

#[allow(dead_code)]
mod support;

const OWN_NB: &str = "9900001000001";
const OWN_LF: &str = "9900001000002";

/// Build an `MpIdRegistry` where both NB and LF MP-IDs are our own.
fn viu_registry() -> makod::party_registry::MpIdRegistry {
    use makod::config::PartyConfig;
    makod::party_registry::MpIdRegistry::from_config(&[
        PartyConfig {
            mp_id: OWN_NB.to_owned(),
            roles: vec!["NB".to_owned()],
            primary: true,
            agency: None,
        },
        PartyConfig {
            mp_id: OWN_LF.to_owned(),
            roles: vec!["LF".to_owned()],
            primary: false,
            agency: None,
        },
    ])
    .expect("valid VIU registry")
}

/// The full loopback round-trip: a UTILMD outbox message addressed to our own
/// NB MP-ID is rendered, re-parsed, and dispatched in-process; afterwards the
/// process registry holds the spawned process for the MaLo.
#[tokio::test]
async fn loopback_round_trip_spawns_process() {
    use asx_rs::core::SessionContextBuilder;
    use asx_rs::observability::EventBus;
    use mako_as4::testing::{BdewCertPurpose, generate_self_signed_bdew_keypair};
    use mako_engine::ids::TenantId;
    use mako_engine::registry::ProcessRegistry as _;
    use mako_engine::store_slatedb::SlateDbStore;

    let _ = tracing_subscriber::fmt()
        .with_env_filter("info,makod=debug")
        .try_init();
    let registry = Arc::new(viu_registry());
    let store = SlateDbStore::open_in_memory()
        .await
        .expect("in-memory store");
    let tenant = TenantId::from_party_id(registry.primary_mp_id());

    // Dispatcher + loopback state, mirroring `main.rs` wiring.
    let dispatcher = Arc::new(makod::ingest_dispatcher::EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        tenant,
    ));
    let mut pid_router = mako_engine::pid_router::PidRouter::new();
    // 55001 — Anmeldung Lieferbeginn (LF → NB), spawn-capable on the NB side.
    pid_router.register(55001, "gpke-supplier-change");
    let platform = Arc::new(edi_energy::Platform::with_all_profiles());
    let loopback = Arc::new(makod::edifact_api::EdifactApiState {
        platform: Arc::clone(&platform),
        pid_router,
        cedar: Arc::new(
            makod::cedar_authz::CedarAuthorizer::unauthenticated().expect("infallible"),
        ),
        max_body_bytes: 1024 * 1024,
        partner_store: None,
        tenant_id: tenant,
        dl_sink: Arc::new(mako_engine::dead_letter::LogDeadLetterSink),
        dispatcher: Some(Arc::clone(&dispatcher)),
        contrl_ack: None,
    });

    // Real sender with test PKI — signing material never used on loopback,
    // but constructing it proves the production wiring shape.
    let signing = generate_self_signed_bdew_keypair("CN=VIU Test", BdewCertPurpose::Signing);
    let session = SessionContextBuilder::new("loopback-test", OWN_LF)
        .with_signing_material(signing.cert_pem_str().to_owned(), signing.key_pem_str())
        .with_trust_anchor_pem(signing.cert_pem_str().to_owned())
        .build()
        .expect("session");
    let malo_sender = makod::malo_ident_sender::MaloIdentSender::new(
        makod::malo_cache::SlateDbMaloCache::new(store.clone()),
        reqwest::Client::new(),
        std::collections::HashMap::new(),
        None,
        store.clone(),
    );
    let sender = makod::as4_sender::BdewAs4Sender::new(
        Arc::new(session),
        Arc::new(EventBus::new_for_testing()),
        Arc::new(mako_as4::BdewAs4Profile::new()),
        malo_sender,
        Arc::clone(&registry),
        Some(loopback),
        platform,
        false,
    )
    .expect("sender");

    // A UTILMD 55001 addressed to OUR OWN NB MP-ID.
    let malo = "51238696781";
    let msg = support::outbox_message(
        "UTILMD",
        OWN_NB,
        serde_json::json!({
            "pid": 55001,
            "sender": OWN_LF,
            "receiver": OWN_NB,
            "malo": malo,
            "process_date": "2026-03-01",
        }),
    );

    // The rendered UTILMD must pass its own AHB profile — the pre-send gate
    // dead-letters anything that does not.
    {
        let r = makod::edifact_renderer::render_to_wire_bytes(&msg, &registry).unwrap();
        let parsed = edi_energy::Platform::with_all_profiles()
            .parse(&r.bytes)
            .unwrap();
        use edi_energy::EdiEnergyMessage as _;
        let report = parsed.validate().unwrap();
        assert!(
            report.is_valid(),
            "rendered UTILMD 55001 violates its AHB profile: {:?}",
            report.errors()
        );
    }
    use mako_engine::builder::As4Sender as _;
    sender.send(&msg).await.expect("loopback delivery succeeds");

    // The dispatch spawned a process — visible via the correlated (MaLo) index.
    let found = store
        .as_process_registry()
        .lookup_correlated(tenant, malo)
        .await
        .expect("registry lookup");
    assert!(
        found
            .iter()
            .any(|id| id.workflow_id.name.as_ref() == "gpke-supplier-change"),
        "loopback dispatch must spawn a gpke-supplier-change process for the MaLo; got: {found:?}"
    );
}

/// One instance, both active FVs: the same UTILMD wire dispatches under
/// `FV2025-10-01` and `FV2026-10-01` through the same static registry — the
/// property the ~7-day annual transition window depends on.
#[test]
fn same_instance_dispatches_both_active_fvs() {
    use std::any::Any;

    let registry = viu_registry();
    let msg = support::outbox_message(
        "UTILMD",
        OWN_NB,
        serde_json::json!({
            "pid": 55001,
            "sender": OWN_LF,
            "receiver": OWN_NB,
            "malo": "51238696781",
            "process_date": "2026-03-01",
        }),
    );
    let rendered = makod::edifact_renderer::render_to_wire_bytes(&msg, &registry).expect("render");
    let parsed = edi_energy::Platform::with_all_profiles()
        .parse(&rendered.bytes)
        .expect("parse");

    for fv in ["FV2025-10-01", "FV2026-10-01"] {
        let fv = mako_engine::version::FormatVersion::new(fv);
        let cmd = makod::adapters::gpke_registry().dispatch(&parsed as &dyn Any, &fv);
        assert!(
            cmd.is_ok(),
            "UTILMD 55001 must dispatch under {fv:?}: {:?}",
            cmd.err()
        );
    }
}

/// An unknown format version is a structured error, never a panic or a
/// silent fallback.
#[test]
fn unknown_fv_is_a_structured_error() {
    use std::any::Any;

    let registry = viu_registry();
    let msg = support::outbox_message(
        "UTILMD",
        OWN_NB,
        serde_json::json!({
            "pid": 55001,
            "sender": OWN_LF,
            "receiver": OWN_NB,
            "malo": "51238696781",
            "process_date": "2026-03-01",
        }),
    );
    let rendered = makod::edifact_renderer::render_to_wire_bytes(&msg, &registry).expect("render");
    let parsed = edi_energy::Platform::with_all_profiles()
        .parse(&rendered.bytes)
        .expect("parse");

    let future_fv = mako_engine::version::FormatVersion::new("FV2031-10-01");
    let result = makod::adapters::gpke_registry().dispatch(&parsed as &dyn Any, &future_fv);
    assert!(
        result.is_err(),
        "an unregistered FV must produce a structured dispatch error"
    );
}
