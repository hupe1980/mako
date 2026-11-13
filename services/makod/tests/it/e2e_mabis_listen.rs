//! MaBiS on both ends of the Lieferantenclearingliste, and on an LF-only
//! deployment.
//!
//! The Lieferant is party to five MaBiS families (Anwendungsübersicht der
//! Prüfidentifikatoren 4.0, sheet „Prüf-ID Prozessschritt"): the LF-SZR on
//! MSCONS 13003 (lfd. Nr. 7080/7160) checked from `E_0007` / `E_0041`, the
//! LF-AASZR on 13023 (9580) checked from `E_0093`, the normierten Profile
//! 13010–13012 (7000–7015) reclaimed from `E_0100`, the Lieferantenclearingliste
//! 55065 (7110/7190) and the LF-AACL 55201 (9610), answered by 55066 / 55202.
//! A `rollen = [lf-strom]` deployment routes all of them and none of the
//! Netzbetreiber's — 55235–55237 run NB → NB / NB → ÜNB (19120–19190).
//!
//! The Netzbetreiber distributes 55065 / 55201 itself (`mabis.liste.senden`),
//! and the Lieferant's Korrekturliste resumes that process by the Listennummer
//! it echoes in `SG6 RFF+TN` (UTILMD AHB Strom 2.2 Kap. 13.2, Bedingung `[631]`,
//! S. 1112), its codes checked against the tree the list's Anforderungsart
//! selects (EBD 4.3 Kap. 7.14.2, S. 445: `E_0047` for an Einzelanforderung).

use std::collections::BTreeSet;
use std::sync::Arc;

use mako_engine::event_store::EventStore as _;
use mako_engine::ids::{StreamId, TenantId};
use mako_engine::marktrolle::DeploymentRoles;
use mako_engine::outbox::OutboxStore as _;
use mako_engine::pid_router::PidRouter;
use mako_engine::process::Process;
use mako_engine::registry::ProcessRegistry as _;
use mako_engine::store_slatedb::SlateDbStore;
use mako_engine::workflow::Workflow as _;
use mako_mabis::listenabgleich::{
    Anforderungsart, ListenabgleichCommand, ListenabgleichState, MabisListenabgleichWorkflow,
};
use makod::deployment::Rolle;
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

const LF_MP: &str = "9900357000004";
const NB_MP: &str = "9900000000003";
const MABIS_ZP: &str = "DE0004096999000000000000000000009";
const MALO: &str = "51238696012";

/// The router a deployment declaring `rollen` builds under `roles`.
fn router_for(rollen: &[Rolle], roles: &DeploymentRoles) -> PidRouter {
    let rollen: BTreeSet<Rolle> = rollen.iter().copied().collect();
    let mut router = PidRouter::new();
    for module in makod::startup::modules_for(&rollen) {
        module.register_pids_with_roles(&mut router, roles);
    }
    router
}

/// The newest profile's skeleton of `pid`, from `sender` to `receiver`.
fn skeleton(pid: u32, sender: &str, receiver: &str) -> String {
    use edi_energy::profile::SkeletonParties;
    let code = edi_energy::Pruefidentifikator::new(pid).expect("five digits");
    let profile = edi_energy::ReleaseRegistry::global()
        .all_profiles()
        .iter()
        .filter(|p| p.has_anwendungsfall(code))
        .max_by_key(|p| p.valid_from())
        .unwrap_or_else(|| panic!("no profile carries {pid}"));
    let af = profile.anwendungsfall(pid).expect("the column");
    let bytes = profile
        .skeleton_interchange(
            af,
            &SkeletonParties {
                sender: sender.to_owned(),
                receiver: receiver.to_owned(),
            },
        )
        .expect("skeleton");
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Replace the first data element after `prefix` with `value`.
fn set_element(wire: &str, prefix: &str, value: &str) -> String {
    let start = wire
        .find(prefix)
        .unwrap_or_else(|| panic!("the message carries {prefix}: {wire}"));
    let at = start + prefix.len();
    let end = at
        + wire[at..]
            .find(['\'', ':', '+'])
            .unwrap_or_else(|| panic!("{prefix} is terminated"));
    format!("{}{value}{}", &wire[..at], &wire[end..])
}

fn parse(wire: &str) -> edi_energy::AnyMessage {
    edi_energy::parse(wire.as_bytes()).unwrap_or_else(|e| panic!("parses: {e}\n{wire}"))
}

async fn state_for(
    own: &str,
    rolle: &str,
) -> (
    makod::commands_api::CommandsApiState,
    EdifactIngestDispatcher,
) {
    use makod::{
        cedar_authz::{CedarAuthorizer, DefaultPolicy},
        commands_api::CommandsApiState,
        malo_cache::{MaloIdentResultCache, SlateDbMaloCache},
    };
    let store = SlateDbStore::open_in_memory()
        .await
        .expect("in-memory store");
    let tenant_id = TenantId::from_party_id(own);
    let dispatcher = EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        tenant_id,
    );
    let cedar = Arc::new(
        CedarAuthorizer::new(vec![], None, None, None, DefaultPolicy::PermitAll)
            .expect("cedar build"),
    );
    let state = CommandsApiState {
        tenant_id,
        sender_party_id: own.to_owned(),
        configured_marktrollen: vec![rolle.to_owned()],
        max_body_bytes: 1_048_576,
        snapshot_interval: 100,
        cedar,
        snapshot_store: store.as_snapshot_store(),
        malo_cache: Arc::new(SlateDbMaloCache::new(store.clone(), tenant_id)),
        maloid_result_cache: MaloIdentResultCache::new(store.clone()),
        store: Arc::new(store),
        marktd_client: None,
    };
    (state, dispatcher)
}

/// Every process of `workflow` correlated under `key`.
async fn processes_on(
    state: &makod::commands_api::CommandsApiState,
    key: &str,
    workflow: &str,
) -> Vec<mako_engine::ids::ProcessIdentity> {
    state
        .store
        .as_process_registry()
        .lookup_correlated(state.tenant_id, key)
        .await
        .expect("lookup")
        .into_iter()
        .filter(|id| id.workflow_id.name.as_ref() == workflow)
        .collect()
}

// ── An LF-only deployment ────────────────────────────────────────────────────

/// A `rollen = [lf-strom]` deployment routes every MaBiS PID the Lieferant is
/// party to, under the workflow the Netzbetreiber's registration names, and
/// none that runs between network operators, the BKV or the BIKO.
#[test]
fn an_lf_strom_deployment_routes_the_mabis_pids_it_is_party_to() {
    let router = router_for(&[Rolle::LfStrom], &DeploymentRoles::lf());
    for (pid, workflow) in [
        (13_003, "mabis-billing"),
        (13_023, "mabis-billing"),
        (13_010, "mabis-profile"),
        (13_011, "mabis-profile"),
        (13_012, "mabis-profile"),
        (55_065, "mabis-listenabgleich"),
        (55_201, "mabis-listenabgleich"),
        (55_073, "mabis-clearingliste"),
        (55_062, "mabis-zp-lifecycle"),
        (55_199, "mabis-zp-lifecycle"),
    ] {
        assert_eq!(router.route(pid), Some(workflow), "{pid}");
    }
    for pid in [
        55_235, 55_236, 55_237, 55_195, 55_223, 55_071, 55_203, 55_064, 21_003, 13_020, 17_207,
    ] {
        assert_eq!(
            router.route(pid),
            None,
            "{pid} is not addressed to a Lieferant"
        );
    }
}

/// An LF-SZR reaches its settlement on an LF deployment and the Lieferant
/// answers it from `E_0007` (EBD 4.3 Kap. 7.12.1), on its own IFTSTA 21000
/// (Anwendungsübersicht 4.0 lfd. Nr. 7090, LF an NB). The normierten Profile
/// reach `mabis-profile` there too.
#[tokio::test]
async fn an_lf_szr_reaches_the_lf_and_is_answered_from_e_0007() {
    use mako_mabis::{BillingCommand, BillingState, MabisBillingWorkflow};
    let (state, dispatcher) = state_for(LF_MP, "LF").await;
    let router = router_for(&[Rolle::LfStrom], &DeploymentRoles::lf());
    const ZP: &str = "DE00056266802AO6G56M11SN51G21M24S";

    // The 13003 column carries no `CCI` (MSCONS AHB 3.2 Kap. 8.3.3): the
    // series is the one the NB activated the MaBiS-ZP for, the LF-SZR
    // (Kategorie A).
    aktiviere(
        &state,
        ZP,
        mako_mabis::ZpSerie::LieferantensummenzeitreiheNb,
        NB_MP,
        LF_MP,
    )
    .await;
    let wire = skeleton(13_003, NB_MP, LF_MP);
    assert!(
        !wire.contains("CCI+"),
        "the 13003 column has no CCI: {wire}"
    );
    let workflow = router.route(13_003).expect("routed on the LF deployment");
    let outcome = dispatcher
        .dispatch(
            &parse(&wire),
            workflow,
            13_003,
            time::OffsetDateTime::now_utc(),
        )
        .await;
    assert!(
        matches!(outcome, Ok(IngestOutcome::Spawned { .. })),
        "13003: {outcome:?}"
    );

    let ids = processes_on(&state, ZP, "mabis-billing").await;
    assert_eq!(ids.len(), 1, "one settlement on the MaBiS-Zählpunkt");
    let process =
        Process::<MabisBillingWorkflow, _>::from_identity(state.store.clone(), ids[0].clone());
    let BillingState::Offen(data) = process.state().await.expect("state") else {
        panic!("the LF-SZR opened its settlement");
    };
    let version = data.hoechste_version().expect("a version").clone();
    process
        .run(
            BillingCommand::SendPruefmitteilung {
                version,
                pid: mako_engine::types::Pruefidentifikator::const_new(21_000),
                antwortcode: "A06".to_owned(),
                grund: None,
                message_ref: mako_engine::types::MessageRef::new("PM-1"),
            },
            mako_engine::process::RunOptions::new(),
        )
        .await
        .expect("A06 is E_0007's Zustimmung");

    let profil = parse(&skeleton(13_010, NB_MP, LF_MP));
    let outcome = dispatcher
        .dispatch(
            &profil,
            router.route(13_010).expect("routed"),
            13_010,
            time::OffsetDateTime::now_utc(),
        )
        .await;
    assert!(
        matches!(outcome, Ok(IngestOutcome::Spawned { .. })),
        "13010: {outcome:?}"
    );
}

/// A valid LF-AASZR reaches the LF's settlement and is answered from `E_0093`
/// (EBD 4.3, „LF-AASZR prüfen") on 21000 (Anwendungsübersicht 4.0 lfd. Nr.
/// 9590). The 13023 column carries no `CCI` (MSCONS AHB 3.2 Kap. 8.3.3, S.
/// 94–97): its PID alone names the series — one row, NB an LF, LF-AASZR (Kap.
/// 8.3.2, S. 93), so the skeleton goes in unchanged.
#[tokio::test]
async fn an_lf_aaszr_reaches_the_lf_and_is_answered_from_e_0093() {
    use mako_mabis::{BillingCommand, BillingState, MabisBillingWorkflow};
    let (state, dispatcher) = state_for(LF_MP, "LF").await;
    let router = router_for(&[Rolle::LfStrom], &DeploymentRoles::lf());
    const ZP: &str = "DE00056266802AO6G56M11SN51G21M24S";

    let wire = skeleton(13_023, NB_MP, LF_MP);
    assert!(wire.contains(&format!("LOC+172+{ZP}'")), "{wire}");
    assert!(
        !wire.contains("CCI+"),
        "the 13023 column has no CCI: {wire}"
    );
    let workflow = router.route(13_023).expect("routed on the LF deployment");
    let outcome = dispatcher
        .dispatch(
            &parse(&wire),
            workflow,
            13_023,
            time::OffsetDateTime::now_utc(),
        )
        .await;
    assert!(
        matches!(outcome, Ok(IngestOutcome::Spawned { .. })),
        "13023: {outcome:?}"
    );

    let ids = processes_on(&state, ZP, "mabis-billing").await;
    assert_eq!(ids.len(), 1, "one settlement on the MaBiS-Zählpunkt");
    let process =
        Process::<MabisBillingWorkflow, _>::from_identity(state.store.clone(), ids[0].clone());
    let BillingState::Offen(data) = process.state().await.expect("state") else {
        panic!("the LF-AASZR opened its settlement");
    };
    assert_eq!(
        data.zeitreihe.familie(),
        mako_mabis::Familie::LfAaszr,
        "13023 is the LF-AASZR"
    );
    let version = data.hoechste_version().expect("a version").clone();
    process
        .run(
            BillingCommand::SendPruefmitteilung {
                version,
                pid: mako_engine::types::Pruefidentifikator::const_new(21_000),
                antwortcode: "A06".to_owned(),
                grund: None,
                message_ref: mako_engine::types::MessageRef::new("PM-AASZR"),
            },
            mako_engine::process::RunOptions::new(),
        )
        .await
        .expect("A06 is E_0093's Zustimmung");
}

/// A Lieferantenclearingliste reaches the LF's `mabis-listenabgleich` and is
/// answered with a 55066 out of `E_0049` when the list serves an Abonnement.
#[tokio::test]
async fn a_lieferantenclearingliste_reaches_the_lf_and_is_answered_by_55066() {
    let (state, dispatcher) = state_for(LF_MP, "LF").await;
    let router = router_for(&[Rolle::LfStrom], &DeploymentRoles::lf());
    for (pid, antwort) in [(55_065_u32, 55_066_u64), (55_201, 55_202)] {
        let wire = set_element(&skeleton(pid, NB_MP, LF_MP), "LOC+Z15+", MABIS_ZP);
        let msg = parse(&wire);
        let workflow = router.route(pid).expect("the LF receives the list");
        let outcome = dispatcher
            .dispatch(&msg, workflow, pid, time::OffsetDateTime::now_utc())
            .await
            .expect("dispatch");
        assert!(
            matches!(outcome, IngestOutcome::Spawned { .. }),
            "{pid}: {outcome:?}"
        );

        let answered = makod::commands_api::dispatch_command(
            &state,
            "mabis.liste.korrigieren",
            &serde_json::json!({
                "malo_id": MABIS_ZP,
                "sender_rolle": "NB",
                "anforderungsart": "abonnement",
                "marktrolle": "LFN",
                "positionen": [],
            }),
        )
        .await
        .unwrap_or_else(|e| panic!("{pid} is answerable: {e:?}"));
        assert!(
            matches!(
                answered,
                makod::commands_api::DispatchOutcome::Dispatched { .. }
            ),
            "{answered:?}"
        );
        let pending = state.store.pending_now(10).await.expect("outbox");
        let reply = pending
            .iter()
            .find(|p| p.payload["pid"] == antwort)
            .unwrap_or_else(|| panic!("{antwort} is written"));
        let tree = if pid == 55_065 { "E_0049" } else { "E_0096" };
        assert_eq!(reply.payload["antwort_codeliste"], tree);
    }
}

// ── The Netzbetreiber's list ─────────────────────────────────────────────────

fn liste(pid: u32, listennummer: &str) -> serde_json::Value {
    serde_json::json!({
        "pid": pid,
        "anforderungsart": "einzelanforderung",
        "anfrage_referenz": "ORD-17202-1",
        "receiver_mp_id": LF_MP,
        "bilanzierungsmonat": "2026-09",
        "listennummer": listennummer,
        "mabis_zp_id": MABIS_ZP,
        "zeitreihen_version": "20261005090000+00",
        "mitglieder": [{
            "malo": MALO,
            "bilanzierungsbeginn": "202608312200",
            "stammdaten": { "bloecke": [
                { "seq": "Z01", "segmente": [
                    { "tag": "RFF", "elemente": [["Z18", MALO]] },
                    { "tag": "QTY", "elemente": [["Z07", "3500", "KWH"]] },
                    { "tag": "CCI", "elemente": [["Z18"], [], ["10YDE-RWENET---I"]] },
                    { "tag": "CCI", "elemente": [["Z19"], [], ["11XBK-LF-------9"]] },
                    { "tag": "CCI", "elemente": [["Z20"], [], ["11YR00000000000U"]] },
                    { "tag": "CCI", "elemente": [["15"], [], ["Z21"]] },
                    { "tag": "CAV", "elemente": [["LGS"]] },
                ]},
            ]},
        }],
    })
}

/// The NB sends a 55065 and the process waits for the Korrekturliste; the
/// list it writes is a Listennachricht naming the Anfrage and the member.
#[tokio::test]
async fn an_nb_sends_a_55065_and_waits_for_the_korrekturliste() {
    let (state, _) = state_for(NB_MP, "NB").await;
    let outcome = makod::commands_api::dispatch_command(
        &state,
        "mabis.liste.senden",
        &liste(55_065, "LCL-2026-09-1"),
    )
    .await
    .unwrap_or_else(|e| panic!("mabis.liste.senden: {e:?}"));
    assert!(
        matches!(
            outcome,
            makod::commands_api::DispatchOutcome::Spawned { .. }
        ),
        "{outcome:?}"
    );
    let ids = processes_on(
        &state,
        &mako_mabis::listenabgleich::liste_key("LCL-2026-09-1"),
        "mabis-listenabgleich",
    )
    .await;
    assert_eq!(ids.len(), 1);
    let process = Process::<MabisListenabgleichWorkflow, _>::from_identity(
        state.store.clone(),
        ids[0].clone(),
    );
    assert_eq!(
        process.state().await.expect("state").label(),
        "ListeGesendet"
    );
    let pending = state.store.pending_now(10).await.expect("outbox");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].payload["pid"], 55_065);
    assert_eq!(pending[0].payload["anfrage_referenz"], "ORD-17202-1");
}

/// The Lieferant's own rendering of its Korrekturliste, from the receiver's
/// workflow, as it travels back to the NB.
fn korrekturliste_der_lf(listennummer: &str, grund: Option<&str>) -> edi_energy::AnyMessage {
    let empfangen = MabisListenabgleichWorkflow::handle(
        &ListenabgleichState::New,
        ListenabgleichCommand::ReceiveListe {
            pid: mako_engine::types::Pruefidentifikator::const_new(55_065),
            sender: mako_engine::types::MarktpartnerCode::new(NB_MP),
            receiver: mako_engine::types::MarktpartnerCode::new(LF_MP),
            billing_period: mako_engine::types::BillingPeriod::new("2026-09"),
            message_ref: mako_engine::types::MessageRef::new("LIST-1"),
            mabis_zaehlpunkt: MABIS_ZP.to_owned(),
            zeitreihen_version: "20261005090000+00".to_owned(),
            listennummer: listennummer.to_owned(),
            validation_passed: true,
            validation_errors: vec![],
        },
    )
    .expect("the LF records the list");
    let state = empfangen
        .events
        .iter()
        .fold(ListenabgleichState::New, MabisListenabgleichWorkflow::apply);
    let positionen = grund
        .map(|_| {
            vec![mako_pruefung::mabis::Korrekturposition {
                malo: MALO.to_owned(),
                grund: mako_pruefung::mabis::Korrekturgrund::DatenFehlerhaft,
            }]
        })
        .unwrap_or_default();
    let out = MabisListenabgleichWorkflow::handle(
        &state,
        ListenabgleichCommand::SendKorrektur {
            sender_rolle: "NB".to_owned(),
            anforderungsart: Some(Anforderungsart::Einzelanforderung),
            positionen,
        },
    )
    .expect("the LF answers");
    let pending = &out.outbox[0];
    let msg = mako_engine::outbox::OutboxMessage::new(
        StreamId::new("process/lf"),
        mako_engine::ids::ProcessId::new(),
        TenantId::from_party_id(LF_MP),
        mako_engine::ids::CorrelationId::new(),
        mako_engine::ids::ConversationId::new(),
        mako_engine::ids::EventId::new(),
        "UTILMD",
        NB_MP,
        pending.payload.clone(),
    );
    let registry =
        makod::party_registry::MpIdRegistry::from_config(&[makod::config::PartyConfig {
            mp_id: LF_MP.to_owned(),
            roles: vec!["LF".to_owned()],
            primary: true,
            agency: None,
            as4: None,
        }])
        .expect("one party");
    let wire = makod::edifact_renderer::render_to_wire_bytes(&msg, &registry)
        .expect("the Korrekturliste renders");
    let text = String::from_utf8_lossy(&wire.bytes).into_owned();
    let text = match grund {
        Some(code) => text.replace("STS+E01++A07:E_0047", &format!("STS+E01++{code}:E_0047")),
        None => text,
    };
    parse(&text)
}

/// A 55066 closes the Vorgang, checked against `E_0047`.
#[tokio::test]
async fn a_55066_resumes_the_nbs_list_and_closes_it() {
    let (state, dispatcher) = state_for(NB_MP, "NB").await;
    makod::commands_api::dispatch_command(&state, "mabis.liste.senden", &liste(55_065, "LCL-A"))
        .await
        .expect("the NB sends the list");

    let antwort = korrekturliste_der_lf("LCL-A", Some("A07"));
    let outcome = dispatcher
        .dispatch(
            &antwort,
            "mabis-listenabgleich",
            55_066,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("dispatch");
    assert!(
        matches!(outcome, IngestOutcome::Dispatched { .. }),
        "{outcome:?}"
    );

    let ids = processes_on(
        &state,
        &mako_mabis::listenabgleich::liste_key("LCL-A"),
        "mabis-listenabgleich",
    )
    .await;
    let process = Process::<MabisListenabgleichWorkflow, _>::from_identity(
        state.store.clone(),
        ids[0].clone(),
    );
    match process.state().await.expect("state") {
        ListenabgleichState::Korrigiert { korrekturen, .. } => assert_eq!(korrekturen, 1),
        other => panic!("expected Korrigiert, got {}", other.label()),
    }
    let events = state
        .store
        .load(&StreamId::for_process(state.tenant_id, &ids[0].process_id))
        .await
        .expect("stream");
    assert!(
        events
            .iter()
            .any(|e| e.event_type.as_ref() == "MabisKorrekturErhalten")
    );
}

/// A code outside `E_0047` is refused, and the list stays open for the answer
/// it is owed.
#[tokio::test]
async fn a_code_outside_e_0047_is_refused() {
    let (state, dispatcher) = state_for(NB_MP, "NB").await;
    makod::commands_api::dispatch_command(&state, "mabis.liste.senden", &liste(55_065, "LCL-B"))
        .await
        .expect("the NB sends the list");

    // `A08` exists in no Clearinglisten tree.
    let antwort = korrekturliste_der_lf("LCL-B", Some("A08"));
    let outcome = dispatcher
        .dispatch(
            &antwort,
            "mabis-listenabgleich",
            55_066,
            time::OffsetDateTime::now_utc(),
        )
        .await;
    assert!(
        !matches!(outcome, Ok(IngestOutcome::Dispatched { .. })),
        "a code E_0047 does not publish must not close the list: {outcome:?}"
    );
    let ids = processes_on(
        &state,
        &mako_mabis::listenabgleich::liste_key("LCL-B"),
        "mabis-listenabgleich",
    )
    .await;
    let process = Process::<MabisListenabgleichWorkflow, _>::from_identity(
        state.store.clone(),
        ids[0].clone(),
    );
    assert_eq!(
        process.state().await.expect("state").label(),
        "ListeGesendet"
    );
}

/// Record the activation of `zp` for `serie` as this deployment received it —
/// the 55062 a receiver of a Summenzeitreihe holds before any version arrives
/// (BK6-24-174 Anlage 3 Kap. 7.4.1, S. 83).
async fn aktiviere(
    state: &makod::commands_api::CommandsApiState,
    zp: &str,
    serie: mako_mabis::ZpSerie,
    sender: &str,
    receiver: &str,
) {
    use mako_engine::event_store::CorrelationEntry;
    use mako_engine::process::RunOptions;
    use mako_engine::types::{BillingPeriod, MarktpartnerCode, MessageRef, Pruefidentifikator};
    let process = Process::<mako_mabis::MabisZpLifecycleWorkflow, _>::new(
        state.store.clone(),
        state.tenant_id,
        mako_engine::version::WorkflowId::new("mabis-zp-lifecycle", "FV2026-10-01"),
    );
    let korrelation = CorrelationEntry {
        tenant_id: state.tenant_id,
        tag: zp.to_owned(),
        process_id: process.process_id(),
        identity: process.identity(),
    };
    process
        .run(
            mako_mabis::ZpLifecycleCommand::ReceiveAnfrage {
                pid: Pruefidentifikator::const_new(55_062),
                serie,
                vorgang: mako_mabis::ZpVorgang::Aktivierung,
                mabis_zp_id: zp.to_owned(),
                sender: MarktpartnerCode::new(sender),
                receiver: MarktpartnerCode::new(receiver),
                billing_period: BillingPeriod::new("2026-09"),
                document_date: "20260801".to_owned(),
                message_ref: MessageRef::new("AKT-1"),
                validation_passed: true,
                validation_errors: vec![],
            },
            RunOptions::new().correlations(&[korrelation]),
        )
        .await
        .expect("the activation is recorded");
}

/// An unmodified 13003 skeleton is accepted on every deployment the PID table
/// names as a receiver (NB → LF, BIKO → NB, BIKO → BKV, BIKO → ÜNB, NB → BIKO),
/// its series read off the MaBiS-ZP's activation; without one it is refused
/// by name, never guessed.
#[tokio::test]
async fn a_conformant_13003_is_accepted_by_every_receiver() {
    use mako_engine::builder::EngineModule as _;
    use mako_engine::marktrolle::Marktrolle;
    use mako_mabis::{BillingState, MabisBillingWorkflow, ZpSerie};
    const BIKO_MP: &str = "9900000000027";
    for (empfaenger, rolle, serie, sender) in [
        (
            Marktrolle::Lf,
            "LF",
            ZpSerie::LieferantensummenzeitreiheNb,
            NB_MP,
        ),
        (
            Marktrolle::Nb,
            "NB",
            ZpSerie::Abrechnungssummenzeitreihe,
            BIKO_MP,
        ),
        (
            Marktrolle::Bkv,
            "BKV",
            ZpSerie::BilanzkreissummenzeitreiheNb,
            BIKO_MP,
        ),
        (
            Marktrolle::Uenb,
            "ÜNB",
            ZpSerie::Abrechnungssummenzeitreihe,
            BIKO_MP,
        ),
        (
            Marktrolle::Biko,
            "BIKO",
            ZpSerie::BilanzkreissummenzeitreiheNb,
            NB_MP,
        ),
    ] {
        let mut router = PidRouter::new();
        mako_mabis::MabisModule
            .register_pids_with_roles(&mut router, &DeploymentRoles::from_roles([empfaenger]));
        let workflow = router
            .route(13_003)
            .unwrap_or_else(|| panic!("{rolle} receives 13003"));
        let (state, dispatcher) = state_for(LF_MP, rolle).await;
        let wire = skeleton(13_003, sender, LF_MP);
        let zp = makod::ingest_dispatcher::extract_malo_from_msg(&parse(&wire));

        let ohne = dispatcher
            .dispatch(
                &parse(&wire),
                workflow,
                13_003,
                time::OffsetDateTime::now_utc(),
            )
            .await;
        assert!(
            ohne.as_ref()
                .is_err_and(|e| e.to_string().contains("Aktivierung")),
            "{rolle}: without an activation the series is unknown: {ohne:?}"
        );

        aktiviere(&state, zp.as_str(), serie, sender, LF_MP).await;
        let outcome = dispatcher
            .dispatch(
                &parse(&wire),
                workflow,
                13_003,
                time::OffsetDateTime::now_utc(),
            )
            .await;
        assert!(
            matches!(outcome, Ok(IngestOutcome::Spawned { .. })),
            "{rolle}: {outcome:?}"
        );
        let ids = processes_on(&state, zp.as_str(), "mabis-billing").await;
        let process =
            Process::<MabisBillingWorkflow, _>::from_identity(state.store.clone(), ids[0].clone());
        let BillingState::Offen(data) = process.state().await.expect("state") else {
            panic!("{rolle}: the version opened its settlement");
        };
        assert_eq!(Some(data.zeitreihe), serie.zeitreihe(), "{rolle}");
    }
}

/// The NB receives the LF's Profil-Reklamation (ORDERS 17211, ORDERS AHB 1.1b
/// Kap. 5.5, S. 126–127) on `mabis-profile`, keyed on its sender and `BGM`
/// DE 1004 — the ORDERS names no Lokation — and holds its `E_0100` code. No
/// answer goes out: the NB corrects by a new version (BK6-24-174 Anlage 3
/// Kap. 6.5.2 Nr. 2, S. 69).
#[tokio::test]
async fn the_nb_receives_a_profil_reklamation() {
    use mako_mabis::{MabisProfilWorkflow, ProfilState};
    let (state, dispatcher) = state_for(NB_MP, "NB").await;
    let router = router_for(&[Rolle::NbStrom], &DeploymentRoles::nb());
    let wire = skeleton(17_211, LF_MP, NB_MP);
    let msg = parse(&wire);
    let workflow = router.route(17_211).expect("the NB receives 17211");
    let outcome = dispatcher
        .dispatch(&msg, workflow, 17_211, time::OffsetDateTime::now_utc())
        .await;
    assert!(
        matches!(outcome, Ok(IngestOutcome::Spawned { .. })),
        "17211: {outcome:?}"
    );
    let key = mako_mabis::reklamation_key(LF_MP, "DOK17211");
    let ids = processes_on(&state, &key, "mabis-profile").await;
    assert_eq!(ids.len(), 1, "one record per Reklamation");
    let process =
        Process::<MabisProfilWorkflow, _>::from_identity(state.store.clone(), ids[0].clone());
    let ProfilState::ReklamationErhalten(r) = process.state().await.expect("state") else {
        panic!("the NB holds the Reklamation");
    };
    assert!(!r.antwortcode.is_empty(), "FTX+AFH carries the E_0100 code");
    let pending = state.store.pending_now(10).await.expect("outbox");
    assert!(
        pending
            .iter()
            .all(|p| matches!(&*p.message_type, "APERAK" | "CONTRL")),
        "nothing but the technical acknowledgement answers a Reklamation"
    );
}
