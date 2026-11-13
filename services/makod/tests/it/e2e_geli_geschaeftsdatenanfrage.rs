//! The Geschäftsdatenanfrage Gas reaches its own process on both sides.
//!
//! BK7-24-01-009 Anlage Kap. 4.4 (S. 14): the LF asks the Gasnetzbetreiber
//! with ORDERS 17101, the MSB with 17126; the NB answers within the 10. WT with
//! UTILMD 44035 / 44060 or refuses with ORDRSP 19101 (Anwendungsübersicht der
//! Prüfidentifikatoren 4.0 lfd. Nr. 38480–38580). 19101 is the Strom
//! Geschäftsdatenanfrage's Ablehnung as well, so it routes by the recipient's
//! Sparte: a Gas 19101 resumes the GeLi Gas Anfragender, a Strom one
//! `gpke-geschaeftsdatenanfrage`, whatever the module order.

use std::collections::BTreeSet;
use std::sync::Arc;

use mako_engine::deadline::DeadlineStore as _;
use mako_engine::marktrolle::DeploymentRoles;
use mako_engine::outbox::OutboxStore as _;
use mako_engine::pid_router::PidRouter;
use mako_engine::process::Process;
use mako_engine::registry::ProcessRegistry as _;
use mako_engine::types::Sparte;
use mako_engine::{ids::TenantId, store_slatedb::SlateDbStore};
use mako_geli_gas::geschaeftsdatenanfrage::{
    ANTWORT_WINDOW_LABEL, GdaGasState, GeliGasGeschaeftsdatenanfrageWorkflow, WORKFLOW_NAME,
    anfragender_key, netzbetreiber_key,
};
use makod::commands_api::{CommandsApiState, DispatchOutcome, dispatch_command};
use makod::deployment::Rolle;
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

// DVGW-Codenummern: NAD DE 3055 `332`.
const GNB_MP: &str = "9807317000007";
const LF_MP: &str = "9870012345678";
const MALO: &str = "51238696012";

fn router_for(rollen: &[Rolle], roles: &DeploymentRoles, reversed: bool) -> PidRouter {
    let rollen: BTreeSet<Rolle> = rollen.iter().copied().collect();
    let mut modules = makod::startup::modules_for(&rollen);
    if reversed {
        modules.reverse();
    }
    let mut router = PidRouter::new();
    for module in modules {
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

async fn party(own: &str, rolle: &str) -> (CommandsApiState, EdifactIngestDispatcher) {
    use makod::{
        cedar_authz::{CedarAuthorizer, DefaultPolicy},
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

async fn state_on(state: &CommandsApiState, key: &str) -> GdaGasState {
    let identity = state
        .store
        .as_process_registry()
        .lookup_correlated(state.tenant_id, key)
        .await
        .expect("lookup")
        .into_iter()
        .find(|id| id.workflow_id.name.as_ref() == WORKFLOW_NAME)
        .unwrap_or_else(|| panic!("no Geschäftsdatenanfrage Gas on {key}"));
    Process::<GeliGasGeschaeftsdatenanfrageWorkflow, _>::from_identity(
        state.store.clone(),
        identity,
    )
    .state()
    .await
    .expect("state")
}

// ── Routing ───────────────────────────────────────────────────────────────────

/// The Gas NB routes both Anfragen, the LF its answer, and a 19101 addressed
/// to a Gas party reaches the Gas process while a Strom one reaches GPKE — in
/// either module registration order.
#[test]
fn the_gas_geschaeftsdatenanfrage_routes_by_role_and_sparte() {
    let nb = router_for(&[Rolle::NbGas], &DeploymentRoles::nb(), false);
    for pid in [17_101, 17_126] {
        assert_eq!(nb.route(pid), Some(WORKFLOW_NAME), "{pid}");
    }
    let lf = router_for(&[Rolle::LfGas], &DeploymentRoles::lf(), false);
    assert_eq!(lf.route(44_035), Some(WORKFLOW_NAME));
    assert_eq!(
        lf.route_with_sparte(19_101, Sparte::Gas),
        Some(WORKFLOW_NAME)
    );

    for reversed in [false, true] {
        let both = router_for(
            &[Rolle::LfStrom, Rolle::LfGas],
            &DeploymentRoles::lf(),
            reversed,
        );
        assert_eq!(
            both.route_with_sparte(19_101, Sparte::Gas),
            Some(WORKFLOW_NAME),
            "Gas 19101, reversed = {reversed}"
        );
        assert_eq!(
            both.route_with_sparte(19_101, Sparte::Strom),
            Some("gpke-geschaeftsdatenanfrage"),
            "Strom 19101, reversed = {reversed}"
        );
    }
}

// ── Anfragender side ──────────────────────────────────────────────────────────

/// An LF sends a 17101 and the NB's 44035 resumes that process by the
/// Anfrage-Nummer it echoes in `SG6 RFF+AAV`.
#[tokio::test]
async fn an_outbound_17101_is_resumed_by_the_44035() {
    let (lf, dispatcher) = party(LF_MP, "LF").await;
    let outcome = dispatch_command(
        &lf,
        "geli.geschaeftsdatenanfrage.anfragen",
        &serde_json::json!({
            "lokation_id": MALO,
            "nb_mp_id": GNB_MP,
            "anfragender": "LF",
            "anfrage_nummer": "GDAGAS1",
        }),
    )
    .await
    .expect("anfragen");
    assert!(
        matches!(outcome, DispatchOutcome::Spawned { .. }),
        "{outcome:?}"
    );
    let pending = lf.store.pending_now(10).await.expect("outbox");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].message_type.as_ref(), "ORDERS");
    assert_eq!(pending[0].payload["pid"], 17_101);
    assert_eq!(pending[0].payload["document_code"], "Z61");
    // It renders as the ORDERS AHB 1.1b Kap. 4.1.1.2 (S. 10) lays it out and
    // passes its profile.
    let registry =
        makod::party_registry::MpIdRegistry::from_config(&[makod::config::PartyConfig {
            mp_id: LF_MP.to_owned(),
            roles: vec!["LFG".to_owned()],
            primary: true,
            agency: None,
            as4: None,
        }])
        .expect("one party");
    let wire = makod::edifact_renderer::render_to_wire_bytes(&pending[0], &registry)
        .expect("the 17101 renders");
    let text = String::from_utf8_lossy(&wire.bytes);
    assert!(text.contains("BGM+Z61+GDAGAS1"), "{text}");
    assert!(text.contains("RFF+Z13:17101"), "{text}");
    let report = edi_energy::Platform::with_all_profiles()
        .validate(&parse(&text))
        .expect("validate");
    assert!(report.errors().is_empty(), "{text}\n{:?}", report.errors());

    let wire = set_element(&skeleton(44_035, GNB_MP, LF_MP), "RFF+AAV:", "GDAGAS1");
    let outcome = dispatcher
        .dispatch(
            &parse(&wire),
            WORKFLOW_NAME,
            44_035,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("dispatch");
    assert!(
        matches!(outcome, IngestOutcome::Dispatched { .. }),
        "{outcome:?}"
    );
    assert!(matches!(
        state_on(&lf, &anfragender_key("GDAGAS1")).await,
        GdaGasState::Beantwortet { antwort_pid, .. } if antwort_pid.as_u32() == 44_035
    ));
}

/// A Gas 19101 resumes the Anfragender's process by its `SG1 RFF+ON`.
#[tokio::test]
async fn a_gas_19101_resumes_the_anfragender() {
    let (lf, dispatcher) = party(LF_MP, "LF").await;
    dispatch_command(
        &lf,
        "geli.geschaeftsdatenanfrage.anfragen",
        &serde_json::json!({
            "lokation_id": MALO,
            "nb_mp_id": GNB_MP,
            "anfragender": "LF",
            "anfrage_nummer": "GDAGAS2",
        }),
    )
    .await
    .expect("anfragen");
    let wire = set_element(&skeleton(19_101, GNB_MP, LF_MP), "RFF+ON:", "GDAGAS2");
    let outcome = dispatcher
        .dispatch(
            &parse(&wire),
            WORKFLOW_NAME,
            19_101,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("dispatch");
    assert!(
        matches!(outcome, IngestOutcome::Dispatched { .. }),
        "{outcome:?}"
    );
    assert!(matches!(
        state_on(&lf, &anfragender_key("GDAGAS2")).await,
        GdaGasState::Abgelehnt { .. }
    ));
}

// ── NB side ───────────────────────────────────────────────────────────────────

/// An inbound 17101 opens the Gas NB's process with the 10-WT window of
/// Kap. 4.4, and the NB's Ablehnung is drawn from `G_0049`.
#[tokio::test]
async fn an_inbound_17101_opens_the_nbs_process_with_its_ten_werktage() {
    let (nb, dispatcher) = party(GNB_MP, "NB").await;
    let wire = skeleton(17_101, LF_MP, GNB_MP);
    let msg = parse(&wire);
    let nummer = makod::adapters::gda_anfrage_nummer(&msg);
    assert!(!nummer.is_empty(), "the ORDERS names its Anfrage");
    let eingang = time::macros::datetime!(2026-10-02 09:00 UTC);
    let outcome = dispatcher
        .dispatch(&msg, WORKFLOW_NAME, 17_101, eingang)
        .await
        .expect("dispatch");
    assert!(
        matches!(outcome, IngestOutcome::Spawned { .. }),
        "{outcome:?}"
    );

    let deadlines: Vec<_> = nb
        .store
        .as_deadline_store()
        .due_now(100)
        .await
        .expect("deadlines")
        .deadlines
        .iter()
        .filter(|d| d.label() == ANTWORT_WINDOW_LABEL)
        .map(mako_engine::deadline::Deadline::due_at)
        .collect();
    assert_eq!(
        deadlines,
        vec![
            mako_fristen::antwort::antwort_deadline(17_101, eingang)
                .expect("17101 has a published Antwortfrist")
        ]
    );

    let key = netzbetreiber_key(LF_MP, &nummer);
    let offen = state_on(&nb, &key).await;
    assert!(matches!(offen, GdaGasState::Offen(_)), "{offen:?}");
    dispatch_command(
        &nb,
        "geli.geschaeftsdatenanfrage.ablehnen",
        &serde_json::json!({
            "anfragender_mp_id": LF_MP,
            "anfrage_nummer": nummer,
            "antwortcode": "Z15",
        }),
    )
    .await
    .expect("ablehnen");
    let pending = nb.store.pending_now(10).await.expect("outbox");
    let ordrsp = pending
        .iter()
        .find(|p| p.message_type.as_ref() == "ORDRSP")
        .expect("the 19101 is queued");
    assert_eq!(ordrsp.payload["antwort_codeliste"], "G_0049");
    assert!(matches!(
        state_on(&nb, &key).await,
        GdaGasState::Abgelehnt { .. }
    ));
}
