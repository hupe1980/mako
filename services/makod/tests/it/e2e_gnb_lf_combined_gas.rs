//! A deployment that is GNB and LF at once runs both sides of one Gas EoG.
//!
//! The GNB sends the Anmeldung EoG 44013 to the Grundversorger, and on a
//! combined NB+LF deployment the Grundversorger is the same makod: its AS4
//! loopback hands the 44013 straight back to its own ingest. Both sides run the
//! one workflow name `geli-gas-supplier-change` on the one Marktlokation — the
//! GNB waiting in `GnbPending` for the answer, the LF owing it (UTILMD AHB Gas,
//! 44013 NB → LF, 44014/44015 LF → NB). The LF's copy must open the LF's own
//! process rather than resume the GNB's, which refuses an Anfrage; and the
//! LF's 44014 must reach the GNB's process, which records the code and the
//! Entscheidungsbaum `E_3008` that defines it.
//!
//! The loopback is driven as production drives it: the outbox entry is
//! rendered to wire, parsed, validated against its AHB profile — the pre-send
//! gate's check — routed by the `PidRouter` a `nb-gas` + `lf-gas` deployment
//! builds, and dispatched into the one tenant both roles share. The 44013 is
//! the one `geli.eog.anmelden` renders from the Marktlokation's Stammdaten,
//! which makod reads from a stand-in marktd.

use std::collections::BTreeSet;
use std::sync::Arc;

use mako_engine::event_store::EventStore as _;
use mako_engine::marktrolle::{DeploymentRoles, Marktrolle};
use mako_engine::outbox::OutboxStore;
use mako_engine::pid_router::PidRouter;
use mako_engine::{
    ids::{ProcessId, StreamId, TenantId},
    store_slatedb::SlateDbStore,
};
use mako_geli_gas::{
    GasSupplierChangeEvent, GasSupplierChangeState, GeliGasSupplierChangeWorkflow,
};
use makod::commands_api::{CommandsApiState, DispatchOutcome, dispatch_command};
use makod::deployment::Rolle;
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

use crate::support;

// DVGW-Codenummern: NAD DE 3055 `332`, the code a Gas MIG admits.
const GNB_MP: &str = "9807317000007";
const LF_MP: &str = "9870012345678";
const MALO: &str = "51238696012";

struct Combined {
    store: Arc<SlateDbStore>,
    state: CommandsApiState,
    dispatcher: EdifactIngestDispatcher,
    router: PidRouter,
    registry: makod::party_registry::MpIdRegistry,
}

async fn combined() -> Combined {
    combined_with(Some(support::gas_stammdaten(MALO, GNB_MP))).await
}

async fn combined_with(
    record: Option<mako_markt::utilmd_stammdaten::MaloUtilmdStammdaten>,
) -> Combined {
    use makod::config::PartyConfig;
    let store = Arc::new(
        SlateDbStore::open_in_memory()
            .await
            .expect("in-memory store"),
    );
    // One tenant for the deployment, both roles inside it.
    let tenant_id = TenantId::from_party_id(GNB_MP);
    let dispatcher = EdifactIngestDispatcher::new(
        Arc::clone(&store),
        store.as_snapshot_store(),
        100,
        tenant_id,
    );
    let state = CommandsApiState {
        tenant_id,
        sender_party_id: GNB_MP.to_owned(),
        configured_marktrollen: vec!["GNB".to_owned(), "LFG".to_owned()],
        max_body_bytes: 1_048_576,
        snapshot_interval: 100,
        cedar: Arc::new(
            makod::cedar_authz::CedarAuthorizer::unauthenticated().expect("infallible"),
        ),
        snapshot_store: store.as_snapshot_store(),
        malo_cache: Arc::new(makod::malo_cache::SlateDbMaloCache::new((*store).clone())),
        maloid_result_cache: makod::malo_cache::MaloIdentResultCache::new((*store).clone()),
        store: Arc::clone(&store),
        marktd_client: Some(Arc::new(support::stub_marktd(record).await)),
    };
    let rollen: BTreeSet<Rolle> = [Rolle::NbGas, Rolle::LfGas].into_iter().collect();
    let roles = DeploymentRoles::from_roles([Marktrolle::Nb, Marktrolle::Lf]);
    let mut router = PidRouter::new();
    for module in makod::startup::modules_for(&rollen) {
        module.register_pids_with_roles(&mut router, &roles);
    }
    let registry = makod::party_registry::MpIdRegistry::from_config(&[
        PartyConfig {
            mp_id: GNB_MP.to_owned(),
            roles: vec!["NB".to_owned()],
            primary: true,
            agency: None,
            as4: None,
        },
        PartyConfig {
            mp_id: LF_MP.to_owned(),
            roles: vec!["LF".to_owned()],
            primary: false,
            agency: None,
            as4: None,
        },
    ])
    .expect("valid combined registry");
    Combined {
        store,
        state,
        dispatcher,
        router,
        registry,
    }
}

impl Combined {
    /// Deliver every pending UTILMD to this deployment's own ingest, as the AS4
    /// loopback does for a message addressed to one of its own MP-IDs, and
    /// acknowledge everything else the outbox holds — ERP notifications are not
    /// market messages. Every UTILMD — the GNB's 44013 and the LF's 44014 —
    /// must pass its AHB profile first, as the pre-send gate
    /// (`transport::as4_sender`) requires on the loopback.
    async fn loopback(&self) -> Vec<(u32, IngestOutcome)> {
        use edi_energy::EdiEnergyMessage as _;
        let now = time::OffsetDateTime::now_utc() + time::Duration::days(1);
        let pending = OutboxStore::pending(&*self.store, 100, now)
            .await
            .expect("pending outbox");
        let mut outcomes = Vec::new();
        for om in &pending {
            if &*om.message_type == "UTILMD" {
                let msg = gated(om, &self.registry);
                let pid = msg
                    .detect_pruefidentifikator()
                    .expect("a Prüfidentifikator")
                    .as_u32();
                assert!(
                    self.registry.is_own_mp_id(&om.recipient),
                    "{pid} is addressed to this deployment, got {}",
                    om.recipient
                );
                let workflow = self
                    .router
                    .route(pid)
                    .unwrap_or_else(|| panic!("{pid} is routed on a combined deployment"));
                let outcome = self
                    .dispatcher
                    .dispatch(&msg, workflow, pid, time::OffsetDateTime::now_utc())
                    .await
                    .unwrap_or_else(|e| panic!("{pid} dispatches on its loopback: {e}"));
                outcomes.push((pid, outcome));
            }
            OutboxStore::acknowledge(&*self.store, om.message_id)
                .await
                .expect("acknowledge");
        }
        outcomes
    }

    async fn state_of(&self, process: ProcessId) -> GasSupplierChangeState {
        use mako_engine::workflow::Workflow as _;
        self.events_of(process)
            .await
            .iter()
            .map(|e| serde_json::from_value(e.payload.clone()).expect("a GeLi Gas event"))
            .fold(GasSupplierChangeState::default(), |s, e| {
                GeliGasSupplierChangeWorkflow::apply(s, &e)
            })
    }

    async fn events_of(&self, process: ProcessId) -> Vec<mako_engine::envelope::EventEnvelope> {
        self.store
            .load(&StreamId::for_process(self.state.tenant_id, &process))
            .await
            .expect("stream loads")
    }
}

/// Render `om` to wire, parse it and run the pre-send gate's AHB check on it,
/// as `transport::as4_sender` does before any delivery.
fn gated(
    om: &mako_engine::outbox::OutboxMessage,
    registry: &makod::party_registry::MpIdRegistry,
) -> edi_energy::AnyMessage {
    use edi_energy::EdiEnergyMessage as _;
    let wire = makod::edifact_renderer::render_to_wire_bytes(om, registry)
        .unwrap_or_else(|e| panic!("render {}: {e:?}", om.message_type));
    let msg = edi_energy::parse(&wire.bytes).unwrap_or_else(|e| panic!("parse: {e:?}"));
    let report = msg
        .validate_on_date(mako_fristen::heute())
        .expect("the AHB validation runs");
    assert!(
        report.is_valid(),
        "the rendered {} violates its AHB profile: {:#?}\n{}",
        om.message_type,
        report
            .errors()
            .iter()
            .map(|e| &e.message)
            .collect::<Vec<_>>(),
        String::from_utf8_lossy(&wire.bytes).replace('\'', "'\n"),
    );
    msg
}

/// **The GNB's 44013 is a complete Anmeldung EoG**: what `geli.eog.anmelden`
/// writes to the outbox renders to a UTILMD its AHB profile accepts — the
/// check the pre-send gate runs on the network and on the loopback alike.
#[tokio::test]
async fn geli_eog_anmelden_renders_a_44013_its_profile_accepts() {
    use edi_energy::EdiEnergyMessage as _;
    let c = combined().await;
    dispatch_command(
        &c.state,
        "geli.eog.anmelden",
        &support::geli_eog_anmeldung(MALO, LF_MP),
    )
    .await
    .expect("geli.eog.anmelden spawns");
    let now = time::OffsetDateTime::now_utc() + time::Duration::days(1);
    let pending = OutboxStore::pending(&*c.store, 100, now)
        .await
        .expect("pending outbox");
    let utilmd: Vec<_> = pending
        .iter()
        .filter(|om| &*om.message_type == "UTILMD")
        .collect();
    let [om] = utilmd.as_slice() else {
        panic!("one UTILMD is written, got {}", utilmd.len());
    };
    let msg = gated(om, &c.registry);
    assert_eq!(
        msg.detect_pruefidentifikator().map(|p| p.as_u32()).ok(),
        Some(44013)
    );
}

/// **A command lacking a fact the 44013 must carry is refused**, naming it —
/// never sent as a message the gate dead-letters. The command's own facts are
/// the caller's; the Marktlokation's Stammdaten are marktd's, and a caller
/// sending them is refused like any undeclared key.
#[tokio::test]
async fn geli_eog_anmelden_without_a_muss_fact_is_refused() {
    let c = combined().await;
    for missing in ["transaktionsgrund", "bilanzierungsbeginn"] {
        let mut body = support::geli_eog_anmeldung(MALO, LF_MP);
        body.as_object_mut().expect("an object").remove(missing);
        let err = dispatch_command(&c.state, "geli.eog.anmelden", &body)
            .await
            .expect_err("a 44013 without it is refused");
        assert!(
            format!("{err:?}").contains(missing),
            "the refusal names {missing:?}: {err:?}"
        );
    }
    let mut body = support::geli_eog_anmeldung(MALO, LF_MP);
    body["stammdaten"] = serde_json::json!({ "lokationen": [], "bloecke": [] });
    let err = dispatch_command(&c.state, "geli.eog.anmelden", &body)
        .await
        .expect_err("Stammdaten from the caller are refused");
    assert!(
        format!("{err:?}").contains("stammdaten"),
        "the refusal names the undeclared key: {err:?}"
    );
    assert_nothing_written(&c).await;
}

/// **A Marktlokation without an address is refused**, naming the
/// Marktlokationsanschrift — the `SG12 NAD+DP` is Muss on every 44013 and
/// marktd holds no value for it.
#[tokio::test]
async fn geli_eog_anmelden_for_a_marktlokation_without_an_address_is_refused() {
    let mut record = support::gas_stammdaten(MALO, GNB_MP);
    record.anschrift = None;
    let c = combined_with(Some(record)).await;
    let err = dispatch_command(
        &c.state,
        "geli.eog.anmelden",
        &support::geli_eog_anmeldung(MALO, LF_MP),
    )
    .await
    .expect_err("a 44013 without its Marktlokationsanschrift is refused");
    match &err {
        makod::commands_api::DispatchError::StammdatenUnvollstaendig { malo_id, fehlend } => {
            assert_eq!(malo_id, MALO);
            assert!(
                fehlend
                    .iter()
                    .any(|f| f.contains("Marktlokationsanschrift")),
                "the refusal names the Marktlokationsanschrift: {fehlend:?}"
            );
        }
        other => panic!("refused as incomplete Stammdaten, got {other:?}"),
    }
    assert_nothing_written(&c).await;
}

/// **Master data that cannot be read refuses the command** — marktd holding
/// no such Marktlokation, or not answering at all. No 44013 goes out empty.
#[tokio::test]
async fn geli_eog_anmelden_without_readable_master_data_is_refused() {
    let c = combined_with(None).await;
    let err = dispatch_command(
        &c.state,
        "geli.eog.anmelden",
        &support::geli_eog_anmeldung(MALO, LF_MP),
    )
    .await
    .expect_err("an unknown Marktlokation is refused");
    assert!(
        matches!(
            err,
            makod::commands_api::DispatchError::StammdatenUnvollstaendig { ref malo_id, .. }
                if malo_id == MALO
        ),
        "{err:?}"
    );
    assert_nothing_written(&c).await;

    let mut c = combined().await;
    c.state.marktd_client = Some(Arc::new(support::unreachable_marktd()));
    let err = dispatch_command(
        &c.state,
        "geli.eog.anmelden",
        &support::geli_eog_anmeldung(MALO, LF_MP),
    )
    .await
    .expect_err("an unreachable marktd refuses the command");
    assert!(
        matches!(
            err,
            makod::commands_api::DispatchError::MarktdUnavailable(_)
        ),
        "{err:?}"
    );
    c.state.marktd_client = None;
    let err = dispatch_command(
        &c.state,
        "geli.eog.anmelden",
        &support::geli_eog_anmeldung(MALO, LF_MP),
    )
    .await
    .expect_err("a deployment without marktd refuses the command");
    assert!(
        matches!(
            err,
            makod::commands_api::DispatchError::MarktdUnavailable(_)
        ),
        "{err:?}"
    );
    assert_nothing_written(&c).await;
}

async fn assert_nothing_written(c: &Combined) {
    let pending = OutboxStore::pending(
        &*c.store,
        100,
        time::OffsetDateTime::now_utc() + time::Duration::days(1),
    )
    .await
    .expect("pending outbox");
    assert!(
        pending.is_empty(),
        "nothing is written for a refused command"
    );
}

/// The LF's 44014 as `geli.eog.bestaetigen` writes it on a combined
/// deployment — the GNB's 44013 delivered to the LF side, then answered.
pub(crate) async fn rendered_44014() -> mako_engine::outbox::OutboxMessage {
    let c = combined().await;
    dispatch_command(
        &c.state,
        "geli.eog.anmelden",
        &support::geli_eog_anmeldung(MALO, LF_MP),
    )
    .await
    .expect("geli.eog.anmelden spawns");
    c.loopback().await;
    dispatch_command(&c.state, "geli.eog.bestaetigen", &bestaetigung())
        .await
        .expect("the LF answers");
    let now = time::OffsetDateTime::now_utc() + time::Duration::days(1);
    OutboxStore::pending(&*c.store, 100, now)
        .await
        .expect("pending outbox")
        .into_iter()
        .find(|om| &*om.message_type == "UTILMD" && om.payload["pid"] == 44014)
        .expect("the 44014 is written")
}

/// The LF's Bestätigung EoG: the Antwortcode, its Bilanzkreis and its
/// Abrechnungsintervall.
fn bestaetigung() -> serde_json::Value {
    serde_json::json!({
        "malo_id": MALO,
        "antwort_code": "E15",
        "bilanzkreis": BILANZKREIS,
        "abrechnungsintervall_monate": 12,
    })
}

/// A Gas Bilanzkreis code as the LF's own fact — synthetic, no assignment.
const BILANZKREIS: &str = "BKGAS0001";

/// **A 44014 without the LF's own Muss facts is refused**, naming each — never
/// written as a message the pre-send gate dead-letters. The process still owes
/// its answer afterwards.
#[tokio::test]
async fn geli_eog_bestaetigen_without_an_lf_fact_is_refused() {
    let c = combined().await;
    dispatch_command(
        &c.state,
        "geli.eog.anmelden",
        &support::geli_eog_anmeldung(MALO, LF_MP),
    )
    .await
    .expect("geli.eog.anmelden spawns");
    let lf = match c.loopback().await.as_slice() {
        [(44013, IngestOutcome::Spawned { process_id, .. })] => *process_id,
        other => panic!("the LF's copy of the 44013 opens its process, got {other:?}"),
    };
    for missing in ["bilanzkreis", "abrechnungsintervall_monate"] {
        let mut body = bestaetigung();
        body.as_object_mut().expect("an object").remove(missing);
        let err = dispatch_command(&c.state, "geli.eog.bestaetigen", &body)
            .await
            .expect_err("a 44014 without it is refused");
        assert!(
            format!("{err:?}").contains(missing),
            "the refusal names {missing:?}: {err:?}"
        );
    }
    assert!(
        matches!(
            c.state_of(lf).await,
            GasSupplierChangeState::ValidationPassed(_)
        ),
        "a refused answer leaves the LF's process owing it"
    );
}

/// **A 44013 runs to its 44014 on one combined deployment**: the LF's copy
/// opens the LF's own process, the LF answers it, and the answer reaches the
/// GNB's process, so both sides end where their half of the Vorgang ends.
#[tokio::test]
async fn a_combined_deployment_runs_the_gas_eog_to_its_answer() {
    let c = combined().await;

    // 1. The GNB assigns the Marktlokation to its own Grundversorger.
    let gnb = match dispatch_command(
        &c.state,
        "geli.eog.anmelden",
        &support::geli_eog_anmeldung(MALO, LF_MP),
    )
    .await
    .expect("geli.eog.anmelden spawns")
    {
        DispatchOutcome::Spawned { process_id } => process_id,
        other => panic!("geli.eog.anmelden must spawn, got {other:?}"),
    };

    // 2. The loopback delivers the 44013 to the LF side.
    let delivered = c.loopback().await;
    let lf = match delivered.as_slice() {
        [(44013, IngestOutcome::Spawned { process_id, .. })] => *process_id,
        other => panic!("the LF's copy of the 44013 must open the LF's own process, got {other:?}"),
    };
    assert_ne!(lf, gnb, "the two sides run two processes");
    assert!(
        matches!(c.state_of(gnb).await, GasSupplierChangeState::GnbPending(_)),
        "the GNB still waits for the answer"
    );

    // 3. The LF confirms the Ersatz-/Grundversorgung out of `E_3008`, stating
    //    the two facts of its own a 44014 carries beside the Marktlokation's
    //    record; the loopback below holds the rendering to its profile.
    let answered = dispatch_command(&c.state, "geli.eog.bestaetigen", &bestaetigung())
        .await
        .expect("the LF answers its own process");
    assert!(
        matches!(answered, DispatchOutcome::Dispatched { process_id } if process_id == lf),
        "geli.eog.bestaetigen answers the LF's process, got {answered:?}"
    );
    match c.state_of(lf).await {
        GasSupplierChangeState::AntwortGesendet { response_pid, .. } => {
            assert_eq!(response_pid.map(|p| p.as_u32()), Some(44014));
        }
        other => panic!("the LF's process records its sent 44014, got {other:?}"),
    }

    // 4. The loopback delivers the 44014 to the GNB side.
    let delivered = c.loopback().await;
    match delivered.as_slice() {
        [(44014, IngestOutcome::Dispatched { process_id, .. })] => assert_eq!(
            *process_id, gnb,
            "the 44014 resumes the GNB's process, not the LF's"
        ),
        other => panic!("the 44014 must resume the GNB's process, got {other:?}"),
    }
    assert!(
        matches!(c.state_of(gnb).await, GasSupplierChangeState::Completed(_)),
        "the GNB's EoG is concluded by the Bestätigung"
    );
    let events = c.events_of(gnb).await;
    let last = events.last().expect("the answer is recorded");
    let event: GasSupplierChangeEvent =
        serde_json::from_value(last.payload.clone()).expect("a GeLi Gas event");
    let GasSupplierChangeEvent::GnbAntwortErhalten {
        response_pid,
        accepted,
        ..
    } = event
    else {
        panic!("the GNB records GnbAntwortErhalten, got {event:?}");
    };
    assert_eq!(response_pid.as_u32(), 44014);
    assert!(accepted);
    let antwort = last.antwort.as_ref().expect("the answer code is kept");
    assert_eq!(antwort.code, "E15");
    assert_eq!(antwort.codeliste.as_deref(), Some("E_3008"));

    // The LF's process is untouched by the answer it sent.
    assert!(
        matches!(
            c.state_of(lf).await,
            GasSupplierChangeState::AntwortGesendet { .. }
        ),
        "the 44014 must not reach the LF's own process"
    );
}
