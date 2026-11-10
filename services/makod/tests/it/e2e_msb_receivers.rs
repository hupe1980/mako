//! The ORDERS a Messstellenbetreiber Strom receives reach a process on a MSB
//! deployment, and their Antwortfrist is registered.
//!
//! Four GPKE ORDERS are addressed to the MSB and owe it an answer: the
//! Anfrage Sperrung 17116 (NB an MSB, ORDERS AHB 1.1b Kap. 4.3, S. 25), the
//! Anfrage von Werten 17102 (LF an MSB, Kap. 4.1.2.2, S. 15), the Anforderung
//! von Werten 17004 (LF · NB · MSB an MSB, Kap. 4.13, S. 90) and the
//! Reklamation von Werten 17113 (an MSB, Kap. 4.8, S. 70). A `rollen =
//! [msb-strom]` deployment that dead-letters them never sees the Frist it is
//! running out of — and for 17116 the lapse decides the matter: „Verstreicht
//! die Frist, ohne dass die Antwort auf die Anfrage beim NB eingeht, gilt dies
//! als Zustimmung" (BK6-24-174 GPKE Teil 2 Kap. 3.5.1.2 Nr. 4, S. 107).
//!
//! The ORDRSP 19007, the Ablehnung of an Anforderung von Werten (ORDRSP AHB
//! 1.1b Kap. 4.13, S. 56), is the other side's: it resumes the requesting
//! `gpke-datenabruf` process by the ORDERS' Dokumentennummer it echoes in
//! `SG1 RFF+ON`.

use std::collections::BTreeSet;
use std::sync::Arc;

use mako_engine::deadline::DeadlineStore as _;
use mako_engine::event_store::CorrelationEntry;
use mako_engine::marktrolle::DeploymentRoles;
use mako_engine::pid_router::PidRouter;
use mako_engine::process::{Process, RunOptions};
use mako_engine::registry::ProcessRegistry as _;
use mako_engine::types::{MarktpartnerCode, MessageRef, Pruefidentifikator, Sparte};
use mako_engine::version::WorkflowId;
use mako_engine::{ids::TenantId, store_slatedb::SlateDbStore};
use mako_gpke::{
    DATENABRUF_MSB_WORKFLOW_NAME, DATENABRUF_WORKFLOW_NAME, DatenabrufCommand, DatenabrufMsbState,
    DatenabrufState, GpkeDatenabrufMsbWorkflow, GpkeDatenabrufWorkflow, GpkeSperrungMsbWorkflow,
    SPERRUNG_MSB_WORKFLOW_NAME, SperrungMsbState,
};
use makod::deployment::Rolle;
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

const OWN_MP: &str = "9900357000004";
const NB_MP: &str = "9900000000003";
const MALO: &str = "51238696012";
const MELO: &str = "DE00056266802AO6G56M11SN51G21M24S";

/// The router a deployment declaring `rollen` builds under `roles`.
fn router_for(rollen: &[Rolle], roles: &DeploymentRoles) -> PidRouter {
    let rollen: BTreeSet<Rolle> = rollen.iter().copied().collect();
    let mut router = PidRouter::new();
    for module in makod::startup::modules_for(&rollen) {
        module.register_pids_with_roles(&mut router, roles);
    }
    router
}

/// The newest profile's skeleton of `pid`, from `NB_MP` to this MSB.
fn skeleton(pid: u32) -> String {
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
                sender: NB_MP.to_owned(),
                receiver: OWN_MP.to_owned(),
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

async fn store_and_dispatcher() -> (SlateDbStore, EdifactIngestDispatcher) {
    let store = SlateDbStore::open_in_memory()
        .await
        .expect("in-memory store");
    let dispatcher = EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        TenantId::from_party_id(OWN_MP),
    );
    (store, dispatcher)
}

/// Every process of `workflow` correlated under `key`.
async fn processes_on(
    store: &SlateDbStore,
    key: &str,
    workflow: &str,
) -> Vec<mako_engine::ids::ProcessIdentity> {
    store
        .as_process_registry()
        .lookup_correlated(TenantId::from_party_id(OWN_MP), key)
        .await
        .expect("lookup")
        .into_iter()
        .filter(|id| id.workflow_id.name.as_ref() == workflow)
        .collect()
}

/// Every registered deadline carrying `label`.
async fn deadlines_labelled(store: &SlateDbStore, label: &str) -> Vec<time::OffsetDateTime> {
    store
        .as_deadline_store()
        .due_now(1_000)
        .await
        .expect("read deadlines")
        .deadlines
        .iter()
        .filter(|d| d.label() == label)
        .map(mako_engine::deadline::Deadline::due_at)
        .collect()
}

// ── Routing ──────────────────────────────────────────────────────────────────

/// A MSB Strom deployment routes the four ORDERS it receives to the MSB-side
/// workflows — and no GPKE PID of the LF or the NB, so a misaddressed
/// Anmeldung is refused as an unknown PID rather than answered as a NB.
#[test]
fn a_msb_strom_deployment_routes_the_orders_it_receives() {
    let router = router_for(&[Rolle::MsbStrom], &DeploymentRoles::msb());
    for pid in [17_004, 17_102, 17_113] {
        assert_eq!(
            router.route(pid),
            Some(DATENABRUF_MSB_WORKFLOW_NAME),
            "{pid}"
        );
    }
    assert_eq!(router.route(17_116), Some(SPERRUNG_MSB_WORKFLOW_NAME));
    assert_eq!(
        router.route_with_sparte(17_116, Sparte::Strom),
        Some(SPERRUNG_MSB_WORKFLOW_NAME)
    );
    for pid in [
        55_001, 55_004, 17_115, 17_117, 19_007, 19_102, 19_118, 17_132,
    ] {
        let routed = router.route(pid);
        assert!(
            routed.is_none_or(|wf| !wf.starts_with("gpke-")),
            "{pid} reaches {routed:?} on a MSB-only deployment"
        );
    }
}

/// A NB or LF deployment sends those ORDERS and never receives them: none
/// of them routes there, and the MSB's answers resume the sender's process.
#[test]
fn a_nb_or_lf_deployment_does_not_route_the_msbs_orders() {
    for (rollen, roles) in [
        (&[Rolle::NbStrom][..], DeploymentRoles::nb()),
        (&[Rolle::LfStrom][..], DeploymentRoles::lf()),
    ] {
        let router = router_for(rollen, &roles);
        for pid in [17_004, 17_102, 17_113, 17_116] {
            assert_eq!(router.route(pid), None, "{pid} on {rollen:?}");
        }
        for pid in [19_007, 19_102, 19_114] {
            assert_eq!(
                router.route(pid),
                Some(DATENABRUF_WORKFLOW_NAME),
                "{pid} on {rollen:?}"
            );
        }
    }
    // A Stadtwerk that is NB and MSB at once receives its own Anfrage
    // Sperrung on the MSB side, never on the NB's Sperrauftrag.
    let both = router_for(
        &[Rolle::NbStrom, Rolle::MsbStrom],
        &DeploymentRoles::nb_msb(),
    );
    assert_eq!(both.route(17_116), Some(SPERRUNG_MSB_WORKFLOW_NAME));
    assert_eq!(both.route(17_115), Some("gpke-sperrung"));
}

// ── Anfrage Sperrung 17116 ───────────────────────────────────────────────────

#[tokio::test]
async fn an_inbound_anfrage_sperrung_opens_the_msbs_process_with_its_three_werktage() {
    let (store, dispatcher) = store_and_dispatcher().await;
    let msg = parse(&set_element(&skeleton(17_116), "LOC+172+", MALO));
    // Freitag 02.10.2026 → Ablauf Mittwoch 07.10.2026 (Samstag 03.10. is no
    // Werktag either way). A past instant, so the deadline reads back as due.
    let eingang = time::macros::datetime!(2026-10-02 09:00 UTC);
    let outcome = dispatcher
        .dispatch(&msg, SPERRUNG_MSB_WORKFLOW_NAME, 17_116, eingang)
        .await
        .expect("dispatch");
    assert!(
        matches!(outcome, IngestOutcome::Spawned { .. }),
        "{outcome:?}"
    );

    let ids = processes_on(&store, MALO, SPERRUNG_MSB_WORKFLOW_NAME).await;
    assert_eq!(ids.len(), 1, "one MSB process on the MaLo");
    let state = Process::<GpkeSperrungMsbWorkflow, _>::from_identity(
        Arc::new(store.clone()),
        ids[0].clone(),
    )
    .state()
    .await
    .expect("state");
    assert!(matches!(state, SperrungMsbState::Offen(_)), "{state:?}");

    assert_eq!(
        deadlines_labelled(&store, mako_gpke::sperrung_msb::ANTWORT_WINDOW_LABEL).await,
        vec![
            mako_fristen::antwort::antwort_deadline(17_116, eingang)
                .expect("17116 has a published Antwortfrist")
        ],
        "the 3-WT window of GPKE Teil 2 Kap. 3.5.1.2 Nr. 4"
    );
    assert_eq!(
        mako_fristen::antwort::antwort_deadline(17_116, eingang),
        Some(mako_fristen::end_of_day_berlin(time::macros::date!(
            2026 - 10 - 07
        )))
    );
}

// ── Datenabruf 17102 / 17004 / 17113 ─────────────────────────────────────────

#[tokio::test]
async fn an_inbound_anfrage_von_werten_opens_the_msbs_process_with_its_werktag() {
    let (store, dispatcher) = store_and_dispatcher().await;
    let msg = parse(&skeleton(17_102));
    // Freitag 02.10.2026 → Ablauf Montag 05.10.2026.
    let eingang = time::macros::datetime!(2026-10-02 09:00 UTC);
    let outcome = dispatcher
        .dispatch(&msg, DATENABRUF_MSB_WORKFLOW_NAME, 17_102, eingang)
        .await
        .expect("dispatch");
    assert!(
        matches!(outcome, IngestOutcome::Spawned { .. }),
        "{outcome:?}"
    );

    let key = mako_gpke::datenabruf_msb::anfrage_key(NB_MP, "DOK17102");
    let ids = processes_on(&store, &key, DATENABRUF_MSB_WORKFLOW_NAME).await;
    assert_eq!(ids.len(), 1);
    assert_eq!(
        processes_on(&store, MELO, DATENABRUF_MSB_WORKFLOW_NAME)
            .await
            .len(),
        1,
        "found by the Lokation too"
    );
    let state = Process::<GpkeDatenabrufMsbWorkflow, _>::from_identity(
        Arc::new(store.clone()),
        ids[0].clone(),
    )
    .state()
    .await
    .expect("state");
    assert!(
        matches!(&state, DatenabrufMsbState::Offen { pid, .. } if pid.as_u32() == 17_102),
        "{state:?}"
    );
    assert_eq!(
        deadlines_labelled(&store, mako_gpke::datenabruf_msb::ANTWORT_WINDOW_LABEL).await,
        vec![mako_fristen::end_of_day_berlin(time::macros::date!(
            2026 - 10 - 05
        ))],
        "1 WT — GPKE Teil 4 Kap. 3.2 Nr. 4"
    );
}

/// Each Anforderung is its own process: two on one Messlokation both open,
/// each with the Ablehnung window its row of `mako_fristen::antwort`
/// publishes — 2 WT for the Anforderung, 3 WT for the Reklamation (WiM Strom
/// Teil 2 Kap. 2.6.3 / 2.7.3 Nr. 2, S. 40 / 47) — and none under the 17102
/// answer label.
#[tokio::test]
async fn every_anforderung_von_werten_opens_its_own_process() {
    let (store, dispatcher) = store_and_dispatcher().await;
    let eingang = time::macros::datetime!(2026-10-02 09:00 UTC);
    for nummer in ["DOK-A", "DOK-B"] {
        let msg = parse(&set_element(&skeleton(17_004), "BGM+7+", nummer));
        let outcome = dispatcher
            .dispatch(&msg, DATENABRUF_MSB_WORKFLOW_NAME, 17_004, eingang)
            .await
            .expect("dispatch");
        assert!(
            matches!(outcome, IngestOutcome::Spawned { .. }),
            "{nummer}: {outcome:?}"
        );
    }
    let msg = parse(&skeleton(17_113));
    let outcome = dispatcher
        .dispatch(&msg, DATENABRUF_MSB_WORKFLOW_NAME, 17_113, eingang)
        .await
        .expect("dispatch");
    assert!(
        matches!(outcome, IngestOutcome::Spawned { .. }),
        "17113: {outcome:?}"
    );
    assert_eq!(
        processes_on(&store, MELO, DATENABRUF_MSB_WORKFLOW_NAME)
            .await
            .len(),
        3
    );
    assert!(
        deadlines_labelled(&store, mako_gpke::datenabruf_msb::ANTWORT_WINDOW_LABEL)
            .await
            .is_empty(),
        "17004 and 17113 open no answer window"
    );
    // Freitag 02.10.2026 → Dienstag 06.10. (2 WT) and Mittwoch 07.10. (3 WT).
    let mut ablehnung =
        deadlines_labelled(&store, mako_gpke::datenabruf_msb::ABLEHNUNG_WINDOW_LABEL).await;
    ablehnung.sort();
    assert_eq!(
        ablehnung,
        vec![
            mako_fristen::end_of_day_berlin(time::macros::date!(2026 - 10 - 06)),
            mako_fristen::end_of_day_berlin(time::macros::date!(2026 - 10 - 06)),
            mako_fristen::end_of_day_berlin(time::macros::date!(2026 - 10 - 07)),
        ]
    );
}

// ── Ablehnung 19007 ──────────────────────────────────────────────────────────

/// The MSB's 19007 resumes the 17004 process this side sent, found by the
/// Dokumentennummer the ORDRSP echoes in `SG1 RFF+ON`.
#[tokio::test]
async fn a_19007_resumes_the_anforderung_it_answers() {
    let (store, dispatcher) = store_and_dispatcher().await;
    let tenant = TenantId::from_party_id(OWN_MP);
    let sent = time::macros::datetime!(2026-11-06 09:00 UTC);
    let process = Process::<GpkeDatenabrufWorkflow, _>::new(
        Arc::new(store.clone()),
        tenant,
        WorkflowId::new(DATENABRUF_WORKFLOW_NAME, "FV2026-10-01"),
    );
    let key = CorrelationEntry {
        tenant_id: tenant,
        tag: "DOK17004".to_owned(),
        process_id: process.process_id(),
        identity: process.identity(),
    };
    process
        .run(
            DatenabrufCommand::SendAnfrage {
                orders_pid: Pruefidentifikator::new(17_004).expect("five digits"),
                recipient: MarktpartnerCode::new(NB_MP),
                malo: None,
                message_ref: MessageRef::new("DOK17004"),
                payload: serde_json::json!({}),
                gesendet_am: sent,
            },
            RunOptions::new().correlations(&[key]),
        )
        .await
        .expect("the Anforderung is sent");

    let msg = parse(&set_element(&skeleton(19_007), "RFF+ON:", "DOK17004"));
    let outcome = dispatcher
        .dispatch(&msg, DATENABRUF_WORKFLOW_NAME, 19_007, sent)
        .await
        .expect("dispatch");
    assert!(
        matches!(outcome, IngestOutcome::Dispatched { .. }),
        "{outcome:?}"
    );
    let state = process.state().await.expect("state");
    assert!(
        matches!(&state, DatenabrufState::Abgelehnt { ordrsp_pid, .. } if ordrsp_pid.as_u32() == 19_007),
        "{state:?}"
    );
}

// ── Technikänderung 17011 ────────────────────────────────────────────────────

/// The MSB routes the Beauftragung Änderung der Technik (ORDERS AHB 1.1b
/// Kap. 4.12, „NB, LF an MSB"); a NB, which sends it, does not.
#[test]
fn only_a_msb_routes_the_technikaenderung_auftrag() {
    let msb = router_for(&[Rolle::MsbStrom], &DeploymentRoles::msb());
    assert_eq!(
        msb.route(17_011),
        Some(mako_wim::TECHNIK_AENDERUNG_WORKFLOW_NAME)
    );
    let nb = router_for(&[Rolle::NbStrom], &DeploymentRoles::nb());
    assert_eq!(nb.route(17_011), None);
    assert_eq!(
        nb.route(19_005),
        Some(mako_wim::TECHNIK_AENDERUNG_WORKFLOW_NAME),
        "the NB receives the MSB's answer"
    );
}

#[tokio::test]
async fn an_inbound_technikaenderung_auftrag_opens_the_msbs_process_with_its_ten_werktage() {
    let (store, dispatcher) = store_and_dispatcher().await;
    let msg = parse(&skeleton(17_011));
    let eingang = time::macros::datetime!(2026-10-02 09:00 UTC);
    let outcome = dispatcher
        .dispatch(
            &msg,
            mako_wim::TECHNIK_AENDERUNG_WORKFLOW_NAME,
            17_011,
            eingang,
        )
        .await
        .expect("dispatch");
    assert!(
        matches!(outcome, IngestOutcome::Spawned { .. }),
        "{outcome:?}"
    );
    let key = mako_wim::technik_aenderung::auftrag_key(NB_MP, "DOK17011");
    let ids = processes_on(&store, &key, mako_wim::TECHNIK_AENDERUNG_WORKFLOW_NAME).await;
    assert_eq!(ids.len(), 1);
    assert_eq!(
        processes_on(&store, MELO, mako_wim::TECHNIK_AENDERUNG_WORKFLOW_NAME)
            .await
            .len(),
        1,
        "found by the Messlokation too"
    );
    let state = Process::<mako_wim::WimTechnikAenderungWorkflow, _>::from_identity(
        Arc::new(store.clone()),
        ids[0].clone(),
    )
    .state()
    .await
    .expect("state");
    assert!(
        matches!(
            state,
            mako_wim::TechnikAenderungState::AuftragEingegangen(_)
        ),
        "{state:?}"
    );
    assert_eq!(
        deadlines_labelled(&store, mako_wim::technik_aenderung::EINGANG_WINDOW_LABEL).await,
        vec![
            mako_fristen::antwort::antwort_deadline(17_011, eingang)
                .expect("17011 has a published Antwortfrist")
        ],
        "10 WT — WiM Teil 1 Kap. 3.3.1.2 Nr. 2"
    );
}

// ── Anfrage Sperrung Gas 17116 (GNB an gMSB) ─────────────────────────────────

/// A gMSB deployment routes the GNB's Anfrage Sperrung to its own workflow —
/// Sparte-qualified Gas — and no GeLi Gas PID of the GNB or the LF; on a
/// deployment that is GNB and gMSB at once the GNB's Sperrauftrag stays on the
/// GNB's side.
#[test]
fn a_msb_gas_deployment_routes_the_anfrage_sperrung_to_the_gmsb() {
    let gmsb = mako_geli_gas::sperrung_msb::WORKFLOW_NAME;
    let router = router_for(&[Rolle::MsbGas], &DeploymentRoles::msb());
    assert_eq!(router.route_with_sparte(17_116, Sparte::Gas), Some(gmsb));
    for pid in [44_001, 44_013, 17_115, 17_117, 19_118, 31_011] {
        let routed = router.route(pid);
        assert!(
            routed.is_none_or(|wf| !wf.starts_with("geli-gas-")),
            "{pid} reaches {routed:?} on a gMSB-only deployment"
        );
    }

    let both = router_for(&[Rolle::NbGas, Rolle::MsbGas], &DeploymentRoles::nb_msb());
    assert_eq!(both.route_with_sparte(17_116, Sparte::Gas), Some(gmsb));
    assert_eq!(
        both.route_with_sparte(17_115, Sparte::Gas),
        Some("geli-gas-sperrung-nb")
    );

    let gnb = router_for(&[Rolle::NbGas], &DeploymentRoles::nb());
    assert_eq!(gnb.route(17_116), None, "the GNB sends the 17116");
}

/// An inbound Gas 17116 opens the gMSB's process on the Marktlokation. The
/// gMSB's Frist is set by the AWH Sperrprozesse Gas, which is not mirrored, so
/// no window is registered.
#[tokio::test]
async fn an_inbound_gas_anfrage_sperrung_opens_the_gmsbs_process() {
    use mako_geli_gas::sperrung_msb::{
        GeliGasSperrungMsbWorkflow, SperrungMsbGasState, WORKFLOW_NAME,
    };
    let (store, dispatcher) = store_and_dispatcher().await;
    let msg = parse(&set_element(&skeleton(17_116), "LOC+172+", MALO));
    let eingang = time::macros::datetime!(2026-10-02 09:00 UTC);
    let outcome = dispatcher
        .dispatch(&msg, WORKFLOW_NAME, 17_116, eingang)
        .await
        .expect("dispatch");
    assert!(
        matches!(outcome, IngestOutcome::Spawned { .. }),
        "{outcome:?}"
    );
    let ids = processes_on(&store, MALO, WORKFLOW_NAME).await;
    assert_eq!(ids.len(), 1, "one gMSB process on the MaLo");
    let state = Process::<GeliGasSperrungMsbWorkflow, _>::from_identity(
        Arc::new(store.clone()),
        ids[0].clone(),
    )
    .state()
    .await
    .expect("state");
    assert!(
        matches!(&state, SperrungMsbGasState::Offen(a) if a.msb.as_str() == OWN_MP),
        "{state:?}"
    );
}

// ── GPKE Teil 3 Konfiguration ────────────────────────────────────────────────

/// Each role routes the GPKE Teil 3 ORDERS it receives to the receiving side
/// of `gpke-konfiguration-aenderung` (Anwendungsübersicht 4.0 lfd. Nr.
/// 22180–23280): the MSB the NB's and the LF's Bestellungen, the weiterer MSB's
/// 17118 / 17128–17130 and the Einrichtung 17134 / 17135; the NB the LF's
/// 17120 / 17122 / 17123 / 17133; the LF the Reklamation einer Definition
/// 17122. 17118 and the 19127 that answers it resolve to one workflow.
#[test]
fn each_role_routes_the_konfiguration_orders_it_receives() {
    let wf = mako_gpke::KONFIGURATION_AENDERUNG_WORKFLOW_NAME;
    let msb = router_for(&[Rolle::MsbStrom], &DeploymentRoles::msb());
    for pid in [
        17_118, 17_121, 17_123, 17_128, 17_129, 17_130, 17_131, 17_134, 17_135, 19_127,
    ] {
        assert_eq!(msb.route(pid), Some(wf), "{pid} on a MSB deployment");
    }
    assert_eq!(msb.route(17_120), None, "the LF's 17120 is the NB's");

    let nb = router_for(&[Rolle::NbStrom], &DeploymentRoles::nb());
    for pid in [17_120, 17_122, 17_123, 17_133] {
        assert_eq!(nb.route(pid), Some(wf), "{pid} on a NB deployment");
    }
    for pid in [17_118, 17_121, 17_134] {
        assert_eq!(nb.route(pid), None, "{pid} is the MSB's");
    }

    let lf = router_for(&[Rolle::LfStrom], &DeploymentRoles::lf());
    assert_eq!(lf.route(17_122), Some(wf));
    assert_eq!(lf.route(17_120), None);

    let all = router_for(
        &[Rolle::NbStrom, Rolle::LfStrom, Rolle::MsbStrom],
        &DeploymentRoles::all(),
    );
    assert_eq!(all.route(17_118), all.route(19_127));
    assert_eq!(all.route(17_118), Some(wf));
}

/// Every GPKE Teil 3 ORDERS opens the recipient's process under its sender
/// and `BGM` DE 1004 instead of dead-lettering; the 17120 opens the NB's 1-WT
/// window (GPKE Teil 3 Kap. 1.3.2.2 Nr. 2).
#[tokio::test]
async fn every_konfiguration_orders_opens_the_recipients_process() {
    use mako_gpke::KonfigurationAenderungState;
    let wf = mako_gpke::KONFIGURATION_AENDERUNG_WORKFLOW_NAME;
    let (store, dispatcher) = store_and_dispatcher().await;
    let eingang = time::macros::datetime!(2026-10-07 10:00 UTC);
    for pid in [
        17_118, 17_120, 17_121, 17_122, 17_123, 17_128, 17_129, 17_130, 17_131, 17_133, 17_134,
        17_135,
    ] {
        let wire = skeleton(pid);
        let msg = parse(&wire);
        let outcome = dispatcher
            .dispatch(&msg, wf, pid, eingang)
            .await
            .expect("dispatch");
        assert!(
            matches!(outcome, IngestOutcome::Spawned { .. }),
            "{pid}: {outcome:?}"
        );
        let nummer = match &msg {
            edi_energy::AnyMessage::Orders(o) => o
                .segments()
                .iter()
                .find(|s| s.tag == "BGM")
                .and_then(|s| s.component_str(1, 0))
                .expect("BGM DE 1004")
                .to_owned(),
            other => panic!("{pid}: not an ORDERS: {other:?}"),
        };
        let key = mako_gpke::konfiguration_aenderung::auftrag_key(NB_MP, &nummer);
        let ids = processes_on(&store, &key, wf).await;
        assert_eq!(ids.len(), 1, "{pid}");
        let state = Process::<mako_gpke::GpkeKonfigurationAenderungWorkflow, _>::from_identity(
            Arc::new(store.clone()),
            ids[0].clone(),
        )
        .state()
        .await
        .expect("state");
        assert!(
            matches!(&state, KonfigurationAenderungState::AuftragEingegangen { pid: p, .. } if p.as_u32() == pid),
            "{pid}: {state:?}"
        );
    }
    assert_eq!(
        deadlines_labelled(
            &store,
            mako_gpke::konfiguration_aenderung::EINGANG_WINDOW_LABEL
        )
        .await,
        vec![mako_fristen::end_of_day_berlin(time::macros::date!(
            2026 - 10 - 08
        ))],
        "only the 17120 opens a window"
    );
}
