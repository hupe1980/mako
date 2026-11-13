//! The NB's own Lieferende — GPKE Teil 2 Kap. 2.5.2.2 SD „Lieferende von NB an
//! LF", S. 60–62 — through the production door and dispatchers.
//!
//! `gpke.nb-lieferende.ankuendigen` sends the 55007 (Nr. 1). The LF's 55008 /
//! 55009 resume the NB's process (Nr. 2); on silence the 05:00 window sends the
//! Beendigung aufgrund fehlender Antwort, a 55007 `BGM+Z90` (Nr. 3). Either
//! ending queues the `ProcessCompleted` marktd ends the supply on and edmd
//! orders the Schlussablesung on.
//!
//! The deployment is NB and LF at once, so the 55007 reaches the LF's own
//! process through the loopback and the LF's answer reaches the NB's — every
//! UTILMD rendered and held to its AHB profile on the way, as the pre-send gate
//! does.

use std::collections::BTreeSet;
use std::sync::Arc;

use mako_engine::deadline::DeadlineStore as _;
use mako_engine::event_store::EventStore as _;
use mako_engine::marktrolle::{DeploymentRoles, Marktrolle};
use mako_engine::outbox::{OutboxMessage, OutboxStore};
use mako_engine::pid_router::PidRouter;
use mako_engine::workflow::Workflow as _;
use mako_engine::{
    ids::{ProcessId, StreamId, TenantId},
    store_slatedb::SlateDbStore,
};
use mako_gpke::nb_lieferende::{BEENDIGUNG_SPAETESTENS_LABEL, LF_ANTWORT_WINDOW_LABEL};
use mako_gpke::{GpkeNbLieferendeWorkflow, NbLieferendeState};
use makod::commands_api::{CommandsApiState, DispatchOutcome, dispatch_command};
use makod::deployment::Rolle;
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};
use makod::orchestrator::deadline_dispatch::{dispatch_deadline, production_drivers};

const NB_MP: &str = "9900001000001";
const LF_MP: &str = "9900001000002";
const MALO: &str = "51238696012";

struct Combined {
    store: Arc<SlateDbStore>,
    state: CommandsApiState,
    dispatcher: EdifactIngestDispatcher,
    router: PidRouter,
    registry: makod::party_registry::MpIdRegistry,
}

async fn combined() -> Combined {
    use makod::config::PartyConfig;
    let store = Arc::new(
        SlateDbStore::open_in_memory()
            .await
            .expect("in-memory store"),
    );
    let tenant_id = TenantId::from_party_id(NB_MP);
    let dispatcher = EdifactIngestDispatcher::new(
        Arc::clone(&store),
        store.as_snapshot_store(),
        100,
        tenant_id,
    );
    let state = CommandsApiState {
        tenant_id,
        sender_party_id: NB_MP.to_owned(),
        configured_marktrollen: vec!["NB".to_owned(), "LF".to_owned()],
        max_body_bytes: 1_048_576,
        snapshot_interval: 100,
        cedar: Arc::new(
            makod::cedar_authz::CedarAuthorizer::unauthenticated().expect("infallible"),
        ),
        snapshot_store: store.as_snapshot_store(),
        malo_cache: Arc::new(makod::malo_cache::SlateDbMaloCache::new(
            (*store).clone(),
            tenant_id,
        )),
        maloid_result_cache: makod::malo_cache::MaloIdentResultCache::new((*store).clone()),
        store: Arc::clone(&store),
        marktd_client: None,
    };
    let rollen: BTreeSet<Rolle> = [Rolle::NbStrom, Rolle::LfStrom].into_iter().collect();
    let roles = DeploymentRoles::from_roles([Marktrolle::Nb, Marktrolle::Lf]);
    let mut router = PidRouter::new();
    for module in makod::startup::modules_for(&rollen) {
        module.register_pids_with_roles(&mut router, &roles);
    }
    let registry = makod::party_registry::MpIdRegistry::from_config(&[
        PartyConfig {
            mp_id: NB_MP.to_owned(),
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
    /// Take every pending outbox entry: each UTILMD is rendered and held to its
    /// AHB profile, delivered to this deployment's own ingest when `deliver`
    /// says so, and everything is acknowledged. Returns the UTILMD payloads,
    /// the ingest outcomes and the ERP notifications.
    async fn drain(
        &self,
        deliver: bool,
    ) -> (
        Vec<serde_json::Value>,
        Vec<(u32, IngestOutcome)>,
        Vec<serde_json::Value>,
    ) {
        let (utilmd, outcomes, completed, _) = self.drain_wire(deliver).await;
        (utilmd, outcomes, completed)
    }

    /// [`Self::drain`], also returning each UTILMD as the wire carries it.
    async fn drain_wire(
        &self,
        deliver: bool,
    ) -> (
        Vec<serde_json::Value>,
        Vec<(u32, IngestOutcome)>,
        Vec<serde_json::Value>,
        Vec<edi_energy::AnyMessage>,
    ) {
        use edi_energy::EdiEnergyMessage as _;
        let now = time::OffsetDateTime::now_utc() + time::Duration::days(3);
        let pending = OutboxStore::pending(&*self.store, 100, now)
            .await
            .expect("pending outbox");
        let (mut utilmd, mut outcomes, mut completed, mut wire) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for om in &pending {
            match &*om.message_type {
                "UTILMD" => {
                    let msg = gated(om, &self.registry);
                    utilmd.push(om.payload.clone());
                    if deliver {
                        let pid = msg
                            .detect_pruefidentifikator()
                            .expect("a Prüfidentifikator")
                            .as_u32();
                        let workflow = self
                            .router
                            .route(pid)
                            .unwrap_or_else(|| panic!("{pid} is routed"));
                        let outcome = self
                            .dispatcher
                            .dispatch(&msg, workflow, pid, time::OffsetDateTime::now_utc())
                            .await
                            .unwrap_or_else(|e| panic!("{pid} dispatches on its loopback: {e}"));
                        outcomes.push((pid, outcome));
                    }
                    wire.push(msg);
                }
                "ProcessCompleted" => completed.push(om.payload.clone()),
                _ => {}
            }
            OutboxStore::acknowledge(&*self.store, om.message_id)
                .await
                .expect("acknowledge");
        }
        (utilmd, outcomes, completed, wire)
    }

    async fn state_of(&self, process: ProcessId) -> NbLieferendeState {
        self.store
            .load(&StreamId::for_process(self.state.tenant_id, &process))
            .await
            .expect("stream loads")
            .iter()
            .filter_map(|e| serde_json::from_value(e.payload.clone()).ok())
            .fold(NbLieferendeState::default(), |s, e| {
                GpkeNbLieferendeWorkflow::apply(s, &e)
            })
    }

    /// Fire every deadline of `process`, soonest first, as the scheduler does.
    async fn fire_all(&self, process: ProcessId) -> Vec<String> {
        let deadlines = self.store.as_deadline_store();
        let mut due = deadlines
            .for_stream(&StreamId::for_process(self.state.tenant_id, &process))
            .await
            .expect("the stream's deadlines");
        due.sort_by_key(mako_engine::deadline::Deadline::due_at);
        let mut fired = Vec::new();
        for deadline in due {
            fired.push(deadline.label().to_owned());
            let id = deadline.deadline_id();
            dispatch_deadline(
                production_drivers(),
                deadline,
                Arc::clone(&self.store),
                self.store.as_snapshot_store(),
                0,
            )
            .await
            .expect("a fired deadline dispatches");
            deadlines.cancel(id).await.expect("retire the deadline");
        }
        fired
    }
}

/// Render `om` to wire, parse it and run the pre-send gate's AHB check on it.
fn gated(
    om: &OutboxMessage,
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
        report.is_sendable(),
        "the rendered {} violates its AHB profile: {:#?}\n{}",
        om.message_type,
        report
            .sending_refusals()
            .map(|e| &e.message)
            .collect::<Vec<_>>(),
        String::from_utf8_lossy(&wire.bytes).replace('\'', "'\n"),
    );
    msg
}

fn ankuendigung() -> serde_json::Value {
    serde_json::json!({
        "malo_id": MALO,
        "lf_mp_id": LF_MP,
        "zuordnungsende": "2027-01-31",
        "transaktionsgrund": "ZQ7",
        "transaktionsgrund_ergaenzung": "ZW4",
    })
}

async fn ankuendigen(c: &Combined) -> ProcessId {
    match dispatch_command(&c.state, "gpke.nb-lieferende.ankuendigen", &ankuendigung())
        .await
        .expect("gpke.nb-lieferende.ankuendigen spawns")
    {
        DispatchOutcome::Spawned { process_id } => process_id,
        other => panic!("the Ankündigung must spawn, got {other:?}"),
    }
}

/// **An unanswered 55007 ends the Zuordnung in its window** (Nr. 3): the 05:00
/// deadline sends the 55007 `BGM+Z90` restating Nr. 1's Vorgangsnummer, and
/// the completion carries the Zuordnungsende and the LF whose Zuordnung ends.
/// The 06:00 bound then finds it sent.
#[tokio::test]
async fn an_unanswered_55007_ends_the_zuordnung_at_its_window() {
    let c = combined().await;
    let nb = ankuendigen(&c).await;

    let (sent, delivered, completed) = c.drain(true).await;
    let lf = match delivered.as_slice() {
        [(55_007, IngestOutcome::Spawned { process_id, .. })] => *process_id,
        other => panic!("the 55007 opens the LF's own process, got {other:?}"),
    };
    let [ankuendigung] = &sent[..] else {
        panic!("one 55007 goes out, got {sent:#?}")
    };
    assert_eq!(ankuendigung["document_code"], "E02");
    assert_eq!(ankuendigung["process_date"], "20270131");
    assert!(completed.is_empty(), "the Ankündigung ends nothing");

    let fired = c.fire_all(nb).await;
    assert_eq!(
        fired,
        [LF_ANTWORT_WINDOW_LABEL, BEENDIGUNG_SPAETESTENS_LABEL],
        "both bounds of Nr. 3 are registered on the process"
    );
    assert!(matches!(
        c.state_of(nb).await,
        NbLieferendeState::Beendet {
            ohne_antwort: true,
            ..
        }
    ));
    let (sent, delivered, completed) = c.drain(true).await;
    let [beendigung] = &sent[..] else {
        panic!("exactly one Beendigung goes out, got {sent:#?}")
    };
    // The loopback hands Nr. 3 to the LF's own process, which it ends.
    match delivered.as_slice() {
        [(55_007, IngestOutcome::Dispatched { process_id, .. })] => {
            assert_eq!(*process_id, lf, "Nr. 3 resumes the LF's Ankündigung");
        }
        other => panic!("Nr. 3 must resume the LF's process, got {other:?}"),
    }
    let (_, _, lf_completed) = c.drain(false).await;
    let lf_ende = lf_completed
        .iter()
        .find(|p| p["pid"] == 55_007)
        .unwrap_or_else(|| panic!("the LF side completes on Nr. 3, got {lf_completed:#?}"));
    assert_eq!(lf_ende["ohne_antwort"], true);
    assert_eq!(lf_ende["old_supplier"], LF_MP);
    assert_eq!(beendigung["pid"], 55_007);
    assert_eq!(beendigung["document_code"], "Z90");
    assert_eq!(
        beendigung["referenz_vorgangsnummer"],
        ankuendigung["vorgangsnummer"]
    );
    let [c55007] = &completed[..] else {
        panic!("one completion, got {completed:#?}")
    };
    assert_eq!(c55007["pid"], 55_007);
    assert_eq!(c55007["ohne_antwort"], true);
    assert_eq!(c55007["accepted"], true);
    assert_eq!(c55007["process_date"], "20270131");
    assert_eq!(c55007["old_supplier"], LF_MP);
}

/// **An answered 55007 ends the Zuordnung on the LF's Zustimmung**: the 55007
/// opens the LF's own process through the loopback, the LF agrees, and its
/// 55008 resumes the NB's process by the Vorgangsnummer — no Nr. 3 follows.
#[tokio::test]
async fn an_answered_55007_runs_to_its_zustimmung() {
    let c = combined().await;
    let nb = ankuendigen(&c).await;

    let (_, delivered, _) = c.drain(true).await;
    let lf = match delivered.as_slice() {
        [(55_007, IngestOutcome::Spawned { process_id, .. })] => *process_id,
        other => panic!("the 55007 opens the LF's own process, got {other:?}"),
    };
    assert_ne!(lf, nb);

    dispatch_command(
        &c.state,
        "gpke.nb-lieferende.bestaetigen",
        &serde_json::json!({ "malo_id": MALO, "antwort_code": "A10" }),
    )
    .await
    .expect("the LF agrees");
    let (_, delivered, _) = c.drain(true).await;
    match delivered.as_slice() {
        [(55_008, IngestOutcome::Dispatched { process_id, .. })] => {
            assert_eq!(*process_id, nb, "the 55008 resumes the NB's process");
        }
        other => panic!("the 55008 must resume the NB's process, got {other:?}"),
    }
    assert!(matches!(
        c.state_of(nb).await,
        NbLieferendeState::Beendet {
            ohne_antwort: false,
            ..
        }
    ));
    let (sent, _, completed) = c.drain(false).await;
    assert!(sent.is_empty(), "no message follows a Zustimmung");
    let ende = completed
        .iter()
        .find(|p| p["pid"] == 55_008 && p["grid_operator"] == NB_MP)
        .unwrap_or_else(|| panic!("the NB completes on the 55008, got {completed:#?}"));
    assert_eq!(ende["accepted"], true);
    assert_eq!(ende["antwortcode"], "A10");
    assert_eq!(ende["process_date"], "20270131");

    // Both deadlines still fire on the healthy path and change nothing.
    c.fire_all(nb).await;
    let (sent, _, _) = c.drain(false).await;
    assert!(sent.is_empty(), "no Beendigung after a Zustimmung");
}

/// A second Ankündigung on a Marktlokation whose first still runs is refused;
/// a Stilllegung, whose `DTM+206` the payload cannot carry, is refused before
/// anything is spawned.
#[tokio::test]
async fn a_running_ankuendigung_and_a_stilllegung_are_refused() {
    let c = combined().await;
    ankuendigen(&c).await;
    assert!(
        dispatch_command(&c.state, "gpke.nb-lieferende.ankuendigen", &ankuendigung())
            .await
            .is_err()
    );
    let c = combined().await;
    let mut body = ankuendigung();
    body["transaktionsgrund"] = serde_json::json!("Z33");
    assert!(
        dispatch_command(&c.state, "gpke.nb-lieferende.ankuendigen", &body)
            .await
            .is_err()
    );
    body["transaktionsgrund"] = serde_json::json!("ZQ7");
    body["unbekannt"] = serde_json::json!(true);
    assert!(
        dispatch_command(&c.state, "gpke.nb-lieferende.ankuendigen", &body)
            .await
            .is_err(),
        "an unknown key is refused"
    );
}

/// **An LF-only deployment ends its Abmeldung on the NB's Nr. 3**: the 55007
/// opens the LF's process, the `BGM+Z90` 55007 resumes that same process by
/// its `SG6 RFF+TN` and ends it — it never opens a second Ankündigung.
#[tokio::test]
async fn an_lf_only_deployment_ends_its_abmeldung_on_the_nbs_beendigung() {
    use mako_gpke::{GpkeLfAbmeldungWorkflow, LfAbmeldungState};
    // The NB is a counterparty here: its door produces the two messages.
    let nb = combined().await;
    let process = ankuendigen(&nb).await;
    let (_, _, _, ankuendigung) = nb.drain_wire(false).await;
    nb.fire_all(process).await;
    let (_, _, _, beendigung) = nb.drain_wire(false).await;

    let store = Arc::new(
        SlateDbStore::open_in_memory()
            .await
            .expect("in-memory store"),
    );
    let tenant_id = TenantId::from_party_id(LF_MP);
    let lf = EdifactIngestDispatcher::new(
        Arc::clone(&store),
        store.as_snapshot_store(),
        100,
        tenant_id,
    );
    let rollen: BTreeSet<Rolle> = [Rolle::LfStrom].into_iter().collect();
    let roles = DeploymentRoles::from_roles([Marktrolle::Lf]);
    let mut router = PidRouter::new();
    for module in makod::startup::modules_for(&rollen) {
        module.register_pids_with_roles(&mut router, &roles);
    }
    let deliver = |msg: edi_energy::AnyMessage| {
        let lf = &lf;
        let router = &router;
        async move {
            use edi_energy::EdiEnergyMessage as _;
            let pid = msg.detect_pruefidentifikator().expect("a PID").as_u32();
            let workflow = router
                .route(pid)
                .expect("55007 is routed on an LF deployment");
            lf.dispatch(&msg, workflow, pid, time::OffsetDateTime::now_utc())
                .await
                .expect("the 55007 dispatches")
        }
    };

    let [ankuendigung] = <[_; 1]>::try_from(ankuendigung).expect("one Ankündigung");
    let opened = match deliver(ankuendigung).await {
        IngestOutcome::Spawned { process_id, .. } => process_id,
        other => panic!("the Ankündigung opens the LF's process, got {other:?}"),
    };
    let [beendigung] = <[_; 1]>::try_from(beendigung).expect("one Beendigung");
    match deliver(beendigung).await {
        IngestOutcome::Dispatched { process_id, .. } => assert_eq!(process_id, opened),
        other => panic!("Nr. 3 resumes the Ankündigung, got {other:?}"),
    }
    let state = store
        .load(&StreamId::for_process(tenant_id, &opened))
        .await
        .expect("stream loads")
        .iter()
        .filter_map(|e| serde_json::from_value(e.payload.clone()).ok())
        .fold(LfAbmeldungState::default(), |s, e| {
            GpkeLfAbmeldungWorkflow::apply(s, &e)
        });
    assert!(
        matches!(state, LfAbmeldungState::Beendet(_)),
        "Nr. 3 ends the LF's Abmeldung, got {state:?}"
    );
    let now = time::OffsetDateTime::now_utc() + time::Duration::days(3);
    let pending = OutboxStore::pending(&*store, 100, now)
        .await
        .expect("pending outbox");
    let ende = pending
        .iter()
        .find(|om| &*om.message_type == "ProcessCompleted" && om.payload["pid"] == 55_007)
        .expect("the supply ends on the LF side");
    assert_eq!(ende.payload["ohne_antwort"], true);
    assert_eq!(ende.payload["process_date"], "20270131");
}

/// **A Stilllegung (`Z33`) runs to its Meldungen** (GPKE Teil 2 Kap. 2.5.2,
/// S. 57 and 61–64): the 55007 and its Nr. 3 carry the Geräteausbauzeitpunkt
/// (`DTM+206`, Bedingung `[79]`); the ending ends the MSB's Zuordnung (55611
/// `ZC8`) and cancels the MSBZ's (55611 `ZH1`) and the LFZ's (55038 `ZH1`),
/// every message held to its AHB profile (UTILMD AHB Strom 2.2 S. 195 f.); the
/// 07:00 Meldepflicht windows are registered on the process and find them sent.
#[tokio::test]
async fn a_stilllegung_ends_the_msb_and_cancels_lfz_and_msbz() {
    let c = combined().await;
    let mut body = ankuendigung();
    body["transaktionsgrund"] = serde_json::json!("Z33");
    body["geraeteausbaudatum"] = serde_json::json!("2027-01-15");
    body["msb_mp_id"] = serde_json::json!("9900001000003");
    body["msbz"] =
        serde_json::json!({ "mp_id": "9900001000004", "zuordnungsbeginn": "2027-03-01" });
    body["lfz"] = serde_json::json!({ "mp_id": "9900001000005", "zuordnungsbeginn": "2027-04-01" });
    let nb = match dispatch_command(&c.state, "gpke.nb-lieferende.ankuendigen", &body)
        .await
        .expect("a Stilllegung is announced")
    {
        DispatchOutcome::Spawned { process_id } => process_id,
        other => panic!("the Ankündigung must spawn, got {other:?}"),
    };
    let (sent, _, _) = c.drain(false).await;
    assert_eq!(sent[0]["geraeteausbaudatum"], "20270115");

    let fired = c.fire_all(nb).await;
    assert!(
        fired
            .iter()
            .any(|l| l == mako_gpke::nb_lieferende::MELDEPFLICHT_MSB_LABEL),
        "the MSB's Meldepflicht is registered: {fired:?}"
    );
    assert!(
        fired
            .iter()
            .any(|l| l == mako_gpke::nb_lieferende::MELDEPFLICHT_LFZ_LABEL),
        "the LFZ's Meldepflicht is registered: {fired:?}"
    );
    let (sent, _, _) = c.drain(false).await;
    let mut gesendet: Vec<(u64, String, String)> = sent
        .iter()
        .map(|p| {
            (
                p["pid"].as_u64().expect("pid"),
                p["receiver"].as_str().unwrap_or_default().to_owned(),
                p["transaktionsgrund"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            )
        })
        .collect();
    gesendet.sort();
    assert_eq!(
        gesendet,
        [
            (55_007, LF_MP.to_owned(), "Z33".to_owned()),
            (55_038, "9900001000005".to_owned(), "ZH1".to_owned()),
            (55_611, "9900001000003".to_owned(), "ZC8".to_owned()),
            (55_611, "9900001000004".to_owned(), "ZH1".to_owned()),
        ]
    );
    let events = c
        .store
        .load(&StreamId::for_process(c.state.tenant_id, &nb))
        .await
        .expect("stream loads");
    assert!(
        events
            .iter()
            .all(|e| e.payload["type"] != "MeldepflichtVerstrichen"),
        "the Meldungen went out with the ending, so no window finds them owed"
    );
}

/// A Stilllegung without its Geräteausbauzeitpunkt or its MSB is refused, and
/// so are those facts under another Grund.
#[tokio::test]
async fn a_stilllegung_without_its_facts_is_refused() {
    let c = combined().await;
    let mut body = ankuendigung();
    body["transaktionsgrund"] = serde_json::json!("Z33");
    assert!(
        dispatch_command(&c.state, "gpke.nb-lieferende.ankuendigen", &body)
            .await
            .is_err()
    );
    let mut body = ankuendigung();
    body["geraeteausbaudatum"] = serde_json::json!("2027-01-15");
    body["msb_mp_id"] = serde_json::json!("9900001000003");
    assert!(
        dispatch_command(&c.state, "gpke.nb-lieferende.ankuendigen", &body)
            .await
            .is_err()
    );
}

/// The NB's own Meldung doors render `ZH1` — UTILMD AHB Strom 2.2 S. 195 f.
/// lists it on 55038 and 55611 — and the message passes its AHB profile, as
/// the AS4 sender's pre-send gate requires.
#[tokio::test]
async fn the_meldung_doors_render_zh1_their_profile_admits() {
    let c = combined().await;
    dispatch_command(
        &c.state,
        "gpke.zuordnung.aufheben",
        &serde_json::json!({
            "malo_id": MALO,
            "empfaenger_mp_id": "9900001000005",
            "transaktionsgrund": "ZH1",
            "process_date": "20270401",
            "beteiligte_marktpartner": [NB_MP],
        }),
    )
    .await
    .expect("the 55038 is queued");
    dispatch_command(
        &c.state,
        "gpke.msb-zuordnung.beenden",
        &serde_json::json!({
            "malo_id": MALO,
            "empfaenger_mp_id": "9900001000004",
            "transaktionsgrund": "ZH1",
            "process_date": "20270301",
        }),
    )
    .await
    .expect("the 55611 is queued");
    // `drain` renders each UTILMD and holds it to its AHB profile.
    let (sent, _, _) = c.drain(false).await;
    let mut pids: Vec<u64> = sent.iter().filter_map(|p| p["pid"].as_u64()).collect();
    pids.sort_unstable();
    assert_eq!(pids, [55_038, 55_611]);
}

/// **The Lieferbeginn's Meldepflichten ride the Abmeldeanfrage** (GPKE Teil 2
/// Kap. 2.1.2, S. 19–23): Nr. 2, the Information über existierende Zuordnung,
/// is owed exactly where Nr. 3 is („Parallel zu Nr. 2"), and Nr. 10, the
/// Beendigung, once the LFA released. The Anfrage's spawn registers both
/// windows on the Anmeldung's Eingang; the Information door discharges its
/// window, and the Beendigung nobody sent fires as owed.
#[tokio::test]
async fn the_lieferbeginn_meldepflichten_ride_the_abmeldeanfrage() {
    let c = combined().await;
    let anfrage = match dispatch_command(
        &c.state,
        "gpke.beendigung-zuordnung.anfragen",
        &serde_json::json!({
            "malo_id": MALO,
            "lfa_mp_id": LF_MP,
            "process_date": "2026-04-01",
            "anmeldung_process_id": "11111111-1111-1111-1111-111111111111",
            "anmeldung_eingang": "2026-03-02T08:00:00Z",
            "lfn_mp_id": "9900001000005",
        }),
    )
    .await
    .expect("the Anfrage is sent")
    {
        DispatchOutcome::Spawned { process_id } => process_id,
        other => panic!("the Anfrage must spawn, got {other:?}"),
    };
    dispatch_command(
        &c.state,
        "gpke.zuordnung.informieren",
        &serde_json::json!({
            "malo_id": MALO,
            "empfaenger_mp_id": "9900001000005",
            "transaktionsgrund": "Z26",
            "beteiligte_marktpartner": [LF_MP],
            "referenz_vorgangsnummer": "VG-LFN",
        }),
    )
    .await
    .expect("the Information is sent");

    let fired = c.fire_all(anfrage).await;
    for label in [
        mako_gpke::beendigung_zuordnung::MELDEPFLICHT_INFORMATION_LABEL,
        mako_gpke::beendigung_zuordnung::MELDEPFLICHT_BEENDIGUNG_LABEL,
    ] {
        assert!(fired.iter().any(|l| l == label), "{label}: {fired:?}");
    }
    let verstrichen: Vec<String> = c
        .store
        .load(&StreamId::for_process(c.state.tenant_id, &anfrage))
        .await
        .expect("stream loads")
        .iter()
        .filter(|e| e.payload["type"] == "MeldepflichtVerstrichen")
        .map(|e| {
            e.payload["data"]["label"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    assert_eq!(
        verstrichen,
        [mako_gpke::beendigung_zuordnung::MELDEPFLICHT_BEENDIGUNG_LABEL],
        "the sent Information is discharged; the unsent Beendigung is owed"
    );
}
