//! The Geschäftsdatenanfrage Strom reaches its own process, and only it does.
//!
//! ORDERS 17132 is a GPKE Teil 4 Kap. 3 Geschäftsdatenanfrage (LF · MSB → NB),
//! answered by UTILMD 55035/55095 (to a LF) or 55060 (to a MSB) and refused by
//! ORDRSP 19101 (ORDERS AHB 1.1b Kap. 4.1.1, S. 7–8; Anwendungsübersicht der
//! Prüfidentifikatoren 4.0 lfd. Nr. 25540–25640, S. 55). The ORDERS PIDs beside
//! it are other Anwendungsfälle — 17103 Brennwert, 17126 Stammdaten
//! Messlokation Gas, 17118 Konfigurationsänderung — and must never reach the
//! Geschäftsdatenanfrage: on a NB, a router that sends them there turns a
//! foreign Anwendungsfall into a Stammdaten process.

use std::any::Any;
use std::collections::BTreeSet;
use std::sync::Arc;

use mako_engine::event_store::CorrelationEntry;
use mako_engine::marktrolle::{DeploymentRoles, Marktrolle};
use mako_engine::pid_router::PidRouter;
use mako_engine::process::{Process, RunOptions};
use mako_engine::registry::ProcessRegistry as _;
use mako_engine::types::{MarktpartnerCode, MessageRef};
use mako_engine::version::{FormatVersion, WorkflowId};
use mako_engine::{ids::TenantId, store_slatedb::SlateDbStore};
use mako_gpke::geschaeftsdatenanfrage::{anfragender_key, netzbetreiber_key};
use mako_gpke::{
    GDA_WORKFLOW_NAME, GdaAnfrage, GdaCommand, GdaState, GpkeGeschaeftsdatenanfrageWorkflow,
};
use makod::adapters::{gda_anfrage_nummer, gpke_geschaeftsdatenanfrage_registry};
use makod::deployment::Rolle;
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

const OWN_MP: &str = "9900357000004";
const ANFRAGENDER: &str = "4012345000023";
const FV: &str = "FV2026-10-01";

/// ORDERS 17132 as the ORDERS AHB 1.1b Kap. 4.1.1.1 lays it out, addressed to
/// this NB: `BGM+Z14` with the Anfrage-Nummer, `RFF+Z13`, `NAD+DP` and
/// `LOC+172` with the Zählpunktbezeichnung.
const ORDERS_17132: &str = "UNB+UNOC:3+4012345000023:14+9900357000004:500+261001:0700+GDA1'\
UNH+1+ORDERS:D:09B:UN:1.4c'\
BGM+Z14+DOK17132'\
DTM+137:202610010000?+00:303'\
RFF+Z13:17132'\
NAD+MS+4012345000023::9'\
NAD+MR+9900357000004::293'\
NAD+DP'\
LOC+172+DE00056266802AO6G56M11SN51G21M24S'\
UNS+S'\
UNT+10+1'\
UNZ+1+GDA1'";

/// The profile's own skeleton of an Anwendungsfall, addressed to `OWN_MP`.
fn skeleton(pid: u32) -> String {
    use edi_energy::profile::SkeletonParties;
    let code = edi_energy::Pruefidentifikator::new(pid).expect("a Prüfidentifikator");
    let profile = edi_energy::ReleaseRegistry::global()
        .all_profiles()
        .iter()
        .filter(|p| p.has_anwendungsfall(code))
        .max_by_key(|p| p.valid_from())
        .unwrap_or_else(|| panic!("no profile carries PID {pid}"));
    let af = profile.anwendungsfall(pid).expect("the column");
    let bytes = profile
        .skeleton_interchange(
            af,
            &SkeletonParties {
                sender: ANFRAGENDER.to_owned(),
                receiver: OWN_MP.to_owned(),
            },
        )
        .expect("skeleton");
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The router a deployment declaring `rollen` builds — the production module
/// list filtered by role, registered in order into one table.
fn router_for(rollen: &[Rolle], roles: &DeploymentRoles) -> PidRouter {
    let rollen: BTreeSet<Rolle> = rollen.iter().copied().collect();
    let mut router = PidRouter::new();
    for module in makod::startup::modules_for(&rollen) {
        module.register_pids_with_roles(&mut router, roles);
    }
    router
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

async fn state_on(store: &SlateDbStore, key: &str) -> GdaState {
    let identity = store
        .as_process_registry()
        .lookup_correlated(TenantId::from_party_id(OWN_MP), key)
        .await
        .expect("lookup")
        .into_iter()
        .find(|id| id.workflow_id.name.as_ref() == GDA_WORKFLOW_NAME)
        .unwrap_or_else(|| panic!("no Geschäftsdatenanfrage on {key}"));
    Process::<GpkeGeschaeftsdatenanfrageWorkflow, _>::from_identity(
        Arc::new(store.clone()),
        identity,
    )
    .state()
    .await
    .expect("state")
}

// ── Routing ───────────────────────────────────────────────────────────────────

/// On a NB, no ORDERS PID but 17132 reaches the Geschäftsdatenanfrage Strom,
/// and none reaches a WiM Stammdaten process: 17103 and 17126 are GeLi Gas's
/// or nobody's.
#[test]
fn a_nb_routes_only_17132_to_the_geschaeftsdatenanfrage() {
    let nb = DeploymentRoles::from_roles([Marktrolle::Nb]);
    for rollen in [
        &[Rolle::NbStrom][..],
        &[Rolle::NbStrom, Rolle::NbGas][..],
        &[Rolle::NbStrom, Rolle::MsbStrom, Rolle::LfStrom][..],
    ] {
        let router = router_for(rollen, &nb);
        assert_eq!(
            router.route(17_132),
            Some(GDA_WORKFLOW_NAME),
            "17132 on {rollen:?}"
        );
        for pid in (17_101..=17_135).filter(|p| *p != 17_132) {
            let routed = router.route(pid);
            assert_ne!(routed, Some(GDA_WORKFLOW_NAME), "{pid} on {rollen:?}");
            assert_ne!(routed, Some("wim-stammdaten"), "{pid} on {rollen:?}");
        }
        // 17126 Anfrage Stammdaten Messlokation (Gas) is the Gas NB's.
        let gas_nb = rollen.contains(&Rolle::NbGas);
        assert_eq!(
            router.route(17_126),
            gas_nb.then_some(mako_geli_gas::geschaeftsdatenanfrage::WORKFLOW_NAME),
            "17126 on {rollen:?}"
        );
        let brennwert = router.route(17_103);
        assert!(
            brennwert.is_none() || brennwert == Some("geli-gas-datenabruf"),
            "17103 Anfrage Brennwert is GeLi Gas's, got {brennwert:?} on {rollen:?}"
        );
    }
}

/// Every answer to the Geschäftsdatenanfrage lands in it.
#[test]
fn the_answers_route_to_the_geschaeftsdatenanfrage() {
    let router = router_for(&[Rolle::LfStrom, Rolle::MsbStrom], &DeploymentRoles::all());
    for pid in [55_035, 55_095, 55_060, 19_101] {
        assert_eq!(router.route(pid), Some(GDA_WORKFLOW_NAME), "{pid}");
    }
}

// ── NB side ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn an_inbound_17132_opens_the_nbs_anfrage_on_its_anfrage_nummer() {
    let msg = edi_energy::parse(ORDERS_17132.as_bytes()).expect("ORDERS 17132 parses");
    assert_eq!(gda_anfrage_nummer(&msg), "DOK17132");

    let cmd = gpke_geschaeftsdatenanfrage_registry()
        .dispatch(&msg as &dyn Any, &FormatVersion::new(FV))
        .expect("adapt");
    let GdaCommand::ReceiveAnfrage { pid, anfrage, .. } = cmd else {
        panic!("expected ReceiveAnfrage, got {cmd:?}");
    };
    assert_eq!(pid.as_u32(), 17_132);
    assert_eq!(anfrage.anfragender.as_str(), ANFRAGENDER);
    assert_eq!(anfrage.netzbetreiber.as_str(), OWN_MP);
    assert_eq!(anfrage.lokation_id, "DE00056266802AO6G56M11SN51G21M24S");

    let (store, dispatcher) = store_and_dispatcher().await;
    let outcome = dispatcher
        .dispatch(
            &msg,
            GDA_WORKFLOW_NAME,
            17_132,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("dispatch");
    assert!(
        matches!(outcome, IngestOutcome::Spawned { .. }),
        "got {outcome:?}"
    );
    let state = state_on(&store, &netzbetreiber_key(ANFRAGENDER, "DOK17132")).await;
    assert!(
        matches!(state, GdaState::Offen(_) | GdaState::Ungueltig { .. }),
        "got {state:?}"
    );
}

/// A deployment that is LF and NB at once holds the Anfrage its LF sent and
/// the copy its NB receives as two processes: the Anfrage-Nummer is the
/// sender's, so the NB's key carries the sender's MP-ID, and the received
/// 17132 opens the NB's side instead of meeting the open `Angefragt` process.
#[tokio::test]
async fn an_lf_and_nb_deployment_receives_its_own_anfrage_as_a_second_process() {
    let msg = edi_energy::parse(ORDERS_17132.as_bytes()).expect("ORDERS 17132 parses");
    let (store, dispatcher) = store_and_dispatcher().await;
    // The same deployment's LF side sent this very Anfrage-Nummer and waits.
    seed_anfrage(&store, "DOK17132").await;
    let outcome = dispatcher
        .dispatch(
            &msg,
            GDA_WORKFLOW_NAME,
            17_132,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("dispatch");
    assert!(
        matches!(outcome, IngestOutcome::Spawned { .. }),
        "the received 17132 opens the NB's own process, got {outcome:?}"
    );
    assert!(matches!(
        state_on(&store, &anfragender_key("DOK17132")).await,
        GdaState::Angefragt(_)
    ));
    assert!(matches!(
        state_on(&store, &netzbetreiber_key(ANFRAGENDER, "DOK17132")).await,
        GdaState::Offen(_) | GdaState::Ungueltig { .. }
    ));
}

// ── Anfragender side ──────────────────────────────────────────────────────────

async fn seed_anfrage(store: &SlateDbStore, anfrage_nummer: &str) {
    let process = Process::<GpkeGeschaeftsdatenanfrageWorkflow, _>::new(
        Arc::new(store.clone()),
        TenantId::from_party_id(OWN_MP),
        WorkflowId::new(GDA_WORKFLOW_NAME, FV),
    );
    let key = CorrelationEntry {
        tenant_id: TenantId::from_party_id(OWN_MP),
        tag: anfragender_key(anfrage_nummer),
        process_id: process.process_id(),
        identity: process.identity(),
    };
    process
        .run(
            GdaCommand::SendAnfrage {
                anfrage: GdaAnfrage {
                    anfragender: MarktpartnerCode::new(OWN_MP),
                    netzbetreiber: MarktpartnerCode::new(ANFRAGENDER),
                    lokation_id: "51238696012".into(),
                    anfrage_nummer: MessageRef::new(anfrage_nummer),
                },
            },
            RunOptions::new().correlations(&[key]),
        )
        .await
        .expect("record the sent 17132");
}

/// A party that sent a 17132 resumes on the NB's UTILMD answer, found by the
/// Anfrage-Nummer the answer echoes in `SG6 RFF+AAV`.
#[tokio::test]
async fn an_anfragender_that_sent_17132_resumes_on_each_utilmd_answer() {
    for pid in [55_035, 55_095, 55_060] {
        let edi = skeleton(pid);
        let msg = edi_energy::parse(edi.as_bytes()).expect("skeleton parses");
        let nummer = gda_anfrage_nummer(&msg);
        assert!(!nummer.is_empty(), "{pid} carries no RFF+AAV");

        let (store, dispatcher) = store_and_dispatcher().await;
        seed_anfrage(&store, &nummer).await;
        let outcome = dispatcher
            .dispatch(
                &msg,
                GDA_WORKFLOW_NAME,
                pid,
                time::OffsetDateTime::now_utc(),
            )
            .await
            .expect("dispatch");
        assert!(
            matches!(outcome, IngestOutcome::Dispatched { .. }),
            "{pid}: got {outcome:?}"
        );
        let state = state_on(&store, &anfragender_key(&nummer)).await;
        assert!(
            matches!(state, GdaState::Beantwortet { antwort_pid, .. } if antwort_pid.as_u32() == pid),
            "{pid}: got {state:?}"
        );
    }
}

/// The ORDRSP 19101 resumes the same process by its `SG1 RFF+ON`.
#[tokio::test]
async fn an_anfragender_that_sent_17132_resumes_on_the_19101() {
    let edi = skeleton(19_101);
    let msg = edi_energy::parse(edi.as_bytes()).expect("skeleton parses");
    let nummer = gda_anfrage_nummer(&msg);
    assert!(!nummer.is_empty(), "19101 carries no RFF+ON");

    let (store, dispatcher) = store_and_dispatcher().await;
    seed_anfrage(&store, &nummer).await;
    let outcome = dispatcher
        .dispatch(
            &msg,
            GDA_WORKFLOW_NAME,
            19_101,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("dispatch");
    assert!(
        matches!(outcome, IngestOutcome::Dispatched { .. }),
        "got {outcome:?}"
    );
    assert!(matches!(
        state_on(&store, &anfragender_key(&nummer)).await,
        GdaState::Abgelehnt { .. }
    ));
}

/// An answer to an Anfrage this party never sent opens nothing.
#[tokio::test]
async fn an_answer_without_an_anfrage_is_not_absorbed() {
    let edi = skeleton(55_035);
    let msg = edi_energy::parse(edi.as_bytes()).expect("skeleton parses");
    let (_store, dispatcher) = store_and_dispatcher().await;
    let outcome = dispatcher
        .dispatch(
            &msg,
            GDA_WORKFLOW_NAME,
            55_035,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("dispatch");
    assert!(
        matches!(
            outcome,
            IngestOutcome::Skipped {
                reason: "process_not_found",
                ..
            }
        ),
        "got {outcome:?}"
    );
}
