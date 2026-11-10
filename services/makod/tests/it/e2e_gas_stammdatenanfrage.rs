//! A GeLi Gas Anfrage zur Stammdatenänderung is answered with the master data
//! marktd holds on its Änderungsdatum.
//!
//! The Lieferant's 44139 asks the NB to change a Stammdatum the NB is
//! Verantwortlicher for; the NB checks it and answers 44142 (UTILMD AHB Gas
//! 1.2 Kap. 5.12.1). On `ZG2` „Gültiges Ergebnis nach der Datenprüfung" the
//! answer carries „die zum Änderungsdatum gültigen Stammdaten … Hierbei ist es
//! unerheblich, ob die aus der Anfrage enthaltenen Stammdaten vom
//! Verantwortlichen übernommen wurden oder nicht" (EBD 4.3 Codeliste `G_0031`,
//! S. 742) — for each group the Anfrage filled (Bedingung `[336]`).
//!
//! The 44139 is rendered and parsed as a counterparty's would be, the answer
//! is built by `geli.stammdaten.anfrage.beantworten` from a stand-in marktd,
//! and the 44142 is rendered and checked against its AHB profile.

use std::collections::BTreeSet;
use std::sync::Arc;

use mako_engine::marktrolle::{DeploymentRoles, Marktrolle};
use mako_engine::outbox::OutboxStore;
use mako_engine::pid_router::PidRouter;
use mako_engine::{ids::TenantId, store_slatedb::SlateDbStore};
use makod::commands_api::{CommandsApiState, DispatchError, dispatch_command};
use makod::deployment::Rolle;
use makod::ingest_dispatcher::EdifactIngestDispatcher;

use crate::support;

// DVGW-Codenummern: NAD DE 3055 `332`.
const GNB_MP: &str = "9807317000007";
const LF_MP: &str = "9870012345678";
const MALO: &str = "51238696012";
/// The Marktgebiet the Lieferant proposes — not the one marktd holds.
const VORGESCHLAGEN: &str = "37Y701125MH0000I";

struct Nb {
    store: Arc<SlateDbStore>,
    state: CommandsApiState,
    dispatcher: EdifactIngestDispatcher,
    router: PidRouter,
    registry: makod::party_registry::MpIdRegistry,
}

async fn nb(record: Option<mako_markt::utilmd_stammdaten::MaloUtilmdStammdaten>) -> Nb {
    use makod::config::PartyConfig;
    let store = Arc::new(
        SlateDbStore::open_in_memory()
            .await
            .expect("in-memory store"),
    );
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
        configured_marktrollen: vec!["GNB".to_owned()],
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
    let rollen: BTreeSet<Rolle> = [Rolle::NbGas].into_iter().collect();
    let roles = DeploymentRoles::from_roles([Marktrolle::Nb]);
    let mut router = PidRouter::new();
    for module in makod::startup::modules_for(&rollen) {
        module.register_pids_with_roles(&mut router, &roles);
    }
    // The Lieferant is registered too, so its 44139 renders with its own
    // agency code; the deployment answers as the GNB.
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
    .expect("valid registry");
    Nb {
        store,
        state,
        dispatcher,
        router,
        registry,
    }
}

/// Render `om`, parse it and hold it against its AHB profile, as the pre-send
/// gate does.
fn gated(
    om: &mako_engine::outbox::OutboxMessage,
    registry: &makod::party_registry::MpIdRegistry,
) -> (edi_energy::AnyMessage, String) {
    use edi_energy::EdiEnergyMessage as _;
    let wire = makod::edifact_renderer::render_to_wire_bytes(om, registry)
        .unwrap_or_else(|e| panic!("render {}: {e:?}", om.message_type));
    let text = String::from_utf8_lossy(&wire.bytes).into_owned();
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
        text.replace('\'', "'\n"),
    );
    (msg, text)
}

/// The Lieferant's 44139: a new Marktgebiet and the Termin der
/// Netznutzungsabrechnung, Gemeinderabatt beside them.
fn anfrage_44139() -> mako_engine::outbox::OutboxMessage {
    let seg = |tag: &str, e: &[&[&str]]| {
        serde_json::json!({
            "tag": tag,
            "elemente": e.iter().map(|c| c.to_vec()).collect::<Vec<_>>(),
        })
    };
    support::outbox_message(
        "UTILMD",
        GNB_MP,
        serde_json::json!({
            "pid": 44139,
            "sender": LF_MP,
            "receiver": GNB_MP,
            "malo": MALO,
            "process_date": "2026-12-01",
            "vertragsbeginn": "2026-12-01",
            "vorgangsnummer": "ANFRAGE-VG-1",
            "transaktionsgrund": "ZF4",
            "stammdaten": {
                "sg6": [
                    seg("RFF", &[&["Z18", MALO]]),
                    seg("DTM", &[&["Z21", "0101", "106"]]),
                    seg("DTM", &[&["Z09", "2027", "602"]]),
                ],
                "bloecke": [
                    { "seq": "Z01", "segmente": [
                        seg("CCI", &[&["Z21"], &[], &[VORGESCHLAGEN]]),
                        seg("CCI", &[&[], &[], &["ZB0"]]),
                    ]},
                    { "seq": "Z12", "segmente": [seg("QTY", &[&["Z16", "5", "P1"]])] },
                ],
            },
        }),
    )
}

impl Nb {
    /// Deliver the Lieferant's 44139 to this NB's ingest.
    async fn receive_anfrage(&self) {
        use edi_energy::EdiEnergyMessage as _;
        let (msg, _) = gated(&anfrage_44139(), &self.registry);
        let pid = msg
            .detect_pruefidentifikator()
            .expect("a Prüfidentifikator")
            .as_u32();
        assert_eq!(pid, 44139);
        let workflow = self.router.route(pid).expect("44139 is routed at a GNB");
        self.dispatcher
            .dispatch(&msg, workflow, pid, time::OffsetDateTime::now_utc())
            .await
            .expect("the 44139 dispatches");
    }

    async fn pending_utilmd(&self) -> Vec<mako_engine::outbox::OutboxMessage> {
        let now = time::OffsetDateTime::now_utc() + time::Duration::days(1);
        OutboxStore::pending(&*self.store, 100, now)
            .await
            .expect("pending outbox")
            .into_iter()
            .filter(|om| &*om.message_type == "UTILMD")
            .collect()
    }
}

fn beantworten(code: &str) -> serde_json::Value {
    serde_json::json!({ "malo_id": MALO, "antwort_code": code })
}

/// The 44142 carries marktd's record for each group the Anfrage
/// filled: marktd's Marktgebiet, not the one proposed, the Termine and the
/// Gemeinderabatt; the Anfrage's dates, Grund and Vorgangsnummer restated.
#[tokio::test]
async fn an_anfrage_is_answered_with_the_master_data_marktd_holds() {
    let c = nb(Some(support::gas_stammdaten(MALO, GNB_MP))).await;
    c.receive_anfrage().await;
    assert!(
        c.pending_utilmd().await.is_empty(),
        "an Anfrage is checked before it is answered — nothing goes out on arrival"
    );

    dispatch_command(
        &c.state,
        "geli.stammdaten.anfrage.beantworten",
        &beantworten("ZG2"),
    )
    .await
    .expect("the Antwort is dispatched");

    let utilmd = c.pending_utilmd().await;
    let [om] = utilmd.as_slice() else {
        panic!("one UTILMD is written, got {}", utilmd.len());
    };
    let (msg, wire) = gated(om, &c.registry);
    {
        use edi_energy::EdiEnergyMessage as _;
        assert_eq!(
            msg.detect_pruefidentifikator().map(|p| p.as_u32()).ok(),
            Some(44142)
        );
    }
    for expected in [
        "STS+E01++ZG2:G_0031",
        "RFF+TN:ANFRAGE-VG-1",
        "STS+7++ZF4",
        &format!("CCI+Z21++{}", support::MARKTGEBIET),
        "CCI+++ZB0",
        &format!("RFF+Z18:{MALO}"),
        "DTM+Z21:",
        "DTM+Z09:",
        "SEQ+Z12",
        "QTY+Z16:",
    ] {
        assert!(wire.contains(expected), "{expected} in\n{wire}");
    }
    assert!(
        !wire.contains(VORGESCHLAGEN),
        "the Antwort states marktd's Marktgebiet, not the proposed one:\n{wire}"
    );
    // Groups the Anfrage did not fill stay out (`[336]`).
    for absent in ["SEQ+Z02", "SEQ+Z18", "NAD+DP", "DTM+Z22"] {
        assert!(!wire.contains(absent), "{absent} not asked for:\n{wire}");
    }
}

/// Partial master data: what marktd does not hold is named, and
/// nothing goes out.
#[tokio::test]
async fn partial_master_data_is_refused_naming_what_marktd_lacks() {
    let mut record = support::gas_stammdaten(MALO, GNB_MP);
    record.marktgebiet = None;
    if let Some(n) = record.netznutzung.as_mut() {
        n.gemeinderabatt_prozent = None;
    }
    let c = nb(Some(record)).await;
    c.receive_anfrage().await;
    let err = dispatch_command(
        &c.state,
        "geli.stammdaten.anfrage.beantworten",
        &beantworten("ZG2"),
    )
    .await
    .expect_err("an Antwort missing a held fact is refused");
    match &err {
        DispatchError::StammdatenUnvollstaendig { malo_id, fehlend } => {
            assert_eq!(malo_id, MALO);
            for name in ["marktgebiet", "gemeinderabatt_prozent"] {
                assert!(
                    fehlend.iter().any(|f| f.contains(name)),
                    "{name} named: {fehlend:?}"
                );
            }
        }
        other => panic!("refused as incomplete Stammdaten, got {other:?}"),
    }
    assert!(c.pending_utilmd().await.is_empty());
}

/// Master data that cannot be read refuses the command: marktd
/// holding no such Marktlokation, not answering, or not configured. No empty
/// Antwort goes out, and the process keeps waiting, so its 10-WT window still
/// escalates an unanswered Anfrage.
#[tokio::test]
async fn unreadable_master_data_refuses_the_antwort() {
    let mut c = nb(None).await;
    c.receive_anfrage().await;
    let err = dispatch_command(
        &c.state,
        "geli.stammdaten.anfrage.beantworten",
        &beantworten("ZG2"),
    )
    .await
    .expect_err("an unknown Marktlokation is refused");
    assert!(
        matches!(err, DispatchError::StammdatenUnvollstaendig { .. }),
        "{err:?}"
    );
    c.state.marktd_client = Some(Arc::new(support::unreachable_marktd()));
    let err = dispatch_command(
        &c.state,
        "geli.stammdaten.anfrage.beantworten",
        &beantworten("ZG2"),
    )
    .await
    .expect_err("an unreachable marktd refuses the Antwort");
    assert!(
        matches!(err, DispatchError::MarktdUnavailable(_)),
        "{err:?}"
    );
    c.state.marktd_client = None;
    let err = dispatch_command(
        &c.state,
        "geli.stammdaten.anfrage.beantworten",
        &beantworten("ZG2"),
    )
    .await
    .expect_err("a deployment without marktd refuses the Antwort");
    assert!(
        matches!(err, DispatchError::MarktdUnavailable(_)),
        "{err:?}"
    );
    assert!(c.pending_utilmd().await.is_empty());

    // Still answerable: the process waits in `AnfrageOffen`.
    c.state.marktd_client = Some(Arc::new(
        support::stub_marktd(Some(support::gas_stammdaten(MALO, GNB_MP))).await,
    ));
    dispatch_command(
        &c.state,
        "geli.stammdaten.anfrage.beantworten",
        &beantworten("ZG2"),
    )
    .await
    .expect("the Antwort goes out once marktd answers");
    assert_eq!(c.pending_utilmd().await.len(), 1);
}

/// A `ZG0` Ablehnung carries no Stammdaten (`[202]`) and so reads nothing.
#[tokio::test]
async fn an_ablehnung_reads_no_master_data() {
    let mut c = nb(None).await;
    c.state.marktd_client = None;
    c.receive_anfrage().await;
    dispatch_command(
        &c.state,
        "geli.stammdaten.anfrage.beantworten",
        &beantworten("ZG0"),
    )
    .await
    .expect("the Ablehnung is dispatched");
    let utilmd = c.pending_utilmd().await;
    let [om] = utilmd.as_slice() else {
        panic!("one UTILMD, got {}", utilmd.len());
    };
    let (_, wire) = gated(om, &c.registry);
    assert!(wire.contains("STS+E01++ZG0:G_0031"), "{wire}");
    assert!(!wire.contains("SEQ+"), "{wire}");

    let err = dispatch_command(
        &c.state,
        "geli.stammdaten.anfrage.beantworten",
        &beantworten("A01"),
    )
    .await
    .expect_err("nothing is left to answer");
    assert!(
        matches!(
            err,
            DispatchError::InvalidPayload(_) | DispatchError::ProcessNotFound { .. }
        ),
        "{err:?}"
    );
}

/// A code the 44142's Codeliste `G_0031` does not publish is refused before
/// anything is read.
#[tokio::test]
async fn a_code_outside_its_codeliste_is_refused() {
    let c = nb(Some(support::gas_stammdaten(MALO, GNB_MP))).await;
    c.receive_anfrage().await;
    let err = dispatch_command(
        &c.state,
        "geli.stammdaten.anfrage.beantworten",
        &beantworten("E15"),
    )
    .await
    .expect_err("E15 is no G_0031 code");
    assert!(format!("{err:?}").contains("G_0031"), "{err:?}");
    assert!(c.pending_utilmd().await.is_empty());
}
