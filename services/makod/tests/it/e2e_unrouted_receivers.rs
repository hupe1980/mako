//! PIDs a counterparty sends mako that used to dead-letter as `UnknownPid`
//! reach a workflow on the deployment that receives them.
//!
//! | PID | Kommunikation (Anwendungsübersicht 4.0, lfd. Nr.) | Workflow |
//! |---|---|---|
//! | 55126 · 55218 · 55672 | NB an LF (21120, 21060, 21130) | `gpke-stammdatenaenderung` — the LF's Rückmeldung, GPKE Teil 2 Kap. 3.1.1.2 / 3.1.2.2 Nr. 2 |
//! | 55173 · 55175 | NB an MSB / LF (23990, 23830) | `gpke-stammdatenaenderung` |
//! | 55225 | NB an LF (23840) | `gpke-stammdatenaenderung` |
//! | 55553 | MSB an NB / LF / weiterer MSB (24300, 24450, 24580) | `gpke-stammdatenaenderung` — answered by 55555, GPKE Teil 4 Kap. 1.4.4 Nr. 2 |
//! | 55194 | NB (Strom) an MSB (Gas) (25620) | `geli-gas-datenabruf`, resuming the 17104 it answers |
//! | 13013 | NB an LF, Gas (12060) | `gabi-gas-mmma` on a `lf-gas` deployment |
//! | 17110 | LF an NB, Gas (12040) | `gabi-gas-mmma` on a `nb-gas` deployment, refused on 19110 out of `G_0001` |
//! | 44143 · 44147 · 44148 | LF an NB [Verteiler] · NB an MSB (Gas) | `geli-gas-stammdatenaenderung`, the receiver's Anfrage |
//! | 55614 · 55675 | ÜNB an NB | `gpke-abrechnungsdaten`, answered on IFTSTA 21047 out of `E_0618` / `E_0613` |

use std::collections::BTreeSet;
use std::sync::Arc;

use mako_engine::event_store::CorrelationEntry;
use mako_engine::marktrolle::DeploymentRoles;
use mako_engine::pid_router::PidRouter;
use mako_engine::process::{Process, RunOptions};
use mako_engine::types::{MarktpartnerCode, MeLo, MessageRef, Pruefidentifikator};
use mako_engine::version::WorkflowId;
use mako_engine::{ids::TenantId, store_slatedb::SlateDbStore};
use makod::deployment::Rolle;
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

const OWN_MP: &str = "9900357000004";
const NB_MP: &str = "9900000000003";
const GNB_MP: &str = "9807317000007";
const LF_GAS_MP: &str = "9870012345678";

/// The router a deployment declaring `rollen` builds under `roles`.
fn router_for(rollen: &[Rolle], roles: &DeploymentRoles) -> PidRouter {
    let rollen: BTreeSet<Rolle> = rollen.iter().copied().collect();
    let mut router = PidRouter::new();
    for module in makod::startup::modules_for(&rollen) {
        module.register_pids_with_roles(&mut router, roles);
    }
    router
}

/// The newest profile's skeleton of `pid`, from `NB_MP` to this deployment.
fn skeleton(pid: u32) -> String {
    skeleton_between(pid, NB_MP, OWN_MP)
}

/// The newest profile's skeleton of `pid`, from `sender` to `receiver`.
fn skeleton_between(pid: u32, sender: &str, receiver: &str) -> String {
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

/// An LF Strom deployment routes the Abrechnungsdaten the NB sends it, and the
/// NB's Lokationsbündel- and NeLo-Änderungen, to the workflow that answers
/// them with a Rückmeldung.
#[test]
fn an_lf_strom_deployment_routes_the_abrechnungsdaten_it_receives() {
    let router = router_for(&[Rolle::LfStrom], &DeploymentRoles::lf());
    for pid in [55_126, 55_218, 55_672, 55_175, 55_225, 55_553] {
        assert_eq!(
            router.route(pid),
            Some(mako_gpke::STAMMDATEN_WORKFLOW_NAME),
            "{pid}"
        );
    }
    // The LF's own Rückmeldung on them reaches the NB, not the LF.
    let nb = router_for(&[Rolle::NbStrom], &DeploymentRoles::nb());
    for pid in [55_156, 55_220, 55_673] {
        assert_eq!(nb.route(pid), Some("gpke-abrechnungsdaten"), "{pid}");
    }
    // 55555 stays the MSB's: the Bearbeitungsstand answers it, in
    // `gpke-anfrage-bestellung`.
    assert_eq!(nb.route(55_555), Some("gpke-anfrage-bestellung"));
    // 55174, 55176, 55178 and 55179 exist in no AHB.
    for pid in [55_174, 55_176, 55_178, 55_179] {
        assert_eq!(router.route(pid), None, "{pid}");
        assert_eq!(nb.route(pid), None, "{pid}");
    }
}

/// An inbound 55126 opens the LF's Rückmeldung process instead of
/// dead-lettering.
#[tokio::test]
async fn an_inbound_55126_opens_the_lfs_rueckmeldung() {
    for pid in [55_126, 55_218, 55_672, 55_173, 55_225] {
        let (_, dispatcher) = store_and_dispatcher().await;
        let msg = edi_energy::parse(skeleton(pid).as_bytes()).expect("parses");
        let outcome = dispatcher
            .dispatch(
                &msg,
                mako_gpke::STAMMDATEN_WORKFLOW_NAME,
                pid,
                time::OffsetDateTime::now_utc(),
            )
            .await;
        assert!(
            matches!(outcome, Ok(IngestOutcome::Spawned { .. })),
            "{pid}: {outcome:?}"
        );
    }
}

/// A `lf-gas` deployment routes the marktlokationsscharfe Allokationsliste
/// Gas, and nothing else of GaBi Gas.
#[test]
fn an_lf_gas_deployment_routes_the_allokationsliste_gas() {
    let router = router_for(&[Rolle::LfGas], &DeploymentRoles::lf());
    assert_eq!(router.route(13_013), Some("gabi-gas-mmma"));
    for pid in [31_007, 31_008, 31_010] {
        assert_eq!(router.route(pid), None, "{pid} is not the LF's");
    }
}

/// A 55194 resumes the 17104 it answers by the Anfrage-Nummer it names.
#[tokio::test]
async fn a_55194_resumes_the_msb_gas_anfrage_it_answers() {
    let (store, dispatcher) = store_and_dispatcher().await;
    let tenant = TenantId::from_party_id(OWN_MP);
    let key = mako_geli_gas::datenabruf::anfrage_key("GDA-17104-1");
    let process = Process::<mako_geli_gas::GeliGasDatenabrufWorkflow, _>::new(
        Arc::new(store.clone()),
        tenant,
        WorkflowId::new(
            mako_geli_gas::GELI_GAS_DATENABRUF_WORKFLOW_NAME,
            "FV2026-10-01",
        ),
    );
    let korrelation = CorrelationEntry {
        tenant_id: tenant,
        tag: key.clone(),
        process_id: process.process_id(),
        identity: process.identity(),
    };
    process
        .run(
            mako_geli_gas::GeliGasDatenabrufCommand::InitiateAnfrage {
                pid: Pruefidentifikator::const_new(17_104),
                sender: MarktpartnerCode::new(OWN_MP),
                receiver: MarktpartnerCode::new(NB_MP),
                message_ref: MessageRef::new("GDA-17104-1"),
                messlokation: MeLo::new("DE00056266802AO6G56M11SN51G21M24S"),
                zeitspanne_von: time::macros::date!(2026 - 09 - 01),
                zeitspanne_bis: time::macros::date!(2026 - 09 - 30),
            },
            RunOptions::new().correlations(&[korrelation]),
        )
        .await
        .expect("the MSB Gas sends its 17104");

    let wire = set_element(&skeleton(55_194), "RFF+AAV:", "GDA-17104-1");
    let msg = edi_energy::parse(wire.as_bytes()).expect("parses");
    let outcome = dispatcher
        .dispatch(
            &msg,
            mako_geli_gas::GELI_GAS_DATENABRUF_WORKFLOW_NAME,
            55_194,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("dispatch");
    assert!(
        matches!(outcome, IngestOutcome::Dispatched { .. }),
        "{outcome:?}"
    );
    let state = process.state().await.expect("state");
    assert_eq!(state.label(), "DatenErhalten");
}

/// The commands door over the dispatcher's store, for `own` in `rolle`.
fn commands_state(
    store: &SlateDbStore,
    own: &str,
    rolle: &str,
) -> makod::commands_api::CommandsApiState {
    use makod::{
        cedar_authz::{CedarAuthorizer, DefaultPolicy},
        commands_api::CommandsApiState,
        malo_cache::{MaloIdentResultCache, SlateDbMaloCache},
    };
    CommandsApiState {
        tenant_id: TenantId::from_party_id(own),
        sender_party_id: own.to_owned(),
        configured_marktrollen: vec![rolle.to_owned()],
        max_body_bytes: 1_048_576,
        snapshot_interval: 100,
        cedar: Arc::new(
            CedarAuthorizer::new(vec![], None, None, None, DefaultPolicy::PermitAll)
                .expect("cedar build"),
        ),
        snapshot_store: store.as_snapshot_store(),
        malo_cache: Arc::new(SlateDbMaloCache::new(
            store.clone(),
            TenantId::from_party_id(own),
        )),
        maloid_result_cache: MaloIdentResultCache::new(store.clone()),
        store: Arc::new(store.clone()),
        marktd_client: None,
    }
}

/// A `nb-gas` deployment routes the LF's Anforderung der Allokationsliste
/// (ORDERS 17110, ORDERS AHB 1.1b Kap. 4.2, „LF an NB") to `gabi-gas-mmma`,
/// and a `nb-strom` one does not; the GNB refuses a Start Abo on ORDRSP 19110
/// out of `E_0803` / `G_0001` (EBD 4.3 Kap. 11.1.1, S. 696), echoing the
/// 17110's `BGM` DE 1004 in `SG1 RFF+ON`.
#[tokio::test]
async fn an_nb_gas_deployment_receives_the_anforderung_allokationsliste() {
    let gnb = router_for(&[Rolle::NbGas], &DeploymentRoles::nb());
    assert_eq!(gnb.route(17_110), Some(mako_gabi_gas::MMMA_WORKFLOW_NAME));
    let lf = router_for(&[Rolle::LfGas], &DeploymentRoles::lf());
    assert_eq!(lf.route(17_110), None, "the LF sends the 17110");

    let store = SlateDbStore::open_in_memory()
        .await
        .expect("in-memory store");
    let dispatcher = EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        TenantId::from_party_id(GNB_MP),
    );
    let wire = set_element(
        &skeleton_between(17_110, LF_GAS_MP, GNB_MP),
        "BGM+Z24+",
        "ABO-17110-1",
    );
    let wire = set_element(&wire, "IMD++", "Z01");
    let msg = edi_energy::parse(wire.as_bytes()).expect("parses");
    let outcome = dispatcher
        .dispatch(
            &msg,
            mako_gabi_gas::MMMA_WORKFLOW_NAME,
            17_110,
            time::OffsetDateTime::now_utc(),
        )
        .await;
    assert!(
        matches!(outcome, Ok(IngestOutcome::Spawned { .. })),
        "17110: {outcome:?}"
    );

    let key = mako_gabi_gas::mmma::anforderung_key(LF_GAS_MP, "ABO-17110-1");
    let ids: Vec<_> = {
        use mako_engine::registry::ProcessRegistry as _;
        store
            .as_process_registry()
            .lookup_correlated(TenantId::from_party_id(GNB_MP), &key)
            .await
            .expect("lookup")
    };
    assert_eq!(ids.len(), 1, "one process per Anforderung");
    let process = Process::<mako_gabi_gas::GaBiGasMmmaWorkflow, _>::from_identity(
        Arc::new(store.clone()),
        ids[0].clone(),
    );
    let mako_gabi_gas::MmmaState::Eingegangen { anforderung } =
        process.state().await.expect("state")
    else {
        panic!("the Anforderung is held");
    };
    assert_eq!(anforderung.abo, "Z01");
    let out = process
        .run(
            mako_gabi_gas::MmmaCommand::SendAblehnung {
                antwortcode: "Z34".to_owned(),
                message_ref: MessageRef::new("ABL-1"),
            },
            RunOptions::new(),
        )
        .await;
    assert!(out.is_ok(), "{out:?}");
    use mako_engine::outbox::OutboxStore as _;
    let pending = store.pending_now(10).await.expect("outbox");
    let ordrsp = pending
        .iter()
        .find(|p| p.payload["pid"] == 19_110)
        .expect("the 19110 is written");
    assert_eq!(ordrsp.payload["antwort_codeliste"], "G_0001");
    assert_eq!(ordrsp.payload["order_reference"], "ABO-17110-1");
    assert_eq!(&*ordrsp.recipient, LF_GAS_MP);
}

/// A `nb-strom` deployment routes the ÜNB's Rückmeldung / Bestellung on the
/// Abrechnungsdaten Bilanzkreisabrechnung (55614, 55675; GPKE Teil 2 Kap.
/// 3.1.2.2 Nr. 5, S. 76 and 3.1.3.3 Nr. 1, S. 81) to `gpke-abrechnungsdaten`,
/// and the NB answers the ÜNB on IFTSTA 21047 out of `E_0618` (IFTSTA AHB 2.1
/// Kap. 7.7, S. 62).
#[tokio::test]
async fn a_uenb_rueckmeldung_reaches_the_nb_and_is_answered_out_of_e_0618() {
    const UENB_MP: &str = "9900000000010";
    let nb = router_for(&[Rolle::NbStrom], &DeploymentRoles::nb());
    for pid in [55_614, 55_675] {
        assert_eq!(nb.route(pid), Some("gpke-abrechnungsdaten"), "{pid}");
    }
    for pid in [55_614_u32, 55_675] {
        let (store, dispatcher) = store_and_dispatcher().await;
        let wire = skeleton_between(pid, UENB_MP, OWN_MP);
        let msg = edi_energy::parse(wire.as_bytes()).expect("parses");
        let outcome = dispatcher
            .dispatch(
                &msg,
                "gpke-abrechnungsdaten",
                pid,
                time::OffsetDateTime::now_utc(),
            )
            .await;
        assert!(
            matches!(outcome, Ok(IngestOutcome::Spawned { .. })),
            "{pid}: {outcome:?}"
        );
        let malo = makod::ingest_dispatcher::extract_malo_from_msg(&msg);
        let state = commands_state(&store, OWN_MP, "NB");
        let answered = makod::commands_api::dispatch_command(
            &state,
            "gpke.abrechnungsdaten.bearbeitungsstand",
            &serde_json::json!({
                "malo_id": malo.as_str(),
                "codeliste": "E_0618",
                "antwort_code": "A01",
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
        use mako_engine::outbox::OutboxStore as _;
        let pending = store.pending_now(10).await.expect("outbox");
        let iftsta = pending
            .iter()
            .find(|p| p.payload["pid"] == 21_047)
            .expect("the 21047 is written");
        assert_eq!(iftsta.payload["antwort_codeliste"], "E_0618");
        assert_eq!(iftsta.payload["sts_code"], "Z47");
        assert_eq!(&*iftsta.recipient, UENB_MP);
    }
}

/// The Anfragen an MSB mit Abhängigkeiten (UTILMD AHB Gas 1.2 Kap. 5.12.6):
/// the GNB as Verteiler receives the LF's 44143 (S. 179), the gMSB the GNB's
/// 44147 / 44148 (S. 192). Each opens the receiver's Anfrage process instead of
/// being applied as an Änderung; the answers 44145 / 44146 / 44149 reach no
/// requester side and are refused by name.
#[tokio::test]
async fn the_gas_anfragen_an_msb_reach_their_receivers() {
    const MSB_GAS_MP: &str = "9800357000011";
    let gnb = router_for(&[Rolle::NbGas], &DeploymentRoles::nb());
    assert_eq!(gnb.route(44_143), Some("geli-gas-stammdatenaenderung"));
    let gmsb = router_for(&[Rolle::MsbGas], &DeploymentRoles::msb());
    for pid in [44_147, 44_148] {
        assert_eq!(
            gmsb.route(pid),
            Some("geli-gas-stammdatenaenderung"),
            "{pid}"
        );
    }
    for (pid, sender, receiver) in [
        (44_143_u32, LF_GAS_MP, GNB_MP),
        (44_147, GNB_MP, MSB_GAS_MP),
        (44_148, GNB_MP, MSB_GAS_MP),
    ] {
        let store = SlateDbStore::open_in_memory()
            .await
            .expect("in-memory store");
        let dispatcher = EdifactIngestDispatcher::new(
            Arc::new(store.clone()),
            store.as_snapshot_store(),
            100,
            TenantId::from_party_id(receiver),
        );
        let msg =
            edi_energy::parse(skeleton_between(pid, sender, receiver).as_bytes()).expect("parses");
        let outcome = dispatcher
            .dispatch(
                &msg,
                "geli-gas-stammdatenaenderung",
                pid,
                time::OffsetDateTime::now_utc(),
            )
            .await;
        assert!(
            matches!(outcome, Ok(IngestOutcome::Spawned { .. })),
            "{pid}: {outcome:?}"
        );
    }
    for pid in [44_145_u32, 44_146, 44_149, 44_164] {
        let (_, dispatcher) = store_and_dispatcher().await;
        let msg =
            edi_energy::parse(skeleton_between(pid, GNB_MP, LF_GAS_MP).as_bytes()).expect("parses");
        let refused = dispatcher
            .dispatch(
                &msg,
                "geli-gas-stammdatenaenderung",
                pid,
                time::OffsetDateTime::now_utc(),
            )
            .await;
        assert!(
            refused
                .as_ref()
                .is_err_and(|e| e.to_string().contains("no requester side")),
            "{pid}: an answer is never applied as an Änderung: {refused:?}"
        );
    }
}
