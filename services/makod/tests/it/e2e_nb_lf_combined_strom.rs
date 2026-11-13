//! A deployment that is NB and LF at once runs both sides of one Strom EoG.
//!
//! The NB sends the Anmeldung / Zuordnung EOG 55013 to the Grundversorger, and
//! on a combined NB+LF deployment the Grundversorger is the same makod: its AS4
//! loopback hands the 55013 straight back to its own ingest. Both sides run the
//! one workflow name `gpke-eog` on the one Marktlokation — the NB waiting in
//! `Angemeldet` for the answer, the LF owing it (UTILMD AHB Strom Kap. 8.6,
//! 55013 NB → LF, 55014/55015 LF → NB). The LF's copy must open the LF's own
//! process rather than resume the NB's, `gpke.eog.bestaetigen` must find the
//! LF's process alone, and the LF's 55014 must reach the NB's process.
//!
//! The loopback is driven as production drives it: the outbox entry is
//! rendered to wire, parsed, validated against its AHB profile — the pre-send
//! gate's check — routed by the `PidRouter` an `nb-strom` + `lf-strom`
//! deployment builds, and dispatched into the one tenant both roles share. The
//! 55013 is the one `gpke.eog.anmelden` renders from the Marktlokation's
//! Stammdaten, which makod reads from a stand-in marktd.

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
use mako_gpke::{EogState, GpkeEogWorkflow};
use makod::commands_api::{CommandsApiState, DispatchOutcome, dispatch_command};
use makod::deployment::Rolle;
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

// BDEW-Codenummern: NAD DE 3055 `293`, the code a Strom MIG admits.
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
    // One tenant for the deployment, both roles inside it.
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
        marktd_client: Some(Arc::new(
            crate::support::stub_marktd(Some(crate::support::strom_stammdaten(MALO, NB_MP))).await,
        )),
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
    /// Deliver every pending UTILMD to this deployment's own ingest, as the AS4
    /// loopback does for a message addressed to one of its own MP-IDs, and
    /// acknowledge everything else the outbox holds — ERP notifications are not
    /// market messages. Every UTILMD must pass its AHB profile first, as the
    /// pre-send gate (`transport::as4_sender`) requires on the loopback.
    async fn loopback(&self) -> Vec<(u32, IngestOutcome)> {
        use edi_energy::EdiEnergyMessage as _;
        let now = time::OffsetDateTime::now_utc() + time::Duration::days(1);
        let pending = OutboxStore::pending(&*self.store, 100, now)
            .await
            .expect("pending outbox");
        let mut outcomes = Vec::new();
        for om in &pending {
            if &*om.message_type == "UTILMD" {
                assert!(
                    self.registry.is_own_mp_id(&om.recipient),
                    "the UTILMD is addressed to this deployment, got {}",
                    om.recipient
                );
                if om.payload["pid"] == 55013 {
                    assert_eq!(
                        &*om.recipient, LF_MP,
                        "the 55013 is addressed to this deployment's own LF"
                    );
                    assert_eq!(om.payload["sender"], NB_MP);
                }
                let msg = gated(om, &self.registry);
                let pid = msg
                    .detect_pruefidentifikator()
                    .expect("a Prüfidentifikator")
                    .as_u32();
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

    async fn state_of(&self, process: ProcessId) -> EogState {
        use mako_engine::workflow::Workflow as _;
        self.store
            .load(&StreamId::for_process(self.state.tenant_id, &process))
            .await
            .expect("stream loads")
            .iter()
            .map(|e| serde_json::from_value(e.payload.clone()).expect("a gpke-eog event"))
            .fold(EogState::default(), |s, e| GpkeEogWorkflow::apply(s, &e))
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

/// **The NB's 55013 is a complete Anmeldung EoG**: `gpke.eog.anmelden` reads
/// the Marktlokation's Stammdaten from marktd and writes a 55013 its AHB
/// profile accepts — `ZW7` with the Messlokation, its MSB and the `SG12`
/// Anschriften.
#[tokio::test]
async fn gpke_eog_anmelden_renders_a_55013_its_profile_accepts() {
    use edi_energy::EdiEnergyMessage as _;
    let c = combined().await;
    dispatch_command(&c.state, "gpke.eog.anmelden", &anmeldung())
        .await
        .expect("gpke.eog.anmelden spawns");
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
        Some(55013)
    );
    let wire = String::from_utf8(
        makod::edifact_renderer::render_to_wire_bytes(om, &c.registry)
            .expect("renders")
            .bytes,
    )
    .expect("UNOC text");
    assert!(wire.contains("STS+7++ZT6+ZW7'"), "{wire}");
    assert!(wire.contains("SEQ+Z98'"), "{wire}");
    assert!(wire.contains("NAD+Z63"), "{wire}");
}

/// **A Pauschale-Marktlokation is a `ZW6` 55013** — marktd assigns it no
/// Messlokation, so it names none and classifies the Marktlokation `Z68`
/// keine Messung.
#[tokio::test]
async fn a_marktlokation_without_messlokation_is_announced_pauschal() {
    let mut record = crate::support::strom_stammdaten(MALO, NB_MP);
    record.messlokationen.clear();
    let mut c = combined().await;
    c.state.marktd_client = Some(Arc::new(crate::support::stub_marktd(Some(record)).await));
    dispatch_command(&c.state, "gpke.eog.anmelden", &anmeldung())
        .await
        .expect("gpke.eog.anmelden spawns");
    let now = time::OffsetDateTime::now_utc() + time::Duration::days(1);
    let pending = OutboxStore::pending(&*c.store, 100, now)
        .await
        .expect("pending outbox");
    let om = pending
        .iter()
        .find(|om| &*om.message_type == "UTILMD")
        .expect("a UTILMD");
    gated(om, &c.registry);
    let wire = String::from_utf8(
        makod::edifact_renderer::render_to_wire_bytes(om, &c.registry)
            .expect("renders")
            .bytes,
    )
    .expect("UNOC text");
    assert!(wire.contains("STS+7++ZT6+ZW6'"), "{wire}");
    assert!(wire.contains("CAV+Z68'"), "{wire}");
}

/// **A Marktlokation without an address is refused** naming the
/// Marktlokationsanschrift, and master data that cannot be read refuses the
/// command — no 55013 goes out without them.
#[tokio::test]
async fn gpke_eog_anmelden_without_its_master_data_is_refused() {
    let mut record = crate::support::strom_stammdaten(MALO, NB_MP);
    record.anschrift = None;
    let mut c = combined().await;
    c.state.marktd_client = Some(Arc::new(crate::support::stub_marktd(Some(record)).await));
    match dispatch_command(&c.state, "gpke.eog.anmelden", &anmeldung()).await {
        Err(makod::commands_api::DispatchError::StammdatenUnvollstaendig { fehlend, .. }) => {
            assert!(
                fehlend
                    .iter()
                    .any(|f| f.contains("Marktlokationsanschrift")),
                "{fehlend:?}"
            );
        }
        other => panic!("refused as incomplete Stammdaten, got {other:?}"),
    }
    c.state.marktd_client = Some(Arc::new(crate::support::unreachable_marktd()));
    assert!(matches!(
        dispatch_command(&c.state, "gpke.eog.anmelden", &anmeldung()).await,
        Err(makod::commands_api::DispatchError::MarktdUnavailable(_))
    ));
    let mut body = anmeldung();
    body["haushaltskunde"] = serde_json::json!(true);
    let err = dispatch_command(&c.state, "gpke.eog.anmelden", &body)
        .await
        .expect_err("an undeclared key is refused");
    assert!(format!("{err:?}").contains("haushaltskunde"), "{err:?}");
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

fn anmeldung() -> serde_json::Value {
    serde_json::json!({
        "malo_id": MALO,
        "gv_mp_id": LF_MP,
        "process_date": "2026-12-01",
        "transaktionsgrund": "ZT6",
    })
}

/// **A 55013 runs to its 55014 on one combined deployment**: the LF's copy
/// opens the LF's own process, the LF answers it, and the answer reaches the
/// NB's process, so both sides end where their half of the Vorgang ends.
#[tokio::test]
async fn a_combined_deployment_runs_the_strom_eog_to_its_answer() {
    let c = combined().await;

    // 1. The NB assigns the Marktlokation to its own Grundversorger.
    let nb = match dispatch_command(&c.state, "gpke.eog.anmelden", &anmeldung())
        .await
        .expect("gpke.eog.anmelden spawns")
    {
        DispatchOutcome::Spawned { process_id } => process_id,
        other => panic!("gpke.eog.anmelden must spawn, got {other:?}"),
    };

    // 2. The loopback delivers the 55013 to the LF side.
    let delivered = c.loopback().await;
    let lf = match delivered.as_slice() {
        [(55013, IngestOutcome::Spawned { process_id, .. })] => *process_id,
        other => panic!("the LF's copy of the 55013 must open the LF's own process, got {other:?}"),
    };
    assert_ne!(lf, nb, "the two sides run two processes");
    assert!(
        matches!(c.state_of(nb).await, EogState::Angemeldet(_)),
        "the NB still waits for the answer"
    );

    // 3. The LF confirms the Ersatzversorgung; the loopback below holds the
    //    55014 to its profile.
    let answered = dispatch_command(&c.state, "gpke.eog.bestaetigen", &bestaetigung())
        .await
        .expect("the LF answers its own process");
    assert!(
        matches!(answered, DispatchOutcome::Dispatched { process_id } if process_id == lf),
        "gpke.eog.bestaetigen answers the LF's process, got {answered:?}"
    );
    assert!(
        matches!(c.state_of(lf).await, EogState::AntwortGesendet { .. }),
        "the LF's process records its sent 55014"
    );

    // 4. The loopback delivers the 55014 to the NB side.
    let delivered = c.loopback().await;
    match delivered.as_slice() {
        [(55014, IngestOutcome::Dispatched { process_id, .. })] => assert_eq!(
            *process_id, nb,
            "the 55014 resumes the NB's process, not the LF's"
        ),
        other => panic!("the 55014 must resume the NB's process, got {other:?}"),
    }
    match c.state_of(nb).await {
        EogState::Zugeordnet { bilanzkreis, .. } => assert_eq!(
            bilanzkreis.as_deref(),
            Some(BILANZKREIS),
            "the NB reads the E/G's Bilanzkreis from SEQ+Z79 CAV+ZV4"
        ),
        other => panic!("the NB's EoG is concluded by the Bestätigung, got {other:?}"),
    }
    assert!(
        matches!(c.state_of(lf).await, EogState::AntwortGesendet { .. }),
        "the 55014 must not reach the LF's own process"
    );
}

/// **A 55013 states its Transaktionsgrundergänzung or does not render**: the
/// column admits `ZW6`/`ZW7` and not the `ZW4` default, so a payload that
/// states none is a fact nobody stated — `RenderError::MissingField` naming
/// the field and the codes the column lists — and one that states `ZW7` with
/// the Marktlokation's Stammdaten validates clean.
#[tokio::test]
async fn a_55013_payload_without_its_ergaenzung_does_not_render() {
    let c = combined().await;
    let payload = serde_json::to_value(mako_engine::payload::UtilmdPayload {
        pid: 55013,
        sender: NB_MP.to_owned(),
        receiver: Some(LF_MP.to_owned()),
        malo: Some(MALO.to_owned()),
        process_date: Some("20261201".to_owned()),
        transaktionsgrund: Some("ZT6".to_owned()),
        ..Default::default()
    })
    .expect("a payload");
    let om = crate::support::outbox_message("UTILMD", LF_MP, payload);
    match makod::edifact_renderer::render_to_wire_bytes(&om, &c.registry) {
        Err(makod::edifact_renderer::RenderError::MissingField { field, .. }) => {
            assert!(field.contains("transaktionsgrund_ergaenzung"), "{field}");
            assert!(field.contains("ZW6") && field.contains("ZW7"), "{field}");
        }
        other => panic!("a 55013 without its Ergänzung must not render, got {other:?}"),
    }

    dispatch_command(&c.state, "gpke.eog.anmelden", &anmeldung())
        .await
        .expect("gpke.eog.anmelden spawns");
    let pending = OutboxStore::pending(
        &*c.store,
        100,
        time::OffsetDateTime::now_utc() + time::Duration::days(1),
    )
    .await
    .expect("pending outbox");
    let om = pending
        .iter()
        .find(|om| &*om.message_type == "UTILMD")
        .expect("a UTILMD");
    assert_eq!(om.payload["transaktionsgrund_ergaenzung"], "ZW7");
    gated(om, &c.registry);
}

/// The NB's 55013 as `gpke.eog.anmelden` writes it from marktd's record.
pub(crate) async fn rendered_55013() -> mako_engine::outbox::OutboxMessage {
    let c = combined().await;
    dispatch_command(&c.state, "gpke.eog.anmelden", &anmeldung())
        .await
        .expect("gpke.eog.anmelden spawns");
    let now = time::OffsetDateTime::now_utc() + time::Duration::days(1);
    OutboxStore::pending(&*c.store, 100, now)
        .await
        .expect("pending outbox")
        .into_iter()
        .find(|om| &*om.message_type == "UTILMD" && om.payload["pid"] == 55013)
        .expect("the 55013 is written")
}

/// The LF's 55014 as `gpke.eog.bestaetigen` writes it on a combined
/// deployment — the NB's 55013 delivered to the LF side, then answered.
pub(crate) async fn rendered_55014() -> mako_engine::outbox::OutboxMessage {
    let c = combined().await;
    dispatch_command(&c.state, "gpke.eog.anmelden", &anmeldung())
        .await
        .expect("gpke.eog.anmelden spawns");
    c.loopback().await;
    dispatch_command(&c.state, "gpke.eog.bestaetigen", &bestaetigung())
        .await
        .expect("the LF answers");
    let now = time::OffsetDateTime::now_utc() + time::Duration::days(1);
    OutboxStore::pending(&*c.store, 100, now)
        .await
        .expect("pending outbox")
        .into_iter()
        .find(|om| &*om.message_type == "UTILMD" && om.payload["pid"] == 55014)
        .expect("the 55014 is written")
}

/// `11XBK-STD------Y`, the Strom Bilanzkreis EIC the profile skeleton samples.
const BILANZKREIS: &str = "11XBK-STD------Y";

/// The E/G's Bestätigung: the Versorgungsart — `ZC9` Ersatzversorgung, as the
/// fixture's Kunde is no Haushaltskunde — and the three facts of its own a
/// 55014 carries beside the Marktlokation's record — the Bilanzkreis, and the
/// Grundlage nach EnFG with the § of the privilege it claims.
fn bestaetigung() -> serde_json::Value {
    serde_json::json!({
        "malo_id": MALO,
        "versorgungsart": "ZC9",
        "bilanzkreis": BILANZKREIS,
        "enfg_grundlage": "ZF9",
        "enfg_grund": "ZU6",
    })
}

/// **A 55014 without one of the E/G's own Muss facts is refused**, naming it —
/// never written as a message the pre-send gate dead-letters — and so is a
/// Marktlokation whose Gruppenzuordnung marktd does not hold. The process
/// still owes its answer afterwards.
#[tokio::test]
async fn gpke_eog_bestaetigen_without_a_muss_fact_is_refused() {
    let c = combined().await;
    dispatch_command(&c.state, "gpke.eog.anmelden", &anmeldung())
        .await
        .expect("gpke.eog.anmelden spawns");
    let lf = match c.loopback().await.as_slice() {
        [(55013, IngestOutcome::Spawned { process_id, .. })] => *process_id,
        other => panic!("the LF's copy of the 55013 opens its process, got {other:?}"),
    };
    for missing in [
        "versorgungsart",
        "bilanzkreis",
        "enfg_grundlage",
        "enfg_grund",
    ] {
        let mut body = bestaetigung();
        body.as_object_mut().expect("an object").remove(missing);
        let err = dispatch_command(&c.state, "gpke.eog.bestaetigen", &body)
            .await
            .expect_err("a 55014 without it is refused");
        assert!(
            format!("{err:?}").contains(missing),
            "the refusal names {missing:?}: {err:?}"
        );
    }
    let mut body = bestaetigung();
    body["enfg_grundlage"] = serde_json::json!("ZG0");
    let err = dispatch_command(&c.state, "gpke.eog.bestaetigen", &body)
        .await
        .expect_err("a Privilegierung beside ZG0 is refused");
    assert!(format!("{err:?}").contains("[463]"), "{err:?}");

    let mut record = crate::support::strom_stammdaten(MALO, NB_MP);
    if let Some(n) = record.netznutzung.as_mut() {
        n.haushaltskunde = None;
    }
    let mut c = c;
    c.state.marktd_client = Some(Arc::new(crate::support::stub_marktd(Some(record)).await));
    match dispatch_command(&c.state, "gpke.eog.bestaetigen", &bestaetigung()).await {
        Err(makod::commands_api::DispatchError::StammdatenUnvollstaendig { fehlend, .. }) => {
            assert!(
                fehlend.iter().any(|f| f.contains("haushaltskunde")),
                "{fehlend:?}"
            );
        }
        other => panic!("refused as incomplete Stammdaten, got {other:?}"),
    }
    assert!(
        matches!(c.state_of(lf).await, EogState::ValidationPassed(_)),
        "a refused answer leaves the LF's process owing it"
    );
}

/// **A 55015 reaches the NB's process by its `RFF+TN` alone**: the Ablehnung's
/// column names no Lokation either (UTILMD AHB Strom 2.2 Kap. 8.6), and the
/// loopback holds it to its profile before it resumes the NB.
#[tokio::test]
async fn a_combined_deployment_runs_the_strom_eog_to_its_ablehnung() {
    let c = combined().await;
    let nb = match dispatch_command(&c.state, "gpke.eog.anmelden", &anmeldung())
        .await
        .expect("gpke.eog.anmelden spawns")
    {
        DispatchOutcome::Spawned { process_id } => process_id,
        other => panic!("gpke.eog.anmelden must spawn, got {other:?}"),
    };
    c.loopback().await;
    dispatch_command(
        &c.state,
        "gpke.eog.ablehnen",
        &serde_json::json!({ "malo_id": MALO, "reason": "A05" }),
    )
    .await
    .expect("the LF refuses");
    match c.loopback().await.as_slice() {
        [(55015, IngestOutcome::Dispatched { process_id, .. })] => assert_eq!(*process_id, nb),
        other => panic!("the 55015 must resume the NB's process, got {other:?}"),
    }
    assert!(
        matches!(c.state_of(nb).await, EogState::Abgelehnt { .. }),
        "the NB's EoG ends in the Ablehnung"
    );
}
