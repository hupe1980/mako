//! The Verantwortlicher's Gas Änderungen sent by
//! `geli.stammdaten-aenderung.senden`, rendered, parsed back and held against
//! their AHB profile:
//!
//! | PID | Data | UTILMD AHB Gas 1.2 |
//! |---|---|---|
//! | 44112 GNB → LF | the changed groups, from marktd on the Änderungsdatum | Kap. 5.10.1, S. 97–110 |
//! | 44123 GNB → LF | `SG8` Daten der Marktlokation whole — the Prognosegrundlage and what it implies | Kap. 5.11.1, S. 114–116 |
//! | 44175 GNB → LF | the Marktlokation and every Messlokation of its structure (`[590]`) | Kap. 5.11.2, S. 119 |
//! | 44120 LF → GNB | `SG8` with the LF's Bilanzkreis | Kap. 5.11.4, S. 127 |

use std::sync::Arc;

use mako_engine::outbox::OutboxStore;
use mako_engine::{ids::TenantId, store_slatedb::SlateDbStore};
use makod::commands_api::{CommandsApiState, DispatchError, dispatch_command};

use crate::support;

// DVGW-Codenummern: NAD DE 3055 `332`.
const GNB_MP: &str = "9807317000007";
const LF_MP: &str = "9870012345678";
const MALO: &str = "51238696012";

async fn gnb(
    record: Option<mako_markt::utilmd_stammdaten::MaloUtilmdStammdaten>,
) -> CommandsApiState {
    let store = Arc::new(
        SlateDbStore::open_in_memory()
            .await
            .expect("in-memory store"),
    );
    CommandsApiState {
        tenant_id: TenantId::from_party_id(GNB_MP),
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
        store,
        marktd_client: Some(Arc::new(support::stub_marktd(record).await)),
    }
}

fn body(gruppen: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "malo_id": MALO,
        "pid": 44112,
        "empfaenger_mp_id": LF_MP,
        "aenderungsdatum": "20261201",
        "vertragsbeginn": "20250101",
        "gruppen": gruppen,
    })
}

#[tokio::test]
async fn the_gnb_sends_a_44112_with_marktds_changed_groups() {
    for gruppen in [
        &["MARKTGEBIET", "NETZNUTZUNG"][..],
        &[
            "TERMIN_NETZNUTZUNGSABRECHNUNG",
            "NETZNUTZUNGSABRECHNUNGSINTERVALL",
            "DRUCKEBENE",
            "MARKTGEBIET",
            "KONZESSIONSABGABE",
            "ZUGEORDNETE_MARKTPARTNER",
            "GASQUALITAET",
            "MARKTLOKATIONSANSCHRIFT",
        ][..],
    ] {
        sends_a_44112(gruppen).await;
    }
}

async fn sends_a_44112(gruppen: &[&str]) {
    let state = gnb(Some(support::gas_stammdaten(MALO, GNB_MP))).await;
    let text = sends(&state, &body(gruppen), LF_MP).await;
    assert!(text.contains("CCI+Z21"), "the Marktgebiet: {text}");
}

/// Dispatch `body` on `state`, and render the one UTILMD it writes to
/// `empfaenger`, held against its profile as the pre-send gate does.
async fn sends(state: &CommandsApiState, body: &serde_json::Value, empfaenger: &str) -> String {
    dispatch_command(
        state,
        mako_markt::commands::GELI_STAMMDATEN_AENDERUNG_SENDEN,
        body,
    )
    .await
    .unwrap_or_else(|e| panic!("the {} is dispatched: {e:?}", body["pid"]));
    let out: Vec<_> = state
        .store
        .pending_now(100)
        .await
        .expect("outbox")
        .into_iter()
        .filter(|m| m.message_type.as_ref() == "UTILMD")
        .collect();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].recipient.as_ref(), empfaenger);
    assert_eq!(out[0].payload["pid"], body["pid"]);

    // Render, parse back and hold against the profile, as the pre-send gate
    // does.
    use edi_energy::EdiEnergyMessage as _;
    let party = |mp: &str, role: &str| makod::config::PartyConfig {
        mp_id: mp.to_owned(),
        roles: vec![role.to_owned()],
        primary: mp == GNB_MP,
        agency: None,
        as4: None,
    };
    let registry = makod::party_registry::MpIdRegistry::from_config(&[
        party(GNB_MP, "NB"),
        party(LF_MP, "LF"),
    ])
    .expect("valid registry");
    let wire =
        makod::edifact_renderer::render_to_wire_bytes(&out[0], &registry).expect("it renders");
    let text = String::from_utf8_lossy(&wire.bytes).into_owned();
    let msg = edi_energy::parse(&wire.bytes).expect("parses");
    let report = msg
        .validate_on_date(mako_fristen::heute())
        .expect("the AHB validation runs");
    assert!(
        report.is_sendable(),
        "{:#?}\n{}",
        report
            .sending_refusals()
            .map(|e| &e.message)
            .collect::<Vec<_>>(),
        text.replace('\'', "'\n")
    );
    text
}

/// A bilanzierungsrelevante or structural Änderung of `pid`, named by no group.
fn ganz(pid: u32) -> serde_json::Value {
    let mut b = body(&[]);
    b["pid"] = serde_json::json!(pid);
    b
}

/// The 44123 carries `SG8` whole: marktd's Prognosegrundlage on Werten with
/// its Fallgruppenzuordnung (Kap. 5.11.1, S. 115, `[19]`). The Änderungsdatum
/// is a Monatserster, as a bilanzierungsrelevante Änderung's must be.
#[tokio::test]
async fn the_gnb_sends_a_44123_with_its_sg8_whole() {
    let state = gnb(Some(support::gas_stammdaten(MALO, GNB_MP))).await;
    let text = sends(&state, &ganz(44123), LF_MP).await;
    for expected in ["STS+7++ZF0", "SEQ+Z01", "CCI+++ZC0", "CCI+++Z17"] {
        assert!(text.contains(expected), "{expected}: {text}");
    }
}

/// The 44175 names the Marktlokation and every Messlokation marktd assigns it
/// (Kap. 5.11.2, S. 119, `[590]`).
#[tokio::test]
async fn the_gnb_sends_a_44175_with_every_messlokation() {
    let state = gnb(Some(support::gas_stammdaten(MALO, GNB_MP))).await;
    let text = sends(&state, &ganz(44175), LF_MP).await;
    for expected in [
        "STS+7++ZI9".to_owned(),
        format!("LOC+172+{MALO}"),
        format!("LOC+172+{}", support::GAS_MELO),
    ] {
        assert!(text.contains(&expected), "{expected}: {text}");
    }
}

/// The LF's 44120 carries its Bilanzkreis in `SG8` (Kap. 5.11.4, S. 127) and
/// is refused without it; a group or another LF fact has no place on it.
#[tokio::test]
async fn the_lf_sends_a_44120_with_its_bilanzkreis() {
    let lf = {
        let mut s = gnb(None).await;
        s.sender_party_id = LF_MP.to_owned();
        s.tenant_id = TenantId::from_party_id(LF_MP);
        s.configured_marktrollen = vec!["LFG".to_owned()];
        s
    };
    let mut b = ganz(44120);
    b["empfaenger_mp_id"] = serde_json::json!(GNB_MP);
    let mut ohne = b.clone();
    ohne["bilanzkreis"] = serde_json::json!(" ");
    let err = dispatch_command(
        &lf,
        mako_markt::commands::GELI_STAMMDATEN_AENDERUNG_SENDEN,
        &ohne,
    )
    .await
    .expect_err("a 44120 without its Bilanzkreis");
    assert!(format!("{err:?}").contains("bilanzkreis"), "{err:?}");
    let mut fremd = b.clone();
    fremd["bilanzkreis"] = serde_json::json!("THE0BFH000000001");
    fremd["haushaltskunde"] = serde_json::json!(true);
    let err = dispatch_command(
        &lf,
        mako_markt::commands::GELI_STAMMDATEN_AENDERUNG_SENDEN,
        &fremd,
    )
    .await
    .expect_err("a 44120 carries no Gruppenzuordnung");
    assert!(format!("{err:?}").contains("haushaltskunde"), "{err:?}");

    b["bilanzkreis"] = serde_json::json!("THE0BFH000000001");
    let text = sends(&lf, &b, GNB_MP).await;
    for expected in ["STS+7++ZE9", "SEQ+Z01", "CCI+Z19++THE0BFH000000001"] {
        assert!(text.contains(expected), "{expected}: {text}");
    }
}

/// Without marktd there is no 44112, and nothing is written.
#[tokio::test]
async fn a_44112_marktd_cannot_serve_is_refused() {
    let state = gnb(None).await;
    let err = dispatch_command(
        &state,
        mako_markt::commands::GELI_STAMMDATEN_AENDERUNG_SENDEN,
        &body(&["MARKTGEBIET"]),
    )
    .await
    .expect_err("refused");
    assert!(
        !matches!(err, DispatchError::DuplicateProcess { .. }),
        "{err:?}"
    );
    assert!(
        state
            .store
            .pending_now(100)
            .await
            .expect("outbox")
            .is_empty()
    );
}

// ── 44113 → 44115: the MSB answers the NB's Änderung ────────────────────────

const MSB_MP: &str = "9800357000011";

/// „Der NB sendet als Verantwortlicher seine Stammdatenänderung an den
/// jeweiligen Marktpartner (an LF mit Prüfidentifikator 44112 bzw. an MSB mit
/// Prüfidentifikator 44113). Die berechtigten Marktpartner antworten mit dem
/// Prüfidentifikator 44115" (UTILMD AHB Gas 1.2 Kap. 5.10.1, S. 97). A
/// deployment that is a Gas MSB alone routes the 44113 to the Stammdaten
/// process — and nothing of the LF's or the NB's GeLi Gas — and its answer
/// passes the 44115 column.
#[tokio::test]
async fn an_msb_deployment_answers_a_44113() {
    use edi_energy::EdiEnergyMessage as _;
    use mako_engine::marktrolle::{DeploymentRoles, Marktrolle};
    use makod::deployment::Rolle;

    // The NB's Änderung of the Messlokation's data, as the GNB door builds it
    // for the LF, addressed to the MSB as the 44113.
    let nb = gnb(Some(support::gas_stammdaten(MALO, GNB_MP))).await;
    sends(
        &nb,
        &body(&["ZUGEORDNETE_MARKTPARTNER", "GASQUALITAET"]),
        LF_MP,
    )
    .await;
    let mut om = nb
        .store
        .pending_now(100)
        .await
        .expect("outbox")
        .into_iter()
        .find(|m| m.message_type.as_ref() == "UTILMD")
        .expect("the Änderung");
    om.payload["pid"] = serde_json::json!(44113);
    om.payload["receiver"] = serde_json::json!(MSB_MP);
    om.recipient = MSB_MP.into();
    let party = |mp: &str, role: &str| makod::config::PartyConfig {
        mp_id: mp.to_owned(),
        roles: vec![role.to_owned()],
        primary: mp == MSB_MP,
        agency: None,
        as4: None,
    };
    let registry = makod::party_registry::MpIdRegistry::from_config(&[
        party(GNB_MP, "NB"),
        party(MSB_MP, "MSB"),
    ])
    .expect("valid registry");
    let wire = makod::edifact_renderer::render_to_wire_bytes(&om, &registry).expect("renders");
    let msg = edi_energy::parse(&wire.bytes).expect("parses");
    assert!(
        msg.validate_on_date(mako_fristen::heute())
            .expect("validates")
            .is_sendable(),
        "the 44113 fixture is conformant"
    );

    // The MSB alone.
    let rollen: std::collections::BTreeSet<Rolle> = [Rolle::MsbGas].into_iter().collect();
    let roles = DeploymentRoles::from_roles([Marktrolle::Msb]);
    let mut router = mako_engine::pid_router::PidRouter::new();
    for module in makod::startup::modules_for(&rollen) {
        module.register_pids_with_roles(&mut router, &roles);
    }
    assert_eq!(
        router.route(44001),
        None,
        "no GeLi Gas Lieferbeginn at an MSB"
    );
    let workflow = router.route(44113).expect("the 44113 is routed");

    let store = Arc::new(SlateDbStore::open_in_memory().await.expect("store"));
    let msb = CommandsApiState {
        tenant_id: TenantId::from_party_id(MSB_MP),
        sender_party_id: MSB_MP.to_owned(),
        configured_marktrollen: vec!["GMSB".to_owned()],
        store: Arc::clone(&store),
        snapshot_store: store.as_snapshot_store(),
        malo_cache: Arc::new(makod::malo_cache::SlateDbMaloCache::new((*store).clone())),
        maloid_result_cache: makod::malo_cache::MaloIdentResultCache::new((*store).clone()),
        marktd_client: None,
        ..gnb(None).await
    };
    makod::ingest_dispatcher::EdifactIngestDispatcher::new(
        Arc::clone(&store),
        store.as_snapshot_store(),
        100,
        msb.tenant_id,
    )
    .dispatch(&msg, workflow, 44113, time::OffsetDateTime::now_utc())
    .await
    .expect("the 44113 dispatches");

    dispatch_command(
        &msb,
        mako_markt::commands::GELI_STAMMDATEN_AENDERUNG_BEANTWORTEN,
        &serde_json::json!({ "malo_id": MALO, "antwort_code": "E15" }),
    )
    .await
    .expect("the MSB answers");
    let antwort = store
        .pending_now(100)
        .await
        .expect("outbox")
        .into_iter()
        .find(|m| m.message_type.as_ref() == "UTILMD")
        .expect("the 44115");
    assert_eq!(antwort.payload["pid"], 44115);
    assert_eq!(antwort.recipient.as_ref(), GNB_MP);
    let wire = makod::edifact_renderer::render_to_wire_bytes(&antwort, &registry).expect("renders");
    let text = String::from_utf8_lossy(&wire.bytes).into_owned();
    let report = edi_energy::parse(&wire.bytes)
        .expect("parses")
        .validate_on_date(mako_fristen::heute())
        .expect("validates");
    assert!(
        report.is_sendable(),
        "{:#?}\n{}",
        report
            .sending_refusals()
            .map(|e| &e.message)
            .collect::<Vec<_>>(),
        text.replace('\'', "'\n")
    );
    assert!(text.contains("STS+E01++E15:G_0016"), "{text}");
}
